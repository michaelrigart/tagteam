//! What every check works with: the paths, the two binaries, the test account, and the helpers
//! that start `claude` and `tagteam` in a compat home, and read or change a home's credential.
//!
//! The test account's refresh token is single-use (R9): only one home may ever refresh it. The
//! helpers keep to that. A profile is dropped only once the vault holds its generation or a newer
//! one, and a credential is changed only by its expiry, never by its tokens.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tagteam_cc::{CcPaths, ItemKind};
use tagteam_provider::atomic::write_atomic_private;
use tagteam_provider::keychain::Keychain as _;
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, Seed};
use tagteam_provider::splice::{get_top_level, replace_top_level};
use tagteam_provider::{Cancel, Read, SystemProcessProbe};

use super::daemon;
use super::guard::{CONFIG_DIR, Roots, cc_env};
use super::keychain::{CcItem, Unlocked, VaultKeychain, random_hex};
use super::layout::{Layout, private_dir};
use super::report::Redactor;
#[cfg(test)]
use super::sys::process_alive;
use super::sys::{Cmd, HarnessError, Pty, Ran, harness, settle};

/// The test account's alias in the compat store, set by `compat login`.
pub const ALIAS_OAUTH: &str = "compat-oauth";
/// A setup token for the same account, set by `compat login` (§12.3's setup-token outcome).
pub const ALIAS_SETUP_TOKEN: &str = "compat-setup-token";
/// A dummy API key some live checks add and remove.
pub const ALIAS_API_KEY: &str = "compat-api-key";
/// One minimal request on the smallest model (§15.4).
pub const PROMPT: &str = "Reply with the single word ok.";
pub const MODEL: &str = "haiku";
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(90);
/// The session-record kinds of `claude --bg`'s supervisor and its workers (Appendix A.7).
pub const DAEMON_KINDS: [&str; 3] = ["daemon", "bg", "daemon-worker"];
/// How long a stopped daemon has to be gone.
const DAEMON_PATIENCE: Duration = Duration::from_secs(30);
/// An expired access token's `expiresAt`: a minute ago.
const EXPIRED_BY_MS: i64 = 60_000;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The generation's fingerprint (§2: the refresh token, else the access token), shortened for
/// a report; `None` for no token at all.
pub fn generation(credential: &[u8]) -> Option<String> {
    tagteam_cc::shape::fingerprint(credential).map(|f| f.short12().to_owned())
}

/// An account of the compat store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub position: u64,
    pub email: String,
}

/// The row of `tagteam list --json` whose alias is `alias`.
pub fn account(list: &Value, alias: &str) -> Option<Account> {
    list["accounts"]
        .as_array()?
        .iter()
        .find(|r| r["alias"] == alias)
        .map(|r| Account {
            id: r["id"].as_str().unwrap_or_default().to_owned(),
            position: r["position"].as_u64().unwrap_or_default(),
            email: r["email"].as_str().unwrap_or_default().to_owned(),
        })
}

/// Where a home's OAuth credential is, as CC reads it: the hashed item first (macOS), then the
/// file (Appendix A.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Item(CcItem),
    File(PathBuf),
}

impl Place {
    pub fn describe(&self) -> Value {
        match self {
            Place::Item(i) => json!({"item": i.service}),
            Place::File(p) => json!({"file": p}),
        }
    }
}

pub struct Ctx {
    pub layout: Layout,
    pub roots: Roots,
    pub tagteam: PathBuf,
    pub claude: PathBuf,
    pub macos: bool,
    pub oauth: Account,
    pub setup_token: Option<Account>,
    /// Unlocked for the run, and locked again whenever the `Ctx` goes.
    pub vault: Option<Unlocked<VaultKeychain>>,
    /// Every child's environment but `CLAUDE_CONFIG_DIR`.
    pub base: Vec<(OsString, OsString)>,
    /// The scratch default home's global config as tagteam created it at activation.
    pub activation_config: Option<Vec<u8>>,
    pub live_active: bool,
    pub ssh: Option<String>,
    /// What every child's output is redacted with as it is captured (`Cmd::redact`).
    pub redact: Redactor,
    /// The token every child it starts stops at: the process's (`sys::cancel`) in a run.
    pub cancel: Cancel,
    /// The homes a check started a daemon in (`must_stop`), which cleanup stops.
    pub daemons: Vec<String>,
}

impl Ctx {
    pub fn live(&self) -> String {
        self.layout.live().to_string_lossy().into_owned()
    }

    /// The environment of a process whose CC home is `spelling`.
    pub fn vars(&self, spelling: &str) -> Vec<(OsString, OsString)> {
        let mut v = self.base.clone();
        v.push((CONFIG_DIR.into(), spelling.into()));
        v
    }

    /// `tagteam <args>` with the scratch default home as its outer home.
    pub fn tagteam(&self, args: &[&str]) -> Cmd {
        Cmd::new(&self.tagteam, self.vars(&self.live()), &self.layout.work())
            .args(args)
            .timeout(LAUNCH_TIMEOUT)
            .redact(&self.redact)
            .cancel(&self.cancel)
    }

    /// `claude <args>` in the CC home `spelling`.
    pub fn claude(&self, spelling: &str, args: &[&str]) -> Cmd {
        Cmd::new(&self.claude, self.vars(spelling), &self.layout.work())
            .args(args)
            .timeout(REQUEST_TIMEOUT)
            .redact(&self.redact)
            .cancel(&self.cancel)
    }

