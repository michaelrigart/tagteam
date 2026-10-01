//! §9.3's usage strategies, decided from each candidate's decision-grade headroom (§8.2,
//! §8.4). Pure: the engine reads the readings and hands in positions and headroom.

use std::cmp::Ordering;

use crate::usage::{Window, is_relevant};

/// One candidate as a usage strategy sees it: its position and its decision-grade headroom
/// (`None`: unknown, §8.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub position: u32,
    pub headroom: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BestOrder {
    /// Positions to try in order: known headroom strictly above `live` (every known one when
    /// `live` is None), most headroom first, ties to the lower position.
    Try(Vec<u32>),
    /// The live headroom is known and no known candidate beats it.
    AlreadyBest,
    /// No candidate has a known headroom.
    UsageUnavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NextAvailable {
    /// `walk` (rotation order) minus candidates whose known headroom is ≤ 0.
    Try { order: Vec<u32>, skipped: Vec<u32> },
    /// Every candidate in a non-empty walk is known to be exhausted.
    Exhausted,
}

/// A headroom a strategy may compare. §8.2's `headroom` is always finite; a NaN reaching here
/// anyway is unknown, never a number to rank by.
fn known(headroom: Option<f64>) -> Option<f64> {
    headroom.filter(|h| !h.is_nan())
}

/// §9.3 `best`: the candidates with a known headroom, most first and ties to the lower
/// position, keeping only those strictly above a known `live` headroom. Unknown candidates are
/// never ordered. No known candidate at all is `UsageUnavailable`, whatever `live` is; a known
/// `live` that none beats is `AlreadyBest`.
pub fn best_order(live: Option<f64>, candidates: &[Candidate]) -> BestOrder {
    let mut ranked: Vec<(u32, f64)> = candidates
        .iter()
        .filter_map(|c| known(c.headroom).map(|h| (c.position, h)))
        .collect();
    if ranked.is_empty() {
        return BestOrder::UsageUnavailable;
    }
    if let Some(live) = known(live) {
        ranked.retain(|&(_, h)| h > live);
        if ranked.is_empty() {
            return BestOrder::AlreadyBest;
        }
    }
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    BestOrder::Try(ranked.into_iter().map(|(position, _)| position).collect())
}

/// §9.3 `next-available`: the rotation's walk, in its order, without the candidates known to
/// be at their limit (headroom ≤ 0). Unknown headroom is never skipped (§8.2). `skipped` keeps
/// the walk's order. `Exhausted` only when the walk had candidates and every one was skipped.
pub fn next_available(walk: &[Candidate]) -> NextAvailable {
    let (skipped, order): (Vec<&Candidate>, Vec<&Candidate>) = walk
        .iter()
        .partition(|c| known(c.headroom).is_some_and(|h| h <= 0.0));
    if order.is_empty() && !skipped.is_empty() {
        return NextAvailable::Exhausted;
    }
    NextAvailable::Try {
        order: order.iter().map(|c| c.position).collect(),
        skipped: skipped.iter().map(|c| c.position).collect(),
    }
}

/// The relevant window with the highest pct (§8.2): what binds the headroom. Ties go to the
/// earlier window in `windows`.
pub fn binding_window<'w>(windows: &'w [Window], models: &[String]) -> Option<&'w Window> {
    windows
        .iter()
        .filter(|w| is_relevant(w, models))
        .fold(None::<&Window>, |best, w| match best {
            Some(b) if b.pct >= w.pct => Some(b),
            _ => Some(w),
        })
}

