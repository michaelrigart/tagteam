//! Prices and cost (token report §8): rate cards in USD per million tokens, the shipped table
//! with the user's overrides merged in, and a response's API cost in integer nano-dollars.

use std::collections::BTreeMap;

use super::counts::TokenCounts;

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateCard {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

/// One model's prices (token report §8.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub base: RateCard,
    /// The card for a response whose prompt is above the threshold, in tokens.
    pub long_prompt: Option<(u64, RateCard)>,
    /// The card for a `speed: "fast"` response.
    pub fast: Option<RateCard>,
    /// USD per thousand web search requests.
    pub web_search_per_1k: Option<f64>,
}

/// A provider's shipped prices, and the date they were taken.
#[derive(Debug)]
pub struct PriceTable {
    pub as_of: &'static str,
    pub models: &'static [(&'static str, ModelPrice)],
}

/// The table of a provider that ships none.
pub static EMPTY_PRICES: PriceTable = PriceTable {
    as_of: "",
    models: &[],
};

/// A rate card's fields as a price table in the config sets them (token report §8.2).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CardOverride {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_write_5m: Option<f64>,
    pub cache_write_1h: Option<f64>,
    pub cache_read: Option<f64>,
}

/// One `[provider.<id>.prices."<model>"]` table (token report §8.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceOverride {
    pub base: CardOverride,
    pub long_prompt_above: Option<u64>,
    pub long_prompt: CardOverride,
    pub fast: CardOverride,
    pub web_search_per_1k: Option<f64>,
}

/// A rate as given, when it is finite and not negative.
fn valid(rate: Option<f64>) -> Option<f64> {
    rate.filter(|r| r.is_finite() && *r >= 0.0)
}

impl CardOverride {
    /// `card` with every rate this sets replaced.
    fn onto(&self, card: RateCard) -> RateCard {
        RateCard {
            input: valid(self.input).unwrap_or(card.input),
            output: valid(self.output).unwrap_or(card.output),
            cache_write_5m: valid(self.cache_write_5m).unwrap_or(card.cache_write_5m),
            cache_write_1h: valid(self.cache_write_1h).unwrap_or(card.cache_write_1h),
            cache_read: valid(self.cache_read).unwrap_or(card.cache_read),
        }
    }

    /// A card of these rates, when all five are set.
    fn whole(&self) -> Option<RateCard> {
        Some(RateCard {
            input: valid(self.input)?,
            output: valid(self.output)?,
            cache_write_5m: valid(self.cache_write_5m)?,
            cache_write_1h: valid(self.cache_write_1h)?,
            cache_read: valid(self.cache_read)?,
        })
    }

    /// `card` overridden, or, for a card the entry lacks, one only when this is whole.
    fn apply(&self, card: Option<RateCard>) -> Option<RateCard> {
        match card {
            Some(card) => Some(self.onto(card)),
            None => self.whole(),
        }
    }
}

impl PriceOverride {
    /// `price` with this override merged in, field by field. A long-prompt card the entry
    /// lacks needs a threshold too.
    fn onto(&self, price: ModelPrice) -> ModelPrice {
        let long_prompt = match (price.long_prompt, self.long_prompt_above) {
            (Some((above, card)), threshold) => {
                Some((threshold.unwrap_or(above), self.long_prompt.onto(card)))
            }
            (None, Some(threshold)) => self.long_prompt.whole().map(|card| (threshold, card)),
            (None, None) => None,
        };
        ModelPrice {
            base: self.base.onto(price.base),
            long_prompt,
            fast: self.fast.apply(price.fast),
            web_search_per_1k: valid(self.web_search_per_1k).or(price.web_search_per_1k),
        }
    }
}

/// The shipped table with the user's overrides merged in: what cost is computed from.
#[derive(Debug, Clone)]
pub struct Prices {
    as_of: &'static str,
    models: BTreeMap<String, ModelPrice>,
}

