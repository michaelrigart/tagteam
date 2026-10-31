//! D4.2's scheduler: §11.4's loop, generalised from one engine per provider to any jobs. Each
//! pass either runs the job whose wall-clock due time has passed (the earliest; on a tie, the
//! first added) or sleeps a slice of at most a second toward the earliest. The cancel token is
//! checked before each (§14.1), and the wall clock is read again after each slice, so a machine
//! suspended past a due time runs the job as soon as it wakes. `config.toml`'s mtime is checked
//! in every slice and before every job, and every job hears of a change before the next job
//! runs.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use tagteam_engine::Engine;
use tagteam_engine::settings::Settings;

use crate::events::{DaemonSink, StopReason};

/// §11.4: the scheduler checks the cancel token and re-reads the wall clock at least this often.
pub const SLICE_MS: i64 = 1_000;

/// How the scheduler waits between its checks. `ThreadSleeper` in the binary; tests advance a
/// fake wall clock instead, and jump it to model a suspend.
pub trait Sleeper {
    fn sleep(&self, d: Duration);
}

/// The thread's own sleep. Its clock may stop while the machine is suspended, so the scheduler
/// never trusts it for a due time: each slice is at most a second, and the wall clock is read
/// again after it (§11.4).
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The jitter draw: uniform in [-1, 1), which `next_delay` maps to `U(0.9, 1.1)`.
pub fn uniform_jitter() -> f64 {
    fastrand::f64() * 2.0 - 1.0
}

/// What a job runs with.
pub struct JobCx<'a> {
    pub engine: &'a Engine,
    /// Where its events go (D5.5).
    pub sink: &'a dyn DaemonSink,
    /// The jitter draw, in [-1, 1).
    pub jitter: &'a mut dyn FnMut() -> f64,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// Run again this many milliseconds from now.
    After(i64),
    /// Stop the scheduler, for this reason.
    Stop(StopReason),
}

/// One piece of scheduled work: a provider's engine, a watch.
pub trait Job {
    /// How `status` names it (D5.4): `provider:claude-code`, `store-watch`.
    fn name(&self) -> String;
    /// One run; returns its next delay, or a reason to stop the scheduler. A run may block, on a
    /// tick's requests for one: the scheduler runs one job at a time.
    fn run(&mut self, cx: &mut JobCx<'_>) -> Ran;
    /// `config.toml` changed (D4.2); default: nothing.
    fn settings_changed(&mut self, _cx: &mut JobCx<'_>) {}
}

/// A job and when it runs next.
struct Slot<'j> {
    job: Box<dyn Job + 'j>,
    /// The wall-clock time of its next run, epoch milliseconds.
    due_ms: i64,
    /// The delay that due time was set for, which the wait never exceeds.
    delay_ms: i64,
}

/// D4.3: a handle a job keeps to make other jobs due at once. `wake(prefix)` makes every job
/// whose name starts with `prefix` due before the scheduler next sleeps or picks a job; each
/// then runs once and keeps its own cadence after. The store watch wakes `provider:` after a
/// `changed` event, so every provider job re-evaluates its mode (D4.4).
#[derive(Debug, Clone, Default)]
pub struct Wake(Arc<Mutex<Vec<String>>>);

impl Wake {
    pub fn wake(&self, prefix: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(prefix.to_owned());
    }

    /// The prefixes woken since the last call.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The jobs, and when each runs next (D4.2).
#[derive(Default)]
pub struct Scheduler<'j> {
    slots: Vec<Slot<'j>>,
    /// `config.toml`'s mtime as the jobs last read their settings; `None` until it is known.
    settings: Option<Option<SystemTime>>,
    /// What jobs woke since the last pass; every handle `wake()` gave out feeds it.
    wake: Wake,
}

impl<'j> Scheduler<'j> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The mtime `config.toml` had when the jobs read their settings, taken with
    /// `Settings::mtime` before that read, so a write landing in between counts as a change at
    /// the first check rather than going unseen. Without it, the first check takes the mtime it
    /// finds and tells no job.
    pub fn with_settings_mtime(mut self, mtime: Option<SystemTime>) -> Self {
        self.settings = Some(mtime);
        self
    }

