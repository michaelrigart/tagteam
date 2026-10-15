//! Day and month bucketing (token report §10.1): calendar days and months in the report's time
//! zone, by the zone's own offsets, DST included, never by 24-hour arithmetic.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};

/// `ms` in `tz`, clamped to the range `jiff` represents.
fn zoned(ms: i64, tz: &TimeZone) -> Zoned {
    let at = Timestamp::from_millisecond(ms).unwrap_or(if ms < 0 {
        Timestamp::MIN
    } else {
        Timestamp::MAX
    });
    at.to_zoned(tz.clone())
}

/// The first instant of `date` in `tz`: its midnight, or the end of a gap that skips midnight.
/// Clamped to the range `jiff` represents.
fn start_of(date: Date, tz: &TimeZone) -> i64 {
    match date.to_zoned(tz.clone()) {
        Ok(z) => z.timestamp().as_millisecond(),
        Err(_) if date.year() < 1970 => Timestamp::MIN.as_millisecond(),
        Err(_) => Timestamp::MAX.as_millisecond(),
    }
}

/// The calendar day of `ms` in `tz`.
pub fn day_of(ms: i64, tz: &TimeZone) -> Date {
    zoned(ms, tz).date()
}

/// The calendar month of `ms` in `tz`, as (year, month).
pub fn month_of(ms: i64, tz: &TimeZone) -> (i16, i8) {
    let date = day_of(ms, tz);
    (date.year(), date.month())
}

/// `[the day's first instant, the next day's)` in `tz`: 23 or 25 hours across a DST change.
pub fn day_bounds_ms(date: Date, tz: &TimeZone) -> (i64, i64) {
    let end = date
        .tomorrow()
        .map_or(Timestamp::MAX.as_millisecond(), |next| start_of(next, tz));
    (start_of(date, tz), end)
}

