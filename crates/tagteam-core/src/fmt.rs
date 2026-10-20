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
