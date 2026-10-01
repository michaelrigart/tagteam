//! Failure backoff (§8.5) and `Retry-After` parsing (§8.1). Pure; nothing here touches a clock.

/// `min(asked, …)` ceilings (§8.5).
const CAP_429_S: f64 = 4500.0;
const CAP_OTHER_S: f64 = 3600.0;
/// `min(30 · 2^(n−1), 600)`, with the exponent clamped at 32.
const BASE_S: i64 = 30;
const BASE_MAX_S: i64 = 600;
const MAX_EXPONENT: u32 = 32;
/// A 429 whose `Retry-After` is below one second (it counts as `0`) still waits this long.
const ZERO_RETRY_AFTER_FLOOR_S: f64 = 300.0;
/// A 429 that asks for more than this gets [`LONG_RETRY_AFTER_MARGIN_S`] on top, because
/// retrying at the deadline re-arms the block.
const LONG_RETRY_AFTER_S: f64 = 600.0;
const LONG_RETRY_AFTER_MARGIN_S: f64 = 900.0;

/// `Retry-After` in its seconds form only (§8.1): an integer or decimal, possibly huge or `inf`.
/// Negative, empty, unparseable and HTTP-date values give `None`.
///
/// Whatever Rust's `f64` parser accepts counts, so the odd forms are accepted too: a leading `+`
/// (`+5`), a bare or trailing dot (`.5`, `5.`), an exponent (`1e3`), and `inf`/`infinity` in any
/// case. They all clamp safely downstream, so they are kept rather than special-cased out.
///
/// A value that overflows to infinity (`1e400`) is returned as infinity; the caller's
/// [`failure_backoff_s`] clamps it, so it is never stored.
pub fn parse_retry_after(value: &str) -> Option<f64> {
    let s = value.trim();
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let seconds: f64 = s.parse().ok()?;
    (!seconds.is_nan()).then_some(seconds)
}