    /// `tagteam run <alias> --require-session -- <args>`: `claude` in the account's session
    /// profile, never in the default home.
    pub fn run_as(&self, alias: &str, args: &[&str]) -> Cmd {
        let mut all = vec!["run", alias, "--require-session", "--"];
        all.extend_from_slice(args);
        self.tagteam(&all).timeout(REQUEST_TIMEOUT)
    }

    pub fn run_profile(&self, args: &[&str]) -> Result<Ran, HarnessError> {
        self.run_as(ALIAS_OAUTH, args).run(&self.roots)
    }

    /// An interactive `claude` in the account's profile, through `tagteam run`.
    pub fn profile_pty(&self, args: &[&str]) -> Result<Pty, HarnessError> {
        self.run_as(ALIAS_OAUTH, args).pty(&self.roots)
    }

    /// A new empty CC home `homes/<name>`, 0700, by a spelling the guard accepts.
    pub fn new_home(&self, name: &str) -> Result<String, HarnessError> {
        let dir = self.layout.homes().join(name);
        private_dir(&dir)?;
        let spelling = dir.to_string_lossy().into_owned();
        self.roots.services(&spelling)?;
        Ok(spelling)
    }

    pub fn paths(&self, spelling: &str) -> CcPaths {
        CcPaths::resolve(&cc_env(
            spelling,
            &self.roots.home,
            self.roots.user.as_deref(),
        ))
    }

    /// The home's CC Keychain item of `kind`: macOS only. Elsewhere CC keeps its credential in
    /// the home's `.credentials.json` and there is no `/usr/bin/security`, so naming an item is
    /// a harness error, never a spawn.
    pub fn item(&self, spelling: &str, kind: ItemKind) -> Result<CcItem, HarnessError> {
        if !self.macos {
            return Err(harness(format!(
                "{spelling}: CC keeps no Keychain item off macOS"
            )));
        }
        CcItem::of(&self.roots, spelling, kind)
    }

