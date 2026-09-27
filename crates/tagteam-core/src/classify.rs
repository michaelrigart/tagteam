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
    /// An OAuth blob with both tokens empty: CC's reaction to `invalid_grant`.
    pub wiped: bool,
    pub oracle: OracleVerdict,
    /// The live credential lacks a refresh token while the vault's has one (§6.2).
    pub lacks_refresh_over_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingClass {
    Ours,
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

/// The §9.4 step 4 table, with the §6.2 rule that an automatic capture never replaces a
/// refresh token with a credential that lacks one.
pub fn decide_outgoing(f: &OutgoingFacts) -> (OutgoingClass, OutgoingAction) {
    if f.bytes_equal_vault || f.fp_equal_vault {
        return (OutgoingClass::Ours, OutgoingAction::Nothing);
    }
    if f.wiped {
        return (OutgoingClass::Wiped, OutgoingAction::Nothing);
    }
    let (class, backfill_uuid) = match f.oracle {
        OracleVerdict::ThisAccount => (OutgoingClass::OursRotated, true),
        OracleVerdict::OtherIdentity => return (OutgoingClass::Foreign, OutgoingAction::Displace),
        OracleVerdict::Unavailable => (OutgoingClass::Unresolved, false),
    };
    if f.lacks_refresh_over_complete {
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
            wiped: false,
            oracle: OracleVerdict::Unavailable,
            lacks_refresh_over_complete: false,
        }
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
}