/// `[the month's first instant, the next month's)` in `tz`. A month outside 1–12, or a year
/// outside what `jiff` represents, is clamped into it.
pub fn month_bounds_ms(year: i16, month: i8, tz: &TimeZone) -> (i64, i64) {
    let first = Date::new(year.clamp(-9999, 9999), month.clamp(1, 12), 1).unwrap_or(Date::MIN);
    (
        day_bounds_ms(first, tz).0,
        day_bounds_ms(first.last_of_month(), tz).1,
    )
}
#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    /// A zone from the system's time zone database, which `TimeZone::system` and `--tz` read.
    fn zone(name: &str) -> TimeZone {
        TimeZone::get(name).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// Epoch milliseconds of an ISO 8601 time, by tagteam's own parser rather than jiff's.
    fn at(iso: &str) -> i64 {
        crate::time::parse_iso8601(iso).unwrap() * 1000
    }

    #[test]
    fn days_of_23_and_25_hours_bucket_by_the_local_calendar() {
        let brussels = zone("Europe/Brussels");
        // 2026-03-29: from 02:00 CET straight to 03:00 CEST, a 23-hour day.
        let (start, end) = day_bounds_ms(date(2026, 3, 29), &brussels);
        assert_eq!(
            (start, end),
            (at("2026-03-28T23:00:00Z"), at("2026-03-29T22:00:00Z"))
        );
        assert_eq!(end - start, 23 * 3_600_000);
        assert_eq!(day_of(start - 1, &brussels), date(2026, 3, 28));
        assert_eq!(day_of(start, &brussels), date(2026, 3, 29));
        assert_eq!(day_of(end - 1, &brussels), date(2026, 3, 29));
        assert_eq!(day_of(end, &brussels), date(2026, 3, 30));
        // 2026-10-25: from 03:00 CEST back to 02:00 CET, a 25-hour day.
        let (start, end) = day_bounds_ms(date(2026, 10, 25), &brussels);
        assert_eq!(
            (start, end),
            (at("2026-10-24T22:00:00Z"), at("2026-10-25T23:00:00Z"))
        );
        assert_eq!(end - start, 25 * 3_600_000);
        // Both 02:30s of the repeated hour, and 23:59 CET, are still the 25th.
        for iso in [
            "2026-10-25T00:30:00Z",
            "2026-10-25T01:30:00Z",
            "2026-10-25T22:59:00Z",
        ] {
            assert_eq!(day_of(at(iso), &brussels), date(2026, 10, 25), "{iso}");
        }
        assert_eq!(day_of(end, &brussels), date(2026, 10, 26));
        // In UTC the same instant falls on another day.
        let late = at("2026-10-25T23:30:00Z");
        assert_eq!(day_of(late, &TimeZone::UTC), date(2026, 10, 25));
        assert_eq!(day_of(late, &brussels), date(2026, 10, 26));
    }

    #[test]
    fn a_half_hour_zone_buckets_at_its_own_midnight() {
        let kolkata = zone("Asia/Kolkata");
        assert_eq!(
            day_bounds_ms(date(2026, 10, 14), &kolkata),
            (at("2026-10-13T18:30:00Z"), at("2026-10-14T18:30:00Z"))
        );
        assert_eq!(
            day_of(at("2026-10-13T18:29:59Z"), &kolkata),
            date(2026, 10, 13)
        );
        assert_eq!(
            day_of(at("2026-10-13T18:30:00Z"), &kolkata),
            date(2026, 10, 14)
        );
        assert_eq!(month_of(at("2026-09-30T18:29:59Z"), &kolkata), (2026, 9));
        assert_eq!(month_of(at("2026-09-30T18:30:00Z"), &kolkata), (2026, 10));
    }

    #[test]
    fn month_bounds_span_whole_local_months() {
        let brussels = zone("Europe/Brussels");
        // March 2026 begins in CET and ends in CEST: 31 days less an hour.
        let (start, end) = month_bounds_ms(2026, 3, &brussels);
        assert_eq!(
            (start, end),
            (at("2026-02-28T23:00:00Z"), at("2026-03-31T22:00:00Z"))
        );
        assert_eq!(end - start, (31 * 24 - 1) * 3_600_000);
        // October 2026 ends in CET: 31 days and an hour.
        let (start, end) = month_bounds_ms(2026, 10, &brussels);
        assert_eq!(
            (start, end),
            (at("2026-09-30T22:00:00Z"), at("2026-10-31T23:00:00Z"))
        );
        assert_eq!(end - start, (31 * 24 + 1) * 3_600_000);
        assert_eq!(month_of(start - 1, &brussels), (2026, 9));
        assert_eq!(month_of(start, &brussels), (2026, 10));
        assert_eq!(month_of(end - 1, &brussels), (2026, 10));
        assert_eq!(month_of(end, &brussels), (2026, 11));
        // December runs into the next year, and a leap February has 29 days.
        assert_eq!(
            month_bounds_ms(2026, 12, &TimeZone::UTC),
            (at("2026-12-01T00:00:00Z"), at("2027-01-01T00:00:00Z"))
        );
        assert_eq!(
            month_bounds_ms(2028, 2, &TimeZone::UTC),
            (at("2028-02-01T00:00:00Z"), at("2028-03-01T00:00:00Z"))
        );
    }

    #[test]
    fn bounds_round_trip_with_day_of() {
        let zones = [
            zone("Europe/Brussels"),
            zone("Asia/Kolkata"),
            zone("America/St_Johns"),
            TimeZone::UTC,
        ];
        // A year in steps that wander through the hour, then each DST change in Brussels in
        // quarter hours.
        let sweeps = [
            (at("2025-12-30T00:00:00Z"), at("2027-01-02T00:00:00Z"), 421),
            (at("2026-03-28T18:00:00Z"), at("2026-03-30T06:00:00Z"), 15),
            (at("2026-10-24T18:00:00Z"), at("2026-10-26T06:00:00Z"), 15),
        ];
        for tz in &zones {
            for (from, until, step_min) in sweeps {
                let mut ms = from;
                while ms < until {
                    let day = day_of(ms, tz);
                    let (start, end) = day_bounds_ms(day, tz);
                    assert!(start <= ms && ms < end, "{ms} in {day}");
                    assert_eq!(day_of(start, tz), day);
                    assert_eq!(day_of(end, tz), day.tomorrow().unwrap());
                    let (year, month) = month_of(ms, tz);
                    let (first, next) = month_bounds_ms(year, month, tz);
                    assert!(first <= ms && ms < next, "{ms} in {year}-{month}");
                    assert_eq!(day_of(first, tz), date(year, month, 1));
                    assert_eq!(
                        day_of(next, tz),
                        date(year, month, 1).last_of_month().tomorrow().unwrap()
                    );
                    ms += step_min * 60_000;
                }
            }
        }
    }

    #[test]
    fn extreme_times_never_panic() {
        for tz in [
            zone("Europe/Brussels"),
            zone("Pacific/Kiritimati"),
            TimeZone::UTC,
        ] {
            for ms in [i64::MIN, -1, 0, i64::MAX] {
                let (start, end) = day_bounds_ms(day_of(ms, &tz), &tz);
                assert!(start < end, "{ms}");
                let (year, month) = month_of(ms, &tz);
                let (first, next) = month_bounds_ms(year, month, &tz);
                assert!(first < next, "{ms}");
            }
            let (start, end) = day_bounds_ms(Date::MAX, &tz);
            assert!(start <= end);
            let (start, end) = month_bounds_ms(i16::MAX, 13, &tz);
            assert!(start <= end, "clamped to December 9999");
            let (start, end) = month_bounds_ms(i16::MIN, 0, &tz);
            assert!(start <= end, "clamped to January -9999");
        }
    }
}
