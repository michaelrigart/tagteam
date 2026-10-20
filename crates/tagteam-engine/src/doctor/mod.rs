//! §13.6 `tagteam doctor`: tagteam's own state and its interop with each provider. Doctor is
//! read-only (B.67): it writes nothing and creates nothing itself. It takes no `MutationGuard`
//! and no account lock, runs no recovery, and takes no lock that anyone waits on. It
//! opens the store read-only and never migrates it. It tests locks without waiting, reads the
//! vault only when the Keychain is unlocked, and creates no data directory, store, log or lock
//! file. The `claude` it runs (`--version`, `auth status`) may make Claude Code's own start-up
//! writes in the home it starts in, as any `claude` start does (Appendix A.7); doctor does not
//! run `claude auth status` in a home whose global config does not exist. Every finding names
//! its fix. The daemon's checks (D10) read `daemon.lock`'s record without trying the lock. They
//! look at its socket without connecting to it, and ask `launchctl`, `systemctl` or
//! `loginctl` only to describe its service, each within 5 s.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_provider::{Check, CheckStatus, LockState, Provider};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::service::ServiceKind;
use crate::store::{AccountRow, Store};

mod accounts;
mod daemon;
mod files;
mod online;
mod profiles;
mod recovery;
mod sessions;
mod store;

/// What `doctor` covers (§13.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorOptions {
    /// `--online`: TLS reachability of each provider's hosts.
    pub online: bool,
    /// `--provider`: only this provider's checks, besides tagteam's own (§13.1).
    pub provider: Option<ProviderId>,
    /// This platform's service manager (D9), which the CLI names from its platform. `None`
    /// leaves out the checks that need one: `daemon.service` and `daemon.notify`.
    pub service: Option<ServiceKind>,
}

/// Every finding, each with the provider it belongs to: `None` for tagteam's own (§13.6's JSON
/// `provider: null`). tagteam's own come first, then each provider's, in check order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorReport {
    pub checks: Vec<(Option<ProviderId>, Check)>,
}

impl DoctorReport {
    /// §13.6: doctor exits 1 if any check fails.
    pub fn ok(&self) -> bool {
        self.checks
            .iter()
            .all(|(_, c)| c.status != CheckStatus::Fail)
    }
}

/// What the store let doctor read.
enum Stored {
    /// No data directory, or no database file in it.
    Nothing,
    /// A database this build cannot read as its own: unopenable, unsound, or another schema.
    Unusable,
    /// A sound store at this build's schema, and every account in it, all providers: `None`
    /// when those rows cannot be read, which `store.accounts` reports.
    Usable {
        store: Store,
        accounts: Option<Vec<AccountRow>>,
    },
}

/// Whether doctor may read secrets from one Keychain: the vault's, or a provider's own stores'
/// (§13.6, Appendix A.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Secrets {
    Readable,
    Locked,
    Unknown,
}

/// One doctor run: the read-only store, what was found, and the state of each Keychain (the
/// vault's, and each provider's own), asked at most once and only when a check needs a secret
/// from it.
struct Run<'e> {
    engine: &'e Engine,
    now_ms: i64,
    stored: Stored,
    secrets: OnceCell<Secrets>,
    provider_secrets: RefCell<BTreeMap<ProviderId, Secrets>>,
    out: Vec<(Option<ProviderId>, Check)>,
}

fn secrets_of(state: LockState) -> Secrets {
    match state {
        LockState::Unlocked => Secrets::Readable,
        LockState::Locked => Secrets::Locked,
        LockState::Unknown => Secrets::Unknown,
    }
}

