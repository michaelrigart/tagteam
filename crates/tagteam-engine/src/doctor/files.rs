use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_provider::atomic::{temp_writer_pid, writable};
use tagteam_provider::doctor::quoted;
use tagteam_provider::env::LOG_ROTATIONS;
use tagteam_provider::{Check, Provider};

use crate::settings;

use super::{Id, Run};

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
pub(super) fn entries(dir: &Path) -> io::Result<Vec<PathBuf>> {
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
pub(super) fn subdirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
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
pub(super) fn readable(path: &Path) -> String {
    format!(
        "make {} and the directories above it readable by this user",
        quoted(path)
    )
}

impl Run<'_> {
    /// §13.6 Store modes: the store 0600, tagteam's directories 0700, and a file vault's files
    /// 0600 (§5). Paths that do not exist are not checked. A mode that cannot be read, or a
    /// directory whose entries cannot be listed, warns, and the check is then not `ok`.
    pub(super) fn modes(&mut self) {
        let engine = self.engine;
        let env = &engine.env;
        let data = env.data_dir();
        let mut found = Vec::new();
        let unlisted = |dir: &Path, e: io::Error| {
            Check::warn(
                Id::STORE_MODE,
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
                            Id::STORE_MODE,
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
                            Id::STORE_MODE,
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
                Id::STORE_MODE,
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
    pub(super) fn temp_files(&mut self, providers: &[Arc<dyn Provider>]) {
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
                Check::ok(Id::STORE_TEMP_FILES, "no write left a temp file behind"),
            );
        }
        for (dir, e) in unlisted {
            self.push(
                None,
                Check::warn(
                    Id::STORE_TEMP_FILES,
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
                    Id::STORE_TEMP_FILES,
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
                    Id::STORE_TEMP_FILES,
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
    pub(super) fn settings(&mut self, only: Option<&ProviderId>) {
        let path = settings::config_path(&self.engine.env);
        match fs::read_to_string(&path) {
            Err(e) if e.kind() == ErrorKind::NotFound => {
                self.push(
                    None,
                    Check::ok(
                        Id::SETTINGS_FILE,
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
                        Id::SETTINGS_FILE,
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
                        Id::SETTINGS_FILE,
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
                Check::ok(Id::SETTINGS_FILE, format!("{} parses", path.display())),
            ),
        }
        let ids: Vec<ProviderId> = match only {
            Some(p) => vec![p.clone()],
            None => self.engine.registry.all().iter().map(|p| p.id()).collect(),
        };
        let (mut warnings, mut unknown) = (Vec::new(), Vec::new());
        for id in &ids {
            let found = settings::inspect(&self.engine.env, id, &self.engine.default_provider);
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
                Check::warn(Id::SETTINGS_VALUE, w)
                    .fix("correct it with `tagteam config set`, or remove it with `tagteam config unset`"),
            );
        }
        for key in unknown {
            self.push(
                None,
                Check::warn(
                    Id::SETTINGS_UNKNOWN,
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
    pub(super) fn log(&mut self) {
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
                        Id::LOG_FILE,
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
                            Id::LOG_FILE,
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
                    Id::LOG_FILE,
                    format!("nothing is logged yet; the log will be {}", log.display()),
                ),
            ),
            Err(e) => self.push(
                None,
                Check::warn(
                    Id::LOG_FILE,
                    format!("{} cannot be read: {e}", log.display()),
                )
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
                            Id::LOG_WRITABLE,
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
                        Id::LOG_WRITABLE,
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
                    Id::LOG_WRITABLE,
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
                Check::ok(
                    Id::LOG_WRITABLE,
                    format!("{} can be written", log.display()),
                ),
            ),
            Ok(false) => self.push(
                None,
                Check::warn(
                    Id::LOG_WRITABLE,
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
                    Id::LOG_WRITABLE,
                    format!(
                        "whether {} can be written cannot be told: {e}",
                        log.display()
                    ),
                )
                .fix(readable(&at)),
            ),
        }
    }
}
