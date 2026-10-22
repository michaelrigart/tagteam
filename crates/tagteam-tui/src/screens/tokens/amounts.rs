//! Amounts as the Tokens screen writes them (redesign §4.3, C-10): dollars and unscaled counts
//! with thousands separators. The CLI keeps its own form (`$1541.64`, §11.1).

use tagteam_engine::words;

/// `n` with thousands separators: `9,891` (C-10).
pub(super) fn grouped(n: u64) -> String {
    separated(&n.to_string())
}

/// Nano-dollars as `words::dollars` rounds them, the dollars grouped: `$1,541.64`.
pub(super) fn dollars(nanos: u64) -> String {
    let plain = words::dollars(nanos);
    let digits = plain.strip_prefix('$').unwrap_or(&plain);
    match digits.split_once('.') {
        Some((whole, cents)) => format!("${}.{cents}", separated(whole)),
        None => format!("${}", separated(digits)),
    }
}

/// `digits` with a `,` before every third digit from the right.
fn separated(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_dollars_take_thousands_separators() {
        for (n, want) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1,000"),
            (9_891, "9,891"),
            (u64::MAX, "18,446,744,073,709,551,615"),
        ] {
            assert_eq!(grouped(n), want, "{n}");
        }
        for (nanos, want) in [
            (0, "$0.00"),
            (5_000_000, "$0.01"),
            (622_016_000, "$0.62"),
            (1_541_640_000_000, "$1,541.64"),
            (u64::MAX, "$18,446,744,073.71"),
        ] {
            assert_eq!(dollars(nanos), want, "{nanos}");
        }
    }
}
