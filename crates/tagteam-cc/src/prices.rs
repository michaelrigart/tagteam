//! The shipped API list prices for Claude Code's models (token report spec §8.1).

use tagteam_core::tokens::PriceTable;

/// Empty until the table ships; a model it lacks is unpriced (spec §8.3).
pub static PRICES: PriceTable = PriceTable {
    as_of: "",
    models: &[],
};
