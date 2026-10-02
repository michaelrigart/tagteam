//! §12.2's session profile on disk, provider-neutral: where a profile lives, tagteam's three
//! files in it (the marker, the seed and the links record), its launch reservations, and the
//! share-list matcher. What a provider shares, and how it spells and reads a profile, come
//! from the `Provider` session methods.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use tagteam_core::{AccountId, Fingerprint, ProviderId};

use crate::atomic::{ensure_private_dir, write_atomic_private};
use crate::env::Env;
use crate::flock::{LockProbe, probe_lock};
use crate::read::{Read, ReadError};

pub const MARKER_FILE: &str = ".tagteam-profile.json";
pub const SEED_FILE: &str = ".tagteam-seed.json";
pub const LINKS_FILE: &str = ".tagteam-links.json";
pub const LAUNCH_DIR: &str = ".tagteam-launch";

const MARKER_FORMAT: &str = "tagteam-profile";
const SEED_FORMAT: &str = "tagteam-seed";
const LINKS_FORMAT: &str = "tagteam-links";
const VERSION: i64 = 1;

/// `<data_dir>/sessions/<id>` (§5).
pub fn profile_path(env: &Env, id: &AccountId) -> PathBuf {
    env.data_dir().join("sessions").join(id.as_str())
}

/// `realpath` of an existing profile directory (§12.2 "One spelling", before the provider's
/// own normalization).
pub fn canonical_profile_path(profile: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(profile)
}

/// Whether `e` says nothing is at the path, or that the path crosses something that is not a
/// directory.
fn no_entry(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// One of tagteam's files in `profile`: `Absent` only when there is no entry at its path, as
/// when `profile` is not a directory at all; `Unreadable` when it exists but cannot be read, a
/// link that dangles or crosses a file included (§4.3), or `parse` refuses it. `parse`'s detail
/// never quotes the file's bytes.
fn read_own_file<T>(
    profile: &Path,
    name: &str,
    parse: impl FnOnce(&Value) -> Result<T, String>,
) -> Read<T> {
    let path = profile.join(name);
    let unreadable =
        |detail: String| Read::Unreadable(ReadError::new(path.display().to_string(), detail));
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        // The read found nothing; the entry itself decides whether nothing is there.
        Err(e) if no_entry(&e) => {
            return match fs::symlink_metadata(&path) {
                Err(m) if no_entry(&m) => Read::Absent,
                Ok(_) => unreadable(format!("it is a link that does not resolve: {e}")),
                Err(m) => unreadable(m.to_string()),
            };
        }
        Err(e) => return unreadable(e.to_string()),
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        return unreadable("it is not JSON".into());
    };
    match parse(&v) {
        Ok(t) => Read::Present(t),
        Err(detail) => unreadable(detail),
    }
}

/// Writes one of tagteam's files in `profile`: atomic, 0600, creating `profile` 0700 if absent.
fn write_own_file(profile: &Path, name: &str, v: &Value) -> io::Result<()> {
    ensure_private_dir(profile)?;
    let mut bytes = serde_json::to_vec_pretty(v).expect("a Value always serializes");
    bytes.push(b'\n');
    write_atomic_private(&profile.join(name), &bytes, 0o600)
}

fn check_envelope(v: &Value, format: &str, what: &str) -> Result<(), String> {
    if v["format"].as_str() == Some(format) && v["version"].as_i64() == Some(VERSION) {
        Ok(())
    } else {
        Err(format!("it is not a version 1 tagteam {what}"))
    }
}

fn non_empty<'v>(v: &'v Value, key: &str, missing: &str) -> Result<&'v str, String> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| missing.to_owned())
}

/// A name tagteam may record for a profile entry: one path component, never `.` or `..`.
/// Anything else in a links record could make link sync touch a path outside the profile.
fn is_entry_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains('/') && !s.contains('\0')
}

/// What makes a directory a run shell's profile (§12.2, §12.8). It holds no secret.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileMarker {
    pub provider: ProviderId,
    pub account_id: AccountId,
    /// The exported spelling (§12.2).
    pub config_dir: String,
    /// The provider's record of the outer home (§4.5 `outer_home`).
    pub outer: Value,
}

