//! The read port (TUI spec §4.4): what the screens show, read from the engine on a lane's own
//! thread, never on the UI thread, and the token index's catch-up (§6.4).

use std::sync::Arc;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::daemon::{daemon_lock_path, read_daemon_record};
use tagteam_engine::error::EngineError;
use tagteam_engine::tokens::{
    IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport, Wait,
};
use tagteam_engine::views::{HistoryView, ProviderAccounts, ShellAccount, StatusView};
use tagteam_provider::{Liveness, Read, RunShell};

/// A read that failed, in the CLI's own words (`kind` is the JSON `error.type`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub kind: String,
    pub message: String,
}

impl From<EngineError> for SourceError {
    fn from(e: EngineError) -> Self {
        SourceError {
            kind: e.kind().to_owned(),
            message: e.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountsSnapshot {
    pub providers: Vec<ProviderAccounts>,
    /// The run shell's own account (`ShellAccount`), for `▶ this`.
    pub this: Option<AccountId>,
    /// The default provider's live login, when tagteam does not manage it
    /// (`StatusView::Unmanaged`).
    pub unmanaged_live: Option<String>,
    pub in_run_shell: bool,
    /// The engine clock, in seconds, when this was read.
    pub read_at_s: i64,
    /// Each listed provider whose engine lock names a live process other than the daemon: its
    /// pid (C-16, T2-11).
    pub auto_holders: Vec<(ProviderId, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectSummary {
    pub fetched: usize,
    pub warnings: Vec<String>,
    /// The accounts whose reading was recorded (`Collected::Recorded`).
    pub updated: usize,
    /// The accounts the usage endpoint refused for rate limiting (`Failed { kind: "http-429" }`).
    pub rate_limited: Vec<AccountId>,
}

pub trait Source {
    fn accounts(&self, provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError>;
    fn history(&self, account: &AccountId, since_s: i64) -> Result<HistoryView, SourceError>;
    fn collect_due(&self, accounts: &[AccountId]) -> Result<CollectSummary, SourceError>;
    /// The token index's catch-up (token report §5), on the index lane: `Busy` at once while
    /// another process refreshes, and stopped at its next chunk by the shutdown token (§9).
    fn refresh_tokens(
        &self,
        provider: &ProviderId,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<RefreshOutcome, SourceError>;
    /// The token report (token report §10), on the report lane. It only reads.
    fn token_report(&self, query: &TokenQuery) -> Result<TokenReport, SourceError>;
    /// The index's state alone (token report §10): a few small reads, for deciding whether to
    /// query again (§6.4).
    fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, SourceError>;
}

/// Builds one `Source` inside the thread that will use it, so neither `Engine` nor the
/// binary's `Context` needs to be `Send` or `Sync`.
pub type SourceFactory = Arc<dyn Fn() -> Box<dyn Source> + Send + Sync>;

pub struct EngineSource {
    engine: Engine,
}

impl EngineSource {
    pub fn new(engine: Engine) -> Self {
        EngineSource { engine }
    }
}

impl Source for EngineSource {
    /// `list`'s view (`Engine::accounts`), with what the TUI shows around it: the run shell's
    /// own account (`shell_account`), the live login when tagteam does not manage it
    /// (`status`), whether this is a run shell, and the engine clock.
    fn accounts(&self, provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
        let engine = &self.engine;
        let providers = engine.accounts(provider)?;
        let this = match engine.shell_account()? {
            ShellAccount::Managed(row) => Some(row.id),
            ShellAccount::NotInShell | ShellAccount::Unmanaged => None,
        };
        // Only a provider with no managed account active can have an unmanaged live login;
        // asking `status` then reads its view once more. One it cannot read is no reason to
        // fail the views: `list` shows no such login either.
        let live = provider.unwrap_or(engine.default_provider());
        let managed = providers
            .iter()
            .any(|p| &p.provider == live && p.active_position.is_some());
        let unmanaged_live = match managed {
            true => None,
            false => match engine.status(live) {
                Ok(StatusView::Unmanaged { email }) => Some(email),
                Ok(StatusView::NoLogin | StatusView::Managed { .. }) | Err(_) => None,
            },
        };
        let auto_holders = foreign_engine_holders(engine, &providers);
        Ok(AccountsSnapshot {
            providers,
            this,
            unmanaged_live,
            in_run_shell: matches!(engine.run_shell(), RunShell::Inside { .. }),
            read_at_s: engine.now_ms().div_euclid(1000),
            auto_holders,
        })
    }

    /// `history`'s view (core §13.4) of every relevant window since `since_s`.
    fn history(&self, account: &AccountId, since_s: i64) -> Result<HistoryView, SourceError> {
        Ok(self.engine.history(account, None, since_s)?)
    }

    /// `list`'s on-demand collection (core §8.3): the collector fetches only what is due.
    /// `fetched` counts the accounts whose stored usage changed, `updated` those with a new
    /// reading, and `rate_limited` lists those the endpoint refused.
    fn collect_due(&self, accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
        if accounts.is_empty() {
            return Ok(CollectSummary {
                fetched: 0,
                warnings: vec![],
                updated: 0,
                rate_limited: vec![],
            });
        }
        let report = self.engine.collect_usage(CollectMode::OnDemand {
            accounts: accounts.to_vec(),
        })?;
        Ok(CollectSummary {
            fetched: report.outcomes.iter().filter(|(_, c)| recorded(c)).count(),
            warnings: report.warnings,
            updated: report
                .outcomes
                .iter()
                .filter(|(_, c)| matches!(c, Collected::Recorded))
                .count(),
            rate_limited: report
                .outcomes
                .iter()
                .filter(|(_, c)| matches!(c, Collected::Failed { kind } if kind == "http-429"))
                .map(|(id, _)| id.clone())
                .collect(),
        })
    }

    /// `Engine::refresh_tokens` as spec §9 has the TUI call it: `Wait::Try` and no budget, with
    /// the engine's own cancel token, which is the shutdown token (§4.3).
    fn refresh_tokens(
        &self,
        provider: &ProviderId,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<RefreshOutcome, SourceError> {
        let engine = &self.engine;
        Ok(engine.refresh_tokens(provider, Wait::Try, None, engine.cancel(), progress)?)
    }

    fn token_report(&self, query: &TokenQuery) -> Result<TokenReport, SourceError> {
        Ok(self.engine.token_report(query)?)
    }

    fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, SourceError> {
        Ok(self.engine.token_index_state(provider)?)
    }
}

/// Each of `providers` whose engine lock record names a process that runs, judged by an exact pid
/// and start-time match as `doctor` does (an undeterminable one counts as live), and that is not
/// the daemon `daemon.lock` names: a `tagteam auto` in another terminal (C-16, T2-11).
fn foreign_engine_holders(
    engine: &Engine,
    providers: &[ProviderAccounts],
) -> Vec<(ProviderId, u32)> {
    let daemon = match read_daemon_record(&daemon_lock_path(engine.env())) {
        Read::Present(r) => Some((r.pid, r.start)),
        Read::Absent | Read::Unreadable(_) => None,
    };
    providers
        .iter()
        .filter_map(|p| match engine.engine_holder(&p.provider) {
            Read::Present(stamp)
                if stamp.liveness() != Liveness::Dead
                    && daemon != Some((stamp.pid, stamp.start)) =>
            {
                Some((p.provider.clone(), stamp.pid))
            }
            Read::Present(_) | Read::Absent | Read::Unreadable(_) => None,
        })
        .collect()
}

/// Whether one account's collection changed what the views show (`Collected`): a reading was
/// recorded (`Recorded`), or a failure was (`Failed`, and `OverBudget`, which the store records
/// as an `over-budget` failure). `Failed { kind: "error" }` recorded nothing, nor did
/// `Ineligible`, `Dropped` or `Unsupported`.
fn recorded(c: &Collected) -> bool {
    match c {
        Collected::Recorded | Collected::OverBudget { .. } => true,
        Collected::Failed { kind } => kind != "error",
        Collected::Ineligible(_) | Collected::Dropped | Collected::Unsupported => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use super::*;

    #[test]
    fn an_engine_error_keeps_its_kind_and_its_words() {
        let e: SourceError = EngineError::NoSuchAccount("9".into()).into();
        assert_eq!(
            e,
            SourceError {
                kind: "no-such-account".into(),
                message: "no account matches \"9\"".into(),
            }
        );
        let e: SourceError = EngineError::InsideRunShell.into();
        assert_eq!(e.kind, EngineError::InsideRunShell.kind());
        assert_eq!(e.message, EngineError::InsideRunShell.to_string());
    }

    /// A source that answers every read with an error naming the read, so a test can tell
    /// which one it called.
    struct Named(&'static str);

    impl Source for Named {
        fn accounts(&self, _: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
            Err(SourceError {
                kind: "accounts".into(),
                message: self.0.into(),
            })
        }
        fn history(&self, _: &AccountId, _: i64) -> Result<HistoryView, SourceError> {
            Err(SourceError {
                kind: "history".into(),
                message: self.0.into(),
            })
        }
        fn collect_due(&self, _: &[AccountId]) -> Result<CollectSummary, SourceError> {
            Ok(CollectSummary {
                fetched: 0,
                warnings: vec![self.0.into()],
                updated: 0,
                rate_limited: vec![],
            })
        }
        fn refresh_tokens(
            &self,
            _: &ProviderId,
            _: &mut dyn FnMut(&IndexProgress),
        ) -> Result<RefreshOutcome, SourceError> {
            Ok(RefreshOutcome::Busy)
        }
        fn token_report(&self, _: &TokenQuery) -> Result<TokenReport, SourceError> {
            Err(SourceError {
                kind: "token_report".into(),
                message: self.0.into(),
            })
        }
        fn token_index_state(&self, _: &ProviderId) -> Result<IndexState, SourceError> {
            Err(SourceError {
                kind: "token_index_state".into(),
                message: self.0.into(),
            })
        }
    }

    #[test]
    fn a_factory_builds_a_source_in_the_thread_that_calls_it() {
        // The factory is shared between threads; the source it builds never leaves its own.
        let built = Arc::new(AtomicUsize::new(0));
        let counter = built.clone();
        let factory: SourceFactory = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Box::new(Named("built"))
        });
        let from_a_thread = factory.clone();
        let message = thread::spawn(move || {
            let source = from_a_thread();
            source.accounts(None).unwrap_err().message
        })
        .join()
        .unwrap();
        assert_eq!(message, "built");
        let source = factory();
        assert_eq!(
            source.collect_due(&[]).unwrap().warnings,
            ["built".to_owned()]
        );
        assert_eq!(built.load(Ordering::SeqCst), 2, "one source per call");
    }
}