    /// Adds a job, first due at `due_ms`, epoch milliseconds.
    pub fn add(&mut self, job: Box<dyn Job + 'j>, due_ms: i64) {
        self.slots.push(Slot {
            job,
            due_ms,
            delay_ms: 0,
        });
    }

    /// The handle jobs wake each other through.
    pub fn wake(&self) -> Wake {
        self.wake.clone()
    }

    /// Every job a `Wake` named since the last pass is due now; a deadline already past keeps
    /// its place.
    fn woken(&mut self, now_ms: i64) {
        for prefix in self.wake.take() {
            for slot in self
                .slots
                .iter_mut()
                .filter(|s| s.job.name().starts_with(&prefix))
            {
                slot.due_ms = slot.due_ms.min(now_ms);
            }
        }
    }

    /// Runs the jobs until one returns `Stop`, or the cancel token is set (`Signal`), which is
    /// checked before every job and in every slice. A signal that lands inside a run is the
    /// job's to end that run at its next cancellation point (§14.1).
    pub fn run(&mut self, cx: &mut JobCx<'_>, sleeper: &dyn Sleeper) -> StopReason {
        let start = cx.engine.now_ms();
        for slot in &mut self.slots {
            slot.delay_ms = (slot.due_ms - start).max(0);
        }
        loop {
            if cx.engine.cancel().requested().is_some() {
                return StopReason::Signal;
            }
            self.check_settings(cx);
            let now_ms = cx.engine.now_ms();
            self.woken(now_ms);
            for slot in &mut self.slots {
                // A wall clock set back would stretch the wait by as much: never wait longer
                // than the delay the due time was set for.
                slot.due_ms = slot.due_ms.min(now_ms + slot.delay_ms);
            }
            let next = (0..self.slots.len()).min_by_key(|&i| self.slots[i].due_ms);
            match next {
                Some(i) if self.slots[i].due_ms <= now_ms => {
                    let slot = &mut self.slots[i];
                    match slot.job.run(cx) {
                        Ran::After(delay_ms) => {
                            slot.delay_ms = delay_ms.max(0);
                            slot.due_ms = cx.engine.now_ms() + slot.delay_ms;
                        }
                        Ran::Stop(reason) => return reason,
                    }
                }
                _ => {
                    let left = next.map_or(SLICE_MS, |i| self.slots[i].due_ms - now_ms);
                    sleeper.sleep(Duration::from_millis(left.min(SLICE_MS) as u64));
                }
            }
        }
    }

