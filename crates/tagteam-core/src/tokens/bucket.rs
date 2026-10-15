//! Day and month bucketing (token report §10.1).

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

/// The calendar day of `ms` in `tz`.
pub fn day_of(ms: i64, tz: &TimeZone) -> Date {
    zoned(ms, tz).date()
}
