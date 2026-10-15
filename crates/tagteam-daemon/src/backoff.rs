//! D4.1: the keychain backoff. A provider run whose secret reads ended locked, with none
//! answered, moves the provider's next run later: 60 s, doubling with each such run to at most
//! 15 min. A run with any answered read ends it. A run that made no counted read (nothing was
//! due, or every item was absent) changes nothing. It is not a mode (D4.3), and it carries
//! over a mode change, since the keychain is no less locked.

use tagteam_provider::keychain::ReadTally;

use crate::events::Backoff;

/// The first delay.
const START_MS: i64 = 60_000;
/// The longest.
const CAP_MS: i64 = 900_000;

/// One provider job's keychain backoff.
#[derive(Debug, Default)]
pub struct KeychainBackoff {
    /// Locked runs in a row; 0 while not backing off.
    level: u32,
    /// When the current delay ends, epoch ms.
    until_ms: Option<i64>,
}

impl KeychainBackoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// After a run that made `reads` (D4.1): `Some(delay_ms)` while backing off, `None` when
    /// this run ended the backoff or none was ever started.
    pub fn after_run(&mut self, reads: ReadTally, now_ms: i64) -> Option<i64> {
        if reads.answered > 0 {
            self.level = 0;
            self.until_ms = None;
            return None;
        }
        if reads.looks_locked() {
            self.level = self.level.saturating_add(1);
        }
        if self.level == 0 {
            return None;
        }
        let delay = (START_MS << (self.level - 1).min(5)).min(CAP_MS);
        self.until_ms = Some(now_ms.saturating_add(delay));
        Some(delay)
    }

    /// The backoff as `status` and the `mode` event show it, while it holds.
    pub fn current(&self) -> Option<Backoff> {
        self.until_ms.map(|ms| Backoff {
            until: (ms + 999).div_euclid(1000),
        })
    }

    /// When the current delay ends, epoch ms.
    pub(crate) fn until_ms(&self) -> Option<i64> {
        self.until_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000_000;

    fn locked() -> ReadTally {
        ReadTally {
            answered: 0,
            locked: 2,
        }
    }

    #[test]
    fn a_locked_run_starts_the_backoff_at_60_s_and_each_doubles_it_to_900_s() {
        let mut b = KeychainBackoff::new();
        let mut now = NOW;
        let mut delays = Vec::new();
        for _ in 0..7 {
            let delay = b.after_run(locked(), now).expect("backing off");
            assert_eq!(
                b.current(),
                Some(Backoff {
                    until: (now + delay) / 1000
                })
            );
            assert_eq!(b.until_ms(), Some(now + delay));
            delays.push(delay / 1000);
            now += delay;
        }
        assert_eq!(delays, [60, 120, 240, 480, 900, 900, 900]);
    }

    #[test]
    fn an_answered_read_ends_it_and_a_run_without_counted_reads_leaves_it_as_it_was() {
        let mut b = KeychainBackoff::new();
        b.after_run(locked(), NOW);
        assert_eq!(b.after_run(locked(), NOW + 60_000), Some(120_000));
        // Nothing was due, or every item was absent: the level holds, from now.
        assert_eq!(
            b.after_run(ReadTally::default(), NOW + 180_000),
            Some(120_000)
        );
        assert_eq!(
            b.current(),
            Some(Backoff {
                until: (NOW + 300_000) / 1000
            })
        );
        // One answered read ends it, however many others ended locked.
        let mixed = ReadTally {
            answered: 1,
            locked: 3,
        };
        assert_eq!(b.after_run(mixed, NOW + 300_000), None);
        assert_eq!((b.current(), b.until_ms()), (None, None));
        // The next locked run starts again from 60 s.
        assert_eq!(b.after_run(locked(), NOW + 360_000), Some(60_000));
    }

    #[test]
    fn runs_that_were_answered_or_read_nothing_never_start_one() {
        let mut b = KeychainBackoff::new();
        for reads in [
            ReadTally::default(),
            ReadTally {
                answered: 2,
                locked: 0,
            },
            ReadTally {
                answered: 1,
                locked: 1,
            },
        ] {
            assert_eq!(b.after_run(reads, NOW), None, "{reads:?}");
            assert_eq!(b.current(), None);
        }
    }
}
