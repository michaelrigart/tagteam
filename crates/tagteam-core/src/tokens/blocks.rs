//! 5-hour blocks (token report §9): windows of one account's usage that never overlap, so each
//! response belongs to exactly one. Observed blocks come from the provider's short-window reset
//! times; inferred blocks cover the rest by ccusage's rule.

/// A block's length: the provider's short window, Claude Code's 5 h.
pub const BLOCK_MS: i64 = 5 * 3_600_000;
/// Reset times this close together are one reset: the API's moves by seconds between readings.
const RESET_SLACK_MS: i64 = 10 * 60_000;
const HOUR_MS: i64 = 3_600_000;
const MINUTE_MS: i64 = 60_000;

/// `[start_ms, end_ms)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan {
    pub start_ms: i64,
    pub end_ms: i64,
    /// From a reset the provider reported, rather than inferred.
    pub observed: bool,
}

/// Token report §9's observed blocks: each distinct reset `r` gives `[r − 5 h, r)`. Resets
/// within ten minutes of the last one kept collapse into the later; in time order, a block
/// starts no earlier than the previous one ends.
pub fn observed_blocks(resets_ms: &[i64]) -> Vec<BlockSpan> {
    let mut sorted = resets_ms.to_vec();
    sorted.sort_unstable();
    let mut resets: Vec<i64> = Vec::with_capacity(sorted.len());
    for r in sorted {
        match resets.last_mut() {
            Some(last) if r.saturating_sub(*last) <= RESET_SLACK_MS => *last = r,
            _ => resets.push(r),
        }
    }
    let mut blocks: Vec<BlockSpan> = Vec::with_capacity(resets.len());
    for r in resets {
        let start = blocks.last().map_or(r.saturating_sub(BLOCK_MS), |prev| {
            r.saturating_sub(BLOCK_MS).max(prev.end_ms)
        });
        if start < r {
            blocks.push(BlockSpan {
                start_ms: start,
                end_ms: r,
                observed: true,
            });
        }
    }
    blocks
}

fn hour_floor(t: i64) -> i64 {
    t.saturating_sub(t.rem_euclid(HOUR_MS))
}

/// Token report §9: the block of each time. `observed` comes from `observed_blocks`, and
/// `times_ms` is sorted. A time inside an observed block is its; any other starts an inferred
/// block at its UTC hour floor, or at the end of the observed block before it if that is
/// later, ending 5 hours after the hour floor or at the next observed block's start if that is
/// earlier. Later times before that end join it. Returns the blocks that hold a time, sorted,
/// and each time's index into them.
pub fn assign_blocks(observed: &[BlockSpan], times_ms: &[i64]) -> (Vec<BlockSpan>, Vec<usize>) {
    let mut blocks: Vec<BlockSpan> = Vec::new();
    let mut of_time = Vec::with_capacity(times_ms.len());
    // The first observed block that has not ended by the time at hand.
    let mut next = 0;
    for &t in times_ms {
        while next < observed.len() && observed[next].end_ms <= t {
            next += 1;
        }
        let block = match (observed.get(next), blocks.last()) {
            (Some(o), _) if o.start_ms <= t => *o,
            (_, Some(last)) if !last.observed && t < last.end_ms => *last,
            _ => {
                let floor = hour_floor(t);
                let after = next.checked_sub(1).map(|i| observed[i].end_ms);
                let before = observed.get(next).map(|o| o.start_ms);
                BlockSpan {
                    start_ms: after.map_or(floor, |end| floor.max(end)),
                    end_ms: before.map_or(floor.saturating_add(BLOCK_MS), |start| {
                        floor.saturating_add(BLOCK_MS).min(start)
                    }),
                    observed: false,
                }
            }
        };
        if blocks.last() != Some(&block) {
            blocks.push(block);
        }
        of_time.push(blocks.len() - 1);
    }
    (blocks, of_time)
}

/// The active block's figures (token report §9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// Since the block's start.
    pub elapsed_s: i64,
    /// Until the block's end.
    pub remaining_s: i64,
    pub tokens_per_min: f64,
    pub cost_nanos_per_hour: Option<u64>,
    /// The block's tokens and cost at its end, at the current rate.
    pub projected_tokens: u64,
    pub projected_cost_nanos: Option<u64>,
}

