//! D6: notifications. A `NotifySink` turns the daemon's `switch` and `account-quarantined`
//! events into notices. The provider job asks `limit_notice` after a run that did not switch
//! (`after_run`). The `Notifier` delivers each notice on a thread of its own through the
//! engine's `ProcessSpawner`, so the scheduler never waits on one (D4).

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tagteam_core::autoswitch::Trigger;
use tagteam_core::rank::{binding_window, span};
use tagteam_core::{ProviderId, Window};
use tagteam_engine::auto::AutoEvent;
use tagteam_engine::daemon::find_on_path;
use tagteam_engine::settings::{NotifyEvent, Settings};
use tagteam_engine::store::AccountRow;
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::Cancel;
use tagteam_provider::process::{Discarded, ProcessSpawner, SpawnSpec};

use crate::events::{DaemonEvent, DaemonSink};

/// D6.2: every notice's title.
const TITLE: &str = "tagteam";
/// D6.2: macOS's own `osascript`, never one found on `PATH`.
const OSASCRIPT: &str = "/usr/bin/osascript";
/// D6.2: the script reads its title and body from argv, after `--`, so no account name can
/// inject AppleScript.
const OSASCRIPT_ARGS: [&str; 7] = [
    "-e",
    "on run argv",
    "-e",
    "display notification (item 2 of argv) with title (item 1 of argv)",
    "-e",
    "end run",
    "--",
];
/// D6.2: `notify-send`'s options; the title and body follow `--`.
const NOTIFY_SEND_ARGS: [&str; 2] = ["--app-name=tagteam", "--"];
/// How many notices wait for the notifier thread. When the queue is full, a new notice is
/// dropped (`NoticeQueue::send`).
const QUEUE: usize = 32;
/// D6.2: one delivery's bound.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// D6.3: a `limit` marker outlives its window's reset by this much…
const AFTER_RESET_MS: i64 = 60_000;
/// …and lives this long when the reset is unknown.
const UNKNOWN_RESET_MS: i64 = 3_600_000;

/// Where notices are shown (D6.2): macOS's `osascript`, or `notify-send` found on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Osascript,
    NotifySend(PathBuf),
}

impl Backend {
    /// macOS always has its own `osascript`. Elsewhere it is `notify-send` on `path_var`, and
    /// without that there is no backend.
    pub fn detect(macos: bool, path_var: Option<&OsStr>) -> Option<Backend> {
        if macos {
            Some(Backend::Osascript)
        } else {
            find_on_path("notify-send", path_var).map(Backend::NotifySend)
        }
    }

    /// As `status` and `doctor` name it (D5.4, D10).
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Osascript => "osascript",
            Backend::NotifySend(_) => "notify-send",
        }
    }

    /// D6.2's command line, with `title` and `body` as the last two arguments, verbatim.
    pub fn spec(&self, title: &str, body: &str) -> SpawnSpec {
        let (program, options): (PathBuf, &[&str]) = match self {
            Backend::Osascript => (PathBuf::from(OSASCRIPT), &OSASCRIPT_ARGS),
            Backend::NotifySend(path) => (path.clone(), &NOTIFY_SEND_ARGS),
        };
        let mut args: Vec<OsString> = options.iter().map(OsString::from).collect();
        args.extend([OsString::from(title), OsString::from(body)]);
        SpawnSpec {
            program,
            args,
            ..SpawnSpec::default()
        }
    }
}

/// One notification: its title and its body, which names accounts only as `{account}` does
/// (§13.5) and never carries a credential, an organization name or a whole email (D6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// Where notices wait for the notifier. Every holder sends from the scheduler's thread.
#[derive(Clone)]
pub struct NoticeQueue {
    tx: SyncSender<Message>,
}

/// What the notifier thread waits for: a notice to deliver, or the stop `Notifier::drop` sends
/// to wake it (H8).
enum Message {
    Notice(Notice),
    Stop,
}

impl NoticeQueue {
    /// Queues `n` and never blocks (D4). A full queue drops it, and so does a notifier that
    /// has stopped.
    pub fn send(&self, n: Notice) {
        match self.tx.try_send(Message::Notice(n)) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                tracing::debug!("the notification queue is full; a notification was dropped");
            }
        }
    }

    /// A queue whose notices a test reads from the receiver, with the notifier's capacity.
    #[cfg(test)]
    pub(crate) fn for_test() -> (NoticeQueue, Queued) {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        (NoticeQueue { tx }, Queued(rx))
    }
}

/// What a test reads queued notices from.
#[cfg(test)]
pub(crate) struct Queued(Receiver<Message>);

#[cfg(test)]
impl Queued {
    /// The notices waiting, in order.
    pub(crate) fn try_iter(&self) -> impl Iterator<Item = Notice> + '_ {
        self.0.try_iter().filter_map(|m| match m {
            Message::Notice(n) => Some(n),
            Message::Stop => None,
        })
    }
}

