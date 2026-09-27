/// The next switchable position after `anchor`, wrapping around (§9.3 rotation). With no
/// anchor, the first switchable position. `None` when no other account is switchable.
pub fn next_in_rotation(accounts: &[(u32, bool)], anchor: Option<u32>) -> Option<u32> {
    let mut switchable = accounts.iter().filter(|(_, s)| *s).map(|(p, _)| *p);
    match anchor {
        None => switchable.next(),
        Some(a) => {
            let all: Vec<u32> = switchable.collect();
            all.iter()
                .copied()
                .find(|p| *p > a)
                .or_else(|| all.iter().copied().find(|p| *p != a))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::next_in_rotation;

    #[test]
    fn picks_the_next_switchable_position_and_wraps() {
        let a = [(1, true), (2, false), (3, true), (5, true)];
        assert_eq!(next_in_rotation(&a, Some(1)), Some(3));
        assert_eq!(next_in_rotation(&a, Some(3)), Some(5));
        assert_eq!(next_in_rotation(&a, Some(5)), Some(1));
        assert_eq!(next_in_rotation(&a, Some(2)), Some(3));
    }

    #[test]
    fn no_anchor_takes_the_first_switchable() {
        assert_eq!(next_in_rotation(&[(2, false), (4, true)], None), Some(4));
    }

    #[test]
    fn only_the_anchor_switchable_yields_none() {
        assert_eq!(next_in_rotation(&[(1, true), (2, false)], Some(1)), None);
        assert_eq!(next_in_rotation(&[], None), None);
    }
}