    /// Deletes the credential CC keeps for the home `spelling`: its OAuth and managed-key items
    /// (macOS), and its `.credentials.json` (the only store elsewhere), once the guard has
    /// accepted the spelling.
    pub fn forget(&self, spelling: &str) -> Result<(), HarnessError> {
        self.roots.services(spelling)?;
        if self.macos {
            for kind in [ItemKind::OAuth, ItemKind::ManagedKey] {
                self.item(spelling, kind)?.delete()?;
            }
        }
        match fs::remove_file(self.paths(spelling).credentials_file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// The account's profile and its recorded spelling (§12.2), when it has one.
    pub fn profile_of(&self, id: &str) -> Result<Option<(PathBuf, String)>, HarnessError> {
        let dir = self.layout.profile(id);
        match ProfileMarker::read(&dir) {
            Read::Present(m) => {
                self.roots.services(&m.config_dir)?;
                Ok(Some((dir, m.config_dir)))
            }
            Read::Absent => Ok(None),
            Read::Unreadable(e) => Err(harness(format!("the profile marker: {e}"))),
        }
    }

    pub fn profile(&self) -> Result<Option<(PathBuf, String)>, HarnessError> {
        self.profile_of(&self.oauth.id)
    }

    /// The profile, bootstrapped by a launch (`claude --version`) if there is none yet.
    pub fn profile_ready(&self) -> Result<(PathBuf, String), HarnessError> {
        if let Some(p) = self.profile()? {
            return Ok(p);
        }
        let ran = self.run_profile(&["--version"])?;
        if !ran.success() {
            return Err(harness(format!(
                "tagteam run could not bootstrap the profile: {}",
                ran.summary()
            )));
        }
        self.profile()?
            .ok_or_else(|| harness("tagteam run made no profile"))
    }

    /// A profile bootstrapped just now: an existing one is captured by a launch, then dropped
    /// (`drop_profile`), and a launch bootstraps it again.
    pub fn fresh_profile(&self) -> Result<(PathBuf, String), HarnessError> {
        if let Some((dir, spelling)) = self.profile()? {
            let ran = self.run_profile(&["--version"])?;
            if !ran.success() {
                return Err(harness(format!(
                    "the launch before dropping the profile failed: {}",
                    ran.summary()
                )));
            }
            self.drop_profile(&dir, &spelling)?;
        }
        self.profile_ready()
    }

    /// Stops the background daemon that `claude` runs in the home `spelling`, if one runs, and
    /// verifies that it is gone. Fail closed: a `claude daemon stop --any` that exits non-zero
    /// is an error carrying its redacted output, and so is anything of the daemon still
    /// running, or a lock, roster or record that cannot be read, once the deadline passes
    /// (`daemon::survey`). A transient daemon, which `claude --bg` starts, is stopped only with
    /// `--any` (Appendix A.7), and compat has no other way to stop it: a `claude` that
    /// rejects the option is an error (`daemon_stop`). Returns the stop command's summary, or
    /// `None` when nothing needed stopping.
    pub fn stop_daemon(&self, spelling: &str) -> Result<Option<Value>, HarnessError> {
        self.stop_and_verify(spelling, &self.cancel, DAEMON_PATIENCE)
    }

    fn stop_and_verify(
        &self,
        spelling: &str,
        token: &Cancel,
        patience: Duration,
    ) -> Result<Option<Value>, HarnessError> {
        let home = Path::new(spelling);
        let probe = SystemProcessProbe;
        if daemon::survey(home, &probe).is_clear() {
            return Ok(None);
        }
        let stop = self.daemon_stop(spelling, token);
        let mut left = daemon::survey(home, &probe);
        settle(patience, || {
            left = daemon::survey(home, &probe);
            left.is_clear()
        });
        let evidence = stop.as_ref().ok().map(Ran::summary);
        let refused = match &stop {
            Ok(ran) if ran.success() => None,
            Ok(ran) => Some(format!(
                "`claude daemon stop --any` failed: {}{}",
                ran.summary(),
                if daemon::rejects_any(&ran.stderr_text(), &ran.stdout_text()) {
                    "; compat needs a Claude Code whose `daemon stop` supports `--any` (2.1.292 does)"
                } else {
                    ""
                }
            )),
            Err(e) => Some(format!("`claude daemon stop --any`: {e}")),
        };
        // Whatever the stop printed stays in the error, whether it or the verification failed.
        let ran_text = evidence
            .as_ref()
            .map_or_else(String::new, |v| format!(" (the stop: {v})"));
        match (refused, left.is_clear()) {
            (None, true) => Ok(evidence),
            (None, false) => Err(harness(format!(
                "{spelling}: after `claude daemon stop --any`, {}{ran_text}",
                left.describe()
            ))),
            (Some(why), true) => Err(harness(format!("{spelling}: {why}"))),
            (Some(why), false) => Err(harness(format!("{spelling}: {why}; {}", left.describe()))),
        }
    }

    /// `claude daemon stop --any` in the home, through the guard, and nothing else: a `claude`
    /// that rejects `--any` (an unknown option or otherwise) is a failed stop, with no second
    /// command, so the one attempt's exit and output are all the evidence there is.
    fn daemon_stop(&self, spelling: &str, token: &Cancel) -> Result<Ran, HarnessError> {
        self.claude(spelling, &["daemon", "stop", "--any"])
            .cancel(token)
            .timeout(Duration::from_secs(60))
            .run(&self.roots)
    }

    /// Registers the home `spelling` before a check starts a daemon in it (`claude --bg`, or
    /// anything else that detaches from the groups the harness ends): cleanup stops it whatever
    /// ends the run (`stop_daemons`).
    pub fn must_stop(&mut self, spelling: &str) {
        if !self.daemons.iter().any(|d| d == spelling) {
            self.daemons.push(spelling.to_owned());
        }
    }

    /// Cleanup, whether or not a signal ended the run: `claude daemon stop --any` in every home
    /// a check registered (`must_stop`), each verified as `stop_daemon` does, within 30 s. Its
    /// spawn takes a token no signal sets, so it is bounded by its timeout but never
    /// cancelled. A home where a daemon is not stopped, or cannot be verified, is a harness
    /// error naming it.
    pub fn stop_daemons(&self) -> Result<(), HarnessError> {
        self.stop_daemons_within(DAEMON_PATIENCE)
    }

    fn stop_daemons_within(&self, patience: Duration) -> Result<(), HarnessError> {
        let mut left = Vec::new();
        for spelling in &self.daemons {
            if let Err(e) = self.stop_and_verify(spelling, &Cancel::new(), patience) {
                left.push(e.0);
            }
        }
        if left.is_empty() {
            Ok(())
        } else {
            Err(harness(format!(
                "a daemon a check started is not stopped: {}",
                left.join("; ")
            )))
        }
    }

    /// The generation a profile holds that neither the vault nor the profile's seed has: a
    /// rotation that only tagteam's capture may take into the vault (§12.5). `None` when the
    /// profile holds no token, is in step with the vault, or still holds its seed, so that the
    /// vault has moved past it.
    pub fn uncaptured(&self, dir: &Path, spelling: &str) -> Result<Option<String>, HarnessError> {
        let Some((held, _)) = self.read_credential(spelling)? else {
            return Ok(None);
        };
        let id = match ProfileMarker::read(dir) {
            Read::Present(m) => m.account_id.as_str().to_owned(),
            _ => return Err(harness(format!("{} has no readable marker", dir.display()))),
        };
        let held = tagteam_cc::shape::fingerprint(&held);
        let vault = tagteam_cc::shape::fingerprint(&self.vault_credential(&id)?);
        let seeded = match Seed::read(dir) {
            Read::Present(s) => Some(s.seed_fp),
            _ => None,
        };
        let behind = held.is_none()
            || held == vault
            || held.as_ref().map(|f| f.as_str().to_owned()) == seeded;
        Ok(if behind {
            None
        } else {
            held.map(|f| f.short12().to_owned())
        })
    }

    /// Deletes a profile: its credential (`forget`), then its directory (its links as links).
    /// Never while it holds an uncaptured generation, which may be the only copy of the newest
    /// one.
    pub fn drop_profile(&self, dir: &Path, spelling: &str) -> Result<(), HarnessError> {
        self.stop_daemon(spelling)?;
        self.assert_quiescent(spelling)?;
        if self.uncaptured(dir, spelling)?.is_some() {
            return Err(harness(format!(
                "{} holds a generation the vault does not have; it is kept",
                dir.display()
            )));
        }
        self.forget(spelling)?;
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// The OAuth credential the home `spelling` holds, where CC would read it.
    pub fn read_credential(
        &self,
        spelling: &str,
    ) -> Result<Option<(Vec<u8>, Place)>, HarnessError> {
        if self.macos {
            let item = self.item(spelling, ItemKind::OAuth)?;
            match item.read() {
                Read::Present(b) => return Ok(Some((b, Place::Item(item)))),
                Read::Absent => {}
                Read::Unreadable(e) => return Err(harness(format!("{}: {e}", item.service))),
            }
        }
        let file = self.paths(spelling).credentials_file;
        match fs::read(&file) {
            Ok(b) => Ok(Some((b, Place::File(file)))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Refuses a profile that is session-owned (`daemon::owners`: a held launch reservation, a
    /// live session record, a live daemon supervisor, or one that cannot be read): its
    /// credential is only ever changed while nothing runs in it. A home that is no profile
    /// (no marker) is the harness's own and is not judged.
    pub fn assert_quiescent(&self, spelling: &str) -> Result<(), HarnessError> {
        let home = Path::new(spelling);
        if !home.join(MARKER_FILE).exists() {
            return Ok(());
        }
        let owners = daemon::owners(home, &SystemProcessProbe);
        if owners.is_clear() {
            Ok(())
        } else {
            Err(harness(format!(
                "{spelling} is session-owned ({}); its credential is changed only while nothing runs in it",
                owners.describe()
            )))
        }
    }

    /// Writes the credential the home `spelling` holds at `place`, once nothing runs in it
    /// (`assert_quiescent`).
    pub fn write_credential(
        &self,
        spelling: &str,
        place: &Place,
        bytes: &[u8],
    ) -> Result<(), HarnessError> {
        self.assert_quiescent(spelling)?;
        match place {
            Place::Item(item) => item.write(bytes),
            Place::File(path) => Ok(write_atomic_private(path, bytes, 0o600)?),
        }
    }

    /// Makes the home's access token expired, its tokens untouched, so CC's next request
    /// refreshes it. Only while nothing runs in that home.
    pub fn expire(&self, spelling: &str) -> Result<Value, HarnessError> {
        self.assert_quiescent(spelling)?;
        let (bytes, place) = self
            .read_credential(spelling)?
            .ok_or_else(|| harness(format!("{spelling} holds no credential to expire")))?;
        let mut v: Value = serde_json::from_slice(&bytes)
            .map_err(|_| harness(format!("{spelling}'s credential is not JSON")))?;
        let oauth = v
            .get_mut("claudeAiOauth")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| harness(format!("{spelling}'s credential has no claudeAiOauth")))?;
        oauth.insert("expiresAt".into(), json!(now_ms() - EXPIRED_BY_MS));
        self.write_credential(
            spelling,
            &place,
            &serde_json::to_vec(&v).expect("a Value serializes"),
        )?;
        Ok(json!({"place": place.describe(), "generation": generation(&bytes)}))
    }

    /// The account's vault entry: the compat keychain file on macOS, the vault file on Linux.
    pub fn vault_credential(&self, id: &str) -> Result<Vec<u8>, HarnessError> {
        let read = match &self.vault {
            Some(v) => v.cli().find("tagteam", id),
            None => match fs::read(
                self.layout
                    .data_dir()
                    .join("vault")
                    .join(format!("{id}.json")),
            ) {
                Ok(b) => Read::Present(b),
                Err(e) => return Err(harness(format!("the vault file of {id}: {e}"))),
            },
        };
        match read {
            Read::Present(b) => Ok(b),
            Read::Absent => Err(harness(format!("the vault holds nothing for {id}"))),
            Read::Unreadable(e) => Err(harness(format!("the vault entry of {id}: {e}"))),
        }
    }

    /// Where the account's lineage stands: the vault's, the default home's and the profile's
    /// generations.
    pub fn lineage(&self) -> Result<Value, HarnessError> {
        let vault = generation(&self.vault_credential(&self.oauth.id)?);
        let live = self
            .read_credential(&self.live())?
            .and_then(|(b, _)| generation(&b));
        let profile = match self.profile()? {
            Some((_, spelling)) => self
                .read_credential(&spelling)?
                .and_then(|(b, _)| generation(&b)),
            None => None,
        };
        Ok(json!({"vault": vault, "live": live, "profile": profile}))
    }

    /// Marks the home's global config as past onboarding, with `work/` trusted, so `claude`'s
    /// interactive interface opens straight onto its prompt. Under CC's config lock (§9.1).
    pub fn seed_trust(&self, spelling: &str) -> Result<(), HarnessError> {
        let paths = self.paths(spelling);
        let _lock =
            tagteam_cc::locks::acquire_config(&paths, Duration::from_secs(9), &Cancel::new())
                .map_err(|e| harness(format!("CC's config lock: {e}")))?;
        let mut doc = match fs::read(&paths.global_config) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => b"{}\n".to_vec(),
            Err(e) => return Err(e.into()),
        };
        let splice = |doc: &[u8], key: &str, v: &Value| {
            replace_top_level(doc, key, v).map_err(|e| harness(format!("splicing {key}: {e}")))
        };
        let get = |doc: &[u8], key: &str| {
            get_top_level(doc, key).map_err(|e| harness(format!("reading {key}: {e}")))
        };
        doc = splice(&doc, "hasCompletedOnboarding", &json!(true))?;
        if get(&doc, "theme")?.is_none() {
            doc = splice(&doc, "theme", &json!("dark"))?;
        }
        let mut projects = get(&doc, "projects")?.unwrap_or_else(|| json!({}));
        let work = self.layout.work().to_string_lossy().into_owned();
        projects[&work]["hasTrustDialogAccepted"] = json!(true);
        doc = splice(&doc, "projects", &projects)?;
        write_atomic_private(&paths.global_config, &doc, 0o600)?;
        Ok(())
    }

    /// `tagteam list --json`.
    pub fn list(&self) -> Result<Value, HarnessError> {
        let ran = self.tagteam(&["list", "--json"]).run(&self.roots)?;
        ran.json()
            .filter(|_| ran.success())
            .ok_or_else(|| harness(format!("tagteam list failed: {}", ran.summary())))
    }

    /// `tagteam switch <alias> --json`; a harness failure unless it switched or found the
    /// account already active.
    pub fn switch(&self, alias: &str) -> Result<Value, HarnessError> {
        let ran = self
            .tagteam(&["switch", alias, "--json"])
            .run(&self.roots)?;
        let out = ran.json().unwrap_or(Value::Null);
        let done = ran.success() && (out["switched"] == true || out["reason"] == "already-active");
        if done {
            Ok(self.redact.value(
                &json!({"to": alias, "reason": out["reason"], "credentialStore": out["credentialStore"]}),
            ))
        } else {
            Err(harness(format!(
                "tagteam switch {alias}: {}",
                ran.summary()
            )))
        }
    }

    /// Adds a dummy API key under `ALIAS_API_KEY`, through standard input, and returns it.
    pub fn add_dummy_api_key(&self) -> Result<String, HarnessError> {
        let _ = self.tagteam(&["remove", ALIAS_API_KEY]).run(&self.roots);
        let key = dummy_key("k1")?;
        let ran = self
            .tagteam(&["add-token", "-", "--alias", ALIAS_API_KEY])
            .stdin(format!("{key}\n"))
            .run(&self.roots)?;
        if !ran.success() {
            return Err(harness(format!("tagteam add-token: {}", ran.summary())));
        }
        Ok(key)
    }

    /// Back to the account, then the dummy key's account removed.
    pub fn drop_dummy_api_key(&self) -> Result<(), HarnessError> {
        self.switch(ALIAS_OAUTH)?;
        let ran = self.tagteam(&["remove", ALIAS_API_KEY]).run(&self.roots)?;
        if ran.success() {
            Ok(())
        } else {
            Err(harness(format!(
                "tagteam remove {ALIAS_API_KEY}: {}",
                ran.summary()
            )))
        }
    }
}

/// A key shaped like an Anthropic API key, never a real one.
pub fn dummy_key(tag: &str) -> Result<String, HarnessError> {
    Ok(format!(
        "sk-ant-api03-tagteam-compat-{tag}-{}-AA",
        random_hex()?
    ))
}

#[cfg(test)]
impl Ctx {
    /// A `Ctx` off macOS over the scratch directory and compat store given, with no vault and
    /// no `claude`: its credentials are files.
    pub(crate) fn offline(scratch: &Path, state: &Path, tagteam: PathBuf, redact: Redactor) -> Ctx {
        Ctx {
            layout: Layout {
                workspace: PathBuf::from("/w"),
                reports: PathBuf::from("/w/target/compat"),
                state: state.to_path_buf(),
                scratch: scratch.to_path_buf(),
            },
            roots: Roots {
                scratch: scratch.to_path_buf(),
                state: state.to_path_buf(),
                users: vec![],
                home: std::env::temp_dir(),
                user: None,
            },
            tagteam,
            claude: PathBuf::from("/nonexistent/claude"),
            macos: false,
            oauth: Account::default(),
            setup_token: None,
            vault: None,
            base: Vec::new(),
            activation_config: None,
            live_active: false,
            ssh: None,
            redact,
            cancel: Cancel::new(),
            daemons: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compat_rows_are_found_by_alias() {
        let list = json!({"accounts": [
            {"position": 1, "id": "01a", "email": "a@x.co", "alias": "work"},
            {"position": 2, "id": "01b", "email": "t@x.co", "alias": "compat-oauth"},
        ]});
        assert_eq!(
            account(&list, ALIAS_OAUTH),
            Some(Account {
                id: "01b".into(),
                position: 2,
                email: "t@x.co".into()
            })
        );
        assert_eq!(account(&list, ALIAS_SETUP_TOKEN), None);
    }

    #[test]
    fn a_generation_is_its_refresh_token_s() {
        let a = br#"{"claudeAiOauth":{"accessToken":"at-1","refreshToken":"rt-1","expiresAt":1}}"#;
        let b = br#"{"claudeAiOauth":{"accessToken":"at-2","refreshToken":"rt-1","expiresAt":2}}"#;
        assert_eq!(
            generation(a),
            generation(b),
            "an expiry or access token is no new generation"
        );
        assert_ne!(
            generation(a),
            generation(br#"{"claudeAiOauth":{"refreshToken":"rt-2"}}"#)
        );
        assert_eq!(generation(b"{}"), None);
    }

    /// A `Ctx` off macOS over a new scratch directory, with no vault and no `claude`.
    fn offline_ctx(scratch: &Path, tagteam: PathBuf, redact: Redactor) -> Ctx {
        Ctx::offline(scratch, Path::new("/nonexistent/state"), tagteam, redact)
    }

    #[test]
    fn off_macos_a_home_s_credential_is_its_file_and_no_item_is_named() {
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let ctx = offline_ctx(
            &scratch,
            PathBuf::from("/nonexistent/tagteam"),
            Redactor::default(),
        );
        let home = ctx.new_home("one").unwrap();
        let file = Path::new(&home).join(".credentials.json");
        fs::write(&file, b"{}").unwrap();
        assert!(
            ctx.item(&home, ItemKind::OAuth).is_err(),
            "no item is named, so no security runs"
        );
        assert_eq!(
            ctx.read_credential(&home).unwrap().map(|(_, at)| at),
            Some(Place::File(file.clone()))
        );
        ctx.forget(&home).unwrap();
        assert!(!file.exists());
        ctx.forget(&home).unwrap();
        assert!(ctx.forget("/etc").is_err(), "the guard comes first");
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn organizations_named_accounts_and_null_break_neither_setup_nor_the_report() {
        let _serial = crate::compat::sys::serial();
        use crate::compat::report::{Report, Status};
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let list = json!({"schemaVersion": 1, "activeAccountNumber": 1, "accounts": [
            {"position": 1, "id": "01a", "email": "t@x.co", "organizationName": "accounts",
             "active": true, "usageStatus": "ok", "usage": null, "alias": "compat-oauth"},
            {"position": 2, "id": "01b", "email": "t@x.co", "organizationName": "null",
             "active": false, "usageStatus": "ok", "lastGoodUsage": null,
             "alias": "compat-setup-token"},
        ]});
        let fake = scratch.join("tagteam");
        fs::write(&fake, format!("#!/bin/sh\nprintf '%s\\n' '{list}'\n")).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let mut redact = Redactor::default();
        redact.learn("accounts", "<account 1 org>".into());
        redact.learn("null", "<account 2 org>".into());
        let ctx = offline_ctx(&scratch, fake, redact);

        // Setup's control flow reads the raw view.
        let parsed = ctx.list().unwrap();
        assert_eq!(
            account(&parsed, ALIAS_OAUTH).map(|a| a.id),
            Some("01a".into())
        );
        assert_eq!(
            account(&parsed, ALIAS_SETUP_TOKEN).map(|a| a.position),
            Some(2)
        );

        // The redacted view keeps JSON's keywords and shape; only identity values go.
        let ran = ctx.tagteam(&["list", "--json"]).run(&ctx.roots).unwrap();
        let shown: Value = serde_json::from_slice(&ran.stdout).unwrap();
        assert_eq!(shown["schemaVersion"], 1);
        let rows = &shown["<account 1 org>"];
        assert_eq!(rows[0]["organizationName"], "<account 1 org>");
        assert_eq!(rows[1]["organizationName"], "<account 2 org>");
        assert!(rows[0]["usage"].is_null() && rows[1]["lastGoodUsage"].is_null());

        let mut report = Report {
            harness_error: Some(format!("tagteam list failed: {}", ran.summary())),
            ..Report::default()
        };
        report.setup.push(crate::compat::report::Evidence {
            label: "tagteam list".into(),
            value: json!(ran.stdout_text()),
            ok: None,
        });
        report.checks.push(crate::compat::report::CheckResult {
            id: "auth-status",
            title: "a check",
            outcome: crate::compat::report::Outcome::error(ran.stdout_text(), Vec::new()),
            seconds: 0.0,
        });
        assert_eq!(report.checks[0].outcome.status, Status::Error);
        let dir = scratch.join("report");
        report.write(&dir).unwrap();
        for name in ["report.json", "report.md"] {
            let written = fs::read_to_string(dir.join(name)).unwrap();
            assert!(!written.contains("accounts"), "{name}: {written}");
            assert!(
                !written.contains("\"null\"") && !written.contains("\\\"null\\\""),
                "{name}: {written}"
            );
            assert!(written.contains("<account 2 org>"), "{name}: {written}");
        }
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_daemon_a_check_started_is_stopped_even_after_a_signal() {
        let _serial = crate::compat::sys::serial();
        use crate::compat::sys::{process_alive, signal};
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = crate::compat::layout::make_scratch().unwrap();
        // `--bg` starts a `sleep` in a session of its own, outside every group the harness
        // ends, and records it as CC records its supervisor; `daemon stop` ends what the
        // records name.
        let fake = scratch.join("claude");
        fs::write(
            &fake,
            r#"#!/bin/sh
home="$CLAUDE_CONFIG_DIR"
case "$1" in
--bg)
    mkdir -p "$home/sessions"
    perl -MPOSIX -e 'setsid() or die; exec @ARGV' sleep 30 >/dev/null 2>&1 &
    printf '{"pid":%s,"kind":"daemon"}
' "$!" > "$home/sessions/$!.json" ;;
daemon)
    for f in "$home"/sessions/*.json; do
        [ -f "$f" ] || continue
        kill "$(basename "$f" .json)"
        rm -f "$f"
    done
    : > "$home/stopped" ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let mut ctx = offline_ctx(
            &scratch,
            PathBuf::from("/nonexistent/tagteam"),
            Redactor::default(),
        );
        ctx.claude = fake;
        ctx.base.push(("PATH".into(), "/bin:/usr/bin".into()));
        let home = ctx.new_home("profile").unwrap();

        ctx.must_stop(&home);
        let started = ctx.claude(&home, &["--bg"]).run(&ctx.roots).unwrap();
        assert!(started.success(), "{started:?}");
        let pid: u32 = fs::read_dir(Path::new(&home).join("sessions"))
            .unwrap()
            .flatten()
            .find_map(|e| e.path().file_stem()?.to_str()?.parse().ok())
            .unwrap();
        assert!(
            process_alive(pid),
            "the daemon outlived its starter's group"
        );

        ctx.cancel.request(15);
        let refused = ctx.claude(&home, &["daemon", "stop"]).run(&ctx.roots);
        assert_eq!(refused.unwrap_err().0, "interrupted by SIGTERM");
        let stopped = ctx.stop_daemons();
        let ended = !process_alive(pid);
        if !ended {
            signal(pid, "KILL", false);
        }
        assert_eq!(stopped, Ok(()));
        assert!(
            Path::new(&home).join("stopped").exists(),
            "cleanup ran daemon stop"
        );
        assert!(ended, "and the daemon is gone");
        fs::remove_dir_all(&scratch).unwrap();
    }

    /// A `sleep` outside the test's own children (so killing it leaves no zombie), and its pid.
    fn orphan_sleep() -> u32 {
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 60 >/dev/null 2>&1 & echo $!"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    /// A fake `claude` whose `daemon stop` runs `body`, with `args` logging its arguments.
    fn fake_claude(scratch: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let fake = scratch.join("claude");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\nhome=\"$CLAUDE_CONFIG_DIR\"\necho \"$*\" >> \"$home/args\"\n{body}\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        fake
    }

    fn daemon_ctx(scratch: &Path, claude: PathBuf) -> Ctx {
        let mut ctx = offline_ctx(
            scratch,
            PathBuf::from("/nonexistent/tagteam"),
            Redactor::default(),
        );
        ctx.claude = claude;
        ctx.base.push(("PATH".into(), "/bin:/usr/bin".into()));
        ctx
    }

    fn lock_naming(home: &str, pid: u32) {
        fs::write(
            Path::new(home).join("daemon.lock"),
            json!({"pid": pid, "origin": "transient"}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn a_daemon_stop_that_exits_non_zero_is_a_harness_error_carrying_its_output() {
        let _serial = crate::compat::sys::serial();
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let claude = fake_claude(
            &scratch,
            r#"echo "refused: a transient daemon" >&2; echo '{"error":"stdout too"}'; exit 1"#,
        );
        let ctx = daemon_ctx(&scratch, claude);
        let home = ctx.new_home("profile").unwrap();
        let pid = orphan_sleep();
        lock_naming(&home, pid);

        let e = ctx
            .stop_and_verify(&home, &Cancel::new(), Duration::from_secs(1))
            .unwrap_err()
            .0;
        crate::compat::sys::signal(pid, "KILL", false);
        assert!(
            e.contains("`claude daemon stop --any` failed")
                && e.contains("refused: a transient daemon")
                && e.contains("stdout too")
                && e.contains(&format!("the supervisor in daemon.lock (pid {pid})")),
            "{e}"
        );
        let args = fs::read_to_string(Path::new(&home).join("args")).unwrap();
        assert_eq!(
            args.trim(),
            "daemon stop --any",
            "the guard-checked spelling, with --any"
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_stop_that_succeeds_but_leaves_the_supervisor_running_is_an_error_and_a_clean_home_is_left_alone()
     {
        let _serial = crate::compat::sys::serial();
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let ctx = daemon_ctx(&scratch, fake_claude(&scratch, "exit 0"));
        let home = ctx.new_home("profile").unwrap();
        // Nothing to stop: claude is not even started.
        assert_eq!(ctx.stop_daemon(&home), Ok(None));
        assert!(!Path::new(&home).join("args").exists());

        let pid = orphan_sleep();
        lock_naming(&home, pid);
        let e = ctx
            .stop_and_verify(&home, &Cancel::new(), Duration::from_secs(1))
            .unwrap_err()
            .0;
        crate::compat::sys::signal(pid, "KILL", false);
        assert!(
            e.contains("after `claude daemon stop --any`") && e.contains("still running"),
            "{e}"
        );

        // An unreadable lock cannot be verified: the same.
        fs::write(Path::new(&home).join("daemon.lock"), "garbage").unwrap();
        let e = ctx
            .stop_and_verify(&home, &Cancel::new(), Duration::from_secs(1))
            .unwrap_err()
            .0;
        assert!(e.contains("cannot be read: daemon.lock"), "{e}");
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_claude_that_rejects_any_is_an_error_and_no_second_command_runs() {
        let _serial = crate::compat::sys::serial();
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let ctx = daemon_ctx(
            &scratch,
            fake_claude(
                &scratch,
                r#"echo "error: unknown option '--any'" >&2; exit 1"#,
            ),
        );
        let home = ctx.new_home("one").unwrap();
        let pid = orphan_sleep();
        lock_naming(&home, pid);
        let e = ctx
            .stop_and_verify(&home, &Cancel::new(), Duration::from_secs(1))
            .unwrap_err()
            .0;
        crate::compat::sys::signal(pid, "KILL", false);
        assert!(
            e.contains("unknown option '--any'")
                && e.contains("\"exit\":1")
                && e.contains("supports `--any` (2.1.292 does)"),
            "{e}"
        );
        let args = fs::read_to_string(Path::new(&home).join("args")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            ["daemon stop --any"],
            "no plain `daemon stop` follows"
        );

        // Any other failure is the same: the one attempt, its output kept, no hint.
        let ctx = daemon_ctx(
            &scratch,
            fake_claude(
                &scratch,
                r#"echo "error: no daemon is running" >&2; exit 1"#,
            ),
        );
        let other = ctx.new_home("two").unwrap();
        let pid = orphan_sleep();
        lock_naming(&other, pid);
        let e = ctx
            .stop_and_verify(&other, &Cancel::new(), Duration::from_secs(1))
            .unwrap_err()
            .0;
        crate::compat::sys::signal(pid, "KILL", false);
        assert!(
            e.contains("no daemon is running") && !e.contains("supports `--any`"),
            "{e}"
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_stop_that_exits_zero_but_leaves_the_daemon_keeps_its_output_in_the_error() {
        let _serial = crate::compat::sys::serial();
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let ctx = daemon_ctx(
            &scratch,
            fake_claude(&scratch, r#"echo "stopping soon" >&2; exit 0"#),
        );
        let home = ctx.new_home("one").unwrap();
        let pid = orphan_sleep();
        lock_naming(&home, pid);
        let e = ctx
            .stop_and_verify(&home, &Cancel::new(), Duration::from_secs(1))
            .unwrap_err()
            .0;
        crate::compat::sys::signal(pid, "KILL", false);
        assert!(
            e.contains("still running") && e.contains("stopping soon"),
            "{e}"
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_session_owned_profile_s_credential_is_neither_expired_nor_written() {
        let _serial = crate::compat::sys::serial();
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let ctx = offline_ctx(
            &scratch,
            PathBuf::from("/nonexistent/tagteam"),
            Redactor::default(),
        );
        let profile = ctx.new_home("profile").unwrap();
        let file = Path::new(&profile).join(".credentials.json");
        let original =
            br#"{"claudeAiOauth":{"accessToken":"at","refreshToken":"rt","expiresAt":1}}"#;
        fs::write(&file, original).unwrap();

        // No marker: not a profile, so not judged.
        let pid = orphan_sleep();
        lock_naming(&profile, pid);
        ctx.expire(&profile).unwrap();
        fs::write(&file, original).unwrap();

        // A profile with a live supervisor: refused, and the credential is untouched.
        fs::write(Path::new(&profile).join(MARKER_FILE), "{}").unwrap();
        let e = ctx.expire(&profile).unwrap_err().0;
        assert!(
            e.contains("session-owned") && e.contains(&format!("pid {pid}")),
            "{e}"
        );
        let place = Place::File(file.clone());
        let e = ctx.write_credential(&profile, &place, b"{}").unwrap_err().0;
        assert!(e.contains("session-owned"), "{e}");
        assert_eq!(fs::read(&file).unwrap(), original);

        // A live session record of any kind does too.
        crate::compat::sys::signal(pid, "KILL", false);
        crate::compat::sys::wait_until(Duration::from_secs(5), || !process_alive(pid));
        ctx.expire(&profile).unwrap();
        fs::write(&file, original).unwrap();
        fs::remove_file(Path::new(&profile).join("daemon.lock")).unwrap();
        fs::create_dir_all(Path::new(&profile).join("sessions")).unwrap();
        let other = orphan_sleep();
        fs::write(
            Path::new(&profile).join("sessions/1.json"),
            json!({"pid": other, "kind": "interactive"}).to_string(),
        )
        .unwrap();
        let e = ctx.expire(&profile).unwrap_err().0;
        assert!(e.contains("record of kind interactive"), "{e}");

        // Quiet: both go ahead.
        crate::compat::sys::signal(other, "KILL", false);
        crate::compat::sys::wait_until(Duration::from_secs(5), || !process_alive(other));
        ctx.expire(&profile).unwrap();
        ctx.write_credential(&profile, &place, original).unwrap();
        assert_eq!(fs::read(&file).unwrap(), original);
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_profile_with_a_live_session_is_not_dropped() {
        let _serial = crate::compat::sys::serial();
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let ctx = offline_ctx(
            &scratch,
            PathBuf::from("/nonexistent/tagteam"),
            Redactor::default(),
        );
        let profile = ctx.new_home("profile").unwrap();
        let dir = PathBuf::from(&profile);
        fs::write(dir.join(MARKER_FILE), "{}").unwrap();
        fs::write(dir.join(".credentials.json"), "{}").unwrap();
        fs::create_dir_all(dir.join("sessions")).unwrap();
        let pid = orphan_sleep();
        // Not a daemon's kind, so `stop_daemon` has nothing to stop and the quiescence
        // assertion is what refuses.
        fs::write(
            dir.join("sessions/1.json"),
            json!({"pid": pid, "kind": "interactive"}).to_string(),
        )
        .unwrap();
        let e = ctx.drop_profile(&dir, &profile).unwrap_err().0;
        crate::compat::sys::signal(pid, "KILL", false);
        assert!(e.contains("session-owned"), "{e}");
        assert!(
            dir.join(".credentials.json").exists(),
            "nothing was deleted"
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_dummy_key_has_the_shape_claude_accepts() {
        let k = dummy_key("k1").unwrap();
        assert!(k.starts_with("sk-ant-api03-tagteam-compat-k1-"));
        assert!(
            k.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }
}
