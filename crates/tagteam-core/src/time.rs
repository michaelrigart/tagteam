//! ISO 8601 UTC, as tagteam's JSON spells a time (§13.2) and as the endpoints and an export
//! file write one (§8.1, §13.3). Only `Z` and numeric offsets are read, and only UTC is
//! written.

/// The value of `n` ASCII digits, or `None` if any byte is not one.
fn digits(b: &[u8]) -> Option<i64> {
    b.iter().try_fold(0i64, |n, c| {
        c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0'))
    })
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01, as `(year, month, day)` (`civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Epoch seconds from `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` (Decision 5). The fraction
/// is dropped. Anything else, a missing offset or an impossible date included, is `None`.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (digits(&b[0..4])?, digits(&b[5..7])?, digits(&b[8..10])?);
    let (hour, minute, second) = (
        digits(&b[11..13])?,
        digits(&b[14..16])?,
        digits(&b[17..19])?,
    );
    let mut rest = &b[19..];
    if let Some(fraction) = rest.strip_prefix(b".") {
        let n = fraction.iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        rest = &fraction[n..];
    }
    let offset = match rest {
        b"Z" => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let (h, m) = (digits(&[*h1, *h2])?, digits(&[*m1, *m2])?);
            if h > 23 || m > 59 {
                return None;
            }
            if *sign == b'-' {
                -(h * 3600 + m * 60)
            } else {
                h * 3600 + m * 60
            }
        }
        _ => return None,
    };
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// `YYYY-MM-DDTHH:MM:SSZ` for epoch seconds: how rendered JSON spells a time (§13.2).
pub fn format_iso8601(epoch_s: i64) -> String {
    let (year, month, day) = civil_from_days(epoch_s.div_euclid(86_400));
    let secs = epoch_s.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_time_is_written_in_utc_and_read_back() {
        // §13.3's `exportedAt` and `addedAt`, which the engine writes and reads.
        assert_eq!(format_iso8601(1_790_000_000), "2026-09-21T14:13:20Z");
        assert_eq!(parse_iso8601("2026-09-21T14:13:20Z"), Some(1_790_000_000));
        assert_eq!(
            parse_iso8601("2026-09-21T16:13:20.5+02:00"),
            Some(1_790_000_000)
        );
    }
}