/// A span of time as tagteam states it: `3d09h`, `2h40m`, `45m`, or `<1m`; a negative span is
/// none. `list`'s countdowns and the strategies' messages share it.
pub fn span(secs: i64) -> String {
    let s = secs.max(0);
    let (days, hours, minutes) = (s / 86_400, s % 86_400 / 3_600, s % 3_600 / 60);
    if days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{WindowKind, headroom};

    fn c(position: u32, headroom: Option<f64>) -> Candidate {
        Candidate { position, headroom }
    }

    fn win(key: &str, label: &str, kind: WindowKind, pct: f64) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct,
            resets_at: None,
            period_s: None,
            detail: None,
        }
    }

    fn models(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn key(w: Option<&Window>) -> Option<&str> {
        w.map(|w| w.key.as_str())
    }

    #[test]
    fn best_orders_the_known_candidates_by_most_headroom() {
        let candidates = [c(1, Some(20.0)), c(2, Some(60.0)), c(3, Some(40.0))];
        assert_eq!(
            best_order(Some(10.0), &candidates),
            BestOrder::Try(vec![2, 3, 1])
        );
    }

    #[test]
    fn best_breaks_ties_to_the_lower_position_whatever_the_input_order() {
        let candidates = [
            c(4, Some(50.0)),
            c(2, Some(50.0)),
            c(3, Some(70.0)),
            c(1, Some(50.0)),
        ];
        assert_eq!(
            best_order(None, &candidates),
            BestOrder::Try(vec![3, 1, 2, 4])
        );
        // Equal as numbers: -0.0 and 0.0 tie.
        assert_eq!(
            best_order(None, &[c(2, Some(0.0)), c(1, Some(-0.0))]),
            BestOrder::Try(vec![1, 2])
        );
    }

    #[test]
    fn best_keeps_only_candidates_strictly_better_than_the_live_account() {
        let candidates = [c(1, Some(30.0)), c(2, Some(30.5)), c(3, Some(29.0))];
        assert_eq!(best_order(Some(30.0), &candidates), BestOrder::Try(vec![2]));
    }

    #[test]
    fn an_unknown_live_headroom_orders_every_known_candidate() {
        let candidates = [c(1, Some(5.0)), c(2, None), c(3, Some(-3.0))];
        assert_eq!(best_order(None, &candidates), BestOrder::Try(vec![1, 3]));
    }

    #[test]
    fn no_known_candidate_is_usage_unavailable_whatever_the_live_headroom() {
        let unknown = [c(1, None), c(2, None)];
        assert_eq!(
            best_order(Some(50.0), &unknown),
            BestOrder::UsageUnavailable
        );
        assert_eq!(best_order(None, &unknown), BestOrder::UsageUnavailable);
        assert_eq!(best_order(Some(50.0), &[]), BestOrder::UsageUnavailable);
    }

    #[test]
    fn a_known_live_headroom_that_no_candidate_beats_is_already_best() {
        let candidates = [c(1, Some(40.0)), c(2, None), c(3, Some(10.0))];
        assert_eq!(
            best_order(Some(40.0), &candidates),
            BestOrder::AlreadyBest,
            "a tie does not beat it"
        );
        assert_eq!(best_order(Some(90.0), &candidates), BestOrder::AlreadyBest);
    }

    #[test]
    fn an_unknown_candidate_is_never_in_the_best_order() {
        let candidates = [c(1, None), c(2, Some(1.0)), c(3, None)];
        for live in [None, Some(0.0)] {
            assert_eq!(
                best_order(live, &candidates),
                BestOrder::Try(vec![2]),
                "{live:?}"
            );
        }
    }

    #[test]
    fn negative_headroom_ranks_below_zero_and_still_beats_a_worse_live_account() {
        let candidates = [c(1, Some(-4.0)), c(2, Some(-10.0)), c(3, Some(0.0))];
        assert_eq!(
            best_order(Some(-5.0), &candidates),
            BestOrder::Try(vec![3, 1])
        );
        assert_eq!(best_order(Some(0.0), &candidates), BestOrder::AlreadyBest);
        assert_eq!(best_order(None, &candidates), BestOrder::Try(vec![3, 1, 2]));
    }

    #[test]
    fn a_nan_headroom_reads_as_unknown_and_never_reaches_an_order() {
        let candidates = [c(1, Some(f64::NAN)), c(2, Some(10.0))];
        assert_eq!(
            best_order(Some(f64::NAN), &candidates),
            BestOrder::Try(vec![2]),
            "a NaN live headroom is unknown"
        );
        assert_eq!(
            best_order(Some(50.0), &[c(1, Some(f64::NAN))]),
            BestOrder::UsageUnavailable
        );
        assert_eq!(
            next_available(&[c(1, Some(f64::NAN))]),
            NextAvailable::Try {
                order: vec![1],
                skipped: vec![]
            },
            "never skipped either"
        );
    }

    #[test]
    fn next_available_skips_only_known_exhausted_candidates_and_keeps_the_walk_order() {
        let walk = [
            c(3, Some(0.0)),
            c(1, None),
            c(2, Some(-4.0)),
            c(5, Some(0.5)),
        ];
        assert_eq!(
            next_available(&walk),
            NextAvailable::Try {
                order: vec![1, 5],
                skipped: vec![3, 2]
            }
        );
    }

    #[test]
    fn next_available_is_exhausted_only_when_every_candidate_is_known_to_be() {
        assert_eq!(
            next_available(&[c(1, Some(0.0)), c(2, Some(-1.0))]),
            NextAvailable::Exhausted
        );
        assert_eq!(
            next_available(&[c(1, Some(0.0)), c(2, None)]),
            NextAvailable::Try {
                order: vec![2],
                skipped: vec![1]
            },
            "an unknown candidate is never skipped"
        );
    }

    #[test]
    fn an_empty_walk_is_an_empty_try_never_exhausted() {
        assert_eq!(
            next_available(&[]),
            NextAvailable::Try {
                order: vec![],
                skipped: vec![]
            }
        );
    }

    #[test]
    fn the_binding_window_is_the_relevant_one_with_the_highest_pct() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 40.0),
            win("7d", "7d", WindowKind::Long, 77.0),
            win("spend", "spend", WindowKind::Spend, 99.0),
            win("scoped:Fable", "Fable", WindowKind::Scoped, 95.0),
        ];
        assert_eq!(
            key(binding_window(&windows, &[])),
            Some("7d"),
            "spend and an unnamed model window never bind"
        );
        assert_eq!(
            key(binding_window(&windows, &models(&["fable"]))),
            Some("scoped:Fable")
        );
        assert_eq!(
            key(binding_window(&windows, &models(&["all"]))),
            Some("scoped:Fable")
        );
        assert_eq!(
            key(binding_window(&windows, &models(&["opus"]))),
            Some("7d")
        );
    }

    #[test]
    fn a_tie_binds_the_earlier_window() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 100.0),
            win("7d", "7d", WindowKind::Long, 100.0),
        ];
        assert_eq!(key(binding_window(&windows, &[])), Some("5h"));
    }

    #[test]
    fn no_relevant_window_binds_nothing() {
        assert_eq!(binding_window(&[], &[]), None);
        let irrelevant = vec![
            win("spend", "spend", WindowKind::Spend, 100.0),
            win("scoped:Fable", "Fable", WindowKind::Scoped, 100.0),
        ];
        assert_eq!(binding_window(&irrelevant, &[]), None);
    }

    #[test]
    fn the_binding_window_is_what_sets_the_headroom() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 104.0),
            win("7d", "7d", WindowKind::Long, 30.0),
            win("scoped:Fable", "Fable", WindowKind::Scoped, 110.0),
        ];
        for m in [models(&[]), models(&["Fable"])] {
            assert_eq!(
                binding_window(&windows, &m).map(|w| 100.0 - w.pct),
                headroom(&windows, &m),
                "{m:?}"
            );
        }
    }

    #[test]
    fn spans_read_as_days_hours_or_minutes() {
        for (secs, text) in [
            (-5, "<1m"),
            (0, "<1m"),
            (59, "<1m"),
            (60, "1m"),
            (3_599, "59m"),
            (3_600, "1h00m"),
            (9_630, "2h40m"),
            (86_399, "23h59m"),
            (86_400, "1d00h"),
            (291_630, "3d09h"),
        ] {
            assert_eq!(span(secs), text, "{secs}");
        }
    }
}