/// §8.5. `consecutive_failures` counts this failure (≥ 1; 0 is treated as 1). A non-finite
/// `retry_after_s` is clamped to the cap; a negative or NaN one is ignored. Returns whole
/// seconds, never negative.
pub fn failure_backoff_s(
    consecutive_failures: u32,
    is_429: bool,
    retry_after_s: Option<f64>,
) -> i64 {
    let exponent = consecutive_failures.saturating_sub(1).min(MAX_EXPONENT);
    let computed = (BASE_S << exponent).min(BASE_MAX_S);
    let cap = if is_429 { CAP_429_S } else { CAP_OTHER_S };

    let asked = match retry_after_s {
        Some(r) if r.is_nan() || r < 0.0 => 0.0,
        Some(r) if !r.is_finite() => cap,
        Some(r) if is_429 && r < 1.0 => ZERO_RETRY_AFTER_FLOOR_S,
        Some(r) if is_429 && r > LONG_RETRY_AFTER_S => r + LONG_RETRY_AFTER_MARGIN_S,
        Some(r) => r,
        None => 0.0,
    };
    let asked = asked.ceil().min(cap) as i64;
    asked.max(computed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_accepts_the_seconds_form() {
        assert_eq!(parse_retry_after("0"), Some(0.0));
        assert_eq!(parse_retry_after("30"), Some(30.0));
        assert_eq!(parse_retry_after("12.5"), Some(12.5));
        assert_eq!(parse_retry_after(" 45 "), Some(45.0));
    }

    #[test]
    fn retry_after_keeps_huge_and_infinite_values_for_the_caller_to_clamp() {
        assert_eq!(parse_retry_after("1e400"), Some(f64::INFINITY));
        assert_eq!(parse_retry_after("inf"), Some(f64::INFINITY));
        assert_eq!(parse_retry_after("1e300"), Some(1e300));
    }

    #[test]
    fn retry_after_accepts_the_odd_forms_rust_parses() {
        assert_eq!(parse_retry_after("+5"), Some(5.0));
        assert_eq!(parse_retry_after(".5"), Some(0.5));
        assert_eq!(parse_retry_after("5."), Some(5.0));
        assert_eq!(parse_retry_after("1e3"), Some(1000.0));
        assert_eq!(parse_retry_after("infinity"), Some(f64::INFINITY));
        assert_eq!(parse_retry_after("Infinity"), Some(f64::INFINITY));
    }

    #[test]
    fn retry_after_rejects_everything_else() {
        for s in [
            "",
            "   ",
            "-5",
            "-0",
            "nan",
            "Wed, 21 Oct 2015 07:28:00 GMT",
            "soon",
            "0x10",
            "5 seconds",
        ] {
            assert_eq!(parse_retry_after(s), None, "{s:?}");
        }
    }

    #[test]
    fn the_base_doubles_from_thirty_seconds_up_to_six_hundred() {
        for (n, expected) in [
            (1, 30),
            (2, 60),
            (3, 120),
            (4, 240),
            (5, 480),
            (6, 600),
            (7, 600),
        ] {
            assert_eq!(failure_backoff_s(n, false, None), expected, "n = {n}");
        }
    }

    #[test]
    fn the_exponent_is_clamped_so_huge_counts_do_not_overflow() {
        assert_eq!(failure_backoff_s(33, false, None), 600);
        assert_eq!(failure_backoff_s(1_000, false, None), 600);
        assert_eq!(failure_backoff_s(u32::MAX, true, None), 600);
    }

    #[test]
    fn a_zero_count_is_treated_as_the_first_failure() {
        assert_eq!(failure_backoff_s(0, false, None), 30);
    }

    #[test]
    fn a_429_without_retry_after_uses_the_base() {
        assert_eq!(failure_backoff_s(1, true, None), 30);
        assert_eq!(failure_backoff_s(3, true, None), 120);
    }

    #[test]
    fn a_429_with_retry_after_zero_waits_at_least_three_hundred_seconds() {
        assert_eq!(failure_backoff_s(1, true, Some(0.0)), 300);
        assert_eq!(failure_backoff_s(5, true, Some(0.0)), 480);
        assert_eq!(failure_backoff_s(6, true, Some(0.0)), 600);
    }

    #[test]
    fn on_a_429_a_retry_after_below_one_second_counts_as_zero() {
        assert_eq!(failure_backoff_s(1, true, Some(0.0)), 300);
        assert_eq!(failure_backoff_s(1, true, Some(0.5)), 300);
        assert_eq!(failure_backoff_s(1, true, Some(0.999)), 300);
        assert_eq!(
            failure_backoff_s(1, true, Some(1.0)),
            30,
            "one second is a real ask, so the base wins"
        );
    }

    #[test]
    fn retry_after_zero_on_another_status_has_no_minimum() {
        assert_eq!(failure_backoff_s(1, false, Some(0.0)), 30);
        assert_eq!(failure_backoff_s(1, false, Some(0.5)), 30);
    }

    #[test]
    fn a_429_honours_a_retry_after_up_to_six_hundred_seconds_as_asked() {
        assert_eq!(failure_backoff_s(1, true, Some(120.0)), 120);
        assert_eq!(failure_backoff_s(1, true, Some(600.0)), 600);
    }

    #[test]
    fn a_429_asking_for_more_than_six_hundred_adds_nine_hundred_of_margin() {
        assert_eq!(failure_backoff_s(1, true, Some(601.0)), 1501);
        assert_eq!(failure_backoff_s(1, true, Some(1000.0)), 1900);
        assert_eq!(failure_backoff_s(1, true, Some(3000.0)), 3900);
    }

    #[test]
    fn a_429_is_capped_at_forty_five_hundred_seconds() {
        assert_eq!(
            failure_backoff_s(1, true, Some(4000.0)),
            4500,
            "4900 capped"
        );
        assert_eq!(failure_backoff_s(1, true, Some(1e300)), 4500);
    }

    #[test]
    fn other_statuses_are_capped_at_thirty_six_hundred_seconds() {
        assert_eq!(failure_backoff_s(1, false, Some(45.0)), 45);
        assert_eq!(failure_backoff_s(1, false, Some(3600.0)), 3600);
        assert_eq!(failure_backoff_s(1, false, Some(7200.0)), 3600);
        assert_eq!(
            failure_backoff_s(1, false, Some(700.0)),
            700,
            "no margin off a 429"
        );
    }

    #[test]
    fn a_fractional_retry_after_rounds_up_to_whole_seconds() {
        assert_eq!(failure_backoff_s(1, false, Some(50.5)), 51);
        assert_eq!(
            failure_backoff_s(1, true, Some(12.5)),
            30,
            "the base is larger"
        );
    }

    #[test]
    fn a_retry_after_shorter_than_the_base_loses_to_the_base() {
        assert_eq!(failure_backoff_s(4, false, Some(10.0)), 240);
        assert_eq!(failure_backoff_s(7, true, Some(100.0)), 600);
    }

    #[test]
    fn a_non_finite_retry_after_is_clamped_to_the_cap() {
        assert_eq!(failure_backoff_s(1, true, Some(f64::INFINITY)), 4500);
        assert_eq!(failure_backoff_s(1, false, Some(f64::INFINITY)), 3600);
    }

    #[test]
    fn a_negative_or_nan_retry_after_is_ignored() {
        assert_eq!(failure_backoff_s(1, true, Some(-5.0)), 30);
        assert_eq!(failure_backoff_s(1, true, Some(f64::NAN)), 30);
        assert_eq!(failure_backoff_s(2, false, Some(-1.0)), 60);
    }

    #[test]
    fn hostile_retry_after_values_never_store_a_non_finite_or_oversized_wait() {
        // Review Focus 3: each header value, parsed then turned into a stored backoff.
        for (header, expected_429, expected_other) in [
            ("0", 300, 30),
            ("1e400", 4500, 3600),
            ("inf", 4500, 3600),
            ("-5", 30, 30),
            ("Wed, 21 Oct 2015 07:28:00 GMT", 30, 30),
            ("", 30, 30),
            ("12.5", 30, 30),
        ] {
            let parsed = parse_retry_after(header);
            assert_eq!(
                failure_backoff_s(1, true, parsed),
                expected_429,
                "429 with {header:?}"
            );
            assert_eq!(
                failure_backoff_s(1, false, parsed),
                expected_other,
                "other with {header:?}"
            );
        }
    }
}