impl ProfileMarker {
    fn parse(v: &Value) -> Result<Self, String> {
        check_envelope(v, MARKER_FORMAT, "profile marker")?;
        let provider = non_empty(v, "provider", "it names no provider")?;
        let account = non_empty(v, "accountId", "it names no account")?;
        let config_dir = v["configDir"]
            .as_str()
            .filter(|s| s.starts_with('/') && !s.contains('\0'))
            .ok_or_else(|| "its configDir is not an absolute path".to_owned())?;
        let outer = v
            .get("outer")
            .filter(|o| o.is_object())
            .ok_or_else(|| "it has no outer record".to_owned())?;
        Ok(Self {
            provider: ProviderId::new(provider),
            account_id: AccountId::from_string(account),
            config_dir: config_dir.to_owned(),
            outer: outer.clone(),
        })
    }

    /// `Absent`: no marker file. `Unreadable`: present but not a valid marker (§12.8).
    pub fn read(profile: &Path) -> Read<ProfileMarker> {
        read_own_file(profile, MARKER_FILE, Self::parse)
    }

    /// Atomic, 0600; creates `profile` 0700 if absent.
    pub fn write(&self, profile: &Path) -> io::Result<()> {
        let v = json!({
            "format": MARKER_FORMAT,
            "version": VERSION,
            "provider": self.provider.as_str(),
            "accountId": self.account_id.as_str(),
            "configDir": self.config_dir,
            "outer": self.outer,
        });
        write_own_file(profile, MARKER_FILE, &v)
    }
}

/// §12.5's profile provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    /// The login epoch the profile was bootstrapped under (§12.5).
    pub login_epoch: i64,
    /// The generation fingerprint the profile last agreed on with the vault.
    pub seed_fp: String,
    /// Set by M4b's per-launch check when validation reported `invalid` (§12.3).
    pub needs_bootstrap: bool,
}

impl Seed {
    fn parse(v: &Value) -> Result<Self, String> {
        check_envelope(v, SEED_FORMAT, "seed")?;
        let login_epoch = v["loginEpoch"]
            .as_i64()
            .ok_or_else(|| "it has no integer loginEpoch".to_owned())?;
        let seed_fp = v["seedFp"]
            .as_str()
            .filter(|s| Fingerprint::parse(s).is_some())
            .ok_or_else(|| "its seedFp is not a fingerprint".to_owned())?;
        let needs_bootstrap = match v.get("needsBootstrap") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("its needsBootstrap is not a boolean".into()),
        };
        Ok(Self {
            login_epoch,
            seed_fp: seed_fp.to_owned(),
            needs_bootstrap,
        })
    }

    pub fn read(profile: &Path) -> Read<Seed> {
        read_own_file(profile, SEED_FILE, Self::parse)
    }

    pub fn write(&self, profile: &Path) -> io::Result<()> {
        let v = json!({
            "format": SEED_FORMAT,
            "version": VERSION,
            "loginEpoch": self.login_epoch,
            "seedFp": self.seed_fp,
            "needsBootstrap": self.needs_bootstrap,
        });
        write_own_file(profile, SEED_FILE, &v)
    }
}

/// §12.2's record of what link sync did in a profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinksRecord {
    /// Entry name → the fully resolved source the link points at, for every link tagteam made.
    pub links: BTreeMap<String, PathBuf>,
    /// Unknown entries already noted once (§12.2).
    pub noted_unknown: BTreeSet<String>,
}

impl LinksRecord {
    fn parse(v: &Value) -> Result<Self, String> {
        check_envelope(v, LINKS_FORMAT, "links record")?;
        let Some(links) = v["links"].as_object() else {
            return Err("its links are not an object".into());
        };
        let mut out = LinksRecord::default();
        for (name, source) in links {
            if !is_entry_name(name) {
                return Err("it records a link name that is not one path component".into());
            }
            let source = source
                .as_str()
                .filter(|s| s.starts_with('/') && !s.contains('\0'))
                .ok_or_else(|| {
                    "it records a link source that is not an absolute path".to_owned()
                })?;
            out.links.insert(name.clone(), PathBuf::from(source));
        }
        let Some(noted) = v["notedUnknown"].as_array() else {
            return Err("its notedUnknown is not a list".into());
        };
        for name in noted {
            match name.as_str() {
                Some(n) if is_entry_name(n) => {
                    out.noted_unknown.insert(n.to_owned());
                }
                _ => return Err("it notes an entry name that is not one path component".into()),
            }
        }
        Ok(out)
    }

