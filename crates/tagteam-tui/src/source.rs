//! The read port (TUI spec §4.4): what the screens show, read from the engine on a lane's own
//! thread, never on the UI thread.

use std::sync::Arc;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::error::EngineError;
use tagteam_engine::views::{HistoryView, ProviderAccounts};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectSummary {
    pub fetched: usize,
    pub warnings: Vec<String>,
}

pub trait Source {
    fn accounts(&self, provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError>;
    fn history(&self, account: &AccountId, since_s: i64) -> Result<HistoryView, SourceError>;
    fn collect_due(&self, accounts: &[AccountId]) -> Result<CollectSummary, SourceError>;
}

/// Builds one `Source` inside the thread that will use it, so neither `Engine` nor the
/// binary's `Context` needs to be `Send` or `Sync`.
pub type SourceFactory = Arc<dyn Fn() -> Box<dyn Source> + Send + Sync>;

pub struct EngineSource {
    #[allow(dead_code)]
    engine: Engine,
}

impl EngineSource {
    pub fn new(engine: Engine) -> Self {
        EngineSource { engine }
    }
}

impl Source for EngineSource {
    fn accounts(&self, _provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
        unimplemented!("EngineSource::accounts")
    }

    fn history(&self, _account: &AccountId, _since_s: i64) -> Result<HistoryView, SourceError> {
        unimplemented!("EngineSource::history")
    }

    fn collect_due(&self, _accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
        unimplemented!("EngineSource::collect_due")
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
