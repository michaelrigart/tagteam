//! §12.5 "Profile provenance": a profile's credential P against the vault's V, through the seed
//! S the two last agreed on, never by expiry (B.52). Pure: the engine reads the three
//! fingerprints and the stale mark, and acts on the verdict.

/// §12.5's table, as a pure function of three generation fingerprints and the stale mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceVerdict {
    /// P = V. `reseed`: the seed differs from V and moves to it.
    InStep { reseed: bool },
    /// P ≠ V, V = S, not stale-marked: the profile rotated; capture P, and the seed becomes P.
    Capture,
    /// P ≠ V, P = S: the vault moved on; P is older and may be consumed. Never captured.
    VaultMovedOn,
    /// Stale-marked and P ≠ V (with P ≠ S): the explicit replacement wins. Never captured.
    ReplacementWins,
    /// P ≠ V, P ≠ S, V ≠ S, not stale-marked: both moved in an unknown order.
    Conflict,
}

/// `p`, `v`, `seed` are generation fingerprints (`sha256:…`, §2). Rows are checked in the
/// spec's order: P = V first, then P = S, then V = S, then the stale mark. V = S with the stale
/// mark set is `ReplacementWins`: a replacement never loses to a capture.
pub fn provenance(p: &str, v: &str, seed: &str, stale_marked: bool) -> ProvenanceVerdict {
    if p == v {
        return ProvenanceVerdict::InStep { reseed: seed != v };
    }
    if p == seed {
        return ProvenanceVerdict::VaultMovedOn;
    }
    match (v == seed, stale_marked) {
        (_, true) => ProvenanceVerdict::ReplacementWins,
        (true, false) => ProvenanceVerdict::Capture,
        (false, false) => ProvenanceVerdict::Conflict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "sha256:p";
    const V: &str = "sha256:v";
    const S: &str = "sha256:s";

    #[test]
    fn every_row_of_the_table() {
        use ProvenanceVerdict::*;
        // (P, V, S, stale-marked) and the verdict, for each row of §12.5's table, with both
        // stale marks wherever the row does not fix it.
        let rows = [
            ((V, V, V, false), InStep { reseed: false }),
            ((V, V, S, false), InStep { reseed: true }),
            ((V, V, V, true), InStep { reseed: false }),
            ((V, V, S, true), InStep { reseed: true }),
            ((P, V, V, false), Capture),
            ((P, V, V, true), ReplacementWins),
            ((P, V, P, false), VaultMovedOn),
            ((P, V, P, true), VaultMovedOn),
            ((P, V, S, true), ReplacementWins),
            ((P, V, S, false), Conflict),
        ];
        for ((p, v, s, stale), want) in rows {
            assert_eq!(
                provenance(p, v, s, stale),
                want,
                "P={p} V={v} S={s} stale={stale}"
            );
        }
    }

    #[test]
    fn only_the_equalities_decide() {
        // Every assignment of three names to P, V and S, both stale marks: the verdict depends
        // on which of them are equal, read in the spec's order, and on nothing else.
        use ProvenanceVerdict::*;
        let names = ["sha256:a", "sha256:b", "sha256:c"];
        for p in names {
            for v in names {
                for s in names {
                    for stale in [false, true] {
                        let want = if p == v {
                            InStep { reseed: s != v }
                        } else if p == s {
                            VaultMovedOn
                        } else if stale {
                            ReplacementWins
                        } else if v == s {
                            Capture
                        } else {
                            Conflict
                        };
                        assert_eq!(provenance(p, v, s, stale), want, "{p} {v} {s} {stale}");
                    }
                }
            }
        }
    }
}
