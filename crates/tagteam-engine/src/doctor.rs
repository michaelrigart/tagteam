//! §13.6 `tagteam doctor`: tagteam's own state and its interop with each provider. Doctor is
//! read-only (B.67). It takes no `MutationGuard` and no account lock, and runs no recovery
//! (Decision 4). It opens the store read-only and never migrates it (Decision 3). It tests locks
//! without waiting, reads the vault only when the Keychain is unlocked, and creates nothing: no
//! data directory, store, log or lock file. Every finding names its fix.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Strategy;
use tagteam_core::rank::span;
use tagteam_core::trust::is_future_stamped;
use tagteam_core::usage::WindowKind;
use tagteam_core::{ProvenanceVerdict, provenance};
use tagteam_provider::atomic::{temp_writer_pid, writable};
use tagteam_provider::doctor::quoted;
use tagteam_provider::env::LOG_ROTATIONS;
use tagteam_provider::http::{HttpError, HttpRequest};
use tagteam_provider::profile::{ProfileMarker, Seed, canonical_profile_path, launch_reservations};
use tagteam_provider::{
    Check, CheckStatus, Env, Liveness, LockProbe, LockState, Provenance, Provider, Read, holders_of,
};

use crate::auto::{engine_lock_path, read_holder};
use crate::engine::Engine;
use crate::error::{EngineError, SessionOwner, daemon_advice};
use crate::profiles::{Held, allowlist, held, is_private, resolved};
use crate::provenance::identity_drifted;
use crate::recover::Direction;
use crate::session::SessionState;
use crate::settings::{self, Settings};
use crate::store::{
    AccountRow, JournalRow, SCHEMA_VERSION, Store, backoff_holds, backoff_is_skewed,
    plan_is_skewed, replacing_meta_parses,
};
use crate::vault::SERVICE;

/// What `doctor` covers (§13.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorOptions {
    /// `--online`: TLS reachability of each provider's hosts.
    pub online: bool,
    /// `--provider`: only this provider's checks, besides tagteam's own (§13.1).
    pub provider: Option<ProviderId>,
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

/// `login_expires_at` this close is reported (§13.6).
const EXPIRY_NOTICE_MS: i64 = 7 * 86_400_000;

/// How long `--online` waits for each host (§13.6).
const ONLINE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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