    /// `Absent`: no record yet, so the caller starts from `LinksRecord::default()`.
    pub fn read(profile: &Path) -> Read<LinksRecord> {
        read_own_file(profile, LINKS_FILE, Self::parse)
    }

    /// Refuses, writing nothing, when a source path is not UTF-8: JSON cannot hold it.
    pub fn write(&self, profile: &Path) -> io::Result<()> {
        let mut links = Map::new();
        for (name, source) in &self.links {
            let source = source.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a linked source path is not UTF-8",
                )
            })?;
            links.insert(name.clone(), Value::String(source.to_owned()));
        }
        let v = json!({
            "format": LINKS_FORMAT,
            "version": VERSION,
            "links": links,
            "notedUnknown": self.noted_unknown,
        });
        write_own_file(profile, LINKS_FILE, &v)
    }
}

/// `<profile>/.tagteam-launch/*.lock`, each probed with `probe_lock`. A missing directory is
/// `Present(vec![])`, and so is one that is not a directory, or under a profile path that is
/// not one (`ENOTDIR`): nothing can be locked there. A directory that cannot be listed, or a
/// reservation that cannot be probed, makes the whole read `Unreadable`: it may hide a live
/// session (§10.3).
pub fn launch_reservations(profile: &Path) -> Read<Vec<(PathBuf, LockProbe)>> {
    let dir = profile.join(LAUNCH_DIR);
    let unreadable = |what: &Path, e: io::Error| {
        Read::Unreadable(ReadError::new(what.display().to_string(), e.to_string()))
    };
    // Nothing that is not a directory, the profile's own path included, holds a lock file.
    let listing = match fs::read_dir(&dir) {
        Ok(l) => l,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Read::Present(vec![]);
        }
        Err(e) => return unreadable(&dir, e),
    };
    let mut paths = Vec::new();
    for entry in listing {
        match entry {
            Ok(e) => {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "lock") {
                    paths.push(path);
                }
            }
            Err(e) => return unreadable(&dir, e),
        }
    }
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        match probe_lock(&path) {
            Ok(state) => out.push((path, state)),
            Err(e) => return unreadable(&path, e),
        }
    }
    Read::Present(out)
}

