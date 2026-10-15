//! Prices and cost (token report §8).

/// `model` without a trailing `-YYYYMMDD`, which a dated model name is priced by when the table
/// has no entry of its own: `claude-haiku-4-5-20251001` as `claude-haiku-4-5` (token report
/// §8.1).
pub fn strip_date_suffix(model: &str) -> &str {
    let b = model.as_bytes();
    match b.len().checked_sub(9) {
        Some(cut) if cut > 0 && b[cut] == b'-' && b[cut + 1..].iter().all(u8::is_ascii_digit) => {
            &model[..cut]
        }
        _ => model,
    }
}
