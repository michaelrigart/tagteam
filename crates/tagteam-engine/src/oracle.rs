use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tagteam_core::OracleVerdict;
use tagteam_provider::http::Http;
use tagteam_provider::{Clock, Credential, Identity, Provider};

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

/// §7.6 over HTTP: the provider asks its own profile endpoint. Never called under a lock (the
/// engine's callers guarantee that); a failure is no answer, logged at DEBUG.
pub struct HttpOracle {
    http: Arc<dyn Http>,
    clock: Arc<dyn Clock>,
}

impl HttpOracle {
    pub fn new(http: Arc<dyn Http>, clock: Arc<dyn Clock>) -> Self {
        Self { http, clock }
    }
}

impl Oracle for HttpOracle {
    fn resolve(&self, provider: &dyn Provider, credential: &Credential) -> Option<Identity> {
        let answer = provider.resolve_owner(self.http.as_ref(), credential, self.clock.now_ms());
        if answer.is_none() {
            tracing::debug!(provider = %provider.id(), "the profile oracle gave no answer");
        }
        answer
    }
}

/// Asks `inner` at most once per process for a given credential (§7.6), keyed by provider and
/// fingerprint. No answer is remembered too: a retry within one command would only repeat a
/// failure the command already treats as advisory. The lock is never held across `inner`.
pub struct CachingOracle<O: Oracle> {
    inner: O,
    answers: Mutex<HashMap<(String, String), Option<Identity>>>,
}

impl<O: Oracle> CachingOracle<O> {
    pub fn new(inner: O) -> Self {
        Self {
            inner,
            answers: Mutex::new(HashMap::new()),
        }
    }
}

impl<O: Oracle> Oracle for CachingOracle<O> {
    fn resolve(&self, provider: &dyn Provider, credential: &Credential) -> Option<Identity> {
        let Some(fp) = provider.fingerprint(credential.bytes()) else {
            return self.inner.resolve(provider, credential);
        };
        let key = (provider.id().to_string(), fp.as_str().to_owned());
        if let Some(answer) = self.answers.lock().unwrap().get(&key) {
            return answer.clone();
        }
        let answer = self.inner.resolve(provider, credential);
        self.answers.lock().unwrap().insert(key, answer.clone());
        answer
    }
}

/// Attribution to an account needs a positive uuid match; with no stored uuid yet, email and
/// org must agree (then `account_uuid` is backfilled). A resolved identity is only usable when
/// its own uuid is a non-empty string (§7.6) — a resolved identity with no uuid of its own
/// carries no attribution signal at all, so it is `Unavailable` rather than compared by email.
pub fn verdict(resolved: Option<&Identity>, account: &AccountRow) -> OracleVerdict {
    let Some(r) = resolved else {
        return OracleVerdict::Unavailable;
    };
    let Some(r_uuid) = r.account_uuid.as_deref().filter(|u| !u.is_empty()) else {
        return OracleVerdict::Unavailable;
    };
    let same = match account.account_uuid.as_deref() {
        Some(a) => a == r_uuid,
        None => r.email == account.email && r.org_uuid == account.org_uuid,
    };
    if same {
        OracleVerdict::ThisAccount
    } else {
        OracleVerdict::OtherIdentity
    }
}