    /// D4.2: when `config.toml`'s mtime changed since the jobs last read their settings,
    /// every job hears of it, in the order they were added.
    fn check_settings(&mut self, cx: &mut JobCx<'_>) {
        let mtime = Settings::mtime(cx.engine.env());
        match self.settings {
            Some(seen) if seen == mtime => {}
            None => self.settings = Some(mtime),
            Some(_) => {
                self.settings = Some(mtime);
                for slot in &mut self.slots {
                    slot.job.settings_changed(cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tagteam_provider::Clock;

    use super::*;
    use crate::events::DaemonEvent;
    use crate::testutil::{Fx, Slices, T0};

    /// Drops every event: the toy jobs emit none.
    struct Quiet;

    impl DaemonSink for Quiet {
        fn emit(&self, _: &DaemonEvent) {}
    }

    /// What the toy jobs did, in order: each run and each settings change, with its time in
    /// milliseconds after `T0`.
    type Log = RefCell<Vec<(i64, String)>>;

    /// A job that writes down its runs and runs again every `every_ms`. With `stop_after`, its
    /// run of that number (from 1) stops the scheduler with `Upgrade` instead.
    struct Toy<'a> {
        name: &'static str,
        every_ms: i64,
        stop_after: Option<usize>,
        runs: usize,
        log: &'a Log,
        /// Called once, as its first run starts: to rewrite the settings, or send a signal.
        before_first: Option<Box<dyn FnOnce() + 'a>>,
    }

    impl<'a> Toy<'a> {
        fn new(name: &'static str, every_ms: i64, log: &'a Log) -> Self {
            Toy {
                name,
                every_ms,
                stop_after: None,
                runs: 0,
                log,
                before_first: None,
            }
        }
    }

    fn at(cx: &JobCx<'_>) -> i64 {
        cx.engine.now_ms() - T0 * 1000
    }

    impl Job for Toy<'_> {
        fn name(&self) -> String {
            self.name.to_owned()
        }

        fn run(&mut self, cx: &mut JobCx<'_>) -> Ran {
            if let Some(before) = self.before_first.take() {
                before();
            }
            self.runs += 1;
            self.log.borrow_mut().push((at(cx), self.name.to_owned()));
            if self.stop_after == Some(self.runs) {
                return Ran::Stop(StopReason::Upgrade);
            }
            Ran::After(self.every_ms)
        }

        fn settings_changed(&mut self, cx: &mut JobCx<'_>) {
            let entry = (at(cx), format!("{}: settings", self.name));
            self.log.borrow_mut().push(entry);
        }
    }

    /// Runs `scheduler` over the fixture's engine with no jitter and no events.
    fn run(fx: &Fx, scheduler: &mut Scheduler<'_>, sleeper: &dyn Sleeper) -> StopReason {
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &Quiet,
            jitter: &mut jitter,
        };
        scheduler.run(&mut cx, sleeper)
    }

    fn entries(log: &Log) -> Vec<(i64, String)> {
        log.borrow().clone()
    }

    fn e(at: i64, what: &str) -> (i64, String) {
        (at, what.to_owned())
    }

    #[test]
    fn the_earliest_due_job_runs_first_and_a_tie_goes_to_the_first_added() {
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("late", 10_000, &log)), T0 * 1000 + 2_000);
        scheduler.add(Box::new(Toy::new("tie-1", 4_000, &log)), T0 * 1000);
        scheduler.add(Box::new(Toy::new("tie-2", 4_000, &log)), T0 * 1000);
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |_| {
            if clock.now_ms() >= (T0 + 9) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(
            entries(&log),
            [
                e(0, "tie-1"),
                e(0, "tie-2"),
                e(2_000, "late"),
                e(4_000, "tie-1"),
                e(4_000, "tie-2"),
                e(8_000, "tie-1"),
                e(8_000, "tie-2"),
            ]
        );
        let taken = slices.taken.borrow();
        assert!(taken.iter().all(|d| *d <= Duration::from_secs(1)));
        assert_eq!(taken.iter().sum::<Duration>(), Duration::from_secs(9));
    }

    #[test]
    fn a_clock_set_back_never_stretches_a_wait_past_its_delay() {
        // The clock is set back an hour during the first slice after a run. The next run still
        // comes the 60 s delay after that, by the clock as it now reads, not an hour later.
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("job", 60_000, &log)), T0 * 1000);
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |n| {
            if n == 0 {
                clock.set(clock.now_ms() - 3_600_000);
            } else if clock.now_ms() > (T0 - 3_539) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(entries(&log), [e(0, "job"), e(-3_539_000, "job")]);
        let taken = slices.taken.borrow();
        assert_eq!(
            taken[..61].iter().sum::<Duration>(),
            Duration::from_secs(61)
        );
    }