/// The CLI words that run `args` against `provider`: `--provider` only when it is not the
/// default, so a single-provider user sees the commands they type.
fn command(engine: &Engine, provider: &ProviderId, args: &str) -> String {
    if provider == engine.default_provider() {
        format!("tagteam {args}")
    } else {
        format!("tagteam --provider {provider} {args}")
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

impl Engine {
    /// §13.6. Every check of tagteam's own, then, for each provider in scope, the engine's
    /// checks of that provider and the provider's own (`Provider::doctor_checks`). The scope is
    /// `--provider`'s alone, or else the default provider and every provider the store has an
    /// account of, in registry order. It fails only for a `--provider` this build lacks, and
    /// when a signal lands (a spawn is the only thing doctor waits on, §14.1).
    pub fn doctor(&self, opts: DoctorOptions) -> Result<DoctorReport, EngineError> {
        if let Some(p) = &opts.provider {
            self.provider(p)?;
        }
        let mut run = Run {
            engine: self,
            now_ms: self.now_ms(),
            stored: Stored::Nothing,
            secrets: OnceCell::new(),
            provider_secrets: RefCell::new(BTreeMap::new()),
            out: Vec::new(),
        };
        run.store();
        run.modes();
        run.settings(opts.provider.as_ref());
        run.log();
        run.daemon(opts.service);
        let providers = run.scope(opts.provider.as_ref());
        run.temp_files(&providers);
        run.vault_orphans();
        run.rescues(opts.provider.as_ref());
        run.displaced(opts.provider.as_ref());
        run.ownership_walk(opts.provider.as_ref());
        run.stray_profiles(opts.provider.as_ref());
        for p in &providers {
            run.accounts(p.as_ref());
            run.usage(p.as_ref());
            run.journal(p.as_ref());
            run.auto(p.as_ref());
            run.sessions(p.as_ref());
            self.check_cancel()?;
            let id = p.id();
            for check in p.doctor_checks(&self.env, self.spawner.as_ref(), self.cancel()) {
                run.push(Some(&id), check);
            }
            self.check_cancel()?;
            if opts.online {
                run.online(p.as_ref())?;
            }
        }
        run.keychain_note(&providers);
        Ok(run.finish(&providers))
    }
}

impl Run<'_> {
    fn push(&mut self, provider: Option<&ProviderId>, check: Check) {
        self.out.push((provider.cloned(), check));
    }

    fn store_ref(&self) -> Option<&Store> {
        match &self.stored {
            Stored::Usable { store, .. } => Some(store),
            _ => None,
        }
    }

    /// Every account, all providers, as `store` read them: `Some(vec![])` with no store at
    /// all, `None` when the store or its accounts cannot be read, which `store.skipped` or
    /// `store.accounts` reports.
    fn all_accounts(&self) -> Option<Vec<AccountRow>> {
        match &self.stored {
            Stored::Nothing => Some(Vec::new()),
            Stored::Unusable => None,
            Stored::Usable { accounts, .. } => accounts.clone(),
        }
    }

    /// The vault's Keychain's lock state, asked once (Appendix A.3's check, which never prompts
    /// and is bounded by its timeout); it gates every vault read. A backend with no Keychain
    /// (Linux) is always readable.
    fn secrets(&self) -> Secrets {
        *self
            .secrets
            .get_or_init(|| match self.engine.vault.keychain() {
                None => Secrets::Readable,
                Some(k) => secrets_of(k.lock_state()),
            })
    }

    /// The lock state of the Keychain `p` reads its own stores from (the live login, its
    /// profiles' credentials), asked once per provider; it gates those reads, which the vault's
    /// state does not speak for when the two Keychains differ. A provider with no Keychain is
    /// always readable.
    fn provider_secrets(&self, p: &dyn Provider) -> Secrets {
        let id = p.id();
        if let Some(known) = self.provider_secrets.borrow().get(&id) {
            return *known;
        }
        let state = p
            .keychain_lock_state()
            .map_or(Secrets::Readable, secrets_of);
        self.provider_secrets.borrow_mut().insert(id, state);
        state
    }

    /// The providers in scope (`DoctorOptions::provider`, or the default provider and every
    /// provider with an account), in registry order.
    fn scope(&self, only: Option<&ProviderId>) -> Vec<Arc<dyn Provider>> {
        let with_accounts: BTreeSet<ProviderId> = self
            .all_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.provider)
            .collect();
        self.engine
            .registry
            .all()
            .iter()
            .filter(|p| {
                let id = p.id();
                match only {
                    Some(only) => &id == only,
                    None => &id == self.engine.default_provider() || with_accounts.contains(&id),
                }
            })
            .cloned()
            .collect()
    }

    /// tagteam's own findings first, then each provider's in scope order, each group in the
    /// order its checks ran (§13.6: grouped per provider, as `list` groups accounts).
    fn finish(mut self, providers: &[Arc<dyn Provider>]) -> DoctorReport {
        let rank = |p: &Option<ProviderId>| match p {
            None => 0,
            Some(id) => {
                1 + providers
                    .iter()
                    .position(|q| &q.id() == id)
                    .unwrap_or(providers.len())
            }
        };
        self.out.sort_by_key(|(p, _)| rank(p));
        DoctorReport { checks: self.out }
    }
}

impl Secrets {
    /// The state that warrants the note, if this is one.
    fn bad(self) -> Option<Secrets> {
        (self != Secrets::Readable).then_some(self)
    }
}

/// The worst of `states`: a locked Keychain over one whose state cannot be told over a readable one.
fn worst(states: impl IntoIterator<Item = Secrets>) -> Secrets {
    let states: Vec<Secrets> = states.into_iter().collect();
    [Secrets::Locked, Secrets::Unknown]
        .into_iter()
        .find(|s| states.contains(s))
        .unwrap_or(Secrets::Readable)
}

/// What is said of `keychain` in `state`: the note's wording for a locked one, or one whose
/// state cannot be told.
fn state_of(keychain: &str, state: Secrets) -> String {
    match state {
        Secrets::Unknown => format!("{keychain}'s lock state cannot be told"),
        _ => format!("{keychain} is locked (common over SSH)"),
    }
}

