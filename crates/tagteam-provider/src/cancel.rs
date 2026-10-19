//! §14.1's cancel token: what a caught signal leaves for the next cancellation point.

use std::cell::RefCell;
use std::marker::PhantomData;
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
    /// for every clone. Only `tagteam run`, and the TUI around the `tagteam run` it starts, take
    /// (M4b Decision 1, TUI spec §7.3):
    /// - `run`'s last look before the spawn, where a signal ends the launch;
    /// - its wait loop, which forwards what arrived while `claude` runs; the TUI's wait for
    ///   `tagteam run` is the same loop;
    /// - once `claude` has exited, to clear what forwarding left;
    /// - `abandon`, before a refused or interrupted launch's exit handling.
    ///
    /// So the exit handling's cancellation points see only new signals. The TUI spends only a
    /// SIGINT, with `take_if`. Everything else only reads.
    pub fn take(&self) -> Option<i32> {
        match self.signal.swap(0, Ordering::SeqCst) {
            0 => None,
            n => Some(n as i32),
        }
    }

    /// Consumes the recorded signal only if it is `signal`, in one step: true when it was. A
    /// signal that replaced it, even an instant before, stays recorded. The TUI spends a
    /// foreground action's SIGINT this way, so a SIGTERM or SIGHUP that arrives with it still
    /// ends the TUI (TUI spec §7.3).
    pub fn take_if(&self, signal: i32) -> bool {
        debug_assert!(signal > 0, "signal numbers are positive");
        self.signal
            .compare_exchange(signal as usize, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
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

thread_local! {
    /// The token of the innermost scope on this thread: `None` outside any scope, and inside an
    /// `uninterruptible` one.
    static SCOPE: RefCell<Option<Cancel>> = const { RefCell::new(None) };
}

/// H4, §14.1: a scope on this thread, alive until the guard is dropped. A Keychain read on this
/// thread is a cancellation point only while an `interruptible` scope lives; a write never is,
/// because it runs inside an `uninterruptible` one. Dropping the guard puts back the scope it
/// replaced, so nested guards dropped in reverse order, as `let` bindings are, restore each one.
/// It never leaves its thread.
#[must_use]
pub struct Interruptible {
    replaced: Option<Cancel>,
    _thread: PhantomData<*const ()>,
}

/// Opens a scope in which this thread's Keychain reads take `cancel` (the same shared token).
pub fn interruptible(cancel: &Cancel) -> Interruptible {
    enter(Some(cancel.clone()))
}

/// Opens a scope with no token: nothing inside it is a cancellation point, whatever scope
/// encloses it. Keychain writes run in one.
pub(crate) fn uninterruptible() -> Interruptible {
    enter(None)
}

/// The token of the innermost scope on this thread, if it has one.
pub fn scoped() -> Option<Cancel> {
    SCOPE.with(|s| s.borrow().clone())
}

fn enter(token: Option<Cancel>) -> Interruptible {
    Interruptible {
        replaced: SCOPE.with(|s| s.replace(token)),
        _thread: PhantomData,
    }
}

impl Drop for Interruptible {
    fn drop(&mut self) {
        let replaced = self.replaced.take();
        // Only a guard dropped while this thread's locals are torn down finds the slot gone.
        let _ = SCOPE.try_with(|s| s.replace(replaced));
    }
}

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
    fn take_if_consumes_only_the_signal_it_names_for_every_clone() {
        let c = Cancel::new();
        let clone = c.clone();
        assert!(!c.take_if(2), "nothing is recorded yet");
        clone.request(2);
        assert!(c.take_if(2));
        assert_eq!(clone.requested(), None);
        assert!(!clone.take_if(2), "a signal is taken once");
        c.request(15);
        assert!(!c.take_if(2), "another signal is left as it is");
        assert_eq!(clone.requested(), Some(15));
    }

    #[test]
    fn spending_a_sigint_never_consumes_a_signal_that_replaced_it() {
        // The TUI's interleaving (TUI spec §7.3): a foreground action's SIGINT is seen, then a
        // SIGTERM replaces it before it is spent. The SIGTERM must stay, to end the TUI.
        let c = Cancel::new();
        c.request(2);
        assert_eq!(c.requested(), Some(2));
        c.request(15);
        assert!(!c.take_if(2));
        assert_eq!(c.requested(), Some(15));
    }

    #[test]
    fn an_interruption_names_its_signal() {
        assert_eq!(Interrupted(15).to_string(), "interrupted by signal 15");
    }

    #[test]
    fn a_scope_sets_this_thread_s_token_until_it_is_dropped() {
        let token = Cancel::new();
        assert!(scoped().is_none());
        {
            let _scope = interruptible(&token);
            token.request(2);
            assert_eq!(
                scoped().and_then(|c| c.requested()),
                Some(2),
                "the token itself, shared"
            );
        }
        assert!(scoped().is_none());
    }

    #[test]
    fn nested_scopes_restore_the_one_they_replaced() {
        let (outer, inner) = (Cancel::new(), Cancel::new());
        outer.request(1);
        inner.request(15);
        let _a = interruptible(&outer);
        {
            let _b = interruptible(&inner);
            assert_eq!(scoped().and_then(|c| c.requested()), Some(15));
            {
                let _w = uninterruptible();
                assert!(scoped().is_none(), "a write sees no token");
            }
            assert_eq!(scoped().and_then(|c| c.requested()), Some(15));
        }
        assert_eq!(scoped().and_then(|c| c.requested()), Some(1));
    }

    #[test]
    fn a_scope_belongs_to_the_thread_that_set_it() {
        let token = Cancel::new();
        let _scope = interruptible(&token);
        assert!(std::thread::spawn(|| scoped().is_none()).join().unwrap());
    }
}
