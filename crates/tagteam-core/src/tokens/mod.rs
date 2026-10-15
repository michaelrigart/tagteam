//! The token report's pure core (token report §3.1): token counts and the merge rule, prices
//! and cost, attribution and the default home's timeline, 5-hour blocks, and day and month
//! bucketing. No I/O; times are epoch milliseconds.

mod attribution;
mod blocks;
mod bucket;
mod counts;
mod price;

pub use attribution::*;
pub use blocks::*;
pub use bucket::*;
pub use counts::*;
pub use price::*;

#[cfg(test)]
pub(crate) mod test_support {
    use proptest::test_runner::{Config, RngAlgorithm, RngSeed};

    /// A fixed seed and `cases` cases, failures not persisted, as in `tests/simulate.rs`: every
    /// run is the same run.
    pub(crate) fn pinned(cases: u32) -> Config {
        Config {
            cases,
            rng_algorithm: RngAlgorithm::ChaCha,
            rng_seed: RngSeed::Fixed(0x746f_6b65_6e73_0000),
            failure_persistence: None,
            ..Config::default()
        }
    }
}
