//! Poll policy (§8.6): when the next usage request for an account should be sent, and whether
//! the hourly budget lets one go out. Ported from cswap's `poll_policy.py`; the hourly budget
//! and the post-jitter floor are tagteam's. Pure: time and jitter are passed in.

/// Growth applied to the previous interval while a reading is not moving (§8.6: `base · 1.5`).
const IDLE_GROWTH: f64 = 1.5;

/// §8.6's constants. Providers return it from `Provider::poll_budget`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollBudget {
    /// No identity is sent more than this many usage requests in any rolling hour.
    pub hourly_requests: u32,
    /// How long a reserved slot stays valid for sending.
    pub slot_valid_s: i64,
    /// The span of `usage_requests` rows that count (the hour plus the slot validity).
    pub count_window_s: i64,
    /// The shortest interval after a fetch, and the serve TTL of a reading.
    pub floor_s: i64,
    /// The active account's interval when it is moving close to the threshold.
    pub urgent_s: i64,
    /// The active account's longest interval; its default is the floor.
    pub active_max_s: i64,
    pub candidate_default_s: i64,
    pub candidate_max_s: i64,
    /// The shortest interval once a window is at its limit.
    pub exhausted_s: i64,
    /// Percentage points that count as movement.
    pub movement_delta: f64,
    /// Jitter as a fraction of the interval (±).
    pub jitter_frac: f64,
    /// The shortest interval after a recent 429.
    pub post_429_min_s: i64,
    /// A 429 is "recent" from when its backoff lifts until this long afterwards.
    pub recent_429_window_s: i64,
    pub post_429_mult: f64,
    pub post_429_max_s: i64,
    /// "Within reach of the threshold" for the urgent interval, in points.
    pub escalation_margin: f64,
    /// How long after a reset the next poll may land.
    pub reset_slack_s: i64,
}

impl PollBudget {
    pub const STANDARD: PollBudget = PollBudget {
        hourly_requests: 20,
        slot_valid_s: 60,
        count_window_s: 3660,
        floor_s: 180,
        urgent_s: 60,
        active_max_s: 300,
        candidate_default_s: 300,
        candidate_max_s: 600,
        exhausted_s: 600,
        movement_delta: 1.0,
        jitter_frac: 0.10,
        post_429_min_s: 360,
        recent_429_window_s: 3600,
        post_429_mult: 1.5,
        post_429_max_s: 1800,
        escalation_margin: 15.0,
        reset_slack_s: 60,
    };

    /// The interval used for an account with no reading to learn from.
    fn default_interval_s(&self, active: bool) -> i64 {
        if active {
            self.floor_s
        } else {
            self.candidate_default_s
        }
    }

