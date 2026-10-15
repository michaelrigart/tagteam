//! Token counts, and the rule that merges a response's lines into one (token report §2.2).

/// One response's token counts, or a sum of them (token report §2.1, §10). Cache writes are
/// split by lifetime; `web_search` counts requests, not tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub web_search: u64,
}

impl TokenCounts {
    /// Both cache-write lifetimes.
    pub fn cache_write(&self) -> u64 {
        self.cache_write_5m.saturating_add(self.cache_write_1h)
    }

    /// The prompt: input, cache writes and cache reads, which picks a long-prompt rate card
    /// (token report §8.1).
    pub fn prompt(&self) -> u64 {
        self.input
            .saturating_add(self.cache_write())
            .saturating_add(self.cache_read)
    }

    /// Every token, the prompt's and the output's. Web searches are not tokens.
    pub fn total(&self) -> u64 {
        self.prompt().saturating_add(self.output)
    }

    /// Token report §2.2: each count of a response is the maximum over its lines, so the result
    /// is the same whatever order its lines and files are read in.
    pub fn merge_max(&mut self, other: &TokenCounts) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cache_write_5m = self.cache_write_5m.max(other.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.max(other.cache_write_1h);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.web_search = self.web_search.max(other.web_search);
    }

    /// Field by field, saturating: a row's or a total's counts.
    pub fn add(&mut self, other: &TokenCounts) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_write_5m = self.cache_write_5m.saturating_add(other.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.saturating_add(other.cache_write_1h);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.web_search = self.web_search.saturating_add(other.web_search);
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::tokens::test_support::pinned;

    fn counts(input: u64, output: u64, cw5: u64, cw1: u64, read: u64, search: u64) -> TokenCounts {
        TokenCounts {
            input,
            output,
            cache_write_5m: cw5,
            cache_write_1h: cw1,
            cache_read: read,
            web_search: search,
        }
    }

    #[test]
    fn merge_max_takes_each_fields_maximum() {
        // A response's stale snapshot, then its final line (token report §2.2): input and cache
        // counts agree, output grows, and only the final line counts web searches.
        let mut merged = counts(3, 8, 1_200, 0, 40_000, 0);
        merged.merge_max(&counts(3, 333, 1_200, 0, 40_000, 1));
        assert_eq!(merged, counts(3, 333, 1_200, 0, 40_000, 1));
        // Field by field: the larger value wins, whichever line holds it.
        let mut mixed = counts(9, 1, 9, 1, 9, 1);
        mixed.merge_max(&counts(1, 9, 1, 9, 1, 9));
        assert_eq!(mixed, counts(9, 9, 9, 9, 9, 9));
        // A zero line merged with a full one is the full one.
        let mut zero = TokenCounts::default();
        zero.merge_max(&counts(1, 2, 3, 4, 5, 6));
        assert_eq!(zero, counts(1, 2, 3, 4, 5, 6));
    }

    #[test]
    fn prompt_and_total_follow_the_spec() {
        let c = counts(1, 2, 3, 4, 5, 6);
        assert_eq!(c.cache_write(), 3 + 4);
        assert_eq!(c.prompt(), 1 + 7 + 5, "input, cache writes and cache reads");
        assert_eq!(c.total(), 1 + 2 + 7 + 5, "web searches are not tokens");
        let mut sum = c;
        sum.add(&counts(10, 20, 30, 40, 50, 60));
        assert_eq!(sum, counts(11, 22, 33, 44, 55, 66));
    }

    #[test]
    fn totals_saturate_instead_of_overflowing() {
        let max = counts(u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX);
        assert_eq!(max.cache_write(), u64::MAX);
        assert_eq!(max.prompt(), u64::MAX);
        assert_eq!(max.total(), u64::MAX);
        assert_eq!(counts(0, 0, u64::MAX, 1, 0, 0).cache_write(), u64::MAX);
        let mut sum = counts(u64::MAX - 1, 1, 0, 0, 0, u64::MAX);
        sum.add(&counts(5, 5, 5, 5, 5, 5));
        assert_eq!(sum, counts(u64::MAX, 6, 5, 5, 5, u64::MAX));
    }

    fn line() -> impl Strategy<Value = TokenCounts> {
        (
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
        )
            .prop_map(|(a, b, c, d, e, f)| counts(a, b, c, d, e, f))
    }

    fn merged(lines: &[TokenCounts]) -> TokenCounts {
        lines.iter().fold(TokenCounts::default(), |mut acc, l| {
            acc.merge_max(l);
            acc
        })
    }

    proptest! {
        #![proptest_config(pinned(256))]

        /// Token report §15: any order of a response's lines gives the same row.
        #[test]
        fn merge_is_order_independent(
            (lines, shuffled) in prop::collection::vec(line(), 1..=9)
                .prop_flat_map(|lines| (Just(lines.clone()), Just(lines).prop_shuffle()))
        ) {
            let once = merged(&lines);
            prop_assert_eq!(once, merged(&shuffled));
            let reversed: Vec<TokenCounts> = lines.iter().rev().copied().collect();
            prop_assert_eq!(once, merged(&reversed));
            prop_assert_eq!(once.output, lines.iter().map(|l| l.output).max().unwrap());
            // A line read again, from a re-read file or a copy in another, changes nothing.
            let mut again = once;
            again.merge_max(&lines[lines.len() - 1]);
            prop_assert_eq!(again, once);
        }
    }
}
