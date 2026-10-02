/// What the identity oracle (§7.6) said about the live credential, already discarded if the
/// live bytes changed after it was asked (§9.4 step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleVerdict {
    Unavailable,
    ThisAccount,
    OtherIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutgoingFacts {
    pub bytes_equal_vault: bool,
    pub fp_equal_vault: bool,
    /// The live credential is the vault's `.prev` generation: an active-token refresh stored a
    /// newer one it could not publish (§7.5), so capturing this one would put a consumed token
    /// back (§9.4 step 4 `Superseded`).
    pub equals_vault_prev: bool,
    /// An OAuth blob with both tokens empty: CC's reaction to `invalid_grant`.
    pub wiped: bool,
    /// The live credential carries no token at all (it has no fingerprint), for example an
    /// entry holding only machine-shared keys: nothing account-scoped to keep.
    pub tokenless: bool,
    pub oracle: OracleVerdict,
    /// The live credential lacks a refresh token while the vault's has one (§6.2).
    pub lacks_refresh_over_complete: bool,
    /// §9.4 step 4 / §12.5: the live store is stale-marked for the outgoing account, so a
    /// capture would undo an explicit replacement. Turns a capture into `Displace`.
    pub live_store_stale: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingClass {
    Ours,
    /// The vault holds a newer generation than the live one (§9.4 step 4, amended).
    Superseded,
    Wiped,
    OursRotated,
    Foreign,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingAction {
    Nothing,
    CaptureToVault { backfill_uuid: bool },
    Displace,
}

/// The §9.4 step 4 table, `Superseded` included, with §6.2's bounds on an automatic capture. A
/// capture never replaces a refresh token with a credential that lacks one, and never takes a
/// stale-marked live store (§12.5), which would undo the replacement that marked it. Either is
/// displaced instead, under its class. A credential with no token at all is never captured
/// either: it is left alone like a wiped blob, and the vault keeps its generation.
pub fn decide_outgoing(f: &OutgoingFacts) -> (OutgoingClass, OutgoingAction) {
    if f.bytes_equal_vault || f.fp_equal_vault {
        return (OutgoingClass::Ours, OutgoingAction::Nothing);
    }
    if f.equals_vault_prev {
        return (OutgoingClass::Superseded, OutgoingAction::Nothing);
    }
    if f.wiped || f.tokenless {
        return (OutgoingClass::Wiped, OutgoingAction::Nothing);
    }
    let (class, backfill_uuid) = match f.oracle {
        OracleVerdict::ThisAccount => (OutgoingClass::OursRotated, true),
        OracleVerdict::OtherIdentity => return (OutgoingClass::Foreign, OutgoingAction::Displace),
        OracleVerdict::Unavailable => (OutgoingClass::Unresolved, false),
    };
    if f.lacks_refresh_over_complete || f.live_store_stale {
        (class, OutgoingAction::Displace)
    } else {
        (class, OutgoingAction::CaptureToVault { backfill_uuid })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> OutgoingFacts {
        OutgoingFacts {
            bytes_equal_vault: false,
            fp_equal_vault: false,
            equals_vault_prev: false,
            wiped: false,
            tokenless: false,
            oracle: OracleVerdict::Unavailable,
            lacks_refresh_over_complete: false,
            live_store_stale: false,
        }
    }

    #[test]
    fn precedence_follows_the_table_order_when_facts_combine() {
        // L300: a vault match outranks any oracle answer; a wiped or tokenless credential
        // outranks the oracle; and a foreign answer is displaced whatever else holds.
        let ours = OutgoingFacts {
            fp_equal_vault: true,
            oracle: OracleVerdict::OtherIdentity,
            lacks_refresh_over_complete: true,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&ours),
            (OutgoingClass::Ours, OutgoingAction::Nothing)
        );
        let wiped = OutgoingFacts {
            wiped: true,
            oracle: OracleVerdict::OtherIdentity,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&wiped),
            (OutgoingClass::Wiped, OutgoingAction::Nothing)
        );
        let foreign = OutgoingFacts {
            oracle: OracleVerdict::OtherIdentity,
            lacks_refresh_over_complete: true,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&foreign),
            (OutgoingClass::Foreign, OutgoingAction::Displace)
        );
    }

    #[test]
    fn ours_by_bytes_or_fingerprint() {
        let f = OutgoingFacts {
            bytes_equal_vault: true,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&f),
            (OutgoingClass::Ours, OutgoingAction::Nothing)
        );
        let f = OutgoingFacts {
            fp_equal_vault: true,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&f),
            (OutgoingClass::Ours, OutgoingAction::Nothing)
        );
    }

    #[test]
    fn a_live_generation_the_vault_superseded_is_left_alone() {
        // §9.4 step 4 `Superseded`: an active-token refresh stored a newer generation it could
        // not publish. Capturing the live one would put a consumed token back, whatever the
        // oracle says; `.prev` keeps it, so leaving it alone loses nothing.
        for oracle in [
            OracleVerdict::ThisAccount,
            OracleVerdict::OtherIdentity,
            OracleVerdict::Unavailable,
        ] {
            let f = OutgoingFacts {
                equals_vault_prev: true,
                oracle,
                ..facts()
            };
            assert_eq!(
                decide_outgoing(&f),
                (OutgoingClass::Superseded, OutgoingAction::Nothing),
                "{oracle:?}"
            );
        }
        // The vault's own generation still classifies as `Ours` first.
        let f = OutgoingFacts {
            equals_vault_prev: true,
            fp_equal_vault: true,
            ..facts()
        };
        assert_eq!(decide_outgoing(&f).0, OutgoingClass::Ours);
    }

    #[test]
    fn wiped_keeps_the_vault() {
        let f = OutgoingFacts {
            wiped: true,
            oracle: OracleVerdict::ThisAccount,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&f),
            (OutgoingClass::Wiped, OutgoingAction::Nothing)
        );
    }

    #[test]
    fn a_credential_with_no_token_is_never_captured() {
        // Nothing account-scoped to capture, whatever the oracle says: like a wiped blob.
        for oracle in [
            OracleVerdict::ThisAccount,
            OracleVerdict::OtherIdentity,
            OracleVerdict::Unavailable,
        ] {
            let f = OutgoingFacts {
                tokenless: true,
                oracle,
                ..facts()
            };
            assert_eq!(
                decide_outgoing(&f),
                (OutgoingClass::Wiped, OutgoingAction::Nothing),
                "{oracle:?}"
            );
        }
        // A wiped OAuth blob has no token either, and classifies as it always did.
        let f = OutgoingFacts {
            wiped: true,
            tokenless: true,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&f),
            (OutgoingClass::Wiped, OutgoingAction::Nothing)
        );
    }

    #[test]
    fn oracle_decides_rotated_or_foreign() {
        let f = OutgoingFacts {
            oracle: OracleVerdict::ThisAccount,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&f),
            (
                OutgoingClass::OursRotated,
                OutgoingAction::CaptureToVault {
                    backfill_uuid: true
                }
            )
        );
        let f = OutgoingFacts {
            oracle: OracleVerdict::OtherIdentity,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&f),
            (OutgoingClass::Foreign, OutgoingAction::Displace)
        );
    }

    #[test]
    fn no_verdict_is_unresolved_and_captured() {
        assert_eq!(
            decide_outgoing(&facts()),
            (
                OutgoingClass::Unresolved,
                OutgoingAction::CaptureToVault {
                    backfill_uuid: false
                }
            )
        );
    }

    #[test]
    fn a_blob_without_a_refresh_token_never_replaces_a_complete_one() {
        for oracle in [OracleVerdict::ThisAccount, OracleVerdict::Unavailable] {
            let f = OutgoingFacts {
                oracle,
                lacks_refresh_over_complete: true,
                ..facts()
            };
            assert_eq!(decide_outgoing(&f).1, OutgoingAction::Displace);
        }
    }

    #[test]
    fn a_stale_marked_live_store_is_displaced_never_captured() {
        // §9.4 step 4, §12.5: the class stays, so the log still says which capture it was.
        for (oracle, class) in [
            (OracleVerdict::ThisAccount, OutgoingClass::OursRotated),
            (OracleVerdict::Unavailable, OutgoingClass::Unresolved),
        ] {
            let f = OutgoingFacts {
                oracle,
                live_store_stale: true,
                ..facts()
            };
            assert_eq!(
                decide_outgoing(&f),
                (class, OutgoingAction::Displace),
                "{oracle:?}"
            );
        }
        // The rows that capture nothing are as they were.
        let stale = OutgoingFacts {
            live_store_stale: true,
            ..facts()
        };
        for (f, want) in [
            (
                OutgoingFacts {
                    fp_equal_vault: true,
                    ..stale
                },
                (OutgoingClass::Ours, OutgoingAction::Nothing),
            ),
            (
                OutgoingFacts {
                    equals_vault_prev: true,
                    ..stale
                },
                (OutgoingClass::Superseded, OutgoingAction::Nothing),
            ),
            (
                OutgoingFacts {
                    wiped: true,
                    ..stale
                },
                (OutgoingClass::Wiped, OutgoingAction::Nothing),
            ),
            (
                OutgoingFacts {
                    oracle: OracleVerdict::OtherIdentity,
                    ..stale
                },
                (OutgoingClass::Foreign, OutgoingAction::Displace),
            ),
        ] {
            assert_eq!(decide_outgoing(&f), want);
        }
    }
}
