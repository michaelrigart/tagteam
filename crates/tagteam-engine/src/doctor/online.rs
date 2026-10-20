use std::sync::Arc;

use tagteam_provider::http::{HttpError, HttpRequest};
use tagteam_provider::{Check, Provider};

use crate::error::EngineError;

use super::{Id, Run, Secrets, skipped, state_of, worst};

/// How long `--online` waits for each host (§13.6).
const ONLINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl Run<'_> {
    /// §13.6 `--online`: TLS reachability of `p`'s hosts through the engine's `Http` port, in
    /// parallel, with no credentials: any HTTP response reaches the host, a `PreSend` failure
    /// does not, and an `Ambiguous` one cannot tell. A signal that has landed sends nothing.
    pub(super) fn online(&mut self, p: &dyn Provider) -> Result<(), EngineError> {
        let hosts = p.doctor_hosts();
        self.engine.check_cancel()?;
        let http = self.engine.http.as_ref();
        let replies: Vec<(String, Result<u16, HttpError>)> = std::thread::scope(|s| {
            let handles: Vec<_> = hosts
                .iter()
                .map(|url| {
                    s.spawn(move || {
                        let reply = http.send(&HttpRequest::get(url.clone(), ONLINE_TIMEOUT));
                        (url.clone(), reply.map(|r| r.status))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("a reachability probe never panics"))
                .collect()
        });
        let id = p.id();
        for (url, reply) in replies {
            let check = match reply {
                Ok(status) => Check::ok(Id::ONLINE_REACH, format!("{url} answers (HTTP {status})")),
                Err(HttpError::PreSend(e)) => {
                    Check::fail(Id::ONLINE_REACH, format!("{url} cannot be reached: {e}"))
                        .fix("check the network, any proxy (HTTPS_PROXY) and the TLS trust store")
                }
                Err(HttpError::Ambiguous(e)) => Check::warn(
                    Id::ONLINE_REACH,
                    format!("{url} took the request but gave no answer: {e}"),
                )
                .fix("try again; if it persists, check the network and any proxy"),
            };
            self.push(Some(&id), check);
        }
        Ok(())
    }

    /// §13.6: the one warning for every check skipped because the Keychain could not be read
    /// without unlocking it, the vault's or a provider's. Said once, only when a check needed a
    /// secret from a Keychain that was locked or whose state cannot be told (a locked one over
    /// the other). It names the checks that were skipped, and the Keychain as "the login
    /// keychain" unless only the vault's is the one at fault while a provider's is not in the
    /// same state. Both in one state cannot be told apart from one Keychain, which is every
    /// release run: that is the wording of a single Keychain, unchanged.
    pub(super) fn keychain_note(&mut self, providers: &[Arc<dyn Provider>]) {
        let vault = self
            .secrets
            .get()
            .copied()
            .filter(|s| *s != Secrets::Readable);
        let provider = worst(self.provider_secrets.borrow().values().copied());
        let provider = (provider != Secrets::Readable).then_some(provider);
        let Some(state) = worst(vault.into_iter().chain(provider)).bad() else {
            return;
        };
        // What each side is in now, whether or not a check asked: the same state on both is
        // one Keychain as far as anyone can tell.
        let vault_now = self.secrets();
        let provider_now = worst(providers.iter().map(|p| self.provider_secrets(p.as_ref())));
        let check = if vault_now == provider_now {
            Check::warn(
                Id::KEYCHAIN_LOCKED,
                format!(
                    "{}, so the checks that read it were skipped: {}",
                    state_of("the login keychain", state),
                    skipped(true, true)
                ),
            )
        } else {
            let mut clauses = Vec::new();
            if let Some(v) = vault {
                clauses.push(state_of("the vault's keychain", v));
            }
            if let Some(p) = provider {
                clauses.push(state_of("the login keychain", p));
            }
            Check::warn(
                Id::KEYCHAIN_LOCKED,
                format!(
                    "{}, so the checks that read {} were skipped: {}",
                    clauses.join(", and "),
                    if clauses.len() == 1 { "it" } else { "them" },
                    skipped(vault.is_some(), provider.is_some())
                ),
            )
        };
        let check = match state {
            Secrets::Locked => check.fix("`security unlock-keychain ~/Library/Keychains/login.keychain-db`, then `tagteam doctor` again"),
            _ => check.fix("`security show-keychain-info` shows why; unlock the keychain if it is locked, then `tagteam doctor` again"),
        };
        self.push(None, check);
    }
}