impl Prices {
    /// The shipped table with `overrides` merged in (token report §8.2), and the models an
    /// override changed or added, sorted. An override applies field by field to the entry its
    /// name finds as `lookup` does, and is kept under its own name; one for a model the table
    /// lacks adds it only with a whole base. A rate that is negative or not finite is ignored.
    pub fn new(
        table: &PriceTable,
        overrides: &BTreeMap<String, PriceOverride>,
    ) -> (Prices, Vec<String>) {
        let shipped = Prices {
            as_of: table.as_of,
            models: table
                .models
                .iter()
                .map(|(name, price)| ((*name).to_owned(), *price))
                .collect(),
        };
        let mut models = shipped.models.clone();
        let mut changed = Vec::new();
        for (name, over) in overrides {
            let was = shipped.lookup(name).copied();
            let now = match was {
                Some(price) => Some(over.onto(price)),
                None => over.base.whole().map(|base| {
                    over.onto(ModelPrice {
                        base,
                        long_prompt: None,
                        fast: None,
                        web_search_per_1k: None,
                    })
                }),
            };
            if let Some(price) = now {
                // Kept under its own name even when it equals what the name found, so a dated
                // override pins its entry against an override of the undated name.
                if Some(price) != was {
                    changed.push(name.clone());
                }
                models.insert(name.clone(), price);
            }
        }
        (
            Prices {
                as_of: table.as_of,
                models,
            },
            changed,
        )
    }

    /// The date the shipped prices were taken.
    pub fn as_of(&self) -> &str {
        self.as_of
    }

    /// A model's prices: by its exact name, then without a trailing date (token report §8.1).
    pub fn lookup(&self, model: &str) -> Option<&ModelPrice> {
        self.models
            .get(model)
            .or_else(|| self.models.get(strip_date_suffix(model)))
    }

    /// A response's API cost in nano-dollars (token report §8.1, §8.3, ruling R4): the fast
    /// card for `speed: "fast"`, else the long-prompt card for a prompt above its threshold,
    /// else the base card. A rate is rounded to whole nano-dollars per token, a web search to
    /// whole nano-dollars per request, and a model with no web search price prices its tokens
    /// only. `None` is unpriced: a model with no entry, or a fast response with no fast card.
    pub fn cost_nanos(
        &self,
        model: &str,
        counts: &TokenCounts,
        speed: Option<&str>,
    ) -> Option<u64> {
        let price = self.lookup(model)?;
        let card = match (speed, price.long_prompt) {
            (Some("fast"), _) => price.fast?,
            (_, Some((above, long))) if counts.prompt() > above => long,
            _ => price.base,
        };
        let tokens = [
            (counts.input, card.input),
            (counts.output, card.output),
            (counts.cache_write_5m, card.cache_write_5m),
            (counts.cache_write_1h, card.cache_write_1h),
            (counts.cache_read, card.cache_read),
        ]
        .into_iter()
        .fold(0u64, |sum, (n, rate)| {
            sum.saturating_add(n.saturating_mul(nanos(rate, 1_000.0)))
        });
        let searches = price.web_search_per_1k.map_or(0, |per_1k| {
            counts.web_search.saturating_mul(nanos(per_1k, 1_000_000.0))
        });
        Some(tokens.saturating_add(searches))
    }
}

/// `usd` in whole nano-dollars per unit: `scale` is 1,000 for USD per million tokens, and
/// 1,000,000 for USD per thousand requests.
fn nanos(usd: f64, scale: f64) -> u64 {
    (usd * scale).round() as u64
}

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

#[cfg(test)]
mod tests {
    use super::*;

    const OPUS: RateCard = RateCard {
        input: 4.0,
        output: 20.0,
        cache_write_5m: 5.0,
        cache_write_1h: 8.0,
        cache_read: 0.20,
    };
    const OPUS_FAST: RateCard = RateCard {
        input: 8.0,
        output: 40.0,
        cache_write_5m: 10.0,
        cache_write_1h: 16.0,
        cache_read: 0.40,
    };
    const HAIKU: RateCard = RateCard {
        input: 0.10,
        output: 0.50,
        cache_write_5m: 0.125,
        cache_write_1h: 0.20,
        cache_read: 0.01,
    };
    const HAIKU_LONG: RateCard = RateCard {
        input: 0.50,
        output: 2.50,
        cache_write_5m: 0.625,
        cache_write_1h: 1.00,
        cache_read: 0.05,
    };
    const OLD: RateCard = RateCard {
        input: 1.0,
        output: 5.0,
        cache_write_5m: 1.25,
        cache_write_1h: 2.0,
        cache_read: 0.10,
    };