    #[test]
    fn a_job_that_asks_to_stop_ends_the_run_with_its_reason_at_once() {
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("tick", 1_000, &log)), T0 * 1000);
        let mut watch = Toy::new("watch", 60_000, &log);
        watch.stop_after = Some(1);
        scheduler.add(Box::new(watch), T0 * 1000 + 2_500);
        let slices = Slices::new(&fx.clock, |_| {});
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Upgrade);
        assert_eq!(
            entries(&log),
            [
                e(0, "tick"),
                e(1_000, "tick"),
                e(2_000, "tick"),
                e(2_500, "watch")
            ],
            "nothing runs after the stop"
        );
        assert_eq!(slices.taken.borrow().len(), 3, "and nothing sleeps");
    }

    #[test]
    fn a_set_cancel_token_ends_the_run_with_signal_before_the_next_job() {
        // §14.1: before any job, and between two jobs due at once.
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("never", 1_000, &log)), T0 * 1000);
        fx.engine.cancel().request(libc::SIGINT);
        let none = Slices::none(&fx.clock);
        assert_eq!(run(&fx, &mut scheduler, &none), StopReason::Signal);
        assert!(entries(&log).is_empty());

        let fx = Fx::new();
        let mut scheduler = Scheduler::new();
        let mut signaller = Toy::new("first", 1_000, &log);
        let cancel = fx.engine.cancel().clone();
        signaller.before_first = Some(Box::new(move || cancel.request(libc::SIGTERM)));
        scheduler.add(Box::new(signaller), T0 * 1000);
        scheduler.add(Box::new(Toy::new("second", 1_000, &log)), T0 * 1000);
        let none = Slices::none(&fx.clock);
        assert_eq!(run(&fx, &mut scheduler, &none), StopReason::Signal);
        assert_eq!(entries(&log), [e(0, "first")]);
    }

    #[test]
    fn every_job_hears_of_each_settings_change_once_before_the_next_job_runs() {
        // D4.2: the mtime is checked in every slice. Writing the same mtime again is no
        // change; a later one is.
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new().with_settings_mtime(None);
        scheduler.add(Box::new(Toy::new("a", 100_000, &log)), T0 * 1000);
        scheduler.add(Box::new(Toy::new("b", 100_000, &log)), T0 * 1000);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, |n| match n {
            2 => fx.settings("[autoswitch]\nthreshold = 80\n", 1),
            5 => fx.settings("[autoswitch]\nthreshold = 85\n", 1),
            7 => fx.settings("[autoswitch]\nthreshold = 70\n", 2),
            9 => cancel.request(libc::SIGTERM),
            _ => {}
        });
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(
            entries(&log),
            [
                e(0, "a"),
                e(0, "b"),
                e(3_000, "a: settings"),
                e(3_000, "b: settings"),
                e(8_000, "a: settings"),
                e(8_000, "b: settings"),
            ]
        );
    }

    #[test]
    fn a_change_landing_during_a_job_is_heard_before_the_next_job_runs() {
        // D4.2: a setting takes effect when the running job ends. `writer`'s run rewrites the
        // file, and `reader`, due at the same time, hears of it before it runs.
        let fx = Fx::new();
        let log = Log::default();
        let mut scheduler = Scheduler::new().with_settings_mtime(Settings::mtime(&fx.env));
        let mut writer = Toy::new("writer", 100_000, &log);
        writer.before_first = Some(Box::new(|| {
            fx.settings("[daemon]\nautoswitch = false\n", 1)
        }));
        scheduler.add(Box::new(writer), T0 * 1000);
        scheduler.add(Box::new(Toy::new("reader", 100_000, &log)), T0 * 1000);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, move |_| cancel.request(libc::SIGTERM));
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(
            entries(&log),
            [
                e(0, "writer"),
                e(0, "writer: settings"),
                e(0, "reader: settings"),
                e(0, "reader"),
            ]
        );
    }

    #[test]
    fn without_a_known_mtime_the_first_check_tells_no_job() {
        // A file that was there before the run, and never changes, is no change.
        let fx = Fx::new();
        fx.settings("[autoswitch]\nthreshold = 80\n", 1);
        let log = Log::default();
        let mut scheduler = Scheduler::new();
        scheduler.add(Box::new(Toy::new("job", 100_000, &log)), T0 * 1000);
        let cancel = fx.engine.cancel().clone();
        let slices = Slices::new(&fx.clock, move |n| {
            if n == 3 {
                cancel.request(libc::SIGTERM);
            }
        });
        assert_eq!(run(&fx, &mut scheduler, &slices), StopReason::Signal);
        assert_eq!(entries(&log), [e(0, "job")]);
    }

    #[test]
    fn the_jitter_draw_stays_within_its_range() {
        for _ in 0..1_000 {
            let j = uniform_jitter();
            assert!((-1.0..1.0).contains(&j), "{j}");
        }
    }
}

