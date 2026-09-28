use tagteam_core::OracleVerdict;
use tagteam_provider::{Credential, Identity, Provider};

use crate::store::AccountRow;

/// Resolves who owns a live access token (§7.6). Advisory only; never called under a lock.
pub trait Oracle: Send + Sync {
    fn resolve(&self, provider: &dyn Provider, credential: &Credential) -> Option<Identity>;
}

/// M1 makes no network calls, so the oracle is always unavailable.
pub struct NoOracle;

impl Oracle for NoOracle {
    fn resolve(&self, _: &dyn Provider, _: &Credential) -> Option<Identity> {
        None
    }
}

/// Attribution to an account needs a positive uuid match; with no stored uuid yet, email and
/// org must agree (then `account_uuid` is backfilled).
pub fn verdict(resolved: Option<&Identity>, account: &AccountRow) -> OracleVerdict {
    let Some(r) = resolved else {
        return OracleVerdict::Unavailable;
    };
    let same = match (&account.account_uuid, &r.account_uuid) {
        (Some(a), Some(b)) => a == b,
        (None, _) => r.email == account.email && r.org_uuid == account.org_uuid,
        (Some(_), None) => false,
    };
    if same {
        OracleVerdict::ThisAccount
    } else {
        OracleVerdict::OtherIdentity
    }
}
