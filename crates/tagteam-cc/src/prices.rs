//! The shipped API list prices for Claude Code's models (token report spec §8.1), from
//! <https://platform.claude.com/docs/en/about-claude/pricing> on `as_of`. A release updates
//! them; `[provider.claude-code.prices]` overrides cover the gap (spec §8.2).

use tagteam_core::tokens::{ModelPrice, PriceTable, RateCard};

/// The web search tool: $10 per 1,000 searches, on every model.
const WEB_SEARCH_PER_1K: f64 = 10.0;

/// USD per million tokens.
const fn card(
    input: f64,
    output: f64,
    cache_write_5m: f64,
    cache_write_1h: f64,
    cache_read: f64,
) -> RateCard {
    RateCard {
        input,
        output,
        cache_write_5m,
        cache_write_1h,
        cache_read,
    }
}

const fn priced(base: RateCard, fast: Option<RateCard>) -> ModelPrice {
    ModelPrice {
        base,
        long_prompt: None,
        fast,
        web_search_per_1k: Some(WEB_SEARCH_PER_1K),
    }
}

const FABLE_5_1: RateCard = card(10.0, 50.0, 12.5, 20.0, 0.25);
const FABLE_5: RateCard = card(10.0, 50.0, 12.5, 20.0, 1.0);
const OPUS_5_5: RateCard = card(4.0, 20.0, 5.0, 8.0, 0.2);
/// Fast mode: $8 / $40, with the model's cache multipliers on top.
const OPUS_5_5_FAST: RateCard = card(8.0, 40.0, 10.0, 16.0, 0.4);
const OPUS: RateCard = card(5.0, 25.0, 6.25, 10.0, 0.5);
/// Fast mode on Claude Opus 5 and Claude Opus 4.8: $10 / $50.
const OPUS_FAST: RateCard = card(10.0, 50.0, 12.5, 20.0, 1.0);
const SONNET_5_5: RateCard = card(2.0, 10.0, 2.5, 4.0, 0.1);
const SONNET_5: RateCard = card(2.0, 10.0, 2.5, 4.0, 0.2);
const SONNET_4: RateCard = card(3.0, 15.0, 3.75, 6.0, 0.3);
const HAIKU_5_5: RateCard = card(0.1, 0.5, 0.125, 0.2, 0.01);
/// Claude Haiku 5.5 for a prompt over 100,000 tokens, cache reads and writes included.
const HAIKU_5_5_LONG: RateCard = card(0.5, 2.5, 0.625, 1.0, 0.05);
const HAIKU_4_5: RateCard = card(1.0, 5.0, 1.25, 2.0, 0.1);

/// Every current model on the pricing page (retired ones left out), and so every model of spec
/// §2.6. Dated ids (`claude-haiku-4-5-20251001`) match through `strip_date_suffix`.
pub static PRICES: PriceTable = PriceTable {
    as_of: "2026-10-14",
    models: &[
        ("claude-fable-5-1", priced(FABLE_5_1, None)),
        ("claude-mythos-5-1", priced(FABLE_5_1, None)),
        ("claude-fable-5", priced(FABLE_5, None)),
        ("claude-mythos-5", priced(FABLE_5, None)),
        ("claude-opus-5-5", priced(OPUS_5_5, Some(OPUS_5_5_FAST))),
        ("claude-opus-5", priced(OPUS, Some(OPUS_FAST))),
        ("claude-opus-4-8", priced(OPUS, Some(OPUS_FAST))),
        // Fast mode is refused on Claude Opus 4.7.
        ("claude-opus-4-7", priced(OPUS, None)),
        // Claude Opus 4.6 runs a fast request at standard speed and bills it at standard rates.
        ("claude-opus-4-6", priced(OPUS, Some(OPUS))),
        ("claude-opus-4-5", priced(OPUS, None)),
        ("claude-sonnet-5-5", priced(SONNET_5_5, None)),
        ("claude-sonnet-5", priced(SONNET_5, None)),
        ("claude-sonnet-4-6", priced(SONNET_4, None)),
        ("claude-sonnet-4-5", priced(SONNET_4, None)),
        (
            "claude-haiku-5-5",
            ModelPrice {
                long_prompt: Some((100_000, HAIKU_5_5_LONG)),
                ..priced(HAIKU_5_5, None)
            },
        ),
        ("claude-haiku-4-5", priced(HAIKU_4_5, None)),
    ],
};

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use tagteam_core::time::parse_iso8601;
    use tagteam_core::tokens::Prices;

    use super::*;

    #[test]
    fn every_model_seen_in_transcripts_is_priced() {
        // Spec §2.6's models; the dated Haiku through date stripping (spec §8.1).
        let (prices, overridden) = Prices::new(&PRICES, &BTreeMap::new());
        assert!(overridden.is_empty());
        for model in [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-sonnet-5",
            "claude-opus-5",
            "claude-fable-5-1",
            "claude-haiku-4-5-20251001",
        ] {
            assert!(prices.lookup(model).is_some(), "{model} is unpriced");
        }
        assert_eq!(
            prices.lookup("claude-haiku-4-5-20251001"),
            prices.lookup("claude-haiku-4-5")
        );
        assert_eq!(prices.lookup("<synthetic>"), None);
    }

    #[test]
    fn rates_are_positive_and_cache_reads_cheaper_than_input() {
        let names: BTreeSet<&str> = PRICES.models.iter().map(|(name, _)| *name).collect();
        assert!(!names.is_empty());
        assert_eq!(names.len(), PRICES.models.len(), "each model once");
        for (name, price) in PRICES.models {
            let cards = [
                Some(price.base),
                price.long_prompt.map(|(_, c)| c),
                price.fast,
            ];
            for c in cards.into_iter().flatten() {
                assert!(c.cache_read > 0.0, "{name}: {c:?}");
                assert!(c.cache_read < c.input, "{name}: {c:?}");
                assert!(c.input < c.cache_write_5m, "{name}: {c:?}");
                assert!(c.cache_write_5m < c.cache_write_1h, "{name}: {c:?}");
                assert!(c.input < c.output, "{name}: {c:?}");
            }
            if let Some((above, long)) = price.long_prompt {
                assert!(above > 0 && long.input > price.base.input, "{name}");
            }
            assert_eq!(price.web_search_per_1k, Some(10.0), "{name}");
        }
    }

    #[test]
    fn as_of_is_an_iso_date() {
        assert_eq!(PRICES.as_of.len(), 10);
        assert!(parse_iso8601(&format!("{}T00:00:00Z", PRICES.as_of)).is_some());
    }
}