/// `n × num / den`, saturating at `u64::MAX`.
fn scale(n: u64, num: i64, den: i64) -> u64 {
    let v = u128::from(n) * u128::from(num.max(0) as u64) / u128::from(den.max(1) as u64);
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// The figures of the block `span` at `now`, or `None` unless `start <= now < end`. The rate
/// runs over the time from the block's first response to now, a minute at least, so a first
/// response timestamped after now still gives a finite, non-negative rate.
pub fn project(
    span: &BlockSpan,
    first_response_ms: i64,
    now_ms: i64,
    tokens: u64,
    cost_nanos: Option<u64>,
) -> Option<Projection> {
    if !(span.start_ms <= now_ms && now_ms < span.end_ms) {
        return None;
    }
    let over_ms = now_ms.saturating_sub(first_response_ms).max(MINUTE_MS);
    let remaining_ms = span.end_ms.saturating_sub(now_ms);
    Some(Projection {
        elapsed_s: now_ms.saturating_sub(span.start_ms) / 1000,
        remaining_s: remaining_ms / 1000,
        tokens_per_min: tokens as f64 * MINUTE_MS as f64 / over_ms as f64,
        cost_nanos_per_hour: cost_nanos.map(|c| scale(c, HOUR_MS, over_ms)),
        projected_tokens: tokens.saturating_add(scale(tokens, remaining_ms, over_ms)),
        projected_cost_nanos: cost_nanos.map(|c| c.saturating_add(scale(c, remaining_ms, over_ms))),
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::tokens::test_support::pinned;

    const M: i64 = 60_000;
    const H: i64 = 60 * M;

    /// Epoch milliseconds of a UTC time on 2026-10-14, or of a whole ISO 8601 time.
    fn at(time: &str) -> i64 {
        let iso = if time.len() == 5 {
            format!("2026-10-14T{time}:00Z")
        } else {
            time.to_owned()
        };
        crate::time::parse_iso8601(&iso).unwrap() * 1000
    }

    fn span(start: &str, end: &str, observed: bool) -> BlockSpan {
        BlockSpan {
            start_ms: at(start),
            end_ms: at(end),
            observed,
        }
    }

    #[test]
    fn close_resets_collapse_to_the_latest() {
        // The API's reset time moves by seconds between readings, and can drift further.
        let resets = [
            at("2026-10-14T15:00:00Z"),
            at("2026-10-14T15:00:07Z"),
            at("2026-10-14T14:59:58Z"),
            at("15:09"),
            at("21:00"),
        ];
        assert_eq!(
            observed_blocks(&resets),
            [span("10:09", "15:09", true), span("16:00", "21:00", true)]
        );
        // More than ten minutes apart: two resets, the second clamped to the first's end.
        let apart = [at("15:00"), at("2026-10-14T15:10:01Z")];
        assert_eq!(
            observed_blocks(&apart),
            [
                span("10:00", "15:00", true),
                span("15:00", "2026-10-14T15:10:01Z", true)
            ]
        );
        assert!(observed_blocks(&[]).is_empty());
    }

    #[test]
    fn overlapping_observed_windows_are_clamped() {
        let resets = [at("13:00"), at("10:00")];
        assert_eq!(
            observed_blocks(&resets),
            [span("05:00", "10:00", true), span("10:00", "13:00", true)]
        );
    }

    #[test]
    fn an_inferred_block_starts_after_an_observed_one_in_the_same_hour() {
        let observed = observed_blocks(&[at("10:20")]);
        let times = [
            at("09:00"),
            at("10:40"),
            at("2026-10-14T14:59:59Z"),
            at("15:00"),
        ];
        let (blocks, of) = assign_blocks(&observed, &times);
        assert_eq!(
            blocks,
            [
                span("05:20", "10:20", true),
                span("10:20", "15:00", false),
                span("15:00", "20:00", false),
            ]
        );
        assert_eq!(of, [0, 1, 1, 2]);
    }

    #[test]
    fn an_inferred_block_ends_at_the_next_observed_start() {
        let observed = observed_blocks(&[at("14:00")]);
        let times = [at("06:30"), at("08:59"), at("09:30"), at("14:10")];
        let (blocks, of) = assign_blocks(&observed, &times);
        assert_eq!(
            blocks,
            [
                span("06:00", "09:00", false),
                span("09:00", "14:00", true),
                span("14:00", "19:00", false),
            ]
        );
        assert_eq!(of, [0, 0, 1, 2]);
    }

    #[test]
    fn blocks_without_responses_are_dropped() {
        let observed = observed_blocks(&[at("10:00"), at("20:00")]);
        let (blocks, of) = assign_blocks(&observed, &[at("16:00")]);
        assert_eq!(blocks, [span("15:00", "20:00", true)]);
        assert_eq!(of, [0]);
        assert_eq!(assign_blocks(&observed, &[]), (Vec::new(), Vec::new()));
        // A dropped observed block still bounds the inferred blocks around it.
        let (blocks, _) = assign_blocks(&observed, &[at("03:30"), at("10:30")]);
        assert_eq!(
            blocks,
            [span("03:00", "05:00", false), span("10:00", "15:00", false)]
        );
    }

    #[test]
    fn projection_only_inside_the_active_block() {
        let block = span("10:00", "15:00", false);
        let first = at("11:00");
        let at_now = |now: &str| project(&block, first, at(now), 6_000, Some(1_200_000));
        assert_eq!(at_now("2026-10-14T09:59:59Z"), None, "before it");
        assert_eq!(at_now("15:00"), None, "its end belongs to what follows");
        assert_eq!(
            at_now("12:00"),
            Some(Projection {
                elapsed_s: 2 * 3600,
                remaining_s: 3 * 3600,
                tokens_per_min: 100.0,
                cost_nanos_per_hour: Some(1_200_000),
                projected_tokens: 6_000 + 3 * 60 * 100,
                projected_cost_nanos: Some(1_200_000 + 3 * 1_200_000),
            })
        );
        let unpriced = project(&block, first, at("12:00"), 6_000, None).unwrap();
        assert_eq!(unpriced.cost_nanos_per_hour, None);
        assert_eq!(unpriced.projected_cost_nanos, None);
        // The rate is over a minute at least: a first response just now divides by no zero.
        let fresh = project(&block, at("12:00"), at("12:00"), 600, Some(60)).unwrap();
        assert_eq!(fresh.tokens_per_min, 600.0);
        assert_eq!(fresh.cost_nanos_per_hour, Some(3_600));
    }

    #[test]
    fn a_future_first_response_projects_nothing_negative() {
        let block = span("10:00", "15:00", false);
        let now = at("12:00");
        let p = project(&block, now + 10 * M, now, 1_000, Some(500)).unwrap();
        assert!(p.elapsed_s >= 0 && p.remaining_s >= 0);
        assert_eq!(p.tokens_per_min, 1_000.0, "over the one-minute minimum");
        assert!(p.projected_tokens >= 1_000);
        assert!(p.projected_cost_nanos.unwrap() >= 500);
        // Huge counts saturate instead of overflowing.
        let p = project(&block, now, now, u64::MAX, Some(u64::MAX)).unwrap();
        assert_eq!(p.projected_tokens, u64::MAX);
        assert_eq!(p.projected_cost_nanos, Some(u64::MAX));
        assert_eq!(p.cost_nanos_per_hour, Some(u64::MAX));
    }

    #[test]
    fn extreme_times_never_panic() {
        let observed = observed_blocks(&[i64::MIN, 0, i64::MAX]);
        let (blocks, of) = assign_blocks(&observed, &[i64::MIN, -1, 0, i64::MAX]);
        assert_eq!(of.len(), 4);
        assert!(of.iter().all(|&i| i < blocks.len()));
        for b in &blocks {
            let _ = project(b, i64::MIN, b.start_ms, u64::MAX, Some(u64::MAX));
            let _ = project(b, i64::MAX, b.start_ms, u64::MAX, None);
        }
    }

    proptest! {
        #![proptest_config(pinned(512))]

        /// Token report §9: blocks never overlap, so each response is in exactly one.
        #[test]
        fn every_time_is_in_exactly_one_block(
            resets in prop::collection::vec(0..60 * H, 0..6),
            mut times in prop::collection::vec(0..60 * H, 0..40),
        ) {
            times.sort_unstable();
            let observed = observed_blocks(&resets);
            prop_assert!(observed.windows(2).all(|w| w[0].end_ms <= w[1].start_ms));
            let (blocks, of) = assign_blocks(&observed, &times);
            prop_assert_eq!(of.len(), times.len());
            prop_assert!(blocks.windows(2).all(|w| w[0].end_ms <= w[1].start_ms));
            for (i, b) in blocks.iter().enumerate() {
                prop_assert!(b.start_ms < b.end_ms && b.end_ms - b.start_ms <= BLOCK_MS);
                prop_assert!(of.contains(&i), "a block with no response is dropped");
                if b.observed {
                    prop_assert!(observed.contains(b));
                } else {
                    prop_assert!(observed
                        .iter()
                        .all(|o| o.end_ms <= b.start_ms || b.end_ms <= o.start_ms));
                }
            }
            for (t, &i) in times.iter().zip(&of) {
                prop_assert!(blocks[i].start_ms <= *t && *t < blocks[i].end_ms);
                prop_assert_eq!(
                    blocks.iter().filter(|b| b.start_ms <= *t && *t < b.end_ms).count(),
                    1
                );
            }
        }
    }
}
