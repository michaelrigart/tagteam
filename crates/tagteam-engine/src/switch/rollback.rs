use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{LiveLocks, ProviderError, Undo};

use crate::error::EngineError;
use crate::store::{JournalRow, Store};

/// §9.4 step 10: puts back what the switch wrote, in reverse order, then the journal row's
/// `prior` if it carried one, or no row at all. It runs on an error, and through `Drop` on a
/// panic; it is disarmed once the switch commits or has rolled back. It borrows the live
/// locks its undos run under, so it cannot outlive them (§9.4: "held throughout").
pub(super) struct Rollback<'a, 'g> {
    pub(super) undos: Vec<Box<dyn Undo + 'a>>,
    pub(super) locks: &'a LiveLocks<'g>,
    pub(super) store: &'a Store,
    pub(super) provider: &'a ProviderId,
    /// The switch's outgoing account and its target, which its log lines name (§14.2).
    pub(super) from: Option<AccountId>,
    pub(super) to: AccountId,
    pub(super) prior: Option<Box<JournalRow>>,
    /// A provider write that started and has not returned. Such a write puts its own partial
    /// state back when it fails or unwinds, but after a panic the engine cannot see whether
    /// that worked, so the journal row stays for recovery (§9.6) to confirm.
    pub(super) in_flight: bool,
    pub(super) armed: bool,
}

impl<'a> Rollback<'a, '_> {
    /// Runs one provider write and keeps its undo, handing back what else the write reported.
    pub(super) fn write<T>(
        &mut self,
        write: impl FnOnce() -> Result<(Box<dyn Undo + 'a>, T), ProviderError>,
    ) -> Result<T, ProviderError> {
        self.in_flight = true;
        let written = write();
        self.in_flight = false;
        let (undo, value) = written?;
        self.undos.push(undo);
        Ok(value)
    }

    pub(super) fn disarm(mut self) {
        self.armed = false;
    }

    /// Rolls back after `cause`, and says whether that worked. Either way the rollback is
    /// logged (§14.2: switches and their rollbacks), at WARN when every write was put back and
    /// at ERROR naming what was not. The cause is named by its `kind()` alone: its text may
    /// hold a label.
    pub(super) fn fail(mut self, cause: EngineError) -> EngineError {
        let partial = match &cause {
            EngineError::Provider(ProviderError::RestoreFailed { restore, .. }) => {
                vec![format!("restoring a partial write: {restore}")]
            }
            _ => vec![],
        };
        let failed = self.roll_back(partial);
        let from = self.from.as_ref().map(tracing::field::display);
        if failed.is_empty() {
            tracing::warn!(
                target: "tagteam_engine::switch",
                provider = %self.provider,
                from_account = from,
                to_account = %self.to,
                kind = cause.kind(),
                "rolled back a switch"
            );
            EngineError::RolledBack(cause.to_string())
        } else {
            // A count (§14.2): each entry names a Keychain service or a file path, and an
            // error's text.
            tracing::error!(
                target: "tagteam_engine::switch",
                provider = %self.provider,
                from_account = from,
                to_account = %self.to,
                kind = cause.kind(),
                failed = failed.len(),
                "a switch was not fully rolled back; its journal row stays for recovery"
            );
            EngineError::RollbackFailed {
                cause: cause.to_string(),
                failed: failed.join("; "),
            }
        }
    }

    /// Returns what could not be restored, `failed` included. The journal row is put back
    /// only when nothing failed: otherwise it stays for recovery (§9.6).
    fn roll_back(&mut self, mut failed: Vec<String>) -> Vec<String> {
        self.armed = false;
        if self.in_flight {
            failed.push("a live write interrupted part-way".into());
        }
        for undo in self.undos.drain(..).rev() {
            let what = undo.what();
            if let Err(e) = undo.undo(self.locks) {
                failed.push(format!("{what}: {e}"));
            }
        }
        if failed.is_empty() {
            let journal = match &self.prior {
                Some(row) => self.store.insert_journal(row),
                None => self.store.delete_journal(self.provider),
            };
            if let Err(e) = journal {
                failed.push(format!("the switch journal: {e}"));
            }
        }
        failed
    }
}

impl Drop for Rollback<'_, '_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let failed = self.roll_back(Vec::new());
        if failed.is_empty() {
            tracing::warn!(target: "tagteam_engine::switch", "an unwinding switch was rolled back");
        } else {
            // A count (§14.2): each entry names a Keychain service or a file path, and an
            // error's text.
            tracing::error!(
                target: "tagteam_engine::switch",
                failed = failed.len(),
                "an unwinding switch was not fully rolled back; its journal row stays for recovery"
            );
        }
    }
}