/// The notifier thread (D4, D6.2). It blocks until a notice or the stop message that dropping
/// the `Notifier` sends (H8); dropping it stops it, as `stop` does.
pub struct Notifier {
    stop: Arc<AtomicBool>,
    tx: SyncSender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl Notifier {
    /// Starts the thread that delivers each queued notice through `spawner`. The thread logs
    /// through the dispatcher this call runs under.
    pub fn start(backend: Backend, spawner: Arc<dyn ProcessSpawner>) -> (Notifier, NoticeQueue) {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let dispatch = tracing::dispatcher::get_default(|d| d.clone());
        let thread = thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                deliver_until_stopped(&backend, spawner.as_ref(), &rx, &stopped)
            })
        });
        let notifier = Notifier {
            stop,
            tx: tx.clone(),
            thread: Some(thread),
        };
        (notifier, NoticeQueue { tx })
    }

    /// Stops the thread once the delivery in flight, if any, has ended. Nothing still queued
    /// is delivered (D6.2).
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The queue beside the notifier may outlive it, so the thread would never see the
        // channel close: this message wakes it. The send blocks on a full queue until the
        // delivery in flight ends, which the join waits for anyway; `Err` means the thread has
        // gone.
        let _ = self.tx.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The notifier thread's loop: each notice in turn, until the stop message arrives, a stop is
/// flagged or every queue holder has gone. The flag decides: a notice queued ahead of the stop
/// is not delivered once it is set (D6.2).
fn deliver_until_stopped(
    backend: &Backend,
    spawner: &dyn ProcessSpawner,
    rx: &Receiver<Message>,
    stop: &AtomicBool,
) {
    let mut warned = false;
    loop {
        match rx.recv() {
            Ok(_) if stop.load(Ordering::SeqCst) => return,
            Ok(Message::Notice(notice)) => deliver(backend, spawner, &notice, &mut warned),
            Ok(Message::Stop) | Err(_) => return,
        }
    }
}

/// One delivery (D6.2). It has a cancel token of its own, never the daemon's, so a stop never
/// kills it half-way, and a 5 s bound. A failure is logged at WARN the first time and at DEBUG
/// after. The line names the backend and how it failed, never the text, which names accounts
/// (§14.2). The program's stdin, stdout and stderr are null (D6.2): its output is never
/// captured, so none can grow memory or be held open by a descendant.
fn deliver(backend: &Backend, spawner: &dyn ProcessSpawner, notice: &Notice, warned: &mut bool) {
    let spec = backend.spec(&notice.title, &notice.body);
    let failure = match spawner.run_discarded(&spec, DELIVERY_TIMEOUT, &Cancel::new()) {
        Discarded::Exited { code: Some(0), .. } => None,
        Discarded::Exited {
            code: Some(code), ..
        } => Some(format!("exit {code}")),
        Discarded::Exited {
            signal: Some(signal),
            ..
        } => Some(format!("signal {signal}")),
        Discarded::Exited { .. } => Some("no exit status".to_owned()),
        Discarded::TimedOut => Some("timed out".to_owned()),
        Discarded::Interrupted(signal) => Some(format!("interrupted by signal {signal}")),
        Discarded::SpawnFailed(why) => Some(why),
    };
    match failure {
        None => tracing::debug!(backend = backend.name(), "notification sent"),
        Some(why) if !*warned => {
            *warned = true;
            tracing::warn!(
                backend = backend.name(),
                error = %why,
                "a notification could not be delivered; later failures log at DEBUG"
            );
        }
        Some(why) => tracing::debug!(
            backend = backend.name(),
            error = %why,
            "a notification could not be delivered"
        ),
    }
}

/// D6.4: `notify.enabled`, then the provider's `notify.events`.
fn wanted(settings: &Settings, kind: NotifyEvent) -> bool {
    settings.notify_enabled && settings.notify_events.contains(&kind)
}

/// The part of an email before `@`; a label without one is itself.
fn local_part(email: &str) -> &str {
    email.split_once('@').map_or(email, |(local, _)| local)
}

/// §13.5's `{account}` after its position: `2 · spare`, from the alias, or else the local part
/// of the email (of the label, for a provider whose logins have none).
fn account(position: u32, alias: Option<&str>, email_or_label: &str) -> String {
    let name = alias.unwrap_or_else(|| local_part(email_or_label));
    format!("{position} · {name}")
}

/// `account` for `row`. Where the row is gone, it uses `fallback`, the event's email, and else
/// the position alone.
fn named(row: Option<&AccountRow>, position: u32, fallback: Option<&str>) -> String {
    match (row, fallback) {
        (Some(r), _) => account(
            r.position,
            r.alias.as_deref(),
            r.email.as_deref().unwrap_or(&r.label),
        ),
        (None, Some(email)) => account(position, None, email),
        (None, None) => position.to_string(),
    }
}

/// A window as a notice names it: `7d at 93%`, rounded as `list` rounds it.
fn window_at(w: &Window) -> String {
    format!("{} at {}%", w.label, w.pct.round() as i64)
}

