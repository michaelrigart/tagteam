//! Pace and projection (§8.7): how fast a window is being used and when it runs out. Pure; every
//! projection is measured from the reading's own `fetched_at`, because `pct` is as of then.

use crate::usage::Window;

/// Samples older than this before the reading are not used for the rate.
const REGRESSION_LOOKBACK_S: i64 = 48 * 3600;
/// The regression needs this many samples of the current window instance...
const REGRESSION_MIN_SAMPLES: usize = 3;
/// ...spanning at least this long.
const REGRESSION_MIN_SPAN_S: i64 = 2 * 3600;
/// Two samples belong to the same window instance when their `resets_at` differ by at most this.
const INSTANCE_SLACK_S: i64 = 60;
/// The average-pace fallback is suppressed until this much of the period has elapsed.
const AVERAGE_MIN_ELAPSED_S: i64 = 86_400;
/// `aheadOfPace` is `pct − expected ≥` this many points.
const AHEAD_MARGIN: f64 = 15.0;

/// How a rate was measured (`projectionMethod` in JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMethod {
    Regression,
    Average,
}

impl ProjectionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectionMethod::Regression => "regression",
            ProjectionMethod::Average => "average",
        }
    }
}

/// One stored reading of one window (`usage_samples`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub fetched_at: i64,
    pub pct: f64,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pace {
    /// Where the window would be if usage were spread evenly over its period.
    pub expected_pct: Option<f64>,
    /// `pct − expected ≥ 15`. Never for windows the average fallback does not cover.
    pub ahead: Option<bool>,
    /// Points per hour.
    pub rate_per_hour: Option<f64>,
    pub method: Option<ProjectionMethod>,
    /// When the window reaches 100 % at that rate, epoch seconds; the reading's own time at 100 %.
    pub exhaustion_at: Option<i64>,
    /// Whether the window reaches its reset without reaching 100 %. `None` without a reset time.
    pub will_last_to_reset: Option<bool>,
}

fn same_instance(sample_reset: Option<i64>, current_reset: Option<i64>) -> bool {
    match (sample_reset, current_reset) {
        (Some(a), Some(b)) => (a - b).abs() <= INSTANCE_SLACK_S,
        (None, None) => true,
        _ => false,
    }
}

/// Points per second over the current window instance (same `resets_at` ± 60 s, or both absent;
/// samples from the 48 h up to and including `fetched_at`): the least-squares slope. `None`
/// unless there are ≥ 3 samples spanning ≥ 2 h and the slope is positive.
pub fn regression_rate(
    samples: &[Sample],
    current_reset: Option<i64>,
    fetched_at: i64,
) -> Option<f64> {
    let since = fetched_at.saturating_sub(REGRESSION_LOOKBACK_S);
    let points: Vec<(i64, f64)> = samples
        .iter()
        .filter(|s| s.fetched_at >= since && s.fetched_at <= fetched_at)
        .filter(|s| same_instance(s.resets_at, current_reset))
        .map(|s| (s.fetched_at, s.pct))
        .collect();
    if points.len() < REGRESSION_MIN_SAMPLES {
        return None;
    }
    let first = points.iter().map(|p| p.0).min()?;
    let last = points.iter().map(|p| p.0).max()?;
    if last - first < REGRESSION_MIN_SPAN_S {
        return None;
    }
    let n = points.len() as f64;
    let xs: Vec<f64> = points.iter().map(|p| (p.0 - first) as f64).collect();
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = xs.iter().map(|x| (x - mean_x).powi(2)).sum();
    let sxy: f64 = xs
        .iter()
        .zip(&points)
        .map(|(x, p)| (x - mean_x) * (p.1 - mean_y))
        .sum();
    let slope = sxy / sxx;
    (slope.is_finite() && slope > 0.0).then_some(slope)
}

/// Seconds of the period already used up: `period − ((reset − fetched_at) mod period)`.
/// `None` (suppressed) when the reset is not ahead of the reading, when a whole number of periods
/// remain (the window has just started), or when less than 24 h have elapsed.
fn elapsed_s(period_s: i64, reset: i64, fetched_at: i64) -> Option<i64> {
    if period_s <= 0 {
        return None;
    }
    let remaining = reset.checked_sub(fetched_at)?;
    if remaining <= 0 {
        return None;
    }
    let into_next = remaining % period_s;
    let elapsed = if into_next == 0 {
        0
    } else {
        period_s - into_next
    };
    (elapsed >= AVERAGE_MIN_ELAPSED_S).then_some(elapsed)
}

