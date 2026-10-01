//! Decision-grade trust (§8.4): whether a stored reading may drive a decision or be shown as
//! current usage. Pure; `now_s` is passed in.

/// A reading this young is always trusted.
pub const STALE_OK_S: i64 = 300;
/// Trust extends to this age while failures are being retried, a scheduled plan is in force, or
/// a live lease exists.
pub const TRUST_MAX_AGE_S: i64 = 3600;
/// After a 429, `last_good` is trusted until the earliest relevant reset, but never past
/// `fetched_at` plus this.
pub const POST_429_TRUST_CAP_S: i64 = 7200;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrustInputs {
    pub now_s: i64,
    /// When the reading was taken; `None` when the account has never been read.
    pub fetched_at: Option<i64>,
    pub consecutive_failures: u32,
    /// `next_poll_at > now_s`: a scheduled plan is in force.
    pub plan_in_force: bool,
    /// Another process holds `usage:<id>` right now.
    pub live_lease: bool,
    /// When the last 429's backoff lifts (Decision 2), not when it arrived.
    pub last_429_at: Option<i64>,
    pub earliest_relevant_reset: Option<i64>,
}

/// §8.4: whether the reading may drive a decision (and be shown as `usage`, §13.2).
///
/// - Age ≤ 300 s: trusted. A reading stamped in the future (clock skew) counts as fresh.
/// - Age ≤ 3600 s: trusted while failures are retried, a plan is in force, or a lease is live.
/// - After a 429 (`last_429_at > fetched_at`): trusted until the earliest relevant reset, capped
///   at `fetched_at + 7200 s`, because usage only rises within a window and the old reading is a
///   valid lower bound. With no known reset the cap alone applies.
pub fn decision_grade(t: &TrustInputs) -> bool {
    let Some(fetched_at) = t.fetched_at else {
        return false;
    };
    let age = t.now_s.saturating_sub(fetched_at);
    if age <= STALE_OK_S {
        return true;
    }
    let extended = t.consecutive_failures > 0 || t.plan_in_force || t.live_lease;
    if extended && age <= TRUST_MAX_AGE_S {
        return true;
    }
    if t.last_429_at.is_some_and(|at| at > fetched_at) {
        let cap = fetched_at.saturating_add(POST_429_TRUST_CAP_S);
        let until = t
            .earliest_relevant_reset
            .map_or(cap, |reset| reset.min(cap));
        return t.now_s <= until;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const FETCHED: i64 = 1_000_000;

    /// A reading `age_s` old, with nothing that extends trust.
    fn at_age(age_s: i64) -> TrustInputs {
        TrustInputs {
            now_s: FETCHED + age_s,
            fetched_at: Some(FETCHED),
            consecutive_failures: 0,
            plan_in_force: false,
            live_lease: false,
            last_429_at: None,
            earliest_relevant_reset: None,
        }
    }

    #[test]
    fn the_constants_are_the_specs() {
        assert_eq!(STALE_OK_S, 300);
        assert_eq!(TRUST_MAX_AGE_S, 3600);
        assert_eq!(POST_429_TRUST_CAP_S, 7200);
    }

    #[test]
    fn a_reading_never_taken_is_not_trusted() {
        let t = TrustInputs {
            fetched_at: None,
            plan_in_force: true,
            live_lease: true,
            consecutive_failures: 3,
            ..at_age(0)
        };
        assert!(!decision_grade(&t));
    }

    #[test]
    fn a_reading_up_to_three_hundred_seconds_old_is_trusted() {
        assert!(decision_grade(&at_age(0)));
        assert!(decision_grade(&at_age(300)));
        assert!(!decision_grade(&at_age(301)));
    }

    #[test]
    fn a_reading_from_the_future_counts_as_fresh() {
        assert!(decision_grade(&at_age(-50)));
    }

    #[test]
    fn retried_failures_extend_trust_to_an_hour() {
        let t = |age| TrustInputs {
            consecutive_failures: 1,
            ..at_age(age)
        };
        assert!(decision_grade(&t(301)));
        assert!(decision_grade(&t(3600)));
        assert!(!decision_grade(&t(3601)));
    }

    #[test]
    fn a_plan_in_force_extends_trust_to_an_hour() {
        let t = |age| TrustInputs {
            plan_in_force: true,
            ..at_age(age)
        };
        assert!(decision_grade(&t(3600)));
        assert!(!decision_grade(&t(3601)));
    }

    #[test]
    fn a_live_lease_extends_trust_to_an_hour() {
        let t = |age| TrustInputs {
            live_lease: true,
            ..at_age(age)
        };
        assert!(decision_grade(&t(3600)));
        assert!(!decision_grade(&t(3601)));
    }

    #[test]
    fn a_stale_reading_with_nothing_extending_it_is_not_trusted() {
        assert!(!decision_grade(&at_age(1800)));
    }

    #[test]
    fn after_a_429_trust_lasts_until_the_earliest_relevant_reset() {
        let t = |age| TrustInputs {
            last_429_at: Some(FETCHED + 10),
            earliest_relevant_reset: Some(FETCHED + 5000),
            ..at_age(age)
        };
        assert!(decision_grade(&t(4000)));
        assert!(decision_grade(&t(5000)), "trusted until the reset itself");
        assert!(!decision_grade(&t(5001)));
    }

    #[test]
    fn after_a_429_the_reset_cannot_extend_trust_past_seven_thousand_two_hundred_seconds() {
        let t = |age| TrustInputs {
            last_429_at: Some(FETCHED + 10),
            earliest_relevant_reset: Some(FETCHED + 10_000),
            ..at_age(age)
        };
        assert!(decision_grade(&t(7200)));
        assert!(!decision_grade(&t(7201)));
    }

    #[test]
    fn after_a_429_with_no_known_reset_only_the_cap_applies() {
        let t = |age| TrustInputs {
            last_429_at: Some(FETCHED + 10),
            ..at_age(age)
        };
        assert!(decision_grade(&t(7200)));
        assert!(!decision_grade(&t(7201)));
    }

    #[test]
    fn a_429_that_predates_the_reading_does_not_extend_trust() {
        for last_429_at in [FETCHED - 100, FETCHED] {
            let t = TrustInputs {
                last_429_at: Some(last_429_at),
                earliest_relevant_reset: Some(FETCHED + 5000),
                ..at_age(4000)
            };
            assert!(!decision_grade(&t), "last_429_at = {last_429_at}");
        }
    }

    #[test]
    fn a_reset_already_behind_the_reading_leaves_no_post_429_trust() {
        let t = TrustInputs {
            last_429_at: Some(FETCHED + 10),
            earliest_relevant_reset: Some(FETCHED - 1),
            ..at_age(4000)
        };
        assert!(!decision_grade(&t));
    }

    #[test]
    fn the_post_429_rule_never_shortens_the_other_extensions() {
        let t = TrustInputs {
            last_429_at: Some(FETCHED + 10),
            earliest_relevant_reset: Some(FETCHED + 1000),
            consecutive_failures: 2,
            ..at_age(3000)
        };
        assert!(decision_grade(&t), "failures still extend to an hour");
    }
}