/// D6.1–D6.3: a notice for each `switch` the daemon made (never a dry run) and each
/// `account-quarantined`, when `notify.enabled` is on and the provider's `notify.events` names
/// the kind. The settings are read for each such event (`Settings::load`): the events are rare,
/// and a setting changed a moment ago then always counts.
pub struct NotifySink<'a> {
    engine: &'a Engine,
    queue: NoticeQueue,
    /// Where each queued notice is also announced (`DaemonSink::notified`): `tagteam daemon`'s
    /// sink, for its line per notification (D8.1).
    echo: Option<&'a dyn DaemonSink>,
}

impl<'a> NotifySink<'a> {
    pub fn new(engine: &'a Engine, queue: NoticeQueue) -> Self {
        NotifySink {
            engine,
            queue,
            echo: None,
        }
    }

    /// Announces each notice it queues to `to` as well. `Started::serve` echoes to its caller's
    /// sink.
    pub(crate) fn echoing(mut self, to: &'a dyn DaemonSink) -> Self {
        self.echo = Some(to);
        self
    }

    /// The notice `e` calls for, with its provider. `None` for any other event, or one the
    /// settings turn off.
    fn notice_for(&self, e: &DaemonEvent) -> Option<(ProviderId, Notice)> {
        let DaemonEvent::Auto(e) = e else {
            return None;
        };
        let (provider, kind) = match e {
            AutoEvent::Switch {
                provider,
                dry_run: false,
                ..
            } => (provider, NotifyEvent::Switch),
            AutoEvent::AccountQuarantined { provider, .. } => (provider, NotifyEvent::Quarantine),
            _ => return None,
        };
        let (settings, _) = Settings::load(self.engine.env(), provider);
        if !wanted(&settings, kind) {
            return None;
        }
        let body = match e {
            AutoEvent::Switch {
                trigger, from, to, ..
            } => {
                let left = self.row(provider, *from);
                let why = self.why_switched(*trigger, left.clone(), &settings.models);
                let to_row = self.row(provider, *to);
                format!(
                    "Switched {} → {} ({why})",
                    named(left.as_ref(), *from, None),
                    named(to_row.as_ref(), *to, None)
                )
            }
            AutoEvent::AccountQuarantined {
                number,
                email,
                reason,
                ..
            } => {
                let row = self.row(provider, *number);
                format!(
                    "{} needs a new login ({reason})",
                    named(row.as_ref(), *number, Some(email))
                )
            }
            _ => return None,
        };
        let notice = Notice {
            title: TITLE.to_owned(),
            body,
        };
        Some((provider.clone(), notice))
    }

    /// The account at `position`, as the store now holds it.
    fn row(&self, provider: &ProviderId, position: u32) -> Option<AccountRow> {
        self.engine
            .existing_store()
            .ok()??
            .find_by_position(provider, position)
            .ok()?
    }

    /// Why a switch was made, as its notice says it. Normally this is the binding window of the
    /// account it left (`5h at 92%`), from that account's decision-grade reading (§8.4). A
    /// failover names its trigger instead: the account it left was not at its limit. So does a
    /// switch whose account has no decision-grade reading.
    fn why_switched(
        &self,
        trigger: Trigger,
        left: Option<AccountRow>,
        models: &[String],
    ) -> String {
        let binding = left
            .filter(|_| trigger != Trigger::Failover)
            .and_then(|row| {
                let usage = self.engine.account_view(row, false).usage;
                let grade = usage.decision_grade;
                let windows: Vec<Window> = usage
                    .windows
                    .filter(|_| grade)?
                    .into_iter()
                    .map(|(w, _)| w)
                    .collect();
                binding_window(&windows, models).map(window_at)
            });
        binding.unwrap_or_else(|| trigger.as_str().to_owned())
    }
}

impl DaemonSink for NotifySink<'_> {
    fn emit(&self, e: &DaemonEvent) {
        if let Some((provider, notice)) = self.notice_for(e) {
            if let Some(echo) = self.echo {
                echo.notified(&provider, &notice);
            }
            self.queue.send(notice);
        }
    }
}