/// A share-list entry against a directory entry name: exact, or with each `*` matching any run
/// of characters, possibly empty (`*.lock`, `daemon*`, `.*_auth_refresh-*`).
pub fn entry_matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    // Each middle piece at its earliest place leaves the most room for the rest.
    for part in middle {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// §12.8: where a process stands.
#[derive(Debug, Clone, PartialEq)]
pub enum RunShell {
    Outside,
    Inside {
        profile: PathBuf,
        marker: ProfileMarker,
    },
    Unreadable {
        marker: PathBuf,
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    use crate::flock::FlockGuard;

    fn marker() -> ProfileMarker {
        ProfileMarker {
            provider: ProviderId::new("claude-code"),
            account_id: AccountId::from_string("0192"),
            config_dir: "/data/tagteam/sessions/0192".into(),
            outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": ""}),
        }
    }

    fn seed() -> Seed {
        Seed {
            login_epoch: 3,
            seed_fp: Fingerprint::of_secret(b"rt-1").as_str().to_owned(),
            needs_bootstrap: true,
        }
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn file_json(p: &Path) -> Value {
        serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
    }

    /// Every `(case, bytes)` must read as `Unreadable` naming the file, never quoting it.
    fn assert_all_unreadable<T>(
        name: &str,
        cases: Vec<(&str, Vec<u8>)>,
        read: fn(&Path) -> Read<T>,
    ) {
        for (case, bytes) in cases {
            let d = tempfile::tempdir().unwrap();
            let path = d.path().join(name);
            fs::write(&path, &bytes).unwrap();
            match read(d.path()) {
                Read::Unreadable(e) => {
                    assert_eq!(e.what, path.display().to_string(), "{case}");
                    assert!(!e.detail.contains("SENTINEL"), "{case}: {}", e.detail);
                }
                other => panic!("{case}: {other:?}"),
            }
        }
    }

    /// `base` with `key` set to `v`, or removed when `v` is `None`, as bytes.
    fn with(base: &Value, key: &str, v: Option<Value>) -> Vec<u8> {
        let mut b = base.clone();
        match v {
            Some(v) => b[key] = v,
            None => {
                b.as_object_mut().unwrap().remove(key);
            }
        }
        b.to_string().into_bytes()
    }

    #[test]
    fn a_profile_lives_under_the_data_dir_by_account_id() {
        let env = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(
            profile_path(&env, &AccountId::from_string("0192")),
            Path::new("/tmp/fixture/home/.local/share/tagteam/sessions/0192")
        );
    }

    #[test]
    fn the_canonical_path_resolves_links_and_needs_the_directory() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            canonical_profile_path(&link).unwrap(),
            fs::canonicalize(&real).unwrap()
        );
        assert!(canonical_profile_path(&d.path().join("missing")).is_err());
    }

    #[test]
    fn a_marker_round_trips_privately_in_the_spec_s_shape() {
        let d = tempfile::tempdir().unwrap();
        let profile = profile_path(&Env::for_test(d.path()), &AccountId::from_string("0192"));
        assert!(
            matches!(ProfileMarker::read(&profile), Read::Absent),
            "no directory"
        );
        marker().write(&profile).unwrap();
        assert_eq!(mode(&profile), 0o700);
        assert_eq!(mode(&profile.join(MARKER_FILE)), 0o600);
        assert!(matches!(ProfileMarker::read(&profile), Read::Present(m) if m == marker()));
        let v = file_json(&profile.join(MARKER_FILE));
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "format",
                "version",
                "provider",
                "accountId",
                "configDir",
                "outer"
            ]
        );
        assert_eq!(
            v,
            json!({
                "format": "tagteam-profile", "version": 1, "provider": "claude-code",
                "accountId": "0192", "configDir": "/data/tagteam/sessions/0192",
                "outer": {"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": ""}
            })
        );
    }

    #[test]
    fn a_marker_is_absent_without_its_file_and_ignores_unknown_fields() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(ProfileMarker::read(d.path()), Read::Absent));
        let file = d.path().join("not-a-dir");
        fs::write(&file, b"").unwrap();
        assert!(
            matches!(ProfileMarker::read(&file), Read::Absent),
            "a path that is not a directory holds no marker"
        );
        let mut v = json!({
            "format": "tagteam-profile", "version": 1, "provider": "fake-agent",
            "accountId": "0193", "configDir": "/p", "outer": {}, "later": [1]
        });
        fs::write(d.path().join(MARKER_FILE), v.to_string()).unwrap();
        let Read::Present(m) = ProfileMarker::read(d.path()) else {
            panic!("a valid marker");
        };
        assert_eq!(
            (
                m.provider.as_str(),
                m.account_id.as_str(),
                m.config_dir.as_str()
            ),
            ("fake-agent", "0193", "/p")
        );
        v["outer"] = json!({"FAKEAGENT_HOME": "/h"});
        fs::write(d.path().join(MARKER_FILE), v.to_string()).unwrap();
        assert!(
            matches!(ProfileMarker::read(d.path()), Read::Present(m) if m.outer == json!({"FAKEAGENT_HOME": "/h"}))
        );
    }

    /// Each of tagteam's own files, read in `profile`: `Some(true)` when absent,
    /// `Some(false)` when unreadable, `None` when present.
    fn own_files_absent(profile: &Path) -> Vec<(&'static str, Option<bool>)> {
        fn state<T>(r: Read<T>) -> Option<bool> {
            match r {
                Read::Absent => Some(true),
                Read::Unreadable(_) => Some(false),
                Read::Present(_) => None,
            }
        }
        vec![
            (MARKER_FILE, state(ProfileMarker::read(profile))),
            (SEED_FILE, state(Seed::read(profile))),
            (LINKS_FILE, state(LinksRecord::read(profile))),
        ]
    }

    #[test]
    fn an_own_file_that_is_a_link_to_nothing_is_unreadable_not_absent() {
        // §4.3: unreadable is never absent. A link at the marker's path that dangles, or that
        // crosses a regular file, is an entry that cannot be read, so the outer home is unknown
        // (§12.8). Only no entry at all is absence.
        let d = tempfile::tempdir().unwrap();
        let profile = d.path().join("profile");
        fs::create_dir(&profile).unwrap();
        let file = d.path().join("a-file");
        fs::write(&file, b"").unwrap();
        for (what, target) in [
            ("dangling", d.path().join("nowhere")),
            ("crossing a file", file.join("inside")),
        ] {
            for name in [MARKER_FILE, SEED_FILE, LINKS_FILE] {
                let path = profile.join(name);
                let _ = fs::remove_file(&path);
                std::os::unix::fs::symlink(&target, &path).unwrap();
            }
            for (name, absent) in own_files_absent(&profile) {
                assert_eq!(absent, Some(false), "{what}: {name}");
            }
            let Read::Unreadable(e) = ProfileMarker::read(&profile) else {
                panic!("{what}");
            };
            assert_eq!(e.what, profile.join(MARKER_FILE).display().to_string());
        }
    }

    #[test]
    fn own_files_are_absent_only_where_there_is_no_entry() {
        let d = tempfile::tempdir().unwrap();
        for (name, absent) in own_files_absent(d.path()) {
            assert_eq!(absent, Some(true), "missing: {name}");
        }
        // F5: a profile path that is a regular file holds no entry of its own.
        let file = d.path().join("not-a-dir");
        fs::write(&file, b"").unwrap();
        for (name, absent) in own_files_absent(&file) {
            assert_eq!(absent, Some(true), "under a file: {name}");
        }
        // Nor does a profile path that is itself a link to nothing.
        let gone = d.path().join("gone");
        std::os::unix::fs::symlink(d.path().join("nowhere"), &gone).unwrap();
        for (name, absent) in own_files_absent(&gone) {
            assert_eq!(absent, Some(true), "under a dangling profile link: {name}");
        }
    }

    #[test]
    fn a_marker_that_is_not_valid_is_unreadable_and_never_quoted() {
        let base = json!({
            "format": "tagteam-profile", "version": 1, "provider": "claude-code",
            "accountId": "0192", "configDir": "/p", "outer": {}
        });
        let s = |v: &str| Some(json!(v));
        assert_all_unreadable(
            MARKER_FILE,
            vec![
                ("not JSON", b"SENTINEL".to_vec()),
                ("not an object", br#"["SENTINEL"]"#.to_vec()),
                ("another format", with(&base, "format", s("SENTINEL"))),
                ("version 2", with(&base, "version", Some(json!(2)))),
                ("a string version", with(&base, "version", s("1"))),
                ("no provider", with(&base, "provider", None)),
                ("an empty provider", with(&base, "provider", s(""))),
                ("no accountId", with(&base, "accountId", None)),
                (
                    "a numeric accountId",
                    with(&base, "accountId", Some(json!(192))),
                ),
                ("no configDir", with(&base, "configDir", None)),
                (
                    "a relative configDir",
                    with(&base, "configDir", s("SENTINEL/p")),
                ),
                ("no outer", with(&base, "outer", None)),
                ("a string outer", with(&base, "outer", s("SENTINEL"))),
                ("invalid UTF-8", b"{\"format\": \"\xff SENTINEL\"}".to_vec()),
            ],
            ProfileMarker::read,
        );
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join(MARKER_FILE)).unwrap();
        assert!(
            matches!(ProfileMarker::read(d.path()), Read::Unreadable(_)),
            "a directory where the marker belongs"
        );
    }

    #[test]
    fn a_seed_round_trips_and_an_older_one_needs_no_bootstrap() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(Seed::read(d.path()), Read::Absent));
        seed().write(d.path()).unwrap();
        assert_eq!(mode(&d.path().join(SEED_FILE)), 0o600);
        assert!(matches!(Seed::read(d.path()), Read::Present(s) if s == seed()));
        let without = with(
            &file_json(&d.path().join(SEED_FILE)),
            "needsBootstrap",
            None,
        );
        fs::write(d.path().join(SEED_FILE), without).unwrap();
        assert!(matches!(Seed::read(d.path()), Read::Present(s) if !s.needs_bootstrap));
    }

    #[test]
    fn a_seed_that_is_not_valid_is_unreadable() {
        let base = json!({
            "format": "tagteam-seed", "version": 1, "loginEpoch": 3,
            "seedFp": Fingerprint::of_secret(b"rt-1").as_str(), "needsBootstrap": false
        });
        assert_all_unreadable(
            SEED_FILE,
            vec![
                ("not JSON", b"{SENTINEL".to_vec()),
                (
                    "a marker",
                    with(&base, "format", Some(json!("tagteam-profile"))),
                ),
                ("version 2", with(&base, "version", Some(json!(2)))),
                ("no loginEpoch", with(&base, "loginEpoch", None)),
                (
                    "a string loginEpoch",
                    with(&base, "loginEpoch", Some(json!("3"))),
                ),
                (
                    "a fractional loginEpoch",
                    with(&base, "loginEpoch", Some(json!(3.5))),
                ),
                ("no seedFp", with(&base, "seedFp", None)),
                (
                    "a seedFp that is no fingerprint",
                    with(&base, "seedFp", Some(json!("sha256:SENTINEL"))),
                ),
                (
                    "a string needsBootstrap",
                    with(&base, "needsBootstrap", Some(json!("SENTINEL"))),
                ),
            ],
            Seed::read,
        );
    }

    #[test]
    fn a_links_record_round_trips_and_starts_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(LinksRecord::read(d.path()), Read::Absent));
        LinksRecord::default().write(d.path()).unwrap();
        assert!(
            matches!(LinksRecord::read(d.path()), Read::Present(r) if r == LinksRecord::default())
        );
        let record = LinksRecord {
            links: BTreeMap::from([
                ("projects".to_owned(), PathBuf::from("/h/.claude/projects")),
                ("CLAUDE.md".to_owned(), PathBuf::from("/dotfiles/CLAUDE.md")),
            ]),
            noted_unknown: BTreeSet::from(["new-feature".to_owned()]),
        };
        record.write(d.path()).unwrap();
        assert_eq!(mode(&d.path().join(LINKS_FILE)), 0o600);
        assert!(matches!(LinksRecord::read(d.path()), Read::Present(r) if r == record));
        assert_eq!(
            file_json(&d.path().join(LINKS_FILE)),
            json!({
                "format": "tagteam-links", "version": 1,
                "links": {"CLAUDE.md": "/dotfiles/CLAUDE.md", "projects": "/h/.claude/projects"},
                "notedUnknown": ["new-feature"]
            })
        );
    }

    #[test]
    fn a_links_record_never_names_a_path_outside_the_profile() {
        let base = json!({
            "format": "tagteam-links", "version": 1, "links": {"projects": "/h/p"},
            "notedUnknown": ["x"]
        });
        assert_all_unreadable(
            LINKS_FILE,
            vec![
                ("not JSON", b"SENTINEL".to_vec()),
                ("version 2", with(&base, "version", Some(json!(2)))),
                (
                    "links not an object",
                    with(&base, "links", Some(json!(["SENTINEL"]))),
                ),
                (
                    "a dot-dot name",
                    with(&base, "links", Some(json!({"..": "/h/SENTINEL"}))),
                ),
                (
                    "a nested name",
                    with(&base, "links", Some(json!({"a/SENTINEL": "/h/p"}))),
                ),
                (
                    "an empty name",
                    with(&base, "links", Some(json!({"": "/h/p"}))),
                ),
                (
                    "a relative source",
                    with(&base, "links", Some(json!({"p": "SENTINEL"}))),
                ),
                (
                    "a numeric source",
                    with(&base, "links", Some(json!({"p": 7}))),
                ),
                ("no notedUnknown", with(&base, "notedUnknown", None)),
                (
                    "a non-string noted name",
                    with(&base, "notedUnknown", Some(json!([7]))),
                ),
                (
                    "a noted dot",
                    with(&base, "notedUnknown", Some(json!(["."]))),
                ),
            ],
            LinksRecord::read,
        );
    }

    #[test]
    fn a_links_record_whose_source_is_not_utf8_is_never_written() {
        let d = tempfile::tempdir().unwrap();
        let record = LinksRecord {
            links: BTreeMap::from([(
                "projects".to_owned(),
                PathBuf::from(OsStr::from_bytes(b"/h/\xff")),
            )]),
            noted_unknown: BTreeSet::new(),
        };
        let err = record.write(d.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!d.path().join(LINKS_FILE).exists());
    }

    #[test]
    fn launch_reservations_are_every_lock_file_probed_in_name_order() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(launch_reservations(d.path()), Read::Present(v) if v.is_empty()));
        let dir = d.path().join(LAUNCH_DIR);
        fs::create_dir(&dir).unwrap();
        let _held = FlockGuard::try_lock(&dir.join("200.lock"))
            .unwrap()
            .unwrap();
        fs::write(dir.join("100.lock"), b"").unwrap();
        fs::write(dir.join("notes.txt"), b"").unwrap();
        let Read::Present(found) = launch_reservations(d.path()) else {
            panic!("the directory lists");
        };
        assert_eq!(
            found,
            vec![
                (dir.join("100.lock"), LockProbe::Free),
                (dir.join("200.lock"), LockProbe::Held)
            ]
        );
        assert!(!dir.join("300.lock").exists(), "probing creates nothing");
    }

    #[test]
    fn a_launch_directory_that_cannot_be_listed_is_unreadable() {
        // Run as a non-root user: root lists a 0o000 directory.
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(LAUNCH_DIR);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let found = launch_reservations(d.path());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(found, Read::Unreadable(_)), "{found:?}");
    }

    #[test]
    fn no_reservation_can_lie_under_something_that_is_not_a_directory() {
        // A regular file at the launch directory's path, or at the profile's own, holds no lock
        // file, as `read_own_file` reads no marker there: none, not unreadable.
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(LAUNCH_DIR), b"").unwrap();
        assert!(matches!(launch_reservations(d.path()), Read::Present(v) if v.is_empty()));
        let file = d.path().join("profile");
        fs::write(&file, b"").unwrap();
        assert!(matches!(launch_reservations(&file), Read::Present(v) if v.is_empty()));
    }

    #[test]
    fn share_patterns_match_exactly_or_across_each_star() {
        for (pattern, name, matches) in [
            ("sessions", "sessions", true),
            ("sessions", "sessions2", false),
            ("sessions", "session", false),
            ("*.lock", "daemon.lock", true),
            ("*.lock", ".lock", true),
            ("*.lock", "daemon.lock.owner", false),
            ("*.lock.owner", ".oauth_refresh.lock.owner", true),
            ("daemon*", "daemon", true),
            ("daemon*", "daemon.json", true),
            ("daemon*", "xdaemon", false),
            ("daemon.*", "daemon", false),
            ("daemon.*", "daemon.status.json", true),
            (".*_auth_refresh-*", ".oauth_auth_refresh-123", true),
            (".*_auth_refresh-*", "._auth_refresh-", true),
            (".*_auth_refresh-*", ".oauth_refresh.lock", false),
            (".claude-*-oauth.json", ".claude-staging-oauth.json", true),
            (".claude-*-oauth.json", ".claude.json", false),
            ("policy-limits.json*", "policy-limits.json", true),
            ("policy-limits.json*", "policy-limits.json.signature", true),
            (".tagteam-*", ".tagteam-profile.json", true),
            (".tagteam-*", ".tagteam", false),
            ("a*a", "a", false),
            ("a*a", "aa", true),
            ("a*b*a", "aba", true),
            ("a*b*a", "ab", false),
            ("*", "anything", true),
            ("", "", true),
            ("", "x", false),
        ] {
            assert_eq!(entry_matches(pattern, name), matches, "{pattern} vs {name}");
        }
    }
}
