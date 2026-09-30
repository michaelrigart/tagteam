/// §9.3: the positions a rotation tries, in order. With an anchor, every position after it,
/// wrapping around, and never the anchor itself; with none, every position from the first.
/// The input need not be sorted, and a repeated position is tried once.
pub fn rotation_order(positions: &[u32], anchor: Option<u32>) -> Vec<u32> {
    let mut sorted = positions.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let Some(anchor) = anchor else {
        return sorted;
    };
    let (before, after): (Vec<u32>, Vec<u32>) = sorted
        .into_iter()
        .filter(|p| *p != anchor)
        .partition(|p| *p < anchor);
    after.into_iter().chain(before).collect()
}

#[cfg(test)]
mod tests {
    use super::rotation_order;

    #[test]
    fn positions_after_the_anchor_come_first_and_wrap() {
        let positions = [1, 2, 3, 5];
        assert_eq!(rotation_order(&positions, Some(1)), [2, 3, 5]);
        assert_eq!(rotation_order(&positions, Some(3)), [5, 1, 2]);
        assert_eq!(rotation_order(&positions, Some(5)), [1, 2, 3]);
    }

    #[test]
    fn an_anchor_outside_the_list_still_orders_from_after_it() {
        assert_eq!(rotation_order(&[1, 3, 5], Some(4)), [5, 1, 3]);
    }

    #[test]
    fn no_anchor_takes_every_position_from_the_first() {
        assert_eq!(rotation_order(&[4, 2], None), [2, 4]);
    }

    #[test]
    fn unsorted_or_repeated_input_is_tried_once_in_order() {
        assert_eq!(rotation_order(&[5, 1, 3, 1], Some(3)), [5, 1]);
    }

    #[test]
    fn only_the_anchor_yields_nothing() {
        assert!(rotation_order(&[1], Some(1)).is_empty());
        assert!(rotation_order(&[], None).is_empty());
    }
}