    fn ceiling_s(&self, active: bool) -> i64 {
        if active {
            self.active_max_s
        } else {
            self.candidate_max_s
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollInputs {
    pub now_s: i64,
    /// Whether the account is the live one.
    pub active: bool,
    /// Max relevant pct of the new reading.
    pub pct: Option<f64>,
    /// Max relevant pct of the previous reading.
    pub prev_pct: Option<f64>,
    /// `usage_state.poll_interval_s`.
    pub prev_interval_s: Option<i64>,
    /// `autoswitch.threshold`.
    pub threshold: f64,
    /// When the last 429's backoff lifts (Decision 2).
    pub last_429_at: Option<i64>,
    pub next_relevant_reset: Option<i64>,
}

/// `interval_s` is the policy interval before jitter and clamping, which is what the next plan
/// starts from (`usage_state.poll_interval_s`). `next_poll_at` has jitter and clamps applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollPlan {
    pub interval_s: i64,
    pub next_poll_at: i64,
}

fn grow(base_s: i64, mult: f64) -> i64 {
    (base_s as f64 * mult).round() as i64
}

fn jittered(interval_s: i64, jitter: f64, frac: f64) -> i64 {
    let j = if jitter.is_finite() {
        jitter.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (interval_s as f64 * (1.0 + j * frac)).round() as i64
}

/// §8.6 `plan_after_fetch`, with Decision 9's clamps. `jitter` is in [-1, 1].
///
/// 1. Unknown pct: the role's default (180 s active, 300 s candidate). Otherwise movement of at
///    least `movement_delta` points (up or down; a drop is a window reset) gives
///    `max(180, base/2)`, and no movement gives `min(ceiling, max(180, base · 1.5))`, where
///    `base` is the previous interval or the role's default.
/// 2. Urgent (active, moving, within `escalation_margin` of the threshold, no recent 429): 60 s.
/// 3. Recent 429: `min(1800, max(interval, max(base · 1.5, 360)))`. A 429 is recent from when
///    its backoff lifts (or while it has not lifted yet) for `recent_429_window_s`. This applies
///    with an unknown pct too: the rule protects the endpoint, not the reading.
/// 4. Exhausted (pct ≥ 100): at least 600 s.
/// 5. Jitter, then the floor (180 s; 60 s when urgent), then the reset cap: the next poll is
///    `max(now + floor, min(now + interval, reset + 60))`, and a reset at or before `now` is
///    ignored. When the cap and the floor disagree, the floor wins.
pub fn plan_after_fetch(b: &PollBudget, i: &PollInputs, jitter: f64) -> PollPlan {
    let default_s = b.default_interval_s(i.active);
    let ceiling_s = b.ceiling_s(i.active);
    let base = i.prev_interval_s.unwrap_or(default_s);
    let moving = match (i.pct, i.prev_pct) {
        (Some(p), Some(q)) => (p - q).abs() >= b.movement_delta,
        _ => false,
    };
    let recent_429 = i
        .last_429_at
        .is_some_and(|at| at > i.now_s || i.now_s - at <= b.recent_429_window_s);

    let mut urgent = false;
    let mut interval = match i.pct {
        None => default_s,
        Some(p) => {
            let mut v = if moving {
                (base / 2).max(b.floor_s)
            } else {
                grow(base, IDLE_GROWTH).max(b.floor_s).min(ceiling_s)
            };
            if i.active && moving && !recent_429 && p >= i.threshold - b.escalation_margin {
                urgent = true;
                v = b.urgent_s;
            }
            v
        }
    };
    if recent_429 {
        let grown = grow(base, b.post_429_mult).max(b.post_429_min_s);
        interval = interval.max(grown).min(b.post_429_max_s);
    }
    if i.pct.is_some_and(|p| p >= 100.0) {
        interval = interval.max(b.exhausted_s);
    }

    let floor_s = if urgent { b.urgent_s } else { b.floor_s };
    let mut next = i
        .now_s
        .saturating_add(jittered(interval, jitter, b.jitter_frac));
    if let Some(reset) = i.next_relevant_reset.filter(|r| *r > i.now_s) {
        next = next.min(reset.saturating_add(b.reset_slack_s));
    }
    next = next.max(i.now_s.saturating_add(floor_s));
    PollPlan {
        interval_s: interval,
        next_poll_at: next,
    }
}

/// The default plan for an account whose role changed without a fetch (§8.3's post-switch
/// re-plan): the role's default interval, jittered, from `now_s`, and never below the floor.
pub fn replan_for_role(b: &PollBudget, active: bool, now_s: i64, jitter: f64) -> PollPlan {
    let interval = b.default_interval_s(active);
    let wait = jittered(interval, jitter, b.jitter_frac).max(b.floor_s);
    PollPlan {
        interval_s: interval,
        next_poll_at: now_s.saturating_add(wait),
    }
}

/// When a request may next be sent, given the reservation times in `counted_at` (any order):
/// `None` if a slot is free now.
///
/// A row counts while `now_s − at < count_window_s`, so it leaves the hour exactly
/// `count_window_s` after it was reserved, and the answer is that moment for the row whose
/// leaving brings the count under `hourly_requests`. A row stamped in the future (clock skew)
/// still counts.
pub fn budget_next_free(b: &PollBudget, counted_at: &[i64], now_s: i64) -> Option<i64> {
    let mut counted: Vec<i64> = counted_at
        .iter()
        .copied()
        .filter(|at| now_s.saturating_sub(*at) < b.count_window_s)
        .collect();
    let limit = b.hourly_requests as usize;
    if counted.len() < limit {
        return None;
    }
    counted.sort_unstable();
    Some(counted[counted.len() - limit].saturating_add(b.count_window_s))
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: PollBudget = PollBudget::STANDARD;
    const NOW: i64 = 1_000_000;

    /// A candidate account that has not moved: the baseline each test adjusts.
    fn inputs() -> PollInputs {
        PollInputs {
            now_s: NOW,
            active: false,
            pct: Some(50.0),
            prev_pct: Some(50.0),
            prev_interval_s: Some(300),
            threshold: 90.0,
            last_429_at: None,
            next_relevant_reset: None,
        }
    }

    fn active() -> PollInputs {
        PollInputs {
            active: true,
            ..inputs()
        }
    }

    /// The plan's interval and the seconds until the next poll, with no jitter.
    fn plan(i: PollInputs) -> (i64, i64) {
        let p = plan_after_fetch(&B, &i, 0.0);
        (p.interval_s, p.next_poll_at - NOW)
    }

    #[test]
    fn the_standard_budget_is_the_specs_table() {
        assert_eq!(
            B,
            PollBudget {
                hourly_requests: 20,
                slot_valid_s: 60,
                count_window_s: 3660,
                floor_s: 180,
                urgent_s: 60,
                active_max_s: 300,
                candidate_default_s: 300,
                candidate_max_s: 600,
                exhausted_s: 600,
                movement_delta: 1.0,
                jitter_frac: 0.10,
                post_429_min_s: 360,
                recent_429_window_s: 3600,
                post_429_mult: 1.5,
                post_429_max_s: 1800,
                escalation_margin: 15.0,
                reset_slack_s: 60,
            }
        );
    }

    #[test]
    fn an_unknown_pct_uses_the_roles_default() {
        let unknown = |i: PollInputs| PollInputs {
            pct: None,
            prev_pct: None,
            ..i
        };
        assert_eq!(plan(unknown(active())), (180, 180));
        assert_eq!(plan(unknown(inputs())), (300, 300));
        // The previous interval is not consulted.
        let with_prev = PollInputs {
            prev_interval_s: Some(600),
            ..unknown(inputs())
        };
        assert_eq!(plan(with_prev), (300, 300));
    }

    #[test]
    fn movement_of_a_point_halves_the_interval_down_to_the_floor() {
        let moved = |pct| PollInputs {
            pct: Some(pct),
            prev_pct: Some(50.0),
            ..inputs()
        };
        assert_eq!(plan(moved(52.0)), (180, 180), "max(180, 300/2)");
        assert_eq!(plan(moved(51.0)), (180, 180), "exactly one point moves");
        let slow = PollInputs {
            prev_interval_s: Some(600),
            ..moved(51.0)
        };
        assert_eq!(plan(slow), (300, 300), "max(180, 600/2)");
    }

    #[test]
    fn less_than_a_point_is_not_movement() {
        let i = PollInputs {
            pct: Some(50.9),
            ..inputs()
        };
        assert_eq!(plan(i), (450, 450), "no movement: min(600, max(180, 450))");
    }

    #[test]
    fn a_drop_of_a_point_or_more_counts_as_movement() {
        let i = PollInputs {
            pct: Some(10.0),
            prev_pct: Some(50.0),
            ..inputs()
        };
        assert_eq!(plan(i), (180, 180));
    }

    #[test]
    fn without_movement_the_interval_grows_by_half_up_to_the_ceiling() {
        let cand = |prev| PollInputs {
            prev_interval_s: prev,
            ..inputs()
        };
        assert_eq!(plan(cand(Some(300))), (450, 450));
        assert_eq!(plan(cand(Some(450))), (600, 600), "675 capped at 600");
        assert_eq!(plan(cand(None)), (450, 450), "base is the default, 300");
        let act = |prev| PollInputs {
            prev_interval_s: prev,
            ..active()
        };
        assert_eq!(plan(act(Some(180))), (270, 270));
        assert_eq!(plan(act(Some(270))), (300, 300), "405 capped at 300");
        assert_eq!(plan(act(None)), (270, 270), "base is the default, 180");
    }

    #[test]
    fn the_interval_never_falls_below_the_floor() {
        let i = PollInputs {
            prev_interval_s: Some(100),
            ..active()
        };
        assert_eq!(plan(i), (180, 180), "max(180, 150), below the 300 ceiling");
        let moving = PollInputs {
            pct: Some(52.0),
            prev_interval_s: Some(100),
            ..inputs()
        };
        assert_eq!(plan(moving), (180, 180), "max(180, 50)");
    }

    #[test]
    fn a_first_reading_has_no_movement_to_measure() {
        let i = PollInputs {
            prev_pct: None,
            ..inputs()
        };
        assert_eq!(plan(i), (450, 450));
    }

    #[test]
    fn an_active_account_moving_within_fifteen_points_of_the_threshold_is_urgent() {
        let near = |pct, prev| PollInputs {
            pct: Some(pct),
            prev_pct: Some(prev),
            ..active()
        };
        assert_eq!(plan(near(76.0, 74.0)), (60, 60));
        assert_eq!(
            plan(near(75.0, 73.0)),
            (60, 60),
            "threshold − 15 is inclusive"
        );
        assert_eq!(
            plan(near(74.9, 73.5)),
            (180, 180),
            "just out of reach: max(180, 150)"
        );
    }

    #[test]
    fn urgency_needs_movement_an_active_account_and_no_recent_429() {
        let still = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(80.0),
            prev_interval_s: Some(180),
            ..active()
        };
        assert_eq!(plan(still), (270, 270), "not moving");
        let candidate = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(78.0),
            ..inputs()
        };
        assert_eq!(plan(candidate), (180, 180), "a candidate is never urgent");
        let after_429 = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(78.0),
            last_429_at: Some(NOW - 100),
            ..active()
        };
        assert_eq!(plan(after_429), (450, 450), "max(180, max(450, 360))");
    }

