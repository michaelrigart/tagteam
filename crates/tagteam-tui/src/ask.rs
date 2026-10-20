//! The bridge's `Ask` (spec §7.2, §10.2 step 3): a CLI prompt on the actions lane becomes a
//! dialog on the UI thread, and the lane waits for its answer in slices it can leave.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tagteam_provider::Cancel;

use crate::host::Ask;
use crate::msg::{Answer, AnswerTo, Msg, Prompt, PromptKind};

/// How long the lane waits for an answer before it looks at the shutdown token again.
const SLICE: Duration = Duration::from_millis(100);

/// The senders of the prompts still waiting, shared by the runtime and the actions lane. `State`
/// holds an `AnswerTo`, never a sender (spec §7.2).
#[derive(Clone, Default)]
pub(crate) struct Answers(Arc<Mutex<Waiting>>);

#[derive(Default)]
struct Waiting {
    next: u64,
    senders: BTreeMap<u64, Sender<Answer>>,
}

impl Answers {
    fn lock(&self) -> MutexGuard<'_, Waiting> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `reply` until the prompt is answered, declined or forgotten.
    pub(crate) fn keep(&self, reply: Sender<Answer>) -> AnswerTo {
        let mut waiting = self.lock();
        let id = waiting.next;
        waiting.next += 1;
        waiting.senders.insert(id, reply);
        AnswerTo(id)
    }

    /// Sends `answer` once and forgets the sender; an id no longer waiting gets nothing.
    pub(crate) fn answer(&self, to: AnswerTo, answer: Answer) {
        let sender = self.lock().senders.remove(&to.0);
        if let Some(sender) = sender {
            let _ = sender.send(answer);
        }
    }

    pub(crate) fn forget(&self, to: AnswerTo) {
        self.lock().senders.remove(&to.0);
    }

    /// Drops every sender (spec §10.2 step 3): each waiting lane sees a decline at once.
    pub(crate) fn decline_all(&self) {
        self.lock().senders.clear();
    }

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.lock().senders.len()
    }
}

/// The runtime builds one for each action its actions lane runs.
pub(crate) struct ChannelAsk {
    tx: Sender<Msg>,
    shutdown: Cancel,
    answers: Answers,
}

impl ChannelAsk {
    /// Prompts go to the UI thread on `tx`, their senders wait in `answers`; once `shutdown` is
    /// set, every prompt is declined.
    pub(crate) fn new(tx: Sender<Msg>, shutdown: Cancel, answers: Answers) -> Self {
        ChannelAsk {
            tx,
            shutdown,
            answers,
        }
    }

    /// The answer to `question`, or `None` for a decline: the shutdown has begun (before the
    /// prompt, or while it waits), or the UI thread is gone.
    fn ask(&self, question: &str, kind: PromptKind) -> Option<Answer> {
        if self.shutdown.requested().is_some() {
            return None;
        }
        let (reply, answer) = mpsc::channel();
        let to = self.answers.keep(reply);
        let prompt = Prompt {
            question: question.to_owned(),
            kind,
            to,
        };
        let answered = match self.tx.send(Msg::Prompt(prompt)) {
            Ok(()) => self.wait(&answer),
            Err(_) => None,
        };
        self.answers.forget(to);
        answered
    }

    fn wait(&self, answer: &Receiver<Answer>) -> Option<Answer> {
        loop {
            match answer.recv_timeout(SLICE) {
                Ok(a) if self.shutdown.requested().is_none() => return Some(a),
                Ok(_) | Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {
                    if self.shutdown.requested().is_some() {
                        return None;
                    }
                }
            }
        }
    }
}

