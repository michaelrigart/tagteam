//! §13.6 `tagteam doctor`: tagteam's own state and its interop with each provider. Doctor is
//! read-only (B.67). It takes no `MutationGuard` and no account lock, and runs no recovery
//! (Decision 4). It opens the store read-only and never migrates it (Decision 3). It tests locks
//! without waiting, reads the vault only when the Keychain is unlocked, and creates nothing: no
//! data directory, store, log or lock file. Every finding names its fix.

use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_core::rank::span;
use tagteam_provider::atomic::{temp_writer_pid, writable};
use tagteam_provider::doctor::quoted;
use tagteam_provider::env::LOG_ROTATIONS;
use tagteam_provider::{Check, CheckStatus, LockState, Provider, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::settings;
use crate::store::{AccountRow, SCHEMA_VERSION, Store, replacing_meta_parses};
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

/// Whether doctor may read secrets from the Keychain the vault keeps (§13.6, Appendix A.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Secrets {
    Readable,
    Locked,
    Unknown,
}

/// One doctor run: the read-only store, what was found, and the Keychain's state, asked at
/// most once and only when a check needs a secret.
struct Run<'e> {
    engine: &'e Engine,
    now_ms: i64,
    stored: Stored,
    secrets: OnceCell<Secrets>,
    out: Vec<(Option<ProviderId>, Check)>,
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
            out: Vec::new(),
        };
        run.store();
        run.modes();
        run.settings(opts.provider.as_ref());
        run.log();
        let providers = run.scope(opts.provider.as_ref());
        run.temp_files(&providers);
        run.vault_orphans();
        for p in &providers {
            run.accounts(p.as_ref());
            self.check_cancel()?;
            let id = p.id();
            for check in p.doctor_checks(&self.env, self.spawner.as_ref(), self.cancel()) {
                run.push(Some(&id), check);
            }
            self.check_cancel()?;
        }
        run.keychain_note();
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

    /// The Keychain's lock state, asked once (Appendix A.3's check, which never prompts and is
    /// bounded by its timeout). A backend with no Keychain (Linux) is always readable.
    fn secrets(&self) -> Secrets {
        *self
            .secrets
            .get_or_init(|| match self.engine.vault.keychain() {
                None => Secrets::Readable,
                Some(k) => match k.lock_state() {
                    LockState::Unlocked => Secrets::Readable,
                    LockState::Locked => Secrets::Locked,
                    LockState::Unknown => Secrets::Unknown,
                },
            })
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
                let check = if is_damage(&e) {
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
        let sound = match store.quick_check() {
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
                    ),
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
    /// the check is then not `ok`.
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
                if engine.process.exists(pid) == Some(false) {
                    found.push((path, pid));
                }
            }
        }
        found.sort();
        if found.is_empty() && unlisted.is_empty() {
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
                Check::warn("log.file", format!("{} cannot be read: {e}", log.display())),
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
                ),
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
            ),
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
        let readable = self.secrets() == Secrets::Readable;
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

    /// §13.6: the one warning for every check skipped because the Keychain could not be read
    /// without unlocking it. Said only when a check needed a secret.
    fn keychain_note(&mut self) {
        let Some(state) = self.secrets.get().copied() else {
            return;
        };
        let check = match state {
            Secrets::Readable => return,
            Secrets::Locked => Check::warn(
                "keychain.locked",
                "the login keychain is locked (common over SSH), so the checks that read it were skipped: vault entries, pending replacements, interrupted switches and session credentials",
            )
            .fix("`security unlock-keychain ~/Library/Keychains/login.keychain-db`, then `tagteam doctor` again"),
            Secrets::Unknown => Check::warn(
                "keychain.locked",
                "the login keychain's lock state cannot be told, so the checks that read it were skipped: vault entries, pending replacements, interrupted switches and session credentials",
            )
            .fix("`security show-keychain-info` shows why; unlock the keychain if it is locked, then `tagteam doctor` again"),
        };
        self.push(None, check);
    }
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
    fn a_report_is_ok_until_a_check_fails() {
        let mut report = DoctorReport::default();
        assert!(report.ok());
        report.checks.push((None, Check::warn("a.b", "w")));
        assert!(report.ok());
        report.checks.push((None, Check::fail("a.c", "f")));
        assert!(!report.ok());
    }
}
