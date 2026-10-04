//! §14.1's cancel token: what a caught signal leaves for the next cancellation point.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// §14.1: the signal the process received, shared by every clone. Production registers the
/// CLI's handlers on its cell; tests call `request` directly. `0` in the cell means none.
#[derive(Debug, Clone, Default)]
pub struct Cancel {
    signal: Arc<AtomicUsize>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `signal` (a later one replaces it, as a handler's store does).
    pub fn request(&self, signal: i32) {
        debug_assert!(signal > 0, "signal numbers are positive");
        self.signal.store(signal as usize, Ordering::SeqCst);
    }

    pub fn requested(&self) -> Option<i32> {
        match self.signal.load(Ordering::SeqCst) {
            0 => None,
            n => Some(n as i32),
        }
    }

    /// Consumes the recorded signal (§12.5's forwarding): returns it and leaves the token unset,
    /// for every clone. Only `tagteam run` takes (M4b Decision 1):
    /// - its last look before the spawn, where a signal ends the launch;
    /// - its wait loop, which forwards what arrived while `claude` runs;
    /// - once `claude` has exited, to clear what forwarding left;
    /// - `abandon`, before a refused or interrupted launch's exit handling.
    ///
    /// So the exit handling's cancellation points see only new signals. Everything else only
    /// reads.
    pub fn take(&self) -> Option<i32> {
        match self.signal.swap(0, Ordering::SeqCst) {
            0 => None,
            n => Some(n as i32),
        }
    }

    /// `Err(Interrupted(n))` once a signal is recorded.
    pub fn check(&self) -> Result<(), Interrupted> {
        match self.requested() {
            Some(n) => Err(Interrupted(n)),
            None => Ok(()),
        }
    }

    /// The cell a signal handler stores the signal number into.
    pub fn cell(&self) -> Arc<AtomicUsize> {
        self.signal.clone()
    }
}

/// A cancellation point found the token set (§14.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("interrupted by signal {0}")]
pub struct Interrupted(pub i32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_token_is_unset() {
        let c = Cancel::new();
        assert_eq!(c.requested(), None);
        assert_eq!(c.check(), Ok(()));
    }

    #[test]
    fn every_clone_sees_a_request_and_a_later_one_replaces_it() {
        let c = Cancel::new();
        let clone = c.clone();
        c.request(2);
        assert_eq!(clone.requested(), Some(2));
        assert_eq!(clone.check(), Err(Interrupted(2)));
        clone.request(15);
        assert_eq!(c.requested(), Some(15));
    }

    #[test]
    fn separate_tokens_never_share_a_signal() {
        let (a, b) = (Cancel::new(), Cancel::new());
        a.request(2);
        assert_eq!(b.requested(), None);
    }

    #[test]
    fn a_store_into_the_cell_is_a_request() {
        // What a signal handler does (Task 6): it stores the number and nothing else.
        let c = Cancel::new();
        c.cell().store(1, Ordering::SeqCst);
        assert_eq!(c.requested(), Some(1));
    }

    #[test]
    fn take_returns_the_signal_once_and_clears_it_for_every_clone() {
        let c = Cancel::new();
        let clone = c.clone();
        assert_eq!(c.take(), None, "nothing is recorded yet");
        clone.request(15);
        assert_eq!(c.take(), Some(15));
        assert_eq!(clone.requested(), None);
        assert_eq!(clone.check(), Ok(()));
        assert_eq!(clone.take(), None, "a signal is taken once");
    }

    #[test]
    fn a_signal_after_a_take_is_recorded_again() {
        let c = Cancel::new();
        c.request(2);
        assert_eq!(c.take(), Some(2));
        // What a handler does: it stores into the cell, whatever was taken before.
        c.cell().store(1, Ordering::SeqCst);
        assert_eq!(c.requested(), Some(1));
        assert_eq!(c.take(), Some(1));
    }

    #[test]
    fn an_interruption_names_its_signal() {
        assert_eq!(Interrupted(15).to_string(), "interrupted by signal 15");
    }
}