    #[test]
    fn urgency_follows_the_threshold_setting() {
        let i = PollInputs {
            pct: Some(40.0),
            prev_pct: Some(38.0),
            threshold: 50.0,
            ..active()
        };
        assert_eq!(plan(i), (60, 60), "40 ≥ 50 − 15");
    }

    #[test]
    fn an_urgent_poll_may_land_below_the_normal_floor() {
        let i = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(78.0),
            ..active()
        };
        let next = |jitter| plan_after_fetch(&B, &i, jitter).next_poll_at - NOW;
        assert_eq!(next(1.0), 66);
        assert_eq!(next(-1.0), 60, "54 is clamped to the urgent floor");
    }

    #[test]
    fn after_a_recent_429_the_interval_grows_to_at_least_the_post_429_minimum() {
        let after = |i: PollInputs| PollInputs {
            last_429_at: Some(NOW - 100),
            ..i
        };
        let prev180 = PollInputs {
            prev_interval_s: Some(180),
            ..active()
        };
        assert_eq!(plan(after(prev180)), (360, 360), "max(270, max(270, 360))");
        assert_eq!(
            plan(after(active())),
            (450, 450),
            "prev 300: max(450, max(450, 360))"
        );
        let prev600 = PollInputs {
            prev_interval_s: Some(600),
            ..inputs()
        };
        assert_eq!(plan(after(prev600)), (900, 900), "max(600, max(900, 360))");
    }

    #[test]
    fn the_post_429_interval_is_capped_at_eighteen_hundred_seconds() {
        let i = PollInputs {
            prev_interval_s: Some(1500),
            last_429_at: Some(NOW - 10),
            ..inputs()
        };
        assert_eq!(plan(i), (1800, 1800));
    }

    #[test]
    fn a_429_counts_as_recent_until_an_hour_after_its_backoff_lifts() {
        let with = |at| PollInputs {
            last_429_at: Some(at),
            prev_interval_s: Some(600),
            ..inputs()
        };
        assert_eq!(plan(with(NOW + 200)).0, 900, "not lifted yet");
        assert_eq!(plan(with(NOW)).0, 900, "just lifted");
        assert_eq!(plan(with(NOW - 3600)).0, 900, "exactly an hour ago");
        assert_eq!(plan(with(NOW - 3601)).0, 600, "no longer recent");
    }

    #[test]
    fn a_recent_429_applies_even_when_the_pct_is_unknown() {
        let i = PollInputs {
            pct: None,
            prev_pct: None,
            last_429_at: Some(NOW - 10),
            ..inputs()
        };
        assert_eq!(plan(i), (450, 450), "max(300, max(450, 360))");
    }

    #[test]
    fn an_exhausted_window_polls_at_most_every_six_hundred_seconds() {
        let at_limit = |i: PollInputs, pct| PollInputs {
            pct: Some(pct),
            prev_pct: Some(pct),
            ..i
        };
        assert_eq!(
            plan(at_limit(inputs(), 100.0)),
            (600, 600),
            "450 raised to 600"
        );
        assert_eq!(
            plan(at_limit(active(), 100.0)),
            (600, 600),
            "270 raised to 600"
        );
        assert_eq!(plan(at_limit(inputs(), 105.0)), (600, 600));
        let just_below = at_limit(inputs(), 99.9);
        assert_eq!(plan(just_below), (450, 450));
    }

    #[test]
    fn an_exhausted_window_that_just_moved_is_not_urgent_for_long() {
        let i = PollInputs {
            pct: Some(100.0),
            prev_pct: Some(95.0),
            ..active()
        };
        assert_eq!(plan(i), (600, 600), "urgent 60, then raised to 600");
    }

    #[test]
    fn exhaustion_does_not_shorten_a_longer_post_429_interval() {
        let i = PollInputs {
            pct: Some(100.0),
            prev_pct: Some(100.0),
            prev_interval_s: Some(600),
            last_429_at: Some(NOW - 10),
            ..inputs()
        };
        assert_eq!(plan(i), (900, 900));
    }

    #[test]
    fn jitter_scales_the_interval_by_up_to_ten_percent() {
        let i = PollInputs {
            pct: None,
            prev_pct: None,
            ..inputs()
        };
        let next = |jitter| plan_after_fetch(&B, &i, jitter);
        assert_eq!(next(1.0).next_poll_at - NOW, 330);
        assert_eq!(next(-1.0).next_poll_at - NOW, 270);
        assert_eq!(next(0.5).next_poll_at - NOW, 315);
        assert_eq!(next(0.37).next_poll_at - NOW, 311, "300 · 1.037 = 311.1");
        assert_eq!(
            next(0.37).interval_s,
            300,
            "the stored interval is pre-jitter"
        );
    }

    #[test]
    fn jitter_outside_minus_one_to_one_is_clamped_and_nan_is_ignored() {
        let i = PollInputs {
            pct: None,
            prev_pct: None,
            ..inputs()
        };
        let next = |jitter| plan_after_fetch(&B, &i, jitter).next_poll_at - NOW;
        assert_eq!(next(5.0), 330);
        assert_eq!(next(-5.0), 270);
        assert_eq!(next(f64::NAN), 300);
        assert_eq!(next(f64::INFINITY), 300);
    }

    #[test]
    fn jitter_never_takes_the_poll_below_the_floor() {
        let unknown = PollInputs {
            pct: None,
            prev_pct: None,
            ..active()
        };
        assert_eq!(plan_after_fetch(&B, &unknown, -1.0).next_poll_at - NOW, 180);
        let moving = PollInputs {
            pct: Some(52.0),
            ..active()
        };
        assert_eq!(plan_after_fetch(&B, &moving, -1.0).next_poll_at - NOW, 180);
    }

    #[test]
    fn the_next_poll_is_no_later_than_a_minute_after_the_next_reset() {
        let i = PollInputs {
            next_relevant_reset: Some(NOW + 200),
            ..inputs()
        };
        assert_eq!(plan(i), (450, 260), "min(450, 200 + 60)");
        let exhausted = PollInputs {
            pct: Some(100.0),
            prev_pct: Some(100.0),
            next_relevant_reset: Some(NOW + 300),
            ..inputs()
        };
        assert_eq!(plan(exhausted), (600, 360));
    }

    #[test]
    fn a_reset_beyond_the_interval_changes_nothing() {
        let i = PollInputs {
            next_relevant_reset: Some(NOW + 1000),
            ..inputs()
        };
        assert_eq!(plan(i), (450, 450));
    }

    #[test]
    fn a_reset_at_or_before_now_is_ignored() {
        for reset in [NOW, NOW - 10] {
            let i = PollInputs {
                next_relevant_reset: Some(reset),
                ..inputs()
            };
            assert_eq!(plan(i), (450, 450), "reset = now {:+}", reset - NOW);
        }
    }

    #[test]
    fn when_the_floor_and_the_reset_cap_disagree_the_floor_wins() {
        let i = PollInputs {
            next_relevant_reset: Some(NOW + 50),
            ..inputs()
        };
        assert_eq!(
            plan(i),
            (450, 180),
            "reset + 60 = 110 is below the 180 floor"
        );
    }

    #[test]
    fn an_urgent_poll_keeps_the_reset_cap_above_its_lower_floor() {
        let i = PollInputs {
            pct: Some(80.0),
            prev_pct: Some(78.0),
            next_relevant_reset: Some(NOW + 1),
            ..active()
        };
        assert_eq!(plan(i), (60, 60), "min(60, 61), floor 60");
    }

    #[test]
    fn a_replan_uses_the_roles_default_interval() {
        let p = replan_for_role(&B, true, NOW, 0.0);
        assert_eq!((p.interval_s, p.next_poll_at), (180, NOW + 180));
        let p = replan_for_role(&B, false, NOW, 0.0);
        assert_eq!((p.interval_s, p.next_poll_at), (300, NOW + 300));
    }

    #[test]
    fn a_replan_is_jittered_and_floored() {
        assert_eq!(replan_for_role(&B, true, NOW, 1.0).next_poll_at, NOW + 198);
        assert_eq!(replan_for_role(&B, false, NOW, 1.0).next_poll_at, NOW + 330);
        assert_eq!(replan_for_role(&B, true, NOW, -1.0).next_poll_at, NOW + 180);
        assert_eq!(
            replan_for_role(&B, false, NOW, -1.0).next_poll_at,
            NOW + 270
        );
    }

    fn ascending(count: i64, newest: i64) -> Vec<i64> {
        (0..count).map(|i| newest - (count - 1 - i) * 10).collect()
    }

    #[test]
    fn a_free_slot_means_the_request_may_go_now() {
        assert_eq!(budget_next_free(&B, &[], NOW), None);
        assert_eq!(budget_next_free(&B, &ascending(19, NOW - 5), NOW), None);
    }

    #[test]
    fn at_twenty_counted_requests_the_next_slot_is_when_the_oldest_leaves() {
        let counted = ascending(20, NOW - 5);
        let oldest = counted[0];
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(oldest + 3660));
    }

    #[test]
    fn a_row_a_full_window_old_no_longer_counts() {
        let mut counted = ascending(19, NOW - 5);
        counted.insert(0, NOW - 3660);
        assert_eq!(
            budget_next_free(&B, &counted, NOW),
            None,
            "NOW − 3660 has left"
        );
        counted[0] = NOW - 3659;
        assert_eq!(
            budget_next_free(&B, &counted, NOW),
            Some(NOW + 1),
            "one second younger still counts"
        );
    }

    #[test]
    fn more_than_twenty_rows_wait_for_enough_of_them_to_leave() {
        let counted = ascending(22, NOW - 5);
        // Three must leave before a slot is free: the third-oldest's exit.
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(counted[2] + 3660));
    }

    #[test]
    fn the_counted_times_may_arrive_in_any_order() {
        let mut counted = ascending(20, NOW - 5);
        let oldest = counted[0];
        counted.reverse();
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(oldest + 3660));
    }

    #[test]
    fn a_row_stamped_in_the_future_still_counts() {
        let mut counted = ascending(19, NOW - 5);
        counted.push(NOW + 100);
        assert_eq!(budget_next_free(&B, &counted, NOW), Some(counted[0] + 3660));
    }
}