/// D6.1 `limit`: the provider's managed live account, when its binding relevant window (the
/// one with the highest pct) is at or above `autoswitch.threshold`. `None` covers several
/// cases:
/// - notices are off, or the provider's `notify.events` leaves out `limit` (D6.4)
/// - there is no managed live account, or no decision-grade reading (Task 3's `live_reading`)
/// - the live login cannot be read (`EngineError::Unreadable`), logged at DEBUG: a tick
///   reports that itself, and the run goes on
/// - the window's reset has passed
/// - this window instance was notified before
///
/// D6.3's marker is the lease `notify:limit:<account id>:<window key>`. It is taken before the
/// notice is returned, and expires 60 s after the reset (an hour from now when the reset is
/// unknown), so it holds across a restart. The name carries no reset time: a window's
/// `resets_at` drifts by seconds between polls, and the expiry alone bounds the window
/// instance.
pub fn limit_notice(
    engine: &Engine,
    provider: &ProviderId,
    settings: &Settings,
    now_ms: i64,
) -> Result<Option<Notice>, EngineError> {
    if !wanted(settings, NotifyEvent::Limit) {
        return Ok(None);
    }
    let live = match engine.live_reading(provider, &settings.models) {
        Ok(Some(live)) => live,
        Ok(None) => return Ok(None),
        Err(e @ EngineError::Unreadable(_)) => {
            // §14.2: the kind only; the error's text names a path.
            tracing::debug!(
                provider = %provider,
                kind = e.kind(),
                "the live login could not be read, so its limit was not judged"
            );
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let Some(window) = binding_window(&live.windows, &settings.models) else {
        return Ok(None);
    };
    let now_s = now_ms.div_euclid(1000);
    if window.pct < settings.threshold || window.resets_at.is_some_and(|at| at <= now_s) {
        return Ok(None);
    }
    let Some(store) = engine.existing_store()? else {
        return Ok(None);
    };
    let lease = format!("notify:limit:{}:{}", live.account, window.key);
    let expires_ms = window
        .resets_at
        .map_or(now_ms + UNKNOWN_RESET_MS, |at| at * 1000 + AFTER_RESET_MS);
    let holder = uuid::Uuid::now_v7().to_string();
    if !store.take_lease(&lease, &holder, now_ms, expires_ms)? {
        return Ok(None);
    }
    let name = account(
        live.position,
        live.alias.as_deref(),
        live.email.as_deref().unwrap_or(&live.label),
    );
    let body = match window.resets_at {
        Some(at) => format!(
            "{name}: {} (resets in {})",
            window_at(window),
            span(at - now_s)
        ),
        None => format!("{name}: {}", window_at(window)),
    };
    Ok(Some(Notice {
        title: TITLE.to_owned(),
        body,
    }))
}

/// D6.1 `limit`, after a provider job's run (`ProviderJob::after_run_notices`). The limit is
/// judged, queued and announced to `sink`, unless the run switched or notices cannot be sent
/// (`queue` is `None`). A failure to judge is logged and never fails the run.
pub(crate) fn after_run(
    engine: &Engine,
    provider: &ProviderId,
    settings: &Settings,
    queue: Option<&NoticeQueue>,
    sink: &dyn DaemonSink,
    switched: bool,
) {
    let Some(queue) = queue else {
        return;
    };
    if switched {
        return;
    }
    match limit_notice(engine, provider, settings, engine.now_ms()) {
        Ok(Some(notice)) => {
            sink.notified(provider, &notice);
            queue.send(notice);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(
            provider = %provider,
            kind = e.kind(),
            "could not judge whether the live account is at its limit"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use tagteam_provider::process::{Captured, ScriptedSpawner};

    use super::*;
    use crate::events::Fanout;
    use crate::provider_job::{ProviderJob, ProviderJobCtx};
    use crate::scheduler::{Job, JobCx};
    use crate::testutil::{
        Emitted, Fx, LONG_RESETS, T0, board, capture_logs, cc, job_ctx, windows, within,
    };

    /// What no notice of the fixture may carry (D6.2): either email whole, their domain, the
    /// organization's name, and either account's tokens (`at-<email>`, `rt-<email>`). A token is
    /// matched with its email's start, since a bare `at-` is also inside the trigger `at-limit`.
    const NEVER: [&str; 8] = [
        "w@b.example",
        "a@b.example",
        "b.example",
        "Acme",
        "at-w@",
        "rt-w@",
        "at-a@",
        "rt-a@",
    ];

    fn clean(body: &str) {
        for never in NEVER {
            assert!(!body.contains(never), "{never:?} is in {body:?}");
        }
    }

    fn notice(body: &str) -> Notice {
        Notice {
            title: "tagteam".into(),
            body: body.into(),
        }
    }

    fn exited(code: i32) -> Captured {
        Captured::Exited {
            code: Some(code),
            signal: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    /// `work` (w@b.example) at position 1 and a@b.example at 2, which is live. Both are in the
    /// organization "Acme". The live account is at 93 % of its 7d window, `work` at 20 %, both
    /// read now.
    fn two_at_the_limit() -> Fx {
        let home = Fx::new();
        let w = home.add_as("w@b.example", "Acme", Some("work"));
        let a = home.add_as("a@b.example", "Acme", None);
        home.read_windows(&w, &windows(10.0, 20.0), T0, T0 + 300);
        home.read_windows(&a, &windows(40.0, 93.0), T0, T0 + 300);
        home
    }

    fn switch(trigger: Trigger, from: u32, to: u32, dry_run: bool) -> AutoEvent {
        AutoEvent::Switch {
            provider: cc(),
            trigger,
            from,
            to,
            warnings: Vec::new(),
            dry_run,
        }
    }

    fn quarantined(number: u32, email: &str) -> AutoEvent {
        AutoEvent::AccountQuarantined {
            provider: cc(),
            number,
            email: email.into(),
            reason: "invalid_grant".into(),
        }
    }

    /// The notices `events` give, through a `NotifySink` over `home`'s engine.
    fn notices_for(home: &Fx, events: &[AutoEvent]) -> Vec<Notice> {
        let (queue, rx) = NoticeQueue::for_test();
        let sink = NotifySink::new(&home.engine, queue);
        for e in events {
            sink.emit(&DaemonEvent::Auto(e.clone()));
        }
        rx.try_iter().collect()
    }

    /// Waits up to 5 s for `spawner` to have recorded `n` specs, and returns them.
    fn delivered(spawner: &ScriptedSpawner, n: usize) -> Vec<SpawnSpec> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let specs = spawner.specs();
            if specs.len() >= n {
                return specs;
            }
            assert!(
                Instant::now() < deadline,
                "{} of {n} notices were delivered",
                specs.len()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn macos_always_has_osascript_and_linux_has_notify_send_only_on_path() {
        // D6.2: macOS's own program, never one on PATH; `notify-send` only where PATH has it.
        assert_eq!(Backend::detect(true, None), Some(Backend::Osascript));
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        assert_eq!(Backend::detect(false, Some(bin.as_os_str())), None);
        assert_eq!(Backend::detect(false, None), None);
        let program = bin.join("notify-send");
        fs::write(&program, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            Backend::detect(false, Some(bin.as_os_str())),
            Some(Backend::NotifySend(program.clone()))
        );
        assert_eq!(
            (
                Backend::Osascript.name(),
                Backend::NotifySend(program).name()
            ),
            ("osascript", "notify-send")
        );
    }

    #[test]
    fn each_backend_passes_the_text_as_argv_never_as_script() {
        // D6.2: quotes, a newline and an emoji in an alias reach the program verbatim, as its
        // last argument. Nothing is quoted or escaped, so nothing can be injected.
        let body = "Switched 1 · \"wo'rk\"\nend 🚀 → 2 · spare (5h at 92%)";
        let args = |spec: &SpawnSpec| {
            spec.args
                .iter()
                .map(|a| a.to_str().unwrap().to_owned())
                .collect::<Vec<String>>()
        };
        let mac = Backend::Osascript.spec("tagteam", body);
        assert_eq!(mac.program, PathBuf::from("/usr/bin/osascript"));
        assert_eq!(
            args(&mac),
            [
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
                "--",
                "tagteam",
                body,
            ]
        );
        let linux =
            Backend::NotifySend(PathBuf::from("/usr/bin/notify-send")).spec("tagteam", body);
        assert_eq!(linux.program, PathBuf::from("/usr/bin/notify-send"));
        assert_eq!(args(&linux), ["--app-name=tagteam", "--", "tagteam", body]);
        for spec in [&mac, &linux] {
            assert!(spec.set.is_empty() && spec.remove.is_empty() && spec.cwd.is_none());
            assert_eq!(spec.args.last().unwrap(), &OsString::from(body));
        }
    }

    #[test]
    fn a_failed_delivery_is_logged_once_at_warn_then_at_debug_and_the_next_still_goes() {
        // D6.2: a delivery that times out never stops the next one. Its failure is a WARN
        // line once and DEBUG after, naming the backend and how it failed, never the text.
        let spawner = Arc::new(ScriptedSpawner::new());
        spawner.push(Captured::TimedOut);
        spawner.push(Captured::Exited {
            code: Some(1),
            signal: None,
            stdout: Vec::new(),
            stderr: b"cannot show 2 \xc2\xb7 bravo".to_vec(),
        });
        spawner.push(exited(0));
        let backend = Backend::NotifySend(PathBuf::from("/usr/bin/notify-send"));
        let bodies = ["1 · alpha", "2 · bravo", "3 · charlie"];
        let ((), lines) = capture_logs(|| {
            let (notifier, queue) = Notifier::start(backend, spawner.clone());
            for body in bodies {
                queue.send(notice(body));
            }
            let specs = delivered(&spawner, 3);
            assert_eq!(spawner.discards(), 3, "every delivery discards its output");
            let last: Vec<&OsString> = specs.iter().map(|s| s.args.last().unwrap()).collect();
            assert_eq!(last, bodies.map(OsString::from).iter().collect::<Vec<_>>());
            notifier.stop();
        });
        let failures: Vec<&str> = lines
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.contains("could not be delivered"))
            .collect();
        assert_eq!(failures.len(), 2, "{lines:#?}");
        assert!(
            failures[0].starts_with("WARN")
                && failures[0].contains("backend=\"notify-send\"")
                && failures[0].contains("error=timed out"),
            "{}",
            failures[0]
        );
        assert!(
            failures[1].starts_with("DEBUG") && failures[1].contains("error=exit 1"),
            "{}",
            failures[1]
        );
        let sent: Vec<&String> = lines
            .iter()
            .filter(|l| l.contains("notification sent"))
            .collect();
        assert_eq!(sent.len(), 1, "{lines:#?}");
        assert!(sent[0].trim_start().starts_with("DEBUG"), "{}", sent[0]);
        for line in &lines {
            for name in ["alpha", "bravo", "charlie"] {
                assert!(!line.contains(name), "a notice's text was logged: {line}");
            }
        }
    }

    #[test]
    fn a_full_queue_drops_a_notice_rather_than_wait() {
        // D4: the scheduler never waits on a notification.
        let (queue, rx) = NoticeQueue::for_test();
        let started = Instant::now();
        for n in 0..40 {
            queue.send(notice(&format!("notice {n}")));
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(rx.try_iter().count(), 32);
    }

    #[test]
    fn a_stopped_notifier_delivers_nothing_more() {
        // D6.2: what is sent once the notifier has stopped is dropped.
        let spawner = Arc::new(ScriptedSpawner::new());
        let (notifier, queue) = Notifier::start(Backend::Osascript, spawner.clone());
        notifier.stop();
        queue.send(notice("too late"));
        thread::sleep(Duration::from_millis(200));
        assert!(spawner.specs().is_empty());
    }

    #[test]
    fn a_notifier_dropped_while_its_queue_lives_stops_at_once() {
        // H8, D6: the thread blocks in `recv`, and `Notifier::drop` wakes it with its stop
        // message. `Started` drops its `Notifier` before the queue beside it (`daemon.rs:247`), so
        // each queue is held across its notifier's drop. Ten drops took about a second while the
        // thread polled every 100 ms.
        let took = within("ten notifiers dropped", || {
            let spawner = Arc::new(ScriptedSpawner::new());
            let mut took = Duration::ZERO;
            for _ in 0..10 {
                let (notifier, queue) = Notifier::start(Backend::Osascript, spawner.clone());
                let started = Instant::now();
                drop(notifier);
                took += started.elapsed();
                drop(queue);
            }
            took
        });
        assert!(took < Duration::from_millis(500), "{took:?} for ten drops");
    }

    #[test]
    fn a_switch_and_a_quarantine_are_notified_naming_accounts_by_position_and_alias() {
        // D6.1, D6.2: the daemon's switches (never a dry run) and quarantines. An account is
        // named `<position> · <alias or email's local part>`. A row that is gone falls back to
        // the event's email's local part, or to the position alone.
        let home = two_at_the_limit();
        let got = notices_for(
            &home,
            &[
                switch(Trigger::Proactive, 2, 1, false),
                switch(Trigger::Proactive, 2, 1, true),
                switch(Trigger::Failover, 2, 1, false),
                switch(Trigger::AtLimit, 9, 2, false),
                quarantined(1, "w@b.example"),
                quarantined(9, "gone@b.example"),
                AutoEvent::NoSwitch {
                    provider: cc(),
                    reason: "below-threshold".into(),
                    detail: String::new(),
                },
            ],
        );
        let bodies: Vec<&str> = got.iter().map(|n| n.body.as_str()).collect();
        assert_eq!(
            bodies,
            [
                "Switched 2 · a → 1 · work (7d at 93%)",
                "Switched 2 · a → 1 · work (failover)",
                "Switched 9 → 2 · a (at-limit)",
                "1 · work needs a new login (invalid_grant)",
                "9 · gone needs a new login (invalid_grant)",
            ]
        );
        assert!(got.iter().all(|n| n.title == "tagteam"));
        for body in bodies {
            clean(body);
        }
    }

    #[test]
    fn notify_enabled_and_notify_events_are_read_for_each_event() {
        // D6.4, D7: `notify.enabled` turns every notice off. `notify.events` chooses the kinds,
        // the provider's own table first. A change counts from the next event on.
        let home = two_at_the_limit();
        let events = [
            switch(Trigger::Proactive, 2, 1, false),
            quarantined(1, "w@b.example"),
        ];
        home.settings("[notify]\nenabled = false\n", 1);
        assert!(notices_for(&home, &events).is_empty());
        home.settings("[provider.claude-code.notify]\nevents = [\"limit\"]\n", 2);
        assert!(notices_for(&home, &events).is_empty());
        home.settings(
            "[notify]\nevents = [\"limit\"]\n\n[provider.claude-code.notify]\nevents = [\"quarantine\"]\n",
            3,
        );
        assert_eq!(
            notices_for(&home, &events),
            [notice("1 · work needs a new login (invalid_grant)")]
        );
    }

    #[test]
    fn fanout_passes_each_announced_notice_to_every_sink() {
        // D8.1: `tagteam daemon` prints a line per notification. It is told through
        // `notified`, which `Fanout` hands to each of its sinks.
        let home = Fx::new();
        let (one, two) = (Emitted::new(&home.clock), Emitted::new(&home.clock));
        let quarantine = notice("1 · work needs a new login (invalid_grant)");
        Fanout(vec![&one, &two]).notified(&cc(), &quarantine);
        assert_eq!(one.announced(), [(cc(), quarantine.clone())]);
        assert_eq!(two.announced(), [(cc(), quarantine)]);
        assert!(one.events().is_empty() && two.events().is_empty());
    }

    #[test]
    fn an_echoing_sink_announces_each_notice_it_queues() {
        // D8.1: `tagteam daemon` prints a line per notification, so `serve` builds its
        // `NotifySink` echoing to the caller's sink, inside a `Fanout`. The echo is told of the
        // notice, and is never handed the event by this sink.
        let home = two_at_the_limit();
        let (queue, rx) = NoticeQueue::for_test();
        let echo = Emitted::new(&home.clock);
        let sink = NotifySink::new(&home.engine, queue).echoing(&echo);
        Fanout(vec![&sink]).emit(&DaemonEvent::Auto(quarantined(1, "w@b.example")));
        let queued: Vec<Notice> = rx.try_iter().collect();
        assert_eq!(
            queued,
            [notice("1 · work needs a new login (invalid_grant)")]
        );
        assert_eq!(echo.announced(), [(cc(), queued[0].clone())]);
        assert!(
            echo.events().is_empty(),
            "the echo is told of the notice, never handed the event"
        );
    }

    #[test]
    fn the_limit_is_notified_once_per_window_instance_across_a_restart() {
        // D6.1, D6.3: a's 7d window binds at 93 % ≥ 90 %. The lease marks this window instance
        // in the store. A second call does not notify it again, and neither does a restarted
        // daemon's engine. The window's next instance is notified.
        let home = two_at_the_limit();
        let s = Settings::default();
        let first = limit_notice(&home.engine, &cc(), &s, T0 * 1000).unwrap();
        assert_eq!(first, Some(notice("2 · a: 7d at 93% (resets in 3d09h)")));
        clean(&first.unwrap().body);
        assert_eq!(
            limit_notice(&home.engine, &cc(), &s, T0 * 1000).unwrap(),
            None
        );
        assert_eq!(
            limit_notice(&home.other(), &cc(), &s, T0 * 1000 + 60_000).unwrap(),
            None
        );
        // The window reset and the marker ran out (60 s after the reset), and its next
        // instance is at 95 % already.
        let later = LONG_RESETS + 61;
        home.clock.set(later * 1000);
        let a = home
            .engine
            .store()
            .unwrap()
            .find_by_position(&cc(), 2)
            .unwrap()
            .unwrap()
            .id;
        let mut next = windows(40.0, 95.0);
        next[1].resets_at = Some(LONG_RESETS + 604_800);
        home.read_windows(&a, &next, later, later + 300);
        assert_eq!(
            limit_notice(&home.engine, &cc(), &s, later * 1000).unwrap(),
            Some(notice("2 · a: 7d at 95% (resets in 6d23h)"))
        );
    }

    #[test]
    fn a_resets_at_that_drifts_by_a_second_is_the_same_window_instance() {
        // D6.3: polls of one window differ in `resets_at` by a second or so
        // (`INSTANCE_SLACK_S`), so the marker's name carries no reset time. Its expiry, 60 s
        // after the reset, ends the instance.
        let home = two_at_the_limit();
        let s = Settings::default();
        let a = home
            .engine
            .store()
            .unwrap()
            .find_by_position(&cc(), 2)
            .unwrap()
            .unwrap()
            .id;
        assert_eq!(
            limit_notice(&home.engine, &cc(), &s, T0 * 1000).unwrap(),
            Some(notice("2 · a: 7d at 93% (resets in 3d09h)"))
        );
        let later = T0 + 300;
        home.clock.set(later * 1000);
        let mut drifted = windows(40.0, 94.0);
        drifted[1].resets_at = Some(LONG_RESETS + 1);
        home.read_windows(&a, &drifted, later, later + 300);
        assert_eq!(
            limit_notice(&home.engine, &cc(), &s, later * 1000).unwrap(),
            None,
            "a reset 1 s later is the same instance"
        );
        // A new instance once the marker has expired, 60 s after the reset.
        let next_at = LONG_RESETS + 61;
        home.clock.set(next_at * 1000);
        let mut next = windows(40.0, 95.0);
        next[1].resets_at = Some(LONG_RESETS + 604_800);
        home.read_windows(&a, &next, next_at, next_at + 300);
        assert_eq!(
            limit_notice(&home.engine, &cc(), &s, next_at * 1000).unwrap(),
            Some(notice("2 · a: 7d at 95% (resets in 6d23h)"))
        );
    }

    #[test]
    fn below_the_threshold_with_notices_off_or_without_a_managed_live_account_nothing_is_taken() {
        // D6.1, D6.4: nothing is notified, and no marker is taken that would hide a later
        // notice.
        let home = two_at_the_limit();
        let off = Settings {
            notify_enabled: false,
            ..Settings::default()
        };
        let no_limit = Settings {
            notify_events: vec![NotifyEvent::Switch, NotifyEvent::Quarantine],
            ..Settings::default()
        };
        let higher = Settings {
            threshold: 95.0,
            ..Settings::default()
        };
        for s in [&off, &no_limit, &higher] {
            assert_eq!(
                limit_notice(&home.engine, &cc(), s, T0 * 1000).unwrap(),
                None
            );
        }
        assert!(
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000)
                .unwrap()
                .is_some(),
            "none of the above took the marker"
        );
        // A live login tagteam does not manage.
        let home = two_at_the_limit();
        home.login_as("stranger@b.example", "Acme");
        assert_eq!(
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000).unwrap(),
            None
        );
    }

    #[test]
    fn a_window_whose_reset_has_passed_is_not_notified() {
        let home = Fx::new();
        let a = home.add_as("a@b.example", "Acme", None);
        let mut stale = windows(40.0, 93.0);
        stale[1].resets_at = Some(T0 - 1);
        home.read_windows(&a, &stale, T0, T0 + 300);
        assert_eq!(
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000).unwrap(),
            None
        );
    }

    #[test]
    fn an_unreadable_live_login_is_no_notice_and_takes_no_marker() {
        // Task 3's `live_reading` fails for a live identity it cannot read, as a tick does. The
        // limit is then not judged: one DEBUG line, no notice and no marker, and no error.
        let home = two_at_the_limit();
        let config = home.env.home.join(".claude.json");
        let readable = fs::read(&config).unwrap();
        fs::write(&config, "not json").unwrap();
        let (judged, lines) =
            capture_logs(|| limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000));
        assert_eq!(judged.unwrap(), None);
        let said: Vec<&str> = lines
            .iter()
            .map(|l| l.trim_start())
            .filter(|l| l.contains("the live login could not be read"))
            .collect();
        assert_eq!(said.len(), 1, "{lines:#?}");
        assert!(said[0].starts_with("DEBUG"), "{}", said[0]);
        fs::write(&config, readable).unwrap();
        assert_eq!(
            limit_notice(&home.engine, &cc(), &Settings::default(), T0 * 1000).unwrap(),
            Some(notice("2 · a: 7d at 93% (resets in 3d09h)")),
            "no marker was taken"
        );
    }

    #[test]
    fn a_run_that_switched_or_has_no_queue_judges_no_limit() {
        // D6.1: the limit is judged only after a run that did not switch, and only when
        // notices can be sent. Neither takes the marker.
        let home = two_at_the_limit();
        let s = Settings::default();
        let (queue, rx) = NoticeQueue::for_test();
        let echo = Emitted::new(&home.clock);
        after_run(&home.engine, &cc(), &s, Some(&queue), &echo, true);
        after_run(&home.engine, &cc(), &s, None, &echo, false);
        assert_eq!(rx.try_iter().count(), 0);
        assert!(echo.announced().is_empty());
        after_run(&home.engine, &cc(), &s, Some(&queue), &echo, false);
        let got: Vec<Notice> = rx.try_iter().collect();
        assert_eq!(got, [notice("2 · a: 7d at 93% (resets in 3d09h)")]);
        assert_eq!(echo.announced(), [(cc(), got[0].clone())]);
    }

    /// A provider job of `home`'s Claude Code, its notices into `queue`.
    fn job(home: &Fx, queue: NoticeQueue) -> ProviderJob<'_> {
        let ctx = ProviderJobCtx {
            notices: Some(queue),
            ..job_ctx(&board())
        };
        ProviderJob::new(&home.engine, cc(), ctx)
    }

    fn run_once(home: &Fx, job: &mut ProviderJob<'_>) {
        let sink = Emitted::new(&home.clock);
        let mut jitter = || 0.0;
        job.run(&mut JobCx {
            engine: &home.engine,
            sink: &sink,
            jitter: &mut jitter,
        });
    }

    #[test]
    fn the_limit_is_notified_after_a_collect_round_and_after_a_tick_that_did_not_switch() {
        // D6.1 in each mode (D14). One switchable account is `collect` (D4.3). Two are
        // `auto`, where `work` at 97 % has less headroom than a at 93 %, so the tick stays.
        let home = Fx::new();
        let a = home.add_as("a@b.example", "Acme", None);
        home.read_windows(&a, &windows(40.0, 93.0), T0, T0 + 300);
        let (queue, rx) = NoticeQueue::for_test();
        let mut collecting = job(&home, queue);
        run_once(&home, &mut collecting);
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [notice("1 · a: 7d at 93% (resets in 3d09h)")]
        );
        home.clock.advance_ms(60_000);
        run_once(&home, &mut collecting);
        assert_eq!(rx.try_iter().count(), 0, "once per window instance");

        let home = Fx::new();
        let w = home.add_as("w@b.example", "Acme", Some("work"));
        let a = home.add_as("a@b.example", "Acme", None);
        home.read_windows(&w, &windows(10.0, 97.0), T0, T0 + 300);
        home.read_windows(&a, &windows(40.0, 93.0), T0, T0 + 300);
        let (queue, rx) = NoticeQueue::for_test();
        let mut ticking = job(&home, queue);
        run_once(&home, &mut ticking);
        assert_eq!(
            home.engine.store().unwrap().active(&cc()).unwrap(),
            Some(a),
            "the tick stayed"
        );
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [notice("2 · a: 7d at 93% (resets in 3d09h)")]
        );
    }
}
