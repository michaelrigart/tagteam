use std::sync::{Mutex, MutexGuard, PoisonError};

/// Takes `m`, ignoring poison: a holder that panicked left the value as it was, and a
/// long-lived process must not die of it.
pub fn lock_unpoisoned<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_mutex_still_hands_over_its_value() {
        let m = std::sync::Mutex::new(7);
        let _ = std::thread::scope(|s| {
            s.spawn(|| {
                let _held = m.lock().unwrap();
                panic!("poisoned on purpose");
            })
            .join()
        });
        assert!(m.is_poisoned());
        assert_eq!(*lock_unpoisoned(&m), 7);
    }
}