/// The checks skipped for want of the vault's Keychain and of a provider's, as a sentence part.
fn skipped(vault: bool, provider: bool) -> String {
    let mut parts = Vec::new();
    if vault {
        parts.extend(["vault entries", "pending replacements"]);
    }
    if provider {
        parts.extend(["interrupted switches", "session credentials"]);
    }
    let last = parts.pop().unwrap_or_default();
    if parts.is_empty() {
        last.to_owned()
    } else {
        format!("{} and {last}", parts.join(", "))
    }
}

/// The check IDs `doctor` reports; `--json` consumers match on them.
struct Id;

impl Id {
    const ACCOUNTS_LIVE_STALE: &str = "accounts.live-stale";
    const ACCOUNTS_LOGIN_EXPIRY: &str = "accounts.login-expiry";
    const ACCOUNTS_ORPHANS: &str = "accounts.orphans";
    const ACCOUNTS_QUARANTINED: &str = "accounts.quarantined";
    const ACCOUNTS_REPLACEMENT: &str = "accounts.replacement";
    const ACCOUNTS_VAULT: &str = "accounts.vault";
    const AUTO_ENGINE: &str = "auto.engine";
    const AUTO_MODELS: &str = "auto.models";
    const AUTO_STRATEGY: &str = "auto.strategy";
    const AUTO_UNHEALTHY: &str = "auto.unhealthy";
    const DAEMON_NOTIFY: &str = "daemon.notify";
    const DAEMON_RUNNING: &str = "daemon.running";
    const DAEMON_SERVICE: &str = "daemon.service";
    const DAEMON_SOCKET: &str = "daemon.socket";
    const KEYCHAIN_LOCKED: &str = "keychain.locked";
    const LOG_FILE: &str = "log.file";
    const LOG_WRITABLE: &str = "log.writable";
    const ONLINE_REACH: &str = "online.reach";
    const PENDING_DISPLACED: &str = "pending.displaced";
    const PENDING_DISPLACED_MISSING: &str = "pending.displaced-missing";
    const PENDING_DISPLACED_UNRECORDED: &str = "pending.displaced-unrecorded";
    const PENDING_RESCUE: &str = "pending.rescue";
    const PENDING_RESCUE_DIR: &str = "pending.rescue-dir";
    const PENDING_RESCUE_ORPHAN: &str = "pending.rescue-orphan";
    const SESSIONS_BASELINE: &str = "sessions.baseline";
    const SESSIONS_BOOTSTRAP: &str = "sessions.bootstrap";
    const SESSIONS_CREDENTIAL: &str = "sessions.credential";
    const SESSIONS_DAEMON: &str = "sessions.daemon";
    const SESSIONS_MARKER: &str = "sessions.marker";
    const SESSIONS_ORPHAN: &str = "sessions.orphan";
    const SESSIONS_PROFILES: &str = "sessions.profiles";
    const SESSIONS_PROVENANCE: &str = "sessions.provenance";
    const SESSIONS_RESERVATION: &str = "sessions.reservation";
    const SESSIONS_SEED: &str = "sessions.seed";
    const SESSIONS_SPELLING: &str = "sessions.spelling";
    const SESSIONS_SPLIT: &str = "sessions.split";
    const SESSIONS_STATE: &str = "sessions.state";
    const SESSIONS_UNKNOWN_ENTRIES: &str = "sessions.unknown-entries";
    const SETTINGS_FILE: &str = "settings.file";
    const SETTINGS_UNKNOWN: &str = "settings.unknown";
    const SETTINGS_VALUE: &str = "settings.value";
    const STORE_ACCOUNTS: &str = "store.accounts";
    const STORE_INTEGRITY: &str = "store.integrity";
    const STORE_MODE: &str = "store.mode";
    const STORE_PRESENT: &str = "store.present";
    const STORE_SCHEMA: &str = "store.schema";
    const STORE_SKIPPED: &str = "store.skipped";
    const STORE_TEMP_FILES: &str = "store.temp-files";
    const SWITCH_INTERRUPTED: &str = "switch.interrupted";
    const USAGE_BACKOFF: &str = "usage.backoff";
    const USAGE_BUDGET: &str = "usage.budget";
    const USAGE_CLOCK_SKEW: &str = "usage.clock-skew";
    const USAGE_STATE: &str = "usage.state";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_of_names_reads_as_a_sentence() {
        assert_eq!(and_list(&["A"]), "A");
        assert_eq!(and_list(&["A", "B"]), "A and B");
        assert_eq!(and_list(&["A", "B", "C"]), "A, B and C");
    }

    #[test]
    fn a_report_is_ok_until_a_check_fails() {
        let mut report = DoctorReport::default();
        assert!(report.ok());
        report.checks.push((None, Check::warn("a.b", "w")));
        assert!(report.ok());
        report.checks.push((None, Check::fail("a.c", "f")));
        assert!(!report.ok());
    }
}