impl Ask for ChannelAsk {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        matches!(
            self.ask(question, PromptKind::Confirm { default_yes }),
            Some(Answer::Confirm(true))
        )
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        match self.ask(
            question,
            PromptKind::Choose {
                options: options.to_vec(),
            },
        ) {
            Some(Answer::Choose(choice)) => choice.filter(|i| *i < options.len()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Instant;

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);
    /// What the shutdown token records for a `q` (spec §10.2 step 1).
    const SIGINT: i32 = 2;

    /// The UI thread's side: the next prompt, or `None` when none comes within `within`.
    fn next_prompt(rx: &mpsc::Receiver<Msg>, within: Duration) -> Option<Prompt> {
        match rx.recv_timeout(within) {
            Ok(Msg::Prompt(p)) => Some(p),
            Ok(_) => panic!("only prompts are sent"),
            Err(_) => None,
        }
    }

    #[test]
    fn an_answer_reaches_the_waiting_lane() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        let answers = Answers::default();
        let mut ask = ChannelAsk::new(tx.clone(), shutdown.clone(), answers.clone());
        let lane = thread::spawn(move || ask.confirm("Add it?", true));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        assert_eq!(prompt.question, "Add it?");
        assert!(matches!(
            prompt.kind,
            PromptKind::Confirm { default_yes: true }
        ));
        answers.answer(prompt.to, Answer::Confirm(true));
        assert!(lane.join().unwrap());

        let mut ask = ChannelAsk::new(tx, shutdown, answers.clone());
        let lane = thread::spawn(move || ask.choose("Which?", &["a".into(), "b".into()]));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        let PromptKind::Choose { options } = &prompt.kind else {
            panic!("a choice");
        };
        assert_eq!(options, &["a", "b"]);
        answers.answer(prompt.to, Answer::Choose(Some(1)));
        assert_eq!(lane.join().unwrap(), Some(1));
    }

    #[test]
    fn a_shutdown_before_the_prompt_declines_without_showing_it() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        shutdown.request(SIGINT);
        let mut ask = ChannelAsk::new(tx, shutdown, Answers::default());
        assert!(!ask.confirm("Add it?", true));
        assert_eq!(ask.choose("Which?", &["a".into()]), None);
        assert!(next_prompt(&rx, Duration::from_millis(50)).is_none());
    }

    #[test]
    fn a_shutdown_while_waiting_declines_within_one_slice() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        let answers = Answers::default();
        let mut ask = ChannelAsk::new(tx, shutdown.clone(), answers.clone());
        let lane = thread::spawn(move || ask.confirm("Add it?", true));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        let set = Instant::now();
        shutdown.request(SIGINT);
        assert!(!lane.join().unwrap());
        assert!(set.elapsed() < SLICE * 3, "{:?}", set.elapsed());
        // The dialog's answer, should it still come, goes nowhere.
        answers.answer(prompt.to, Answer::Confirm(true));
    }

    #[test]
    fn declining_every_prompt_frees_its_lane_before_the_slice_ends() {
        let (tx, rx) = mpsc::channel();
        let answers = Answers::default();
        let mut ask = ChannelAsk::new(tx, Cancel::new(), answers.clone());
        let lane = thread::spawn(move || ask.choose("Which?", &["a".into()]));
        let _prompt = next_prompt(&rx, WAIT).unwrap();
        let declined = Instant::now();
        answers.decline_all();
        assert_eq!(lane.join().unwrap(), None);
        assert!(
            declined.elapsed() < SLICE / 2,
            "{:?}, with the shutdown token never set",
            declined.elapsed()
        );
    }

    #[test]
    fn a_prompt_that_stopped_waiting_is_forgotten() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        let answers = Answers::default();
        let mut ask = ChannelAsk::new(tx, shutdown.clone(), answers.clone());
        let lane = thread::spawn(move || ask.confirm("Add it?", true));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        assert_eq!(answers.waiting(), 1);
        shutdown.request(SIGINT);
        assert!(!lane.join().unwrap());
        assert_eq!(answers.waiting(), 0);
        answers.answer(prompt.to, Answer::Confirm(true));
        assert_eq!(answers.waiting(), 0);
    }
}