    static TABLE: PriceTable = PriceTable {
        as_of: "2026-10-14",
        models: &[
            (
                "claude-opus-5-5",
                ModelPrice {
                    base: OPUS,
                    long_prompt: None,
                    fast: Some(OPUS_FAST),
                    web_search_per_1k: Some(10.0),
                },
            ),
            (
                "claude-haiku-5-5",
                ModelPrice {
                    base: HAIKU,
                    long_prompt: Some((100_000, HAIKU_LONG)),
                    fast: None,
                    web_search_per_1k: Some(10.0),
                },
            ),
            (
                "claude-haiku-4-5",
                ModelPrice {
                    base: OLD,
                    long_prompt: None,
                    fast: None,
                    web_search_per_1k: None,
                },
            ),
            (
                "claude-haiku-4-5-20240101",
                ModelPrice {
                    base: HAIKU,
                    long_prompt: None,
                    fast: None,
                    web_search_per_1k: None,
                },
            ),
            // No model has both today (token report §8.1); the rule is tested all the same.
            (
                "claude-both",
                ModelPrice {
                    base: OLD,
                    long_prompt: Some((10, HAIKU_LONG)),
                    fast: Some(OPUS_FAST),
                    web_search_per_1k: None,
                },
            ),
        ],
    };

    fn shipped() -> Prices {
        Prices::new(&TABLE, &BTreeMap::new()).0
    }

    fn tokens(input: u64, output: u64, cw5: u64, cw1: u64, read: u64) -> TokenCounts {
        TokenCounts {
            input,
            output,
            cache_write_5m: cw5,
            cache_write_1h: cw1,
            cache_read: read,
            web_search: 0,
        }
    }

    fn card(input: f64, output: f64, cw5: f64, cw1: f64, read: f64) -> CardOverride {
        CardOverride {
            input: Some(input),
            output: Some(output),
            cache_write_5m: Some(cw5),
            cache_write_1h: Some(cw1),
            cache_read: Some(read),
        }
    }

    fn input_only(input: f64) -> CardOverride {
        CardOverride {
            input: Some(input),
            ..CardOverride::default()
        }
    }