/// cswap's average pace for one window, for the kinds and windows it covers: the expected pct
/// and the average rate in points per second.
fn average(w: &Window, fetched_at: i64) -> Option<(f64, f64)> {
    if !w.kind.has_pace() {
        return None;
    }
    let period = w.period_s?;
    let elapsed = elapsed_s(period, w.resets_at?, fetched_at)?;
    let expected = (elapsed as f64 / period as f64 * 100.0).min(100.0);
    Some((expected, w.pct / elapsed as f64))
}

/// §8.7 for one window as read at `fetched_at`. The regression rate comes first, and the average
/// fallback covers `has_pace` kinds with a `period_s`; `expected_pct` and `ahead` only exist for
/// those kinds (never `Short`), even when the rate itself came from the regression.
pub fn pace(w: &Window, fetched_at: i64, samples: &[Sample]) -> Pace {
    let mut out = Pace::default();
    let avg = average(w, fetched_at);
    if let Some((expected, _)) = avg {
        out.expected_pct = Some(expected);
        out.ahead = Some(w.pct - expected >= AHEAD_MARGIN);
    }
    if w.pct >= 100.0 {
        out.exhaustion_at = Some(fetched_at);
    }
    let (rate, method) = match regression_rate(samples, w.resets_at, fetched_at) {
        Some(r) => (r, ProjectionMethod::Regression),
        None => match avg {
            Some((_, r)) => (r, ProjectionMethod::Average),
            None => return out,
        },
    };
    out.rate_per_hour = Some(rate * 3600.0);
    out.method = Some(method);
    out.exhaustion_at = if w.pct >= 100.0 {
        Some(fetched_at)
    } else if rate > 0.0 {
        let seconds = ((100.0 - w.pct) / rate).round() as i64;
        Some(fetched_at.saturating_add(seconds))
    } else {
        None
    };
    out.will_last_to_reset = w.resets_at.map(|reset| {
        let remaining = reset.saturating_sub(fetched_at).max(0) as f64;
        w.pct <= 0.0 || w.pct + rate * remaining <= 100.0
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{Window, WindowKind};

    const F: i64 = 1_800_000_000;
    const HOUR: i64 = 3600;
    const DAY: i64 = 86_400;
    const WEEK: i64 = 604_800;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn window(kind: WindowKind, pct: f64, resets_at: Option<i64>, period_s: Option<i64>) -> Window {
        Window {
            key: "w".into(),
            label: "w".into(),
            kind,
            pct,
            resets_at,
            period_s,
            detail: None,
        }
    }

    fn weekly(pct: f64, remaining_s: i64) -> Window {
        window(WindowKind::Long, pct, Some(F + remaining_s), Some(WEEK))
    }

    fn sample(offset_s: i64, pct: f64, resets_at: Option<i64>) -> Sample {
        Sample {
            fetched_at: F + offset_s,
            pct,
            resets_at,
        }
    }

    /// Four hourly samples ending at `F`, rising by `step` points an hour.
    fn rising(end_pct: f64, step: f64, resets_at: Option<i64>) -> Vec<Sample> {
        (0..4)
            .map(|i| sample(-(3 - i) * HOUR, end_pct - step * (3 - i) as f64, resets_at))
            .collect()
    }

    #[test]
    fn method_names_are_the_json_values() {
        assert_eq!(ProjectionMethod::Regression.as_str(), "regression");
        assert_eq!(ProjectionMethod::Average.as_str(), "average");
    }

    #[test]
    fn the_regression_rate_is_the_least_squares_slope_in_points_per_second() {
        let samples = rising(16.0, 2.0, None);
        let rate = regression_rate(&samples, None, F).unwrap();
        assert!(close(rate, 2.0 / 3600.0), "{rate}");
    }

    #[test]
    fn the_slope_is_a_fit_not_the_two_end_points() {
        // 10, 12, 13, 20 over three hours: slope = 15.5 / 5 = 3.1 pts/h by least squares (the end points alone give 3.33).
        let samples = [
            sample(-3 * HOUR, 10.0, None),
            sample(-2 * HOUR, 12.0, None),
            sample(-HOUR, 13.0, None),
            sample(0, 20.0, None),
        ];
        let rate = regression_rate(&samples, None, F).unwrap();
        assert!(close(rate * 3600.0, 3.1), "{}", rate * 3600.0);
    }

    #[test]
    fn it_needs_three_samples() {
        let samples = [sample(-3 * HOUR, 10.0, None), sample(0, 16.0, None)];
        assert_eq!(regression_rate(&samples, None, F), None);
    }

    #[test]
    fn it_needs_the_samples_to_span_two_hours() {
        let short = [
            sample(-7199, 10.0, None),
            sample(-3600, 12.0, None),
            sample(0, 14.0, None),
        ];
        assert_eq!(regression_rate(&short, None, F), None);
        let enough = [
            sample(-7200, 10.0, None),
            sample(-3600, 12.0, None),
            sample(0, 14.0, None),
        ];
        assert!(regression_rate(&enough, None, F).is_some());
    }

    #[test]
    fn it_needs_a_positive_slope() {
        let flat = [
            sample(-7200, 10.0, None),
            sample(-3600, 10.0, None),
            sample(0, 10.0, None),
        ];
        assert_eq!(regression_rate(&flat, None, F), None);
        let falling = [
            sample(-7200, 14.0, None),
            sample(-3600, 12.0, None),
            sample(0, 10.0, None),
        ];
        assert_eq!(regression_rate(&falling, None, F), None);
    }

    #[test]
    fn only_samples_of_the_current_window_instance_count() {
        let reset = Some(F + 3 * DAY);
        let old_instance = Some(F - 4 * DAY);
        let mut samples = rising(16.0, 2.0, reset);
        // A previous instance with a steep climb that would wreck the fit if it leaked in.
        samples.extend([
            sample(-3 * HOUR, 90.0, old_instance),
            sample(-2 * HOUR, 95.0, old_instance),
            sample(-HOUR, 99.0, old_instance),
        ]);
        let rate = regression_rate(&samples, reset, F).unwrap();
        assert!(close(rate, 2.0 / 3600.0), "{rate}");
    }

    #[test]
    fn a_reset_within_sixty_seconds_is_the_same_instance() {
        let reset = F + 3 * DAY;
        let jittery = [
            sample(-7200, 10.0, Some(reset + 60)),
            sample(-3600, 12.0, Some(reset - 60)),
            sample(0, 14.0, Some(reset)),
        ];
        assert!(regression_rate(&jittery, Some(reset), F).is_some());
        let apart = [
            sample(-7200, 10.0, Some(reset + 61)),
            sample(-3600, 12.0, Some(reset)),
            sample(0, 14.0, Some(reset)),
        ];
        assert_eq!(
            regression_rate(&apart, Some(reset), F),
            None,
            "61 s apart is a different instance, leaving two samples"
        );
    }

    #[test]
    fn windows_without_a_reset_group_with_samples_that_have_none() {
        let mut samples = rising(16.0, 2.0, None);
        samples.push(sample(-HOUR, 99.0, Some(F + DAY)));
        assert!(close(
            regression_rate(&samples, None, F).unwrap(),
            2.0 / 3600.0
        ));
        assert_eq!(regression_rate(&samples, Some(F + DAY), F), None);
    }

    #[test]
    fn only_the_last_forty_eight_hours_before_the_reading_count() {
        let samples = [
            sample(-49 * HOUR, 1.0, None),
            sample(-48 * HOUR - 1, 2.0, None),
            sample(-HOUR, 50.0, None),
            sample(0, 52.0, None),
        ];
        assert_eq!(
            regression_rate(&samples, None, F),
            None,
            "two samples remain, and they span an hour"
        );
        let edge = [
            sample(-48 * HOUR, 10.0, None),
            sample(-24 * HOUR, 20.0, None),
            sample(0, 30.0, None),
        ];
        assert!(close(
            regression_rate(&edge, None, F).unwrap() * 3600.0,
            10.0 / 24.0
        ));
    }

    #[test]
    fn samples_after_the_reading_are_ignored() {
        let mut samples = rising(16.0, 2.0, None);
        samples.push(sample(HOUR, 80.0, None));
        assert!(close(
            regression_rate(&samples, None, F).unwrap(),
            2.0 / 3600.0
        ));
    }

    #[test]
    fn the_average_fallback_follows_cswaps_formulas() {
        // 3 days into a week: elapsed 259 200 s.
        let p = pace(&weekly(45.0, 4 * DAY), F, &[]);
        assert!(
            close(p.expected_pct.unwrap(), 300.0 / 7.0),
            "{:?}",
            p.expected_pct
        );
        assert_eq!(p.ahead, Some(false), "45 − 42.86 < 15");
        assert_eq!(p.method, Some(ProjectionMethod::Average));
        assert!(
            close(p.rate_per_hour.unwrap(), 45.0 / 72.0),
            "{:?}",
            p.rate_per_hour
        );
        // 45 + rate · 4 days = 45 + 60 = 105 > 100: it will not last.
        assert_eq!(p.will_last_to_reset, Some(false));
        // (100 − 45) / rate = 316 800 s.
        assert_eq!(p.exhaustion_at, Some(F + 316_800));
    }

    #[test]
    fn a_window_fifteen_points_ahead_of_its_expected_pct_is_ahead_of_pace() {
        // Halfway through the week the expected pct is exactly 50.
        let half = 302_400;
        assert_eq!(
            pace(&weekly(65.0, half), F, &[]).ahead,
            Some(true),
            "exactly 15"
        );
        assert_eq!(pace(&weekly(64.9, half), F, &[]).ahead, Some(false));
        assert_eq!(
            pace(&weekly(60.0, 4 * DAY), F, &[]).ahead,
            Some(true),
            "60 − 42.86"
        );
    }

    #[test]
    fn the_average_is_suppressed_until_a_day_of_the_period_has_elapsed() {
        let just_under = pace(&weekly(10.0, WEEK - DAY + 1), F, &[]);
        assert_eq!(just_under, Pace::default());
        let exactly = pace(&weekly(10.0, WEEK - DAY), F, &[]);
        assert!(close(exactly.expected_pct.unwrap(), 100.0 / 7.0));
    }

    #[test]
    fn a_window_that_has_just_started_or_just_ended_has_no_average() {
        assert_eq!(
            pace(&weekly(10.0, WEEK), F, &[]),
            Pace::default(),
            "a full period left"
        );
        assert_eq!(pace(&weekly(10.0, 2 * WEEK), F, &[]), Pace::default());
        assert_eq!(
            pace(&weekly(10.0, 0), F, &[]),
            Pace::default(),
            "reset is now"
        );
        assert_eq!(
            pace(&weekly(10.0, -60), F, &[]),
            Pace::default(),
            "reset is past"
        );
    }

    #[test]
    fn expected_pct_tops_out_at_one_hundred() {
        let p = pace(&weekly(100.0, 1), F, &[]);
        assert!(p.expected_pct.unwrap() <= 100.0);
        assert!(p.expected_pct.unwrap() > 99.99);
    }

    #[test]
    fn a_scoped_window_with_a_period_has_the_average_too() {
        let w = window(WindowKind::Scoped, 45.0, Some(F + 4 * DAY), Some(WEEK));
        let p = pace(&w, F, &[]);
        assert_eq!(p.method, Some(ProjectionMethod::Average));
        assert_eq!(p.ahead, Some(false));
    }

    #[test]
    fn no_period_or_no_reset_means_no_average() {
        let scoped_no_period = window(WindowKind::Scoped, 45.0, Some(F + 4 * DAY), None);
        assert_eq!(pace(&scoped_no_period, F, &[]), Pace::default());
        let long_no_reset = window(WindowKind::Long, 45.0, None, Some(WEEK));
        assert_eq!(pace(&long_no_reset, F, &[]), Pace::default());
    }

    #[test]
    fn a_short_window_has_no_average_and_no_expected_pct() {
        let w = window(WindowKind::Short, 40.0, Some(F + 2 * HOUR), Some(18_000));
        assert_eq!(pace(&w, F, &[]), Pace::default());
    }

    #[test]
    fn a_spend_window_has_no_average() {
        let w = window(WindowKind::Spend, 40.0, Some(F + 10 * DAY), Some(30 * DAY));
        assert_eq!(pace(&w, F, &[]), Pace::default());
    }

    #[test]
    fn the_regression_rate_comes_first_and_projections_start_at_the_reading() {
        let reset = Some(F + 90_000);
        let w = window(WindowKind::Short, 40.0, reset, Some(18_000));
        let p = pace(&w, F, &rising(40.0, 2.0, reset));
        assert_eq!(p.method, Some(ProjectionMethod::Regression));
        assert!(
            close(p.rate_per_hour.unwrap(), 2.0),
            "{:?}",
            p.rate_per_hour
        );
        // (100 − 40) points at 2 a point-hour: 30 h after the reading.
        assert_eq!(p.exhaustion_at, Some(F + 30 * HOUR));
        // 40 + 2/h · 25 h = 90 ≤ 100.
        assert_eq!(p.will_last_to_reset, Some(true));
        assert_eq!(
            (p.expected_pct, p.ahead),
            (None, None),
            "never for a short window"
        );
    }

    #[test]
    fn a_window_that_outruns_its_reset_will_not_last() {
        let reset = Some(F + 200_000);
        let w = window(WindowKind::Short, 40.0, reset, Some(18_000));
        let p = pace(&w, F, &rising(40.0, 2.0, reset));
        assert_eq!(p.will_last_to_reset, Some(false), "40 + 2/h · 55.6 h > 100");
    }

    #[test]
    fn a_long_window_gets_the_regression_rate_and_the_average_expected_pct() {
        let reset = Some(F + 4 * DAY);
        let w = weekly(45.0, 4 * DAY);
        let p = pace(&w, F, &rising(45.0, 1.0, reset));
        assert_eq!(p.method, Some(ProjectionMethod::Regression));
        assert!(
            close(p.rate_per_hour.unwrap(), 1.0),
            "{:?}",
            p.rate_per_hour
        );
        assert!(close(p.expected_pct.unwrap(), 300.0 / 7.0));
        assert_eq!(p.ahead, Some(false));
        // 55 points at 1 a point-hour.
        assert_eq!(p.exhaustion_at, Some(F + 55 * HOUR));
        // 45 + 96 h · 1/h = 141 > 100.
        assert_eq!(p.will_last_to_reset, Some(false));
    }

    #[test]
    fn a_window_at_its_limit_is_exhausted_at_the_reading() {
        let reset = Some(F + DAY);
        let w = window(WindowKind::Short, 100.0, reset, Some(18_000));
        let p = pace(&w, F, &rising(100.0, 2.0, reset));
        assert_eq!(p.exhaustion_at, Some(F));
        assert_eq!(p.will_last_to_reset, Some(false));
    }

    #[test]
    fn without_a_reset_there_is_no_will_last_but_there_is_an_exhaustion_time() {
        let w = window(WindowKind::Short, 40.0, None, None);
        let p = pace(&w, F, &rising(40.0, 2.0, None));
        assert_eq!(p.will_last_to_reset, None);
        assert_eq!(p.exhaustion_at, Some(F + 30 * HOUR));
    }

    #[test]
    fn a_window_at_zero_will_last_and_never_runs_out_at_the_average_rate() {
        let p = pace(&weekly(0.0, 4 * DAY), F, &[]);
        assert_eq!(p.method, Some(ProjectionMethod::Average));
        assert_eq!(p.rate_per_hour, Some(0.0));
        assert_eq!(p.exhaustion_at, None, "a zero rate never gets there");
        assert_eq!(p.will_last_to_reset, Some(true));
    }

    #[test]
    fn a_window_at_its_limit_with_no_rate_is_still_exhausted_at_the_reading() {
        let w = window(WindowKind::Short, 100.0, None, None);
        let p = pace(&w, F, &[sample(0, 100.0, None)]);
        assert_eq!(p.exhaustion_at, Some(F));
        assert_eq!(p.rate_per_hour, None);
        assert_eq!(p.will_last_to_reset, None);
    }

    #[test]
    fn nothing_to_project_from_gives_an_empty_pace() {
        let w = window(WindowKind::Short, 40.0, Some(F + HOUR), Some(18_000));
        assert_eq!(pace(&w, F, &[]), Pace::default());
        assert_eq!(
            pace(&w, F, &rising(40.0, 0.0, Some(F + HOUR))),
            Pace::default()
        );
    }
}