#[cfg(test)]
mod wake_tests {
    use std::cell::RefCell;

    use tagteam_provider::{Clock, FakeClock};

    use super::*;
    use crate::events::StopReason;
    use crate::testutil::{Emitted, Fx, Slices, T0};

    /// A job that runs every `period_ms`, recording each run, and on its `on`-th run wakes
    /// every job whose name starts with `prefix`.
    struct Every<'a> {
        name: &'static str,
        period_ms: i64,
        clock: &'a FakeClock,
        runs: &'a RefCell<Vec<(&'static str, i64)>>,
        wake: Option<(Wake, usize, &'static str)>,
    }

    impl Job for Every<'_> {
        fn name(&self) -> String {
            self.name.to_owned()
        }

        fn run(&mut self, _cx: &mut JobCx<'_>) -> Ran {
            let mut runs = self.runs.borrow_mut();
            runs.push((self.name, self.clock.now_ms().div_euclid(1000) - T0));
            let mine = runs.iter().filter(|(n, _)| *n == self.name).count();
            if let Some((wake, on, prefix)) = &self.wake
                && mine == *on
            {
                wake.wake(prefix);
            }
            Ran::After(self.period_ms)
        }
    }

    #[test]
    fn a_wake_makes_every_job_it_names_due_before_the_next_sleep() {
        // D4.3: after `changed`, every provider job re-evaluates at once. The jobs a wake
        // names run before the scheduler sleeps again, once, and then keep their own
        // cadence; a job it does not name keeps its deadline.
        let fx = Fx::new();
        let runs = RefCell::new(Vec::new());
        let (clock, cancel) = (fx.clock.clone(), fx.engine.cancel().clone());
        let slices = Slices::new(&fx.clock, move |_| {
            if clock.now_ms() >= (T0 + 10) * 1000 {
                cancel.request(libc::SIGTERM);
            }
        });
        let mut scheduler = Scheduler::new();
        let wake = scheduler.wake();
        let now = fx.engine.now_ms();
        for name in ["provider:a", "provider:b", "upgrade-watch"] {
            let job = Every {
                name,
                period_ms: 60_000,
                clock: &fx.clock,
                runs: &runs,
                wake: None,
            };
            scheduler.add(Box::new(job), now);
        }
        let waker = Every {
            name: "store-watch",
            period_ms: 2_000,
            clock: &fx.clock,
            runs: &runs,
            wake: Some((wake, 3, "provider:")),
        };
        scheduler.add(Box::new(waker), now);
        let events = Emitted::new(&fx.clock);
        let mut jitter = || 0.0;
        let mut cx = JobCx {
            engine: &fx.engine,
            sink: &events,
            jitter: &mut jitter,
        };
        assert_eq!(scheduler.run(&mut cx, &slices), StopReason::Signal);
        let of = |name: &str| -> Vec<i64> {
            runs.borrow()
                .iter()
                .filter(|(n, _)| *n == name)
                .map(|(_, at)| *at)
                .collect()
        };
        assert_eq!(of("provider:a"), [0, 4]);
        assert_eq!(of("provider:b"), [0, 4]);
        assert_eq!(of("upgrade-watch"), [0]);
        assert_eq!(of("store-watch"), [0, 2, 4, 6, 8]);
    }
}