/// `path`'s permission bits; `None` when it does not exist, which is not checked.
fn mode_of(path: &Path) -> io::Result<Option<u32>> {
    match fs::metadata(path) {
        Ok(m) => Ok(Some(m.permissions().mode() & 0o777)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The entries of `dir`, sorted; none when it does not exist. An entry the listing cannot
/// return fails the whole listing, so no check judges part of a directory as all of it
/// (§13.6: a check whose input cannot be read reports `warn` and why).
fn entries(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut paths = listing
        .map(|e| e.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}

/// The directories among `dir`'s entries, a link followed as `Path::is_dir` follows it. An
/// entry whose type cannot be read fails the listing, as in `entries`; a dangling link is no
/// directory.
fn subdirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for path in entries(dir)? {
        match fs::metadata(&path) {
            Ok(m) if m.is_dir() => dirs.push(path),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(dirs)
}

/// The fix for a path this user cannot read.
fn readable(path: &Path) -> String {
    format!(
        "make {} and the directories above it readable by this user",
        quoted(path)
    )
}

/// Whether SQLite found the store itself damaged: pages it cannot read as a database
/// (`SQLITE_CORRUPT`), or a file that is no database (`SQLITE_NOTADB`). Any other error keeps
/// the store out of reach, which says nothing of its integrity (§13.6).
fn is_damage(e: &crate::store::StoreError) -> bool {
    use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
    matches!(
        e,
        crate::store::StoreError::Sqlite(rusqlite::Error::SqliteFailure(f, _))
            if matches!(f.code, DatabaseCorrupt | NotADatabase)
    )
}

/// `PRAGMA quick_check` run once more before it fails the store: an immutable open reads the
/// file while a writer that began after the probe may be checkpointing it, which can show a
/// torn page that a second run does not (R-T9). Two failures stand.
fn settled_quick_check(
    first: Result<Vec<String>, crate::store::StoreError>,
    again: impl FnOnce() -> Result<Vec<String>, crate::store::StoreError>,
) -> Result<Vec<String>, crate::store::StoreError> {
    match first {
        Ok(rows) if rows == ["ok"] => Ok(rows),
        _ => match again() {
            Ok(rows) if rows == ["ok"] => Ok(rows),
            second => second,
        },
    }
}

/// `auto.engine` for an engine lock record naming `pid`: an undetermined liveness is an input
/// doctor could not read, so a warning (R-T9-liveness), as for `switch.interrupted`.
fn engine_record_check(pid: u32, liveness: Liveness) -> Check {
    match liveness {
        Liveness::Live => Check::info("auto.engine", format!("auto-switch runs as pid {pid}")),
        Liveness::Dead => Check::ok(
            "auto.engine",
            format!("auto-switch is not running (its last engine, pid {pid}, has exited)"),
        ),
        Liveness::Unknown(e) => Check::warn(
            "auto.engine",
            format!(
                "whether auto-switch runs as pid {pid} cannot be told: the process's start time cannot be read ({e})"
            ),
        )
        .fix(format!(
            "check whether pid {pid} is still running a `tagteam auto`, then `tagteam doctor` again"
        )),
    }
}

/// The fix for a damaged store.
fn restore_backup(path: &Path) -> String {
    format!(
        "restore {} from a backup; failing that, move it aside and add the accounts again",
        quoted(path)
    )
}

/// `store.integrity` for a store this user cannot open or read: a `warn` naming the cause,
/// never a failure of its integrity, which cannot be told.
fn out_of_reach(path: &Path, e: &crate::store::StoreError) -> Check {
    Check::warn(
        "store.integrity",
        format!(
            "{} cannot be opened ({e}), so its integrity cannot be told",
            path.display()
        ),
    )
    .fix(format!(
        "{}; `store.mode` names the `chmod` when the store's mode is the cause",
        readable(path)
    ))
}

/// The fix for store rows that cannot be read, in a store that passed `quick_check`.
fn restore(engine: &Engine) -> String {
    format!(
        "run `tagteam doctor` again; if it persists, restore {} from a backup",
        quoted(&engine.env.data_dir().join("tagteam.db"))
    )
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
        let providers = run.scope(opts.provider.as_ref());
        run.temp_files(&providers);
        run.vault_orphans();
        run.rescues(opts.provider.as_ref());
        run.displaced(opts.provider.as_ref());
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

    /// §13.6 Store: whether there is one, `PRAGMA quick_check`, and its schema version. Only a
    /// sound store at this build's schema is read by the checks after this one.
    fn store(&mut self) {
        let data = self.engine.env.data_dir();
        let path = data.join("tagteam.db");
        match data.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                self.push(
                    None,
                    Check::info(
                        "store.present",
                        format!(
                            "tagteam has no state yet: {} does not exist",
                            data.display()
                        ),
                    ),
                );
                return;
            }
            Err(e) => {
                self.stored = Stored::Unusable;
                self.push(
                    None,
                    Check::warn(
                        "store.present",
                        format!(
                            "whether tagteam has any state cannot be told: {} cannot be read ({})",
                            data.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&data)),
                );
                self.push(
                    None,
                    Check::info(
                        "store.skipped",
                        "the checks that read the store's rows were skipped: the store cannot be read",
                    ),
                );
                return;
            }
        }
        let store = match Store::open_read_only(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                self.push(
                    None,
                    Check::info(
                        "store.present",
                        format!(
                            "tagteam has no store yet: {} does not exist",
                            path.display()
                        ),
                    ),
                );
                return;
            }
            Err(e) => {
                self.stored = Stored::Unusable;
                let check = if let crate::store::StoreError::LopsidedWal { .. } = e {
                    Check::warn(
                        "store.integrity",
                        format!(
                            "{} has a write-ahead log doctor cannot read without writing, so its integrity cannot be told ({e})",
                            path.display()
                        ),
                    )
                    .fix("run any tagteam command, such as `tagteam list`: it recovers the log, then `tagteam doctor` again")
                } else if is_damage(&e) {
                    Check::fail(
                        "store.integrity",
                        format!("{} cannot be opened: {e}", path.display()),
                    )
                    .fix(restore_backup(&path))
                } else {
                    out_of_reach(&path, &e)
                };
                self.push(None, check);
                self.push(
                    None,
                    Check::info(
                        "store.skipped",
                        "the checks that read the store's rows were skipped: the store cannot be read",
                    ),
                );
                return;
            }
        };
        let sound = match settled_quick_check(store.quick_check(), || store.quick_check()) {
            Ok(rows) if rows == ["ok"] => {
                self.push(
                    None,
                    Check::ok(
                        "store.integrity",
                        format!("{} passes PRAGMA quick_check", path.display()),
                    ),
                );
                true
            }
            found => {
                let check = match found {
                    Err(e) if !is_damage(&e) => out_of_reach(&path, &e),
                    found => {
                        let detail = match found {
                            Ok(rows) => rows.into_iter().take(5).collect::<Vec<_>>().join("; "),
                            Err(e) => e.to_string(),
                        };
                        Check::fail(
                            "store.integrity",
                            format!("{} fails PRAGMA quick_check: {detail}", path.display()),
                        )
                        .fix(restore_backup(&path))
                    }
                };
                self.push(None, check);
                false
            }
        };
        let current = match store.schema_version() {
            Ok(v) if v > SCHEMA_VERSION => {
                self.push(
                    None,
                    Check::fail(
                        "store.schema",
                        format!(
                            "the store's schema is v{v}, newer than this tagteam's v{SCHEMA_VERSION}: a newer tagteam wrote it, and this one refuses it"
                        ),
                    )
                    .fix("upgrade tagteam to the version that wrote the store"),
                );
                false
            }
            Ok(v) if v < SCHEMA_VERSION => {
                self.push(
                    None,
                    Check::info(
                        "store.schema",
                        format!(
                            "the store's schema is v{v}, older than this tagteam's v{SCHEMA_VERSION}; the next tagteam command that opens it migrates it"
                        ),
                    )
                    .fix("run `tagteam list` to migrate it, then `tagteam doctor` again"),
                );
                false
            }
            Ok(v) => {
                self.push(
                    None,
                    Check::ok("store.schema", format!("the store's schema is v{v}")),
                );
                true
            }
            Err(e) => {
                self.push(
                    None,
                    Check::warn(
                        "store.schema",
                        format!("the store's schema version cannot be read: {e}"),
                    )
                    .fix(restore(self.engine)),
                );
                false
            }
        };
        self.stored = if sound && current {
            let accounts = match store.all_accounts() {
                Ok(rows) => Some(rows),
                Err(e) => {
                    self.push(
                        None,
                        Check::warn(
                            "store.accounts",
                            format!(
                                "the store's accounts cannot be read ({e}), so orphaned vault files, rescue files, session profiles and which providers have accounts are not checked"
                            ),
                        )
                        .fix(restore(self.engine)),
                    );
                    None
                }
            };
            Stored::Usable { store, accounts }
        } else {
            self.push(
                None,
                Check::info(
                    "store.skipped",
                    "the checks that read the store's rows were skipped: the store is not one this tagteam can read",
                ),
            );
            Stored::Unusable
        };
    }

    /// §13.6 Store modes: the store 0600, tagteam's directories 0700, and a file vault's files
    /// 0600 (§5). Paths that do not exist are not checked. A mode that cannot be read, or a
    /// directory whose entries cannot be listed, warns, and the check is then not `ok`.
    fn modes(&mut self) {
        let engine = self.engine;
        let env = &engine.env;
        let data = env.data_dir();
        let mut found = Vec::new();
        let unlisted = |dir: &Path, e: io::Error| {
            Check::warn(
                "store.mode",
                format!(
                    "{} cannot be listed ({}), so the modes of what it holds are not checked",
                    dir.display(),
                    e.kind()
                ),
            )
            .fix(readable(dir))
        };
        let mut files = vec![data.join("tagteam.db")];
        if let Some(dir) = engine.vault.dir() {
            match entries(dir) {
                Ok(paths) => files.extend(
                    paths
                        .into_iter()
                        .filter(|p| p.extension().is_some_and(|x| x == "json")),
                ),
                Err(e) => found.push(unlisted(dir, e)),
            }
        }
        let mut dirs = vec![data.clone(), env.config_dir(), env.state_dir()];
        for sub in ["vault", "rescue", "displaced", "locks", "sessions"] {
            dirs.push(data.join(sub));
        }
        let sessions = data.join("sessions");
        match subdirs(&sessions) {
            Ok(profiles) => dirs.extend(profiles),
            Err(e) => found.push(unlisted(&sessions, e)),
        }
        for (paths, want) in [(files, 0o600), (dirs, 0o700)] {
            for path in paths {
                match mode_of(&path) {
                    Ok(Some(mode)) if mode != want => found.push(
                        Check::warn(
                            "store.mode",
                            format!(
                                "{} has mode {mode:o}, not {want:o}: other users may read it",
                                path.display()
                            ),
                        )
                        .fix(format!("chmod {want:o} {}", quoted(&path))),
                    ),
                    Ok(_) => {}
                    Err(e) => found.push(
                        Check::warn(
                            "store.mode",
                            format!(
                                "the mode of {} cannot be read ({})",
                                path.display(),
                                e.kind()
                            ),
                        )
                        .fix(readable(&path)),
                    ),
                }
            }
        }
        if found.is_empty() {
            found.push(Check::ok(
                "store.mode",
                "the store, the vault and tagteam's directories are private",
            ));
        }
        for check in found {
            self.push(None, check);
        }
    }

    /// §13.6: a temp file the atomic writer left (§9.5) whose writer is gone, in tagteam's own
    /// directories or beside a file a provider's identity surface names (§3), the only places
    /// tagteam's writer runs. It may hold a secret. A directory that cannot be listed warns, and
    /// so does a file whose writer's liveness cannot be told; the check is then not `ok`.
    fn temp_files(&mut self, providers: &[Arc<dyn Provider>]) {
        let engine = self.engine;
        let env = &engine.env;
        let data = env.data_dir();
        let mut dirs: Vec<PathBuf> = vec![data.clone(), env.config_dir()];
        for sub in ["vault", "rescue", "displaced"] {
            dirs.push(data.join(sub));
        }
        let mut unlisted = Vec::new();
        let sessions = data.join("sessions");
        match subdirs(&sessions) {
            Ok(profiles) => dirs.extend(profiles),
            Err(e) => unlisted.push((sessions, e)),
        }
        for p in providers {
            let surface = p.identity_surface(env);
            let files = surface
                .credential_files
                .into_iter()
                .chain(surface.json_keys.into_iter().map(|(f, _)| f));
            dirs.extend(files.filter_map(|f| f.parent().map(Path::to_path_buf)));
        }
        dirs.sort();
        dirs.dedup();
        let mut found = Vec::new();
        let mut undecided = Vec::new();
        for dir in dirs {
            let paths = match entries(&dir) {
                Ok(paths) => paths,
                Err(e) => {
                    unlisted.push((dir, e));
                    continue;
                }
            };
            for path in paths {
                let name = path.file_name().and_then(|n| n.to_str());
                let Some(pid) = name.and_then(temp_writer_pid) else {
                    continue;
                };
                match engine.process.exists(pid) {
                    Some(false) => found.push((path, pid)),
                    // §13.6: an input that cannot be read warns. Whether its writer still runs
                    // cannot be told, so the file is neither found nor cleared.
                    None => undecided.push((path, pid)),
                    Some(true) => {}
                }
            }
        }
        found.sort();
        undecided.sort();
        if found.is_empty() && unlisted.is_empty() && undecided.is_empty() {
            self.push(
                None,
                Check::ok("store.temp-files", "no write left a temp file behind"),
            );
        }
        for (dir, e) in unlisted {
            self.push(
                None,
                Check::warn(
                    "store.temp-files",
                    format!(
                        "{} cannot be listed ({}), so a temp file a killed write left in it cannot be found",
                        dir.display(),
                        e.kind()
                    ),
                )
                .fix(readable(&dir)),
            );
        }
        for (path, pid) in found {
            self.push(
                None,
                Check::warn(
                    "store.temp-files",
                    format!(
                        "{} is a temp file a write by pid {pid} left when it was killed; it may hold a secret, and is safe to delete",
                        path.display()
                    ),
                )
                .fix(format!("rm {}", quoted(&path))),
            );
        }
        for (path, pid) in undecided {
            self.push(
                None,
                Check::warn(
                    "store.temp-files",
                    format!(
                        "{} is a temp file of a write by pid {pid}, and whether that process is still running cannot be told; it may hold a secret",
                        path.display()
                    ),
                )
                .fix(format!(
                    "run `tagteam doctor` again; if it stays, check whether pid {pid} is a tagteam process before deleting {}",
                    quoted(&path)
                )),
            );
        }
    }

    /// §13.6 Settings: an unparseable file fails, since every command then runs on the
    /// defaults; an invalid value or an unknown key warns, naming it. The file is read as each
    /// provider in scope reads it (§6.4), and a warning two of them share is shown once.
    fn settings(&mut self, only: Option<&ProviderId>) {
        let path = settings::config_path(&self.engine.env);
        match fs::read_to_string(&path) {
            Err(e) if e.kind() == ErrorKind::NotFound => {
                self.push(
                    None,
                    Check::ok(
                        "settings.file",
                        format!(
                            "there is no settings file at {}; the defaults apply",
                            path.display()
                        ),
                    ),
                );
                return;
            }
            Err(e) => {
                self.push(
                    None,
                    Check::warn(
                        "settings.file",
                        format!(
                            "{} cannot be read ({}), so every command runs on the defaults",
                            path.display(),
                            e.kind()
                        ),
                    )
                    .fix(format!("make {} readable", quoted(&path))),
                );
                return;
            }
            Ok(text) if text.parse::<toml_edit::DocumentMut>().is_err() => {
                self.push(
                    None,
                    Check::fail(
                        "settings.file",
                        format!(
                            "{} is not valid TOML, so every command runs on the defaults",
                            path.display()
                        ),
                    )
                    .fix(format!(
                        "correct {} by hand; `tagteam config` refuses to write to it until then",
                        quoted(&path)
                    )),
                );
                return;
            }
            Ok(_) => self.push(
                None,
                Check::ok("settings.file", format!("{} parses", path.display())),
            ),
        }
        let ids: Vec<ProviderId> = match only {
            Some(p) => vec![p.clone()],
            None => self.engine.registry.all().iter().map(|p| p.id()).collect(),
        };
        let (mut warnings, mut unknown) = (Vec::new(), Vec::new());
        for id in &ids {
            let found = settings::inspect(&self.engine.env, id);
            for w in found.warnings {
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
            for k in found.unknown {
                if !unknown.contains(&k) {
                    unknown.push(k);
                }
            }
        }
        for w in warnings {
            self.push(
                None,
                Check::warn("settings.value", w)
                    .fix("correct it with `tagteam config set`, or remove it with `tagteam config unset`"),
            );
        }
        for key in unknown {
            self.push(
                None,
                Check::warn(
                    "settings.unknown",
                    format!("{} has a key tagteam does not know: {key}", path.display()),
                )
                .fix(format!(
                    "remove `{key}` from {} by hand; tagteam ignores it",
                    quoted(&path)
                )),
            );
        }
    }

    /// §13.6 Log: its path and size, and whether it can be written, judged by `access(2)` on
    /// the file or, before it exists, on its nearest existing ancestor.
    fn log(&mut self) {
        let log = self.engine.env.log_file();
        let rotation = |suffix: &str| {
            let mut p: OsString = log.clone().into_os_string();
            p.push(suffix);
            PathBuf::from(p)
        };
        match fs::metadata(&log) {
            Ok(m) => {
                let (mut rotated, mut unreadable) = (0, Vec::new());
                for &suffix in LOG_ROTATIONS {
                    let path = rotation(suffix);
                    match fs::metadata(&path) {
                        Ok(r) => rotated += r.len(),
                        Err(e) if e.kind() == ErrorKind::NotFound => {}
                        Err(e) => unreadable.push((path, e)),
                    }
                }
                self.push(
                    None,
                    Check::info(
                        "log.file",
                        format!(
                            "the log is {}: {} bytes, and {rotated} more in its rotations",
                            log.display(),
                            m.len()
                        ),
                    ),
                );
                for (path, e) in unreadable {
                    self.push(
                        None,
                        Check::warn(
                            "log.file",
                            format!(
                                "the log's rotation {} cannot be read ({}), so its size is not counted",
                                path.display(),
                                e.kind()
                            ),
                        )
                        .fix(readable(&path)),
                    );
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => self.push(
                None,
                Check::info(
                    "log.file",
                    format!("nothing is logged yet; the log will be {}", log.display()),
                ),
            ),
            Err(e) => self.push(
                None,
                Check::warn("log.file", format!("{} cannot be read: {e}", log.display()))
                    .fix(readable(&log)),
            ),
        }
        let mut at = log.clone();
        loop {
            match at.try_exists() {
                Ok(true) => break,
                Ok(false) => match at.parent() {
                    Some(parent) => at = parent.to_path_buf(),
                    None => break,
                },
                Err(e) => {
                    self.push(
                        None,
                        Check::warn(
                            "log.writable",
                            format!(
                                "whether {} can be written cannot be told: {} cannot be read ({})",
                                log.display(),
                                at.display(),
                                e.kind()
                            ),
                        )
                        .fix(readable(&at)),
                    );
                    return;
                }
            }
        }
        let is_dir = match fs::metadata(&at) {
            Ok(m) => Some(m.is_dir()),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => {
                self.push(
                    None,
                    Check::warn(
                        "log.writable",
                        format!(
                            "whether {} can be written cannot be told: {} cannot be read ({})",
                            log.display(),
                            at.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&at)),
                );
                return;
            }
        };
        if let Some(is_dir) = is_dir
            && is_dir != (at != log)
        {
            let (what, fix) = if at == log {
                ("is a directory", "move it aside")
            } else {
                (
                    "is not a directory",
                    "move it aside, or make it a directory",
                )
            };
            self.push(
                None,
                Check::warn(
                    "log.writable",
                    format!(
                        "{} cannot be written: {} {what}, so tagteam logs nothing",
                        log.display(),
                        at.display()
                    ),
                )
                .fix(format!("{fix}: {}", quoted(&at))),
            );
            return;
        }
        match writable(&at) {
            Ok(true) => self.push(
                None,
                Check::ok("log.writable", format!("{} can be written", log.display())),
            ),
            Ok(false) => self.push(
                None,
                Check::warn(
                    "log.writable",
                    format!(
                        "{} cannot be written, so tagteam logs nothing",
                        log.display()
                    ),
                )
                .fix(format!("make {} writable by this user", quoted(&at))),
            ),
            Err(e) => self.push(
                None,
                Check::warn(
                    "log.writable",
                    format!(
                        "whether {} can be written cannot be told: {e}",
                        log.display()
                    ),
                )
                .fix(readable(&at)),
            ),
        }
    }

    /// §13.6 Accounts, orphaned vault items: on Linux a `vault/` file naming no account; on
    /// macOS any item of service `tagteam` while the store has no account, by an attributes-only
    /// probe that never prompts. Every data directory on a Mac shares that service, so such
    /// items may be another data directory's.
    fn vault_orphans(&mut self) {
        let Some(accounts) = self.all_accounts() else {
            return;
        };
        let ids: BTreeSet<&str> = accounts.iter().map(|r| r.id.as_str()).collect();
        if let Some(dir) = self.engine.vault.dir() {
            let dir = dir.to_path_buf();
            let orphans: Vec<PathBuf> = match entries(&dir) {
                Ok(paths) => paths
                    .into_iter()
                    .filter(|p| {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        let stem = name
                            .strip_suffix(".json")
                            .filter(|_| !name.starts_with('.'));
                        stem.is_some_and(|s| !ids.contains(s.strip_suffix(".prev").unwrap_or(s)))
                    })
                    .collect(),
                Err(e) => {
                    self.push(
                        None,
                        Check::warn(
                            "accounts.orphans",
                            format!(
                                "{} cannot be listed ({}), so whether every vault file names an account cannot be told",
                                dir.display(),
                                e.kind()
                            ),
                        )
                        .fix(readable(&dir)),
                    );
                    return;
                }
            };
            if orphans.is_empty() {
                self.push(
                    None,
                    Check::ok("accounts.orphans", "every vault file names an account"),
                );
            }
            for path in orphans {
                self.push(
                    None,
                    Check::warn(
                        "accounts.orphans",
                        format!("{} names no account of this store", path.display()),
                    )
                    .fix(format!(
                        "rm {}, which no account uses; `tagteam purge` deletes it with everything else",
                        quoted(&path)
                    )),
                );
            }
            return;
        }
        let Some(keychain) = self.engine.vault.keychain() else {
            return;
        };
        if !accounts.is_empty() {
            return;
        }
        let check = match keychain.service_has_items(SERVICE) {
            Read::Present(true) => Check::warn(
                "accounts.orphans",
                format!(
                    "the Keychain holds items of service `{SERVICE}` while this store has no account; they may belong to another tagteam data directory on this Mac"
                ),
            )
            .fix("once no other tagteam data directory uses them, `tagteam purge --keychain-orphans` deletes them"),
            Read::Present(false) | Read::Absent => Check::ok(
                "accounts.orphans",
                format!("the Keychain holds no item of service `{SERVICE}`"),
            ),
            Read::Unreadable(e) => Check::warn(
                "accounts.orphans",
                format!("whether the Keychain holds items of service `{SERVICE}` cannot be told: {e}"),
            )
            .fix("run `tagteam doctor` again; if it persists, `security show-keychain-info` shows the Keychain's state"),
        };
        self.push(None, check);
    }

    /// §13.6 Accounts, for `p`: each vault entry readable, quarantines, a login near its
    /// expiry, pending replacements, and a stale-marked live store.
    fn accounts(&mut self, p: &dyn Provider) {
        let Some(store) = self.store_ref() else {
            return;
        };
        let id = p.id();
        let rows = match store.accounts(&id) {
            Ok(rows) => rows,
            Err(e) => {
                let check = Check::warn(
                    "accounts.vault",
                    format!("the store's accounts cannot be read: {e}"),
                )
                .fix(restore(self.engine));
                self.push(Some(&id), check);
                return;
            }
        };
        let mut found = Vec::new();
        let readable = !rows.is_empty() && self.secrets() == Secrets::Readable;
        if readable {
            let mut broken = 0;
            for row in &rows {
                let remove = command(self.engine, &id, &format!("remove {}", row.position));
                match self.engine.vault.read(&row.id) {
                    Read::Present(_) => {}
                    Read::Absent => {
                        broken += 1;
                        found.push(
                            Check::fail(
                                "accounts.vault",
                                format!(
                                    "account {} has no vault entry, as a purge that stopped part way leaves one; it can never be switched to, refreshed or launched",
                                    row.position
                                ),
                            )
                            .fix(format!("`{remove}` finishes deleting it")),
                        );
                    }
                    Read::Unreadable(e) => {
                        broken += 1;
                        found.push(
                            Check::fail(
                                "accounts.vault",
                                format!("account {}'s vault entry cannot be read: {e}", row.position),
                            )
                            .fix(format!(
                                "make the entry readable again; if it is lost, `{remove}` and add the account again"
                            )),
                        );
                    }
                }
            }
            if broken == 0 && !rows.is_empty() {
                found.push(Check::ok(
                    "accounts.vault",
                    format!("the vault holds every account's login ({})", rows.len()),
                ));
            }
        }
        let mut stale_unread = None;
        for row in &rows {
            let add = command(
                self.engine,
                &id,
                &format!("add --position {}", row.position),
            );
            if let Some(reason) = &row.quarantine_reason {
                found.push(
                    Check::warn(
                        "accounts.quarantined",
                        format!(
                            "account {} is quarantined ({reason}): {}",
                            row.position,
                            quarantine_words(reason)
                        ),
                    )
                    .fix(format!(
                        "log in as that account with `{}`, then `{add}`",
                        p.launch_command()
                    )),
                );
            }
            if let Some(at) = row.login_expires_at {
                let left = at - self.now_ms;
                if left <= EXPIRY_NOTICE_MS {
                    let when = if left > 0 {
                        format!("expires in {}", span(left / 1000))
                    } else {
                        format!("expired {} ago", span(-left / 1000))
                    };
                    found.push(
                        Check::warn(
                            "accounts.login-expiry",
                            format!("account {}'s login {when}", row.position),
                        )
                        .fix(format!(
                            "log in as that account with `{}`, then `{add}`",
                            p.launch_command()
                        )),
                    );
                }
            }
            if let Some(fp) = &row.replacing_fp {
                found.push(self.replacement(p, store, row, fp));
            }
            match store.live_store_stale(row) {
                Ok(true) => found.push(
                    Check::warn(
                        "accounts.live-stale",
                        format!(
                            "{} still runs the login an explicit command replaced for account {}",
                            p.display_name(),
                            row.position
                        ),
                    )
                    .fix(format!(
                        "`{}` activates the new login",
                        command(
                            self.engine,
                            &id,
                            &format!("switch {} --force", row.position)
                        )
                    )),
                ),
                Ok(false) => {}
                Err(e) => {
                    stale_unread.get_or_insert(e);
                }
            }
        }
        if let Some(e) = stale_unread {
            found.push(
                Check::warn(
                    "accounts.live-stale",
                    format!(
                        "whether {} still runs a login an explicit command replaced cannot be told: the active account cannot be read ({e})",
                        p.display_name()
                    ),
                )
                .fix(restore(self.engine)),
            );
        }
        for check in found {
            self.push(Some(&id), check);
        }
    }

    /// §12.5: a replacement whose replacer died. When its vault write landed but its recorded
    /// metadata cannot be parsed, every lock holder but `remove` and `purge` refuses the
    /// account (`replacement-unreadable`), which fails; otherwise the next lock holder settles
    /// it. Nothing is reconciled here.
    fn replacement(&self, p: &dyn Provider, store: &Store, row: &AccountRow, fp: &str) -> Check {
        let id = p.id();
        let remove = command(self.engine, &id, &format!("remove {}", row.position));
        // A NULL `replacing_meta` is cleared, never refused (§12.5, Task 2): only recorded
        // details that do not parse block. A column that cannot be read is a check whose input
        // cannot be read (§13.6).
        let parses = match store.replacing_meta(&row.id) {
            Ok(meta) => meta.is_none_or(|m| replacing_meta_parses(&m)),
            Err(e) => {
                return Check::warn(
                    "accounts.replacement",
                    format!(
                        "a replacement of account {} was interrupted, and the login details recorded with it cannot be read from the store ({e})",
                        row.position
                    ),
                )
                .fix(format!(
                    "run `tagteam doctor` again; if it reached the vault and the details stay unreadable, `{remove}` is the way out"
                ));
            }
        };
        let landed = match self.secrets() {
            Secrets::Readable => match self.engine.vault.read(&row.id) {
                Read::Present(b) => Some(p.fingerprint(&b).is_some_and(|f| f.as_str() == fp)),
                Read::Absent => Some(false),
                Read::Unreadable(_) => None,
            },
            Secrets::Locked | Secrets::Unknown => None,
        };
        match (landed, parses) {
            (Some(true), false) => Check::fail(
                "accounts.replacement",
                format!(
                    "account {}'s replacement reached the vault, but the login details recorded with it cannot be read, so every command but `remove` and `purge` refuses the account",
                    row.position
                ),
            )
            .fix(format!("`{remove}`, then add the account again")),
            (None, false) => Check::warn(
                "accounts.replacement",
                format!(
                    "a replacement of account {} was interrupted and the login details recorded with it cannot be read; whether it reached the vault cannot be read now",
                    row.position
                ),
            )
            .fix(format!(
                "run `tagteam doctor` again once the vault can be read; if it reached the vault, `{remove}` is the way out"
            )),
            _ => Check::warn(
                "accounts.replacement",
                format!("a replacement of account {} was interrupted", row.position),
            )
            .fix(format!(
                "nothing to do: the next command that refreshes, switches to or adds account {} finishes or undoes it",
                row.position
            )),
        }
    }

    /// §13.6 Usage, for `p`: backoff, the hourly budget (§8.6), and stamps a skewed clock left
    /// (§8.4).
    fn usage(&mut self, p: &dyn Provider) {
        let Some(store) = self.store_ref() else {
            return;
        };
        let id = p.id();
        // `accounts.vault` reports the same read when it fails.
        let Ok(rows) = store.accounts(&id) else {
            return;
        };
        let now_s = self.now_ms.div_euclid(1000);
        let budget = p.poll_budget();
        let mut found = Vec::new();
        let mut identities = BTreeSet::new();
        for row in &rows {
            if identities.insert(row.identity_key.clone()) {
                let since = now_s - budget.count_window_s;
                match store.usage_request_count(&id, &row.identity_key, since) {
                    Ok(n) if n >= budget.hourly_requests => found.push(
                        Check::warn(
                            "usage.budget",
                            format!(
                                "account {} has used all {} usage requests of the past hour",
                                row.position, budget.hourly_requests
                            ),
                        )
                        .fix("wait: tagteam sends no more until the oldest leaves the hour; a second `tagteam auto`, or another tool polling the account, uses it up"),
                    ),
                    Ok(_) => {}
                    Err(e) => found.push(
                        Check::warn(
                            "usage.budget",
                            format!(
                                "account {}'s usage requests of the past hour cannot be counted: {e}",
                                row.position
                            ),
                        )
                        .fix(restore(self.engine)),
                    ),
                }
            }
            let state = match store.usage_state(&row.id) {
                Ok(Some(state)) => state,
                Ok(None) => continue,
                Err(e) => {
                    found.push(
                        Check::warn(
                            "usage.state",
                            format!("account {}'s usage state cannot be read: {e}", row.position),
                        )
                        .fix(restore(self.engine)),
                    );
                    continue;
                }
            };
            if let Some(until) = state
                .backoff_until
                .filter(|u| backoff_holds(Some(*u), now_s))
            {
                found.push(
                    Check::info(
                        "usage.backoff",
                        format!(
                            "account {}'s usage is backed off after {}; the next try is in {}",
                            row.position,
                            state.last_error.as_deref().unwrap_or("a failure"),
                            span(until - now_s)
                        ),
                    )
                    .fix("nothing to do: tagteam tries again then"),
                );
            }
            let skewed: Vec<(&str, i64)> = [
                state
                    .fetched_at
                    .filter(|t| is_future_stamped(*t, now_s))
                    .map(|t| ("reading", t)),
                state
                    .next_poll_at
                    .filter(|t| plan_is_skewed(*t, now_s, &budget))
                    .map(|t| ("poll plan", t)),
                state
                    .backoff_until
                    .filter(|t| backoff_is_skewed(*t, now_s))
                    .map(|t| ("backoff", t)),
            ]
            .into_iter()
            .flatten()
            .collect();
            for (what, at) in skewed {
                found.push(
                    Check::warn(
                        "usage.clock-skew",
                        format!(
                            "account {}'s {what} is stamped {} ahead of now: the clock is, or was, skewed",
                            row.position,
                            span(at - now_s)
                        ),
                    )
                    .fix("check the system clock; tagteam ignores the stamp, and the next fetch replaces it"),
                );
            }
        }
        if found.is_empty() && !rows.is_empty() {
            found.push(Check::ok(
                "usage.state",
                "no account is backed off, over its budget or stamped by a skewed clock",
            ));
        }
        for check in found {
            self.push(Some(&id), check);
        }
    }

    /// §13.6 Pending storage, `rescue/` (§6.3): a path that is not a listable directory fails;
    /// each pending rescue warns under its account's provider, and one whose account is gone
    /// under tagteam's own.
    fn rescues(&mut self, only: Option<&ProviderId>) {
        let dir = self.engine.env.data_dir().join("rescue");
        let paths = match entries(&dir) {
            Ok(paths) => paths,
            Err(e) => {
                self.push(
                    None,
                    Check::fail(
                        "pending.rescue-dir",
                        format!(
                            "{} is not a directory tagteam can list ({e}), so every account's rescues are unknown: `remove` refuses, and no account can be activated safely",
                            dir.display()
                        ),
                    )
                    .fix(format!(
                        "move {} aside if it is not tagteam's, or make it a directory again; `tagteam purge` deletes it with everything else",
                        quoted(&dir)
                    )),
                );
                return;
            }
        };
        let Some(accounts) = self.all_accounts() else {
            return;
        };
        let files = paths.into_iter().filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.') && n.ends_with(".json"))
        });
        for path in files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let owner = accounts
                .iter()
                .find(|r| name.starts_with(&format!("{}-", r.id)));
            match owner {
                Some(row) => {
                    if only.is_some_and(|p| p != &row.provider) {
                        continue;
                    }
                    let provider = row.provider.clone();
                    self.push(
                        Some(&provider),
                        Check::warn(
                            "pending.rescue",
                            format!(
                                "account {} has a refreshed login in {} that is not in the vault yet",
                                row.position,
                                path.display()
                            ),
                        )
                        .fix("nothing to do: the account's next refresh or activation adopts it; if it stays, check that the vault can be written"),
                    );
                }
                None => self.push(
                    None,
                    Check::warn(
                        "pending.rescue-orphan",
                        format!("{} belongs to no account of this store", path.display()),
                    )
                    .fix(format!(
                        "`tagteam purge` deletes it with everything else, or delete it: rm {}",
                        quoted(&path)
                    )),
                ),
            }
        }
    }

    /// §13.6 Pending storage, `displaced/` (§6.3): its entries, by provider, and a file with no
    /// row or a row with no file, all `info`. Read through M5a's listing, over the read-only
    /// store.
    fn displaced(&mut self, only: Option<&ProviderId>) {
        if matches!(self.stored, Stored::Unusable) {
            return;
        }
        let list = match self.engine.displaced_in(self.store_ref()) {
            Ok(list) => list,
            Err(e) => {
                self.push(
                    None,
                    Check::warn(
                        "pending.displaced",
                        format!("the displaced credentials cannot be listed: {e}"),
                    )
                    .fix(format!(
                        "make {} a directory this user can read, then `tagteam doctor` again",
                        quoted(&self.engine.env.data_dir().join("displaced"))
                    )),
                );
                return;
            }
        };
        let mut counts: Vec<(ProviderId, usize)> = Vec::new();
        for e in &list.entries {
            if let Some(p) = &e.provider {
                if only.is_some_and(|o| o != p) {
                    continue;
                }
                match counts.iter_mut().find(|(q, _)| q == p) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((p.clone(), 1)),
                }
            }
        }
        for (p, n) in counts {
            self.push(
                Some(&p),
                Check::info(
                    "pending.displaced",
                    format!(
                        "{n} displaced credential(s) in {}: logins a command overwrote, kept for a manual restore",
                        list.dir.display()
                    ),
                )
                .fix("`tagteam displaced` lists them; `tagteam displaced --purge ID` deletes one"),
            );
        }
        for e in &list.entries {
            if only.is_some_and(|o| e.provider.as_ref().is_some_and(|p| p != o)) {
                continue;
            }
            let file = list.dir.join(format!("{}.json", e.id));
            if !e.recorded {
                self.push(
                    None,
                    Check::info(
                        "pending.displaced-unrecorded",
                        format!("{} has no row in the store", file.display()),
                    )
                    .fix(format!("`tagteam displaced --purge {}` deletes it", e.id)),
                );
            } else if !e.file_present {
                self.push(
                    e.provider.as_ref(),
                    Check::info(
                        "pending.displaced-missing",
                        format!("the displaced entry {} has a row but no file", e.id),
                    )
                    .fix(format!(
                        "`tagteam displaced --purge {}` deletes the row",
                        e.id
                    )),
                );
            }
        }
    }

    /// §13.6 Interrupted switch, for `p`: each journal row, by its holder's liveness (§12.6).
    /// A dead holder's row warns when §9.6 can decide it from fingerprints alone, and fails
    /// when it cannot. Doctor recovers nothing, and asks no oracle.
    fn journal(&mut self, p: &dyn Provider) {
        let Some(store) = self.store_ref() else {
            return;
        };
        let id = p.id();
        let rows: Vec<JournalRow> = match store.journals() {
            Ok(rows) => rows.into_iter().filter(|r| r.provider == id).collect(),
            Err(e) => {
                self.push(
                    Some(&id),
                    Check::warn(
                        "switch.interrupted",
                        format!("the switch journal cannot be read: {e}"),
                    )
                    .fix(restore(self.engine)),
                );
                return;
            }
        };
        let mut found = Vec::new();
        for row in &rows {
            let position = store
                .account(&row.to_id)
                .ok()
                .flatten()
                .map_or_else(|| row.to_id.to_string(), |a| a.position.to_string());
            let force = command(self.engine, &id, &format!("switch {position} --force"));
            let holder = format!("pid {}, started {}", row.holder.pid, row.holder.start);
            found.push(match row.holder.liveness() {
                Liveness::Live => Check::info(
                    "switch.interrupted",
                    format!("a switch to account {position} is under way ({holder})"),
                ),
                Liveness::Unknown(e) => Check::warn(
                    "switch.interrupted",
                    format!(
                        "a switch to account {position} is journaled by {holder}, which reads as live but may be another user's recycled pid ({e})"
                    ),
                )
                .fix(format!("if no tagteam process is running, `{force}`")),
                Liveness::Dead => self.dead_switch(p, store, row, &position, &force),
            });
        }
        if rows.is_empty() {
            found.push(Check::ok("switch.interrupted", "no switch was interrupted"));
        }
        for check in found {
            self.push(Some(&id), check);
        }
    }

    /// §9.6's table, from the live login's fingerprints alone, for a row whose holder died.
    fn dead_switch(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &JournalRow,
        position: &str,
        force: &str,
    ) -> Check {
        // `switch N` always takes `MutationGuard`, so it recovers the row first (§9.6), then
        // makes the switch that was being made; `list` takes it only when the live account's
        // usage is due.
        let retry = command(self.engine, &p.id(), &format!("switch {position}"));
        if self.provider_secrets(p) != Secrets::Readable {
            return Check::warn(
                "switch.interrupted",
                format!(
                    "a switch to account {position} was interrupted; whether it can be recovered cannot be read while the Keychain is locked"
                ),
            )
            .fix(format!("unlock the Keychain; `{retry}` then recovers it"));
        }
        let live = p.read_live_auth(&self.engine.env);
        // §13.6: a live login that cannot be read is an input that cannot be read, never an
        // undecidable switch, which `direction` would make of it.
        if let Err(e) = crate::switch::refuse_unsafe_live_reads(&live) {
            return Check::warn(
                "switch.interrupted",
                format!(
                    "a switch to account {position} was interrupted, and the live login cannot be read to judge it: {e}"
                ),
            )
            .fix(format!(
                "make the live login readable again; `{retry}` then recovers it"
            ));
        }
        match self.engine.direction(p, store, row, &live, &[]) {
            Ok(Direction::Undecidable) => Check::fail(
                "switch.interrupted",
                format!(
                    "a switch to account {position} was interrupted, and the live login cannot tell whether it landed: commands that change accounts refuse until it is settled"
                ),
            )
            .fix(format!("`{force}` settles it by activating the account you name")),
            Ok(_) => Check::warn(
                "switch.interrupted",
                format!("a switch to account {position} was interrupted"),
            )
            .fix(format!(
                "`{retry}` recovers it, then makes the switch it was making"
            )),
            Err(e) => Check::warn(
                "switch.interrupted",
                format!("a switch to account {position} was interrupted, and cannot be judged: {e}"),
            )
            .fix(format!("`{retry}` recovers it if it can")),
        }
    }

    /// §13.6 Auto-switch, for `p`: whether an engine runs, by its lock record and an exact pid
    /// and start-time match (§11.1); unhealthy ticks; and the settings a tick would warn about.
    fn auto(&mut self, p: &dyn Provider) {
        let id = p.id();
        let lock = engine_lock_path(&self.engine.env, &id);
        let running = match read_holder(&lock) {
            Read::Absent => Check::ok("auto.engine", "auto-switch is not running"),
            Read::Present(stamp) => engine_record_check(stamp.pid, stamp.liveness()),
            Read::Unreadable(e) => Check::warn(
                "auto.engine",
                format!("whether auto-switch runs cannot be told: {e}"),
            )
            .fix(readable(&lock)),
        };
        self.push(Some(&id), running);
        if let Some(store) = self.store_ref() {
            let unhealthy = match store.autoswitch_state(&id) {
                Ok(state) if state.unhealthy_ticks > 0 => Some(
                    Check::warn(
                        "auto.unhealthy",
                        format!(
                            "the last {} auto-switch tick(s) could not read the live account's usage",
                            state.unhealthy_ticks
                        ),
                    )
                    .fix("`tagteam list` shows why the live account's usage cannot be read"),
                ),
                Ok(_) => None,
                Err(e) => Some(
                    Check::warn(
                        "auto.unhealthy",
                        format!("the auto-switch state cannot be read: {e}"),
                    )
                    .fix(restore(self.engine)),
                ),
            };
            if let Some(check) = unhealthy {
                self.push(Some(&id), check);
            }
        }
        let (settings, _) = Settings::load(&self.engine.env, &id);
        if settings.strategy == Strategy::ConsumeFirst && p.primary_long_window().is_none() {
            self.push(
                Some(&id),
                Check::warn(
                    "auto.strategy",
                    format!(
                        "autoswitch.strategy is consume-first, but {} has no long usage window to rank by, so auto-switch runs best",
                        p.display_name()
                    ),
                )
                .fix(format!(
                    "`tagteam config set provider.{id}.autoswitch.strategy best`"
                )),
            );
        }
        self.models(p, &settings);
    }

    /// §11.2 step 3's model check, as the tick's `config-warning` makes it: each
    /// `autoswitch.models` name should be a scoped window some account's last reading reports.
    /// Nothing is said until some account has a reading.
    fn models(&mut self, p: &dyn Provider, settings: &Settings) {
        let Some(store) = self.store_ref() else {
            return;
        };
        let id = p.id();
        let names: Vec<&String> = settings
            .models
            .iter()
            .filter(|m| !m.eq_ignore_ascii_case("all"))
            .collect();
        if names.is_empty() {
            return;
        }
        let (mut read_any, mut scoped, mut unread) = (false, BTreeSet::new(), None);
        // The accounts `accounts.vault` read: it reports them when they cannot be read.
        for row in store.accounts(&id).unwrap_or_default() {
            match store.usage_state(&row.id) {
                Ok(state) => {
                    if let Some(windows) = state.and_then(|s| s.last_good) {
                        read_any = true;
                        scoped.extend(
                            windows
                                .iter()
                                .filter(|w| w.kind == WindowKind::Scoped)
                                .map(|w| w.label.to_lowercase()),
                        );
                    }
                }
                Err(e) => {
                    unread = Some((row.position, e));
                    break;
                }
            }
        }
        if let Some((n, e)) = unread {
            let check = Check::warn(
                "auto.models",
                format!(
                    "whether autoswitch.models names a model no account's usage reports cannot be told: account {n}'s usage state cannot be read ({e})"
                ),
            )
            .fix(restore(self.engine));
            self.push(Some(&id), check);
            return;
        }
        if !read_any {
            return;
        }
        let missing: Vec<String> = names
            .into_iter()
            .filter(|n| !scoped.contains(&n.to_lowercase()))
            .cloned()
            .collect();
        for name in missing {
            self.push(
                Some(&id),
                Check::warn(
                    "auto.models",
                    format!(
                        "autoswitch.models names {name:?}, but no account's usage reports a window for that model"
                    ),
                )
                .fix("check the name against `tagteam list`, then `tagteam config set autoswitch.models …`"),
            );
        }
    }

    /// The fix for a profile directory doctor cannot place: real history first, never deleted
    /// (§12.2), then the directory.
    fn move_then_delete(dir: &Path) -> String {
        format!(
            "move any real `projects/` or `history.jsonl` in {} into the default home's, then delete it once no session runs in it",
            quoted(dir)
        )
    }

    /// Every directory under `sessions/`, with its marker as read (§12.2); an error when
    /// `sessions/` cannot be listed, which `stray_profiles` reports.
    fn profile_dirs(&self) -> io::Result<Vec<(PathBuf, Read<ProfileMarker>)>> {
        let dirs = subdirs(&self.engine.env.data_dir().join("sessions"))?;
        Ok(dirs
            .into_iter()
            .map(|d| {
                let m = ProfileMarker::read(&d);
                (d, m)
            })
            .collect())
    }

    /// §13.6 Session profiles that belong to no provider in scope, found from `sessions/`
    /// itself: a marker that cannot be read, one naming a provider this build lacks or one out
    /// of scope (a registered provider with no account), and a directory with no marker, each
    /// in a directory no stored account owns. An account's own profile, marker or not, is its
    /// provider's to report (`sessions`). Each names what to delete. A `sessions/` that cannot
    /// be listed warns here, once, for every provider.
    fn stray_profiles(&mut self, only: Option<&ProviderId>) {
        let profiles = match self.profile_dirs() {
            Ok(profiles) => profiles,
            Err(e) => {
                let dir = self.engine.env.data_dir().join("sessions");
                self.push(
                    None,
                    Check::warn(
                        "sessions.profiles",
                        format!(
                            "{} cannot be listed ({}), so no session profile is checked",
                            dir.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&dir)),
                );
                return;
            }
        };
        let Some(accounts) = self.all_accounts() else {
            return;
        };
        // The providers whose `sessions` runs, and reports their own orphans (§13.1's scope).
        let in_scope: Vec<ProviderId> = self.scope(only).iter().map(|p| p.id()).collect();
        let mut found = Vec::new();
        for (dir, marker) in profiles {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if accounts.iter().any(|r| r.id.as_str() == name) {
                continue;
            }
            let delete = Self::move_then_delete(&dir);
            // No provider is known for these, so every registered one judges, as `purge` does
            // when a marker names none it can ask.
            let judges: Vec<&dyn Provider> = self
                .engine
                .registry
                .all()
                .iter()
                .map(|p| p.as_ref())
                .collect();
            let orphaned = !matches!(&marker, Read::Present(m)
                if self.engine.registry.get(&m.provider).is_some() && in_scope.contains(&m.provider));
            let reports = orphaned && (only.is_none() || !matches!(&marker, Read::Present(_)));
            if reports {
                found.extend(self.orphan_ownership(&judges, &dir));
            }
            match marker {
                Read::Unreadable(e) => found.push(
                    Check::warn(
                        "sessions.marker",
                        format!(
                            "the profile marker {} cannot be read ({}), so whose profile {} is cannot be told",
                            e.what,
                            e.detail,
                            dir.display()
                        ),
                    )
                    .fix(Self::move_then_delete(&dir)),
                ),
                Read::Present(m) if self.engine.registry.get(&m.provider).is_none() => {
                    if only.is_none() {
                        found.push(
                            Check::warn(
                                "sessions.orphan",
                                format!(
                                    "{} is a profile of {}, which this tagteam does not have",
                                    dir.display(),
                                    m.provider
                                ),
                            )
                            .fix(delete),
                        );
                    }
                }
                // A registered provider with no account is out of scope, so no `sessions` run
                // finds its orphans.
                Read::Present(m) if !in_scope.contains(&m.provider) => {
                    if only.is_none() {
                        found.push(
                            Check::warn(
                                "sessions.orphan",
                                format!(
                                    "{} is a profile of {}, which has no account in this store",
                                    dir.display(),
                                    m.provider
                                ),
                            )
                            .fix(format!(
                                "{delete}; `tagteam purge` deletes it with its Keychain item"
                            )),
                        );
                    }
                }
                Read::Present(_) => {}
                Read::Absent => found.push(
                    Check::warn(
                        "sessions.orphan",
                        format!(
                            "{} has no profile marker and names no account",
                            dir.display()
                        ),
                    )
                    .fix(delete),
                ),
            }
        }
        for check in found {
            self.push(None, check);
        }
    }

    /// §13.6 Session profiles, for `p`: each profile whose marker names `p`, and the entries of
    /// `p`'s source home on no share list. The profiles need every account and `sessions/`'s
    /// listing; when either cannot be read, `store.accounts` (or `store.skipped`) and
    /// `sessions.profiles` say so, once.
    fn sessions(&mut self, p: &dyn Provider) {
        if !p.capabilities().sessions {
            return;
        }
        let id = p.id();
        let mut found = Vec::new();
        let (accounts, profiles) = match (self.all_accounts(), self.profile_dirs()) {
            (Some(accounts), Ok(profiles)) => (accounts, profiles),
            _ => (Vec::new(), Vec::new()),
        };
        for (dir, marker) in profiles {
            // §5: a directory named by a stored account's ID is that account's profile, whatever
            // its marker says, and the account's provider checks it.
            let name = dir.file_name().and_then(|n| n.to_str());
            if let Some(row) = accounts.iter().find(|r| Some(r.id.as_str()) == name) {
                if row.provider != id {
                    continue;
                }
                let why = match &marker {
                    Read::Present(m) if m.account_id == row.id && m.provider == row.provider => {
                        self.profile(p, row, &dir, Some(m), &mut found);
                        continue;
                    }
                    Read::Present(m) => format!("names {} of {} instead", m.account_id, m.provider),
                    Read::Unreadable(e) => format!("cannot be read ({}: {})", e.what, e.detail),
                    Read::Absent => "is missing".to_owned(),
                };
                // §13.6: a marker that cannot be trusted warns, and what needs no marker is
                // still checked.
                let n = row.position;
                found.push(
                    Check::warn(
                        "sessions.marker",
                        format!(
                            "account {n}'s profile marker {why}, so its recorded spelling and outer home are unknown: its credential and provenance are not checked"
                        ),
                    )
                    .fix(format!(
                        "{}; if account {n}'s login then stops working, log in again and run `{}`",
                        Self::move_then_delete(&dir),
                        command(self.engine, &id, &format!("add --position {n}"))
                    )),
                );
                self.profile(p, row, &dir, None, &mut found);
                continue;
            }
            // Any other directory is its marker's provider's; `stray_profiles` reports one
            // with no marker it can read.
            let Read::Present(m) = marker else { continue };
            if m.provider != id {
                continue;
            }
            match accounts
                .iter()
                .find(|r| r.id == m.account_id && r.provider == id)
            {
                Some(row) => found.push(
                    Check::warn(
                        "sessions.marker",
                        format!(
                            "{} holds the marker of account {}'s profile, which lives elsewhere",
                            dir.display(),
                            row.position
                        ),
                    )
                    .fix(Self::move_then_delete(&dir)),
                ),
                None => {
                    found.extend(self.orphan_ownership(&[p], &dir));
                    found.push(
                        Check::warn(
                            "sessions.orphan",
                            format!(
                                "{} is a profile of an account this store no longer has",
                                dir.display()
                            ),
                        )
                        .fix(format!(
                            "{}; `tagteam purge` deletes it with its Keychain item",
                            Self::move_then_delete(&dir)
                        )),
                    );
                }
            }
        }
        self.unknown_entries(p, &mut found);
        for check in found {
            self.push(Some(&id), check);
        }
    }

    /// §13.6: a live background-daemon supervisor in `profile` (`daemon.lock`, §12.6), found at
    /// that path, as the refusal names it. `whose` finishes "runs in ...".
    fn daemon_check(profile: &Path, whose: &str) -> Check {
        Check::info(
            "sessions.daemon",
            format!("a Claude Code background daemon runs in {whose} until it stops"),
        )
        .fix(daemon_advice(profile))
    }

    /// What `state` says about ownership of the profile `whose` names, from the whole aggregate
    /// and through the shared builder: a warning for every input that could not be read, with
    /// its repair, and the daemon, if one is live (§12.6). Nothing is filtered, so the account
    /// checks and the orphan checks report alike. `consequence` finishes the warning.
    fn ownership(
        state: &SessionState,
        whose: &str,
        runs_in: &str,
        consequence: &str,
    ) -> Vec<Check> {
        let mut out = Vec::new();
        if !state.damaged().is_empty() {
            let owner = SessionOwner::of(state);
            out.push(
                Check::warn(
                    "sessions.state",
                    format!(
                        "{whose} session state cannot be read ({}), {consequence}",
                        owner.damaged_list()
                    ),
                )
                .fix(owner.repairs().join("; ")),
            );
        }
        if let Some(profile) = state.daemon_profile() {
            out.push(Self::daemon_check(
                profile,
                &format!("{runs_in}, so it is session-owned"),
            ));
        }
        out
    }

    /// `ownership` for the profile at `dir` that no stored account owns: each of `judges` is
    /// asked, as `purge` asks (§10.5 step 6), and every finding is reported once.
    fn orphan_ownership(&self, judges: &[&dyn Provider], dir: &Path) -> Vec<Check> {
        let whose = format!("the orphaned profile {}'s", dir.display());
        let runs_in = format!("the orphaned profile {}", dir.display());
        let mut out: Vec<Check> = Vec::new();
        for p in judges.iter().filter(|p| p.capabilities().sessions) {
            let state = self.engine.session_state_at(*p, dir);
            for c in Self::ownership(
                &state,
                &whose,
                &runs_in,
                "so it counts as in use: `tagteam purge` refuses it",
            ) {
                if !out.iter().any(|o| o.id == c.id && o.message == c.message) {
                    out.push(c);
                }
            }
        }
        out
    }

    /// One profile of account `row` (§12.2–§12.6), read and probed only. With no marker, its
    /// splits are judged against the default home, as `remove` judges such a profile, and what
    /// needs the recorded spelling (the spelling itself, the credential and the provenance) is
    /// skipped: the caller's `sessions.marker` warning says so.
    fn profile(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        dir: &Path,
        marker: Option<&ProfileMarker>,
        found: &mut Vec<Check>,
    ) {
        let n = row.position;
        let env = &self.engine.env;
        if let Some(marker) = marker {
            self.spelling(p, dir, marker, n, found);
        }
        match marker.map(|m| p.apply_outer_home(env, &m.outer)) {
            Some(Ok(outer)) => self.splits(p, &outer, dir, found),
            Some(Err(e)) => found.push(
                Check::warn(
                    "sessions.marker",
                    format!("account {n}'s profile marker records an outer home that cannot be read: {e}"),
                )
                .fix(format!("nothing to do: account {n}'s next `tagteam run` records it again")),
            ),
            None => self.splits(p, env, dir, found),
        }
        let state = self.engine.session_state(p, row);
        let quiescent = matches!(state, Ok(SessionState::Quiescent { .. }));
        // The whole aggregate, as an orphan's is reported: an unreadable input beside a live
        // owner, and the daemon, whatever else is found.
        match &state {
            Ok(state) => found.extend(Self::ownership(
                state,
                &format!("account {n}'s"),
                &format!("account {n}'s profile"),
                "so the account counts as in a session: commands that change it refuse, and its baseline and provenance are not checked",
            )),
            Err(e) => found.push(
                Check::warn(
                    "sessions.state",
                    format!(
                        "account {n}'s session state cannot be read ({}), so the account counts as in a session: commands that change it refuse, and its baseline and provenance are not checked",
                        e.kind()
                    ),
                )
                .fix("make what it names readable again"),
            ),
        }
        self.reservations(dir, n, found);
        if quiescent && p.has_baseline(dir) {
            found.push(
                Check::info(
                    "sessions.baseline",
                    format!("account {n}'s profile has a config baseline awaiting merge-back"),
                )
                .fix(format!(
                    "nothing to do: account {n}'s next `tagteam run` merges it back"
                )),
            );
        }
        let seed = Seed::read(dir);
        match &seed {
            Read::Present(s) if s.login_epoch != row.login_epoch || s.needs_bootstrap => {
                let why = if s.login_epoch != row.login_epoch {
                    "is stale-marked: an explicit command replaced the account's login"
                } else {
                    "needs a bootstrap: its last login check found it invalid"
                };
                found.push(
                    Check::info("sessions.bootstrap", format!("account {n}'s profile {why}")).fix(
                        format!("nothing to do: account {n}'s next `tagteam run` bootstraps it"),
                    ),
                );
            }
            Read::Unreadable(e) => found.push(
                Check::warn(
                    "sessions.seed",
                    format!(
                        "account {n}'s profile seed {} cannot be read: {}",
                        e.what, e.detail
                    ),
                )
                .fix(format!(
                    "nothing to do: account {n}'s next `tagteam run` bootstraps it"
                )),
            ),
            _ => {}
        }
        // The credential's Keychain item is named from the recorded spelling (§12.2).
        let Some(marker) = marker else { return };
        if self.provider_secrets(p) != Secrets::Readable {
            return;
        }
        let cannot = format!(
            "account {n}'s profile credential cannot be read, so the account cannot switch or launch until it can"
        );
        let held = match p.read_profile_credential(env, dir, &marker.config_dir) {
            Read::Unreadable(e) => {
                found.push(
                    Check::warn("sessions.credential", format!("{cannot}: {e}"))
                        .fix("make it readable again (on macOS, unlock the login keychain)"),
                );
                return;
            }
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                found.push(
                    Check::warn(
                        "sessions.credential",
                        format!("{cannot}: only its file could be read, which may be out of date"),
                    )
                    .fix("make its Keychain item readable again (unlock the login keychain)"),
                );
                return;
            }
            Read::Present(c) => c.bytes().to_vec(),
            Read::Absent => return,
        };
        let Read::Present(seed) = seed else { return };
        if !quiescent {
            return;
        }
        // §12.5: only a login that names another account is drift, which is ignored. An absent
        // identity decides nothing: the table runs, and only a rotation needs one (Decision 9).
        let identity_absent = match p.profile_identity(env, dir) {
            Read::Present(login) if !identity_drifted(&login, row) => false,
            Read::Present(_) => return,
            Read::Absent => true,
            Read::Unreadable(e) => {
                found.push(
                    Check::warn(
                        "sessions.provenance",
                        format!(
                            "whether account {n}'s profile and the vault both moved cannot be told: the profile's identity cannot be read ({e})"
                        ),
                    )
                    .fix("make the file it names readable again"),
                );
                return;
            }
        };
        let Some(p_fp) = p.fingerprint(&held).filter(|_| p.has_refresh_token(&held)) else {
            return;
        };
        // The vault's Keychain can be locked while the provider's is not.
        if self.secrets() != Secrets::Readable {
            return;
        }
        // An entry that cannot be read is `accounts.vault`'s `fail`, in the same run.
        let Read::Present(vault) = self.engine.vault.read(&row.id) else {
            return;
        };
        let Some(v_fp) = p.fingerprint(&vault) else {
            return;
        };
        let stale = seed.login_epoch != row.login_epoch;
        let replace = format!(
            "log in as account {n} with `{}`, then `{}`: an explicit replacement settles it",
            p.launch_command(),
            command(self.engine, &p.id(), &format!("add --position {n}"))
        );
        match provenance(p_fp.as_str(), v_fp.as_str(), &seed.seed_fp, stale) {
            ProvenanceVerdict::Conflict => found.push(
                Check::fail(
                    "sessions.provenance",
                    format!(
                        "account {n}'s profile and the vault both moved since they last agreed: nothing is captured, refreshed or launched for the account"
                    ),
                )
                .fix(replace),
            ),
            // The profile rotated the vault's generation, but names no identity that says the
            // rotation is the account's: no holder of its lock captures it (Decision 9).
            ProvenanceVerdict::Capture if identity_absent => found.push(
                Check::warn(
                    "sessions.provenance",
                    format!(
                        "account {n}'s profile holds a login it rotated but names no identity, so it cannot be told to be the account's and is not captured"
                    ),
                )
                .fix(replace),
            ),
            _ => {}
        }
    }

    /// §13.6: a recorded spelling (§12.2 "One spelling") that is no longer the profile's
    /// canonical path, or a path that cannot be resolved.
    fn spelling(
        &self,
        p: &dyn Provider,
        dir: &Path,
        marker: &ProfileMarker,
        n: u32,
        found: &mut Vec<Check>,
    ) {
        match canonical_profile_path(dir) {
            Ok(canonical) => {
                let now = p.profile_spelling(&canonical);
                if now != marker.config_dir {
                    found.push(
                        Check::warn(
                            "sessions.spelling",
                            format!(
                                "account {n}'s profile is recorded as {} but is now {now}: the data directory moved",
                                marker.config_dir
                            ),
                        )
                        .fix(format!("nothing to do: account {n}'s next `tagteam run` bootstraps it")),
                    );
                }
            }
            Err(e) => found.push(
                Check::warn(
                    "sessions.spelling",
                    format!(
                        "whether account {n}'s profile is still at {} cannot be told: {} cannot be resolved ({})",
                        marker.config_dir,
                        dir.display(),
                        e.kind()
                    ),
                )
                .fix(readable(dir)),
            ),
        }
    }

    /// §12.2: a must-share entry the profile holds as a real copy, or as a link that resolves
    /// elsewhere, fails; another shared entry held as a real copy warns. Read-only: link sync's
    /// own allowlist and matcher, with the provider's `run.share_extra`.
    fn splits(&self, p: &dyn Provider, outer: &Env, dir: &Path, found: &mut Vec<Check>) {
        let policy = p.share_policy(outer);
        let (settings, _) = Settings::load(&self.engine.env, &p.id());
        for w in allowlist(&policy, &settings.share_extra, &mut Vec::new()) {
            let (src, dst) = (policy.source.join(&w.name), dir.join(&w.name));
            let untold = |path: &Path, e: io::Error| {
                Check::warn(
                    "sessions.split",
                    format!(
                        "whether {} is split from {} cannot be told: {} cannot be read ({})",
                        dst.display(),
                        src.display(),
                        path.display(),
                        e.kind()
                    ),
                )
                .fix(readable(path))
            };
            let target = match resolved(&src) {
                Ok(target) => target,
                Err(e) => {
                    found.push(untold(&src, e));
                    continue;
                }
            };
            let split = match held(&dst) {
                Ok(Held::Real) => w.must.is_some() || target.is_some(),
                Ok(Held::Link(_)) if w.must.is_some() && target.is_some() => match resolved(&dst) {
                    Ok(now) => now != target,
                    Err(e) => {
                        found.push(untold(&dst, e));
                        continue;
                    }
                },
                Ok(Held::Link(_) | Held::Nothing) => false,
                Err(e) => {
                    found.push(untold(&dst, e));
                    continue;
                }
            };
            if !split {
                continue;
            }
            found.push(if w.must.is_some() {
                Check::fail(
                    "sessions.split",
                    format!(
                        "{} is not the link to {} it must be: memory or history is split, and `tagteam run` refuses the account",
                        dst.display(),
                        src.display()
                    ),
                )
                .fix(format!(
                    "merge {} into {} by hand, then remove {}",
                    quoted(&dst),
                    quoted(&src),
                    quoted(&dst)
                ))
            } else {
                Check::warn(
                    "sessions.split",
                    format!(
                        "{} is a real copy where {} should be linked: the two have split",
                        dst.display(),
                        src.display()
                    ),
                )
                .fix(format!(
                    "merge them by hand and remove {}; the next launch links it",
                    quoted(&dst)
                ))
            });
        }
    }

    /// §12.5: a reservation whose `tagteam` is gone but whose lock is still held, with the
    /// processes that hold it where the OS can tell. Probed, never waited on, never created.
    fn reservations(&self, dir: &Path, n: u32, found: &mut Vec<Check>) {
        let list = match launch_reservations(dir) {
            Read::Present(list) => list,
            // The session state reads the same reservations, and `sessions.state` reports
            // them when they cannot be read.
            Read::Absent | Read::Unreadable(_) => return,
        };
        for (path, probe) in list {
            if probe != LockProbe::Held {
                continue;
            }
            let Some(parent) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if self.engine.process.exists(parent) != Some(false) {
                continue;
            }
            let holders = holders_of(&path);
            let still_there = holders.is_some() || path.try_exists().unwrap_or(true);
            found.push(held_reservation(&path, parent, n, holders, still_there));
        }
    }

    /// §12.2 "Unknown entries": the entries of `p`'s source home on neither its share lists
    /// nor its known-private list, which each profile keeps its own of.
    fn unknown_entries(&self, p: &dyn Provider, found: &mut Vec<Check>) {
        let policy = p.share_policy(&self.engine.env);
        let (settings, _) = Settings::load(&self.engine.env, &p.id());
        let wanted = allowlist(&policy, &settings.share_extra, &mut Vec::new());
        // A source home that does not exist holds nothing to report.
        let listing = match policy.source.try_exists() {
            Ok(false) => return,
            Ok(true) => entries(&policy.source),
            Err(e) => Err(e),
        };
        let paths = match listing {
            Ok(paths) => paths,
            Err(e) => {
                found.push(
                    Check::warn(
                        "sessions.unknown-entries",
                        format!(
                            "{} cannot be listed ({}), so whether it holds entries on no share list cannot be told",
                            policy.source.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&policy.source)),
                );
                return;
            }
        };
        let unknown: Vec<String> = paths
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !is_private(&policy, n) && !wanted.iter().any(|w| &w.name == n))
            .collect();
        if unknown.is_empty() {
            found.push(Check::ok(
                "sessions.unknown-entries",
                format!(
                    "every entry of {} is on a share list",
                    policy.source.display()
                ),
            ));
            return;
        }
        found.push(
            Check::warn(
                "sessions.unknown-entries",
                format!(
                    "{} holds entries on none of {}'s share lists, so each profile keeps its own: {}",
                    policy.source.display(),
                    p.display_name(),
                    unknown.join(", ")
                ),
            )
            .fix("to share one with every profile, `tagteam config set run.share_extra <names>`"),
        );
    }

    /// §13.6 `--online`: TLS reachability of `p`'s hosts through the engine's `Http` port, in
    /// parallel, with no credentials: any HTTP response reaches the host, a `PreSend` failure
    /// does not, and an `Ambiguous` one cannot tell. A signal that has landed sends nothing.
    fn online(&mut self, p: &dyn Provider) -> Result<(), EngineError> {
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
                Ok(status) => Check::ok("online.reach", format!("{url} answers (HTTP {status})")),
                Err(HttpError::PreSend(e)) => {
                    Check::fail("online.reach", format!("{url} cannot be reached: {e}"))
                        .fix("check the network, any proxy (HTTPS_PROXY) and the TLS trust store")
                }
                Err(HttpError::Ambiguous(e)) => Check::warn(
                    "online.reach",
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
    fn keychain_note(&mut self, providers: &[Arc<dyn Provider>]) {
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
                "keychain.locked",
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
                "keychain.locked",
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

/// `sessions.reservation` for a reservation `path` still held after its tagteam `parent` died.
/// `holders` is `holders_of`'s answer; `None` means either that this OS cannot tell, or (when
/// `still_there` is false) that the lock file went away between the probe and the scan. (The
/// scan's `stat` of the file could hang on a stalled NFS mount; parked, R-T10.)
fn held_reservation(
    path: &Path,
    parent: u32,
    n: u32,
    holders: Option<Vec<u32>>,
    still_there: bool,
) -> Check {
    if holders.is_none() && !still_there {
        return Check::info(
            "sessions.reservation",
            format!(
                "{} was held although its tagteam (pid {parent}) is gone, but the lock went away while doctor looked, so account {n}'s state is not known to have stayed the same",
                path.display()
            ),
        )
        .fix("run `tagteam doctor` again");
    }
    let holders = match holders {
        Some(pids) if !pids.is_empty() => format!(
            "held by pid {}",
            pids.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(_) => "held by a process this user cannot see".to_owned(),
        None => "this OS cannot tell which process holds it".to_owned(),
    };
    Check::info(
        "sessions.reservation",
        format!(
            "{} is still held although its tagteam (pid {parent}) is gone ({holders}): a process its `claude` started still runs, so account {n} stays in a session",
            path.display()
        ),
    )
    .fix("end that process to release the account")
}

/// §7.4's quarantine reasons, in words.
fn quarantine_words(reason: &str) -> &'static str {
    match reason {
        "invalid_grant" => "the token endpoint refused its refresh token",
        "no_refresh_token" => "it has no refresh token to renew it with",
        "identity_conflict" => "the token endpoint named another account",
        "successor_lost" => "a refreshed login could not be stored anywhere",
        _ => "it cannot be refreshed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_whose_lock_went_away_is_not_reported_as_unknowable() {
        let path = Path::new("/d/.tagteam-launch/42.lock");
        let gone = held_reservation(path, 42, 1, None, false);
        assert!(gone.message.contains("lock went away"), "{}", gone.message);
        assert_eq!(gone.fix.as_deref(), Some("run `tagteam doctor` again"));
        let blind = held_reservation(path, 42, 1, None, true);
        assert!(
            blind.message.contains("this OS cannot tell"),
            "{}",
            blind.message
        );
        let seen = held_reservation(path, 42, 1, Some(vec![7, 9]), true);
        assert!(
            seen.message.contains("held by pid 7, 9"),
            "{}",
            seen.message
        );
    }

    #[test]
    fn every_quarantine_reason_has_its_words() {
        for reason in [
            "invalid_grant",
            "no_refresh_token",
            "identity_conflict",
            "successor_lost",
        ] {
            assert_ne!(
                quarantine_words(reason),
                quarantine_words("other"),
                "{reason}"
            );
        }
    }

    #[test]
    fn an_engine_whose_liveness_cannot_be_told_is_a_warning_with_a_fix() {
        let c = engine_record_check(4242, Liveness::Unknown("permission denied".into()));
        assert_eq!(c.id, "auto.engine");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.message.contains("4242") && c.message.contains("permission denied"));
        assert!(c.fix.as_deref().is_some_and(|f| f.contains("pid 4242")));
        assert_eq!(
            engine_record_check(4242, Liveness::Live).status,
            CheckStatus::Info
        );
        assert_eq!(
            engine_record_check(4242, Liveness::Dead).status,
            CheckStatus::Ok
        );
    }

    #[test]
    fn an_integrity_failure_stands_only_when_a_second_run_finds_it_too() {
        let bad = || Ok(vec!["row 1 missing from index".to_owned()]);
        let ok = || Ok(vec!["ok".to_owned()]);
        let rows = |r: Result<Vec<String>, crate::store::StoreError>| r.unwrap();
        assert_eq!(
            rows(settled_quick_check(ok(), || panic!("no second run"))),
            rows(ok())
        );
        assert_eq!(rows(settled_quick_check(bad(), ok)), rows(ok()));
        assert_eq!(rows(settled_quick_check(bad(), bad)), rows(bad()));
        let unreadable = || Err(crate::store::StoreError::Corrupt("x".into()));
        assert!(settled_quick_check(unreadable(), unreadable).is_err());
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