    #[test]
    fn exact_then_date_stripped_lookup() {
        let prices = shipped();
        assert_eq!(prices.as_of(), "2026-10-14");
        let base = |model: &str| prices.lookup(model).map(|p| p.base);
        assert_eq!(base("claude-haiku-4-5"), Some(OLD));
        assert_eq!(
            base("claude-haiku-4-5-20251001"),
            Some(OLD),
            "the date is stripped"
        );
        assert_eq!(
            base("claude-haiku-4-5-20240101"),
            Some(HAIKU),
            "an exact name wins"
        );
        assert_eq!(
            base("claude-haiku-4-5-2025100"),
            None,
            "seven digits are no date"
        );
        assert_eq!(base("claude-opus-5-5[1m]"), None, "only a date is stripped");
        assert_eq!(
            strip_date_suffix("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
        assert_eq!(strip_date_suffix("claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(
            strip_date_suffix("-20251001"),
            "-20251001",
            "nothing would be left"
        );
        assert_eq!(strip_date_suffix("model-2025100a"), "model-2025100a");
        assert_eq!(strip_date_suffix("model_20251001"), "model_20251001");
        assert_eq!(strip_date_suffix(""), "");
    }

    #[test]
    fn cost_is_integer_nanos_per_token() {
        let prices = shipped();
        // $4, $20, $5, $8 and $0.20 per million tokens are 4,000, 20,000, 5,000, 8,000 and 200
        // nano-dollars per token.
        let c = tokens(1_000, 500, 2_000, 100, 10_000);
        let want = 1_000 * 4_000 + 500 * 20_000 + 2_000 * 5_000 + 100 * 8_000 + 10_000 * 200;
        assert_eq!(prices.cost_nanos("claude-opus-5-5", &c, None), Some(want));
        assert_eq!(want, 26_800_000);
        let million = tokens(1_000_000, 0, 0, 0, 0);
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &million, None),
            Some(4_000_000_000),
            "a million input tokens at $4 is $4 exactly"
        );
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &TokenCounts::default(), None),
            Some(0),
            "no tokens: priced, at nothing"
        );
        // Each rate rounds to whole nano-dollars per token: $0.0004 is 0, $0.0006 is 1, and
        // $0.30 is 300 although 0.30 × 1000 is not exactly 300 in binary.
        let (odd, _) = Prices::new(
            &EMPTY_PRICES,
            &BTreeMap::from([(
                "m".to_owned(),
                PriceOverride {
                    base: card(0.0004, 0.0006, 0.30, 0.125, 0.01),
                    ..PriceOverride::default()
                },
            )]),
        );
        assert_eq!(
            odd.cost_nanos("m", &tokens(1, 1, 1, 1, 1), None),
            Some(1 + 300 + 125 + 10)
        );
        // Sums saturate instead of overflowing.
        let huge = tokens(u64::MAX, u64::MAX, 0, 0, 0);
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &huge, None),
            Some(u64::MAX)
        );
    }

    #[test]
    fn fast_uses_its_card_and_without_one_is_unpriced() {
        let prices = shipped();
        let c = tokens(1_000, 100, 0, 0, 0);
        let base = 1_000 * 4_000 + 100 * 20_000;
        let fast = 1_000 * 8_000 + 100 * 40_000;
        assert_eq!(prices.cost_nanos("claude-opus-5-5", &c, None), Some(base));
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &c, Some("standard")),
            Some(base)
        );
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &c, Some("fast")),
            Some(fast)
        );
        assert_eq!(
            prices.cost_nanos("claude-haiku-4-5", &c, Some("fast")),
            None,
            "no fast card: unpriced"
        );
        // A fast response keeps its fast card above a long-prompt threshold.
        assert!(c.prompt() > 10);
        assert_eq!(
            prices.cost_nanos("claude-both", &c, Some("fast")),
            Some(fast)
        );
    }

    #[test]
    fn long_prompt_switches_card_above_the_threshold_only() {
        let prices = shipped();
        // The prompt is input, cache writes and cache reads; exactly at the threshold is not
        // above it.
        let at = tokens(40_000, 1_000, 30_000, 10_000, 20_000);
        assert_eq!(at.prompt(), 100_000);
        let base = 40_000 * 100 + 1_000 * 500 + 30_000 * 125 + 10_000 * 200 + 20_000 * 10;
        assert_eq!(prices.cost_nanos("claude-haiku-5-5", &at, None), Some(base));
        let above = tokens(40_001, 1_000, 30_000, 10_000, 20_000);
        let long = 40_001 * 500 + 1_000 * 2_500 + 30_000 * 625 + 10_000 * 1_000 + 20_000 * 50;
        assert_eq!(
            prices.cost_nanos("claude-haiku-5-5", &above, None),
            Some(long)
        );
        // Output never counts toward the threshold.
        let talkative = tokens(10, 1_000_000, 0, 0, 0);
        assert_eq!(
            prices.cost_nanos("claude-haiku-5-5", &talkative, None),
            Some(10 * 100 + 1_000_000 * 500)
        );
    }

    #[test]
    fn web_search_is_priced_per_request() {
        let prices = shipped();
        let searches = TokenCounts {
            web_search: 3,
            ..TokenCounts::default()
        };
        assert_eq!(
            prices.cost_nanos("claude-opus-5-5", &searches, None),
            Some(30_000_000),
            "$10 per thousand: a cent each"
        );
        let mixed = TokenCounts {
            input: 1_000,
            web_search: 2,
            ..TokenCounts::default()
        };
        assert_eq!(
            prices.cost_nanos("claude-haiku-4-5", &mixed, None),
            Some(1_000 * 1_000),
            "no search price: its tokens are priced, its searches add nothing"
        );
    }

    #[test]
    fn overrides_merge_field_by_field_and_complete_base_adds_a_model() {
        let overrides = BTreeMap::from([
            (
                "claude-opus-5-5".to_owned(),
                PriceOverride {
                    base: input_only(3.0),
                    fast: CardOverride {
                        output: Some(30.0),
                        ..CardOverride::default()
                    },
                    ..PriceOverride::default()
                },
            ),
            (
                "claude-new".to_owned(),
                PriceOverride {
                    base: card(1.0, 2.0, 3.0, 4.0, 0.5),
                    web_search_per_1k: Some(5.0),
                    ..PriceOverride::default()
                },
            ),
            (
                "claude-partial".to_owned(),
                PriceOverride {
                    base: input_only(1.0),
                    ..PriceOverride::default()
                },
            ),
            (
                "claude-haiku-4-5".to_owned(),
                PriceOverride {
                    base: input_only(OLD.input),
                    ..PriceOverride::default()
                },
            ),
        ]);
        let (prices, changed) = Prices::new(&TABLE, &overrides);
        assert_eq!(
            changed,
            ["claude-new", "claude-opus-5-5"],
            "an override that matches the shipped price changes nothing"
        );
        assert_eq!(prices.as_of(), "2026-10-14");
        let opus = prices.lookup("claude-opus-5-5").unwrap();
        assert_eq!(opus.base, RateCard { input: 3.0, ..OPUS });
        assert_eq!(
            opus.fast,
            Some(RateCard {
                output: 30.0,
                ..OPUS_FAST
            })
        );
        assert_eq!(opus.web_search_per_1k, Some(10.0));
        let new = prices.lookup("claude-new").unwrap();
        assert_eq!(
            *new,
            ModelPrice {
                base: RateCard {
                    input: 1.0,
                    output: 2.0,
                    cache_write_5m: 3.0,
                    cache_write_1h: 4.0,
                    cache_read: 0.5,
                },
                long_prompt: None,
                fast: None,
                web_search_per_1k: Some(5.0),
            }
        );
        assert_eq!(
            prices.lookup("claude-partial"),
            None,
            "an incomplete base adds nothing"
        );
        assert_eq!(prices.lookup("claude-haiku-4-5").map(|p| p.base), Some(OLD));
    }

    #[test]
    fn a_card_the_entry_lacks_needs_every_rate_and_bad_rates_are_ignored() {
        let overrides = BTreeMap::from([
            // A fast card for a model with none is added only whole.
            (
                "claude-haiku-4-5".to_owned(),
                PriceOverride {
                    fast: input_only(2.0),
                    ..PriceOverride::default()
                },
            ),
            // A new threshold keeps the shipped long-prompt card; a rate that is not finite is
            // ignored.
            (
                "claude-haiku-5-5".to_owned(),
                PriceOverride {
                    base: input_only(f64::NAN),
                    long_prompt_above: Some(50_000),
                    ..PriceOverride::default()
                },
            ),
            // A long-prompt card with no threshold has nothing to switch at.
            (
                "claude-opus-5-5".to_owned(),
                PriceOverride {
                    long_prompt: card(1.0, 1.0, 1.0, 1.0, 1.0),
                    ..PriceOverride::default()
                },
            ),
            // A dated name gets an entry of its own, from the undated one.
            (
                "claude-opus-5-5-20261001".to_owned(),
                PriceOverride {
                    long_prompt_above: Some(200_000),
                    long_prompt: card(1.0, 1.0, 1.0, 1.0, 1.0),
                    ..PriceOverride::default()
                },
            ),
            // A negative rate leaves the base incomplete, so no model is added.
            (
                "claude-negative".to_owned(),
                PriceOverride {
                    base: card(-1.0, 1.0, 1.0, 1.0, 1.0),
                    ..PriceOverride::default()
                },
            ),
        ]);
        let (prices, changed) = Prices::new(&TABLE, &overrides);
        assert_eq!(changed, ["claude-haiku-5-5", "claude-opus-5-5-20261001"]);
        assert_eq!(prices.lookup("claude-haiku-4-5").unwrap().fast, None);
        let haiku = prices.lookup("claude-haiku-5-5").unwrap();
        assert_eq!(haiku.base, HAIKU);
        assert_eq!(haiku.long_prompt, Some((50_000, HAIKU_LONG)));
        assert_eq!(prices.lookup("claude-opus-5-5").unwrap().long_prompt, None);
        let dated = prices.lookup("claude-opus-5-5-20261001").unwrap();
        assert_eq!(dated.base, OPUS);
        assert_eq!(dated.fast, Some(OPUS_FAST));
        assert_eq!(dated.long_prompt.map(|(above, _)| above), Some(200_000));
        assert_eq!(
            prices
                .lookup("claude-opus-5-5-20261002")
                .unwrap()
                .long_prompt,
            None,
            "another date still finds the shipped entry"
        );
        assert_eq!(prices.lookup("claude-negative"), None);
    }

    #[test]
    fn a_dated_override_equal_to_shipped_still_pins_its_entry() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "claude-opus-5-5".to_owned(),
            PriceOverride {
                base: input_only(3.0),
                ..PriceOverride::default()
            },
        );
        overrides.insert(
            "claude-opus-5-5-20261001".to_owned(),
            PriceOverride {
                base: input_only(4.0),
                ..PriceOverride::default()
            },
        );
        let (prices, changed) = Prices::new(&TABLE, &overrides);
        assert_eq!(changed, vec!["claude-opus-5-5".to_owned()]);
        let input = |model: &str| prices.lookup(model).map(|p| p.base.input);
        assert_eq!(input("claude-opus-5-5"), Some(3.0));
        assert_eq!(input("claude-opus-5-5-20261001"), Some(4.0));
        assert_eq!(input("claude-opus-5-5-20270101"), Some(3.0));
    }

    #[test]
    fn an_unknown_model_is_unpriced() {
        let prices = shipped();
        let c = tokens(1, 1, 1, 1, 1);
        assert_eq!(prices.lookup("gpt-x"), None);
        assert_eq!(prices.cost_nanos("gpt-x", &c, None), None);
        assert_eq!(prices.cost_nanos("gpt-x", &c, Some("fast")), None);
        let (empty, changed) = Prices::new(&EMPTY_PRICES, &BTreeMap::new());
        assert!(changed.is_empty());
        assert_eq!(empty.as_of(), "");
        assert_eq!(empty.cost_nanos("claude-opus-5-5", &c, None), None);
    }
}
