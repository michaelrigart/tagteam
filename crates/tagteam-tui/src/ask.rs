//! The bridge's `Ask` (spec §7.2, §10.2 step 3): a CLI prompt on the actions lane becomes a
//! dialog on the UI thread, and the lane waits for its answer in slices it can leave.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use tagteam_provider::Cancel;

use crate::host::Ask;
use crate::msg::{Answer, Msg, Prompt, PromptKind};

/// How long the lane waits for an answer before it looks at the shutdown token again.
#[allow(dead_code)]
const SLICE: Duration = Duration::from_millis(100);

/// Task 11's runtime builds one for each action; until then only the tests do.
#[allow(dead_code)]
pub(crate) struct ChannelAsk {
    tx: Sender<Msg>,
    shutdown: Cancel,
}

#[allow(dead_code)]
impl ChannelAsk {
    /// Prompts go to the UI thread on `tx`; once `shutdown` is set, every prompt is declined.
    pub(crate) fn new(tx: Sender<Msg>, shutdown: Cancel) -> Self {
        ChannelAsk { tx, shutdown }
    }

    /// The answer to `question`, or `None` for a decline: the shutdown has begun (before the
    /// prompt, or while it waits), or the UI thread is gone.
    fn ask(&self, question: &str, kind: PromptKind) -> Option<Answer> {
        if self.shutdown.requested().is_some() {
            return None;
        }
        let (reply, answer) = mpsc::channel();
        let prompt = Prompt {
            question: question.to_owned(),
            kind,
            reply,
        };
        self.tx.send(Msg::Prompt(prompt)).ok()?;
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
        let mut ask = ChannelAsk::new(tx.clone(), shutdown.clone());
        let lane = thread::spawn(move || ask.confirm("Add it?", true));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        assert_eq!(prompt.question, "Add it?");
        assert!(matches!(
            prompt.kind,
            PromptKind::Confirm { default_yes: true }
        ));
        prompt.reply.send(Answer::Confirm(true)).unwrap();
        assert!(lane.join().unwrap());

        let mut ask = ChannelAsk::new(tx, shutdown);
        let lane = thread::spawn(move || ask.choose("Which?", &["a".into(), "b".into()]));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        let PromptKind::Choose { options } = &prompt.kind else {
            panic!("a choice");
        };
        assert_eq!(options, &["a", "b"]);
        prompt.reply.send(Answer::Choose(Some(1))).unwrap();
        assert_eq!(lane.join().unwrap(), Some(1));
    }

    #[test]
    fn a_shutdown_before_the_prompt_declines_without_showing_it() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        shutdown.request(SIGINT);
        let mut ask = ChannelAsk::new(tx, shutdown);
        assert!(!ask.confirm("Add it?", true));
        assert_eq!(ask.choose("Which?", &["a".into()]), None);
        assert!(next_prompt(&rx, Duration::from_millis(50)).is_none());
    }

    #[test]
    fn a_shutdown_while_waiting_declines_within_one_slice() {
        let (tx, rx) = mpsc::channel();
        let shutdown = Cancel::new();
        let mut ask = ChannelAsk::new(tx, shutdown.clone());
        let lane = thread::spawn(move || ask.confirm("Add it?", true));
        let prompt = next_prompt(&rx, WAIT).unwrap();
        let set = Instant::now();
        shutdown.request(SIGINT);
        assert!(!lane.join().unwrap());
        assert!(set.elapsed() < SLICE * 3, "{:?}", set.elapsed());
        // The dialog's answer, should it still come, goes nowhere.
        let _ = prompt.reply.send(Answer::Confirm(true));
    }

    #[test]
    fn a_dialog_dropped_unanswered_declines() {
        let (tx, rx) = mpsc::channel();
        let mut ask = ChannelAsk::new(tx, Cancel::new());
        let lane = thread::spawn(move || ask.choose("Which?", &["a".into()]));
        drop(next_prompt(&rx, WAIT).unwrap());
        assert_eq!(lane.join().unwrap(), None);
    }
}
