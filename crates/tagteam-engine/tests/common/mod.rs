#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::shape::compose;
use tagteam_cc::{CcPaths, ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::oracle::Oracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::store::{JournalRow, LoginMeta};
use tagteam_engine::switch::{SwitchOutcome, SwitchRequest, SwitchTarget};
use tagteam_engine::vault::{FileVault, KeychainVault, SERVICE, Vault, VaultBackend, VaultError};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::splice::{get_top_level, remove_top_level, replace_top_level};
use tagteam_provider::{
    Credential, Env, FakeClock, FakeKeychain, Identity, MutationGuard, ProcessStamp, Provider, Read,
};

/// An oracle that answers whatever the test sets.
#[derive(Default)]
pub struct FixedOracle(pub Mutex<Option<Identity>>);

impl FixedOracle {
    pub fn set(&self, id: Option<Identity>) {
        *self.0.lock().unwrap() = id;
    }
}

impl Oracle for FixedOracle {
    fn resolve(&self, _p: &dyn Provider, _c: &Credential) -> Option<Identity> {
        self.0.lock().unwrap().clone()
    }
}

pub const CLAUDE_JSON: &str = r#"{
  "numStartups": 12,
  "projects": {
    "/work/app": {
      "allowedTools": [],
      "history": ["x"]
    }
  },
  "mcpServers": {
    "local": { "command": "srv" }
  },
  "userID": "user-7",
  "someFutureKey": { "n": 1e400 }
}
"#;

/// An API key an account is added with (`Fx::add_api_key`).
pub const API_KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
/// A managed key that no stored account holds.
pub const STRAY_API_KEY: &str = "sk-ant-api03-stray-key-that-no-vault-holds";

/// Sets one top-level key of the config at `path`, changing nothing else. A free function, so
/// a `'static` race callback can call it without borrowing the fixture.
pub fn splice_config_key(path: &Path, key: &str, value: &Value) {
    let doc = fs::read(path).unwrap();
    fs::write(path, replace_top_level(&doc, key, value).unwrap()).unwrap();
}

/// Replaces `oauthAccount` in the config at `path`, as CC does on a login.
pub fn splice_oauth_account(path: &Path, oauth_account: &Value) {
    splice_config_key(path, "oauthAccount", oauth_account);
}

/// Whether tagteam's mutation lock is free right now; takes and drops it if so.
pub fn mutation_lock_free(env: &Env) -> bool {
    MutationGuard::acquire(env, Duration::ZERO).is_ok()
}

/// A process that has exited: its journal rows are recoverable (§12.6). Shared by `recover.rs`
/// and `invariant.rs`, which both need to plant a crashed switch's journal row.
pub fn dead_holder() -> ProcessStamp {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    ProcessStamp { pid, start: 0 }
}

/// The vault's fingerprint of `id`'s stored generation.
pub fn vault_fp(fx: &Fx, id: &AccountId) -> String {
    fx.cc
        .fingerprint(&fx.vault_bytes(id).unwrap())
        .unwrap()
        .as_str()
        .to_owned()
}

/// The row a switch from `from` to `to` writes at step 6, held by a process that has died.
pub fn crash_row(fx: &Fx, from: &AccountId, to: &AccountId) -> JournalRow {
    let from_row = fx.engine.store().unwrap().account(from).unwrap().unwrap();
    JournalRow {
        provider: fx.provider(),
        holder: dead_holder(),
        from_id: Some(from.clone()),
        to_id: to.clone(),
        from_fp: Some(vault_fp(fx, from)),
        from_identity: Some(from_row.identity_json),
        to_fp: vault_fp(fx, to),
        started_at: 1,
        prior: None,
    }
}

/// What step 7 leaves live: the target credential, composed with the live machine-shared keys.
pub fn write_target_credential(fx: &Fx, to: &AccountId) {
    let live: Value = fx.live_credential().unwrap();
    let composed = compose(&fx.vault_bytes(to).unwrap(), live.as_object()).unwrap();
    fx.set_live_credential(&composed);
}

/// A Keychain vault that runs `on_read` with each key just before reading it: for observing
/// what holds while the engine reads the vault.
pub struct ProbeVault {
    inner: KeychainVault,
    on_read: Box<dyn Fn(&str) + Send + Sync>,
}

impl VaultBackend for ProbeVault {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        (self.on_read)(key);
        self.inner.read(key)
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        self.inner.write(key, bytes)
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.inner.delete(key)
    }
}

pub struct Fx {
    pub dir: tempfile::TempDir,
    pub env: Env,
    pub platform: Platform,
    pub kc: Arc<FakeKeychain>,
    pub oracle: Arc<FixedOracle>,
    pub clock: Arc<FakeClock>,
    pub cc: Arc<ClaudeCode>,
    pub engine: Engine,
}

impl Fx {
    pub fn new() -> Self {
        Self::with_platform(Platform::MacOs)
    }

    pub fn with_platform(platform: Platform) -> Self {
        Self::with(platform, |_| {})
    }

    /// A fixture whose Env is adjusted before anything is created in it.
    pub fn with(platform: Platform, adjust: impl FnOnce(&mut Env)) -> Self {
        Self::build(platform, adjust, |cc| cc)
    }

    /// A macOS fixture whose provider waits only `timeout` for CC's locks, so a held CC lock
    /// can be tested without the real 9 s wait.
    pub fn with_lock_timeout(timeout: Duration) -> Self {
        Self::build(Platform::MacOs, |_| {}, |cc| cc.with_lock_timeout(timeout))
    }

    fn build(
        platform: Platform,
        adjust: impl FnOnce(&mut Env),
        tune: impl FnOnce(ClaudeCode) -> ClaudeCode,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut env = Env::for_test(dir.path());
        adjust(&mut env);
        let claude = env.home.join(".claude");
        fs::create_dir_all(claude.join("projects/-work-app/memory")).unwrap();
        fs::write(
            claude.join("projects/-work-app/memory/MEMORY.md"),
            "remember this\n",
        )
        .unwrap();
        fs::create_dir_all(claude.join("skills/s")).unwrap();
        fs::write(claude.join("skills/s/SKILL.md"), "skill\n").unwrap();
        fs::create_dir_all(claude.join("plugins")).unwrap();
        fs::write(claude.join("plugins/installed.json"), "{}\n").unwrap();
        fs::write(claude.join("CLAUDE.md"), "instructions\n").unwrap();
        fs::write(claude.join("history.jsonl"), "{\"display\":\"hi\"}\n").unwrap();
        fs::write(claude.join("settings.json"), "{\"theme\":\"dark\"}\n").unwrap();
        // Wherever this Env's global config resolves (Appendix A.1), not a hard-coded path.
        fs::write(CcPaths::resolve(&env).global_config, CLAUDE_JSON).unwrap();

        let kc = Arc::new(FakeKeychain::new());
        let oracle = Arc::new(FixedOracle::default());
        let clock = Arc::new(FakeClock::new(1_790_000_000_000));
        let cc = Arc::new(tune(ClaudeCode::with_store(
            LiveStore::new(kc.clone(), platform).with_retry_delay(Duration::ZERO),
        )));
        let vault = match platform {
            Platform::MacOs => Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            Platform::Linux => Vault::new(Box::new(FileVault::new(env.data_dir().join("vault")))),
        };
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(cc.clone()),
            vault,
            oracle: oracle.clone(),
            clock: clock.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
        });
        Fx {
            dir,
            env,
            platform,
            kc,
            oracle,
            clock,
            cc,
            engine,
        }
    }

    pub fn provider(&self) -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    pub fn paths(&self) -> CcPaths {
        CcPaths::resolve(&self.env)
    }

    pub fn credential_json(email: &str, rt: &str) -> Value {
        json!({
            "claudeAiOauth": {
                "accessToken": format!("at-{rt}"),
                "refreshToken": rt,
                "expiresAt": 1_790_003_600_000i64,
                "refreshTokenExpiresAt": 1_797_000_000_000i64,
                "scopes": ["user:inference", "user:profile"],
                "subscriptionType": if email.starts_with("team") { "team" } else { "max" }
            },
            "mcpOAuth": {"srv": {"token": "machine-shared"}}
        })
    }

    pub fn oauth_account(email: &str) -> Value {
        json!({"emailAddress": email, "organizationUuid": "", "organizationName": null, "accountUuid": format!("uuid-{email}")})
    }

    /// What `claude /login` leaves behind: `oauthAccount` plus the live credential.
    pub fn login(&self, email: &str, rt: &str) {
        splice_oauth_account(&self.paths().global_config, &Self::oauth_account(email));
        self.set_live_credential(Self::credential_json(email, rt).to_string().as_bytes());
    }

    pub fn set_live_credential(&self, bytes: &[u8]) {
        match self.platform {
            Platform::MacOs => self.kc.put(
                &keychain_service(&self.env, ItemKind::OAuth),
                &keychain_account(&self.env),
                bytes,
            ),
            Platform::Linux => fs::write(self.paths().credentials_file, bytes).unwrap(),
        }
    }

    /// The (service, account) of the live Keychain item of `kind` that CC writes.
    pub fn live_item(&self, kind: ItemKind) -> (String, String) {
        (
            keychain_service(&self.env, kind),
            keychain_account(&self.env),
        )
    }

    pub fn put_managed_key(&self, key: &[u8]) {
        let (svc, acct) = self.live_item(ItemKind::ManagedKey);
        self.kc.put(&svc, &acct, key);
    }

    pub fn managed_key(&self) -> Option<Vec<u8>> {
        let (svc, acct) = self.live_item(ItemKind::ManagedKey);
        self.kc.get(&svc, &acct)
    }

    /// The contents of every file in `displaced/`.
    pub fn displaced(&self) -> Vec<Vec<u8>> {
        fs::read_dir(self.env.data_dir().join("displaced"))
            .map(|d| d.map(|e| fs::read(e.unwrap().path()).unwrap()).collect())
            .unwrap_or_default()
    }

    /// A manual `switch <id>` from the CLI, optionally forced.
    pub fn switch_request(&self, id: &AccountId, force: bool) -> SwitchRequest {
        SwitchRequest {
            provider: self.provider(),
            target: SwitchTarget::Account(id.clone()),
            force,
            source: "cli",
        }
    }

    pub fn switch_to(&self, id: &AccountId, force: bool) -> Result<SwitchOutcome, EngineError> {
        self.engine.switch(self.switch_request(id, force))
    }

    pub fn live_credential(&self) -> Option<Value> {
        let bytes = match self.platform {
            Platform::MacOs => self.kc.get(
                &keychain_service(&self.env, ItemKind::OAuth),
                &keychain_account(&self.env),
            ),
            Platform::Linux => fs::read(self.paths().credentials_file).ok(),
        }?;
        serde_json::from_slice(&bytes).ok()
    }

    /// CC rotating the live refresh token in place.
    pub fn rotate_live(&self, new_rt: &str) {
        let mut v = self.live_credential().unwrap();
        v["claudeAiOauth"]["refreshToken"] = json!(new_rt);
        self.set_live_credential(v.to_string().as_bytes());
    }

    pub fn live_refresh_token(&self) -> Option<String> {
        self.live_credential()?["claudeAiOauth"]["refreshToken"]
            .as_str()
            .map(str::to_owned)
    }

    pub fn live_email(&self) -> Option<String> {
        match self.cc.live_identity(&self.env) {
            Read::Present(i) => i.email,
            _ => None,
        }
    }

    pub fn vault_bytes(&self, id: &AccountId) -> Option<Vec<u8>> {
        self.engine_vault_read(id)
    }

    fn engine_vault_read(&self, id: &AccountId) -> Option<Vec<u8>> {
        match self.platform {
            Platform::MacOs => self.kc.get(SERVICE, id.as_str()),
            Platform::Linux => {
                fs::read(self.env.data_dir().join("vault").join(format!("{id}.json"))).ok()
            }
        }
    }

    pub fn vault_refresh_token(&self, id: &AccountId) -> Option<String> {
        let v: Value = serde_json::from_slice(&self.vault_bytes(id)?).ok()?;
        v["claudeAiOauth"]["refreshToken"]
            .as_str()
            .map(str::to_owned)
    }

    /// The `AddOptions` every plain `add_live` call in these tests starts from — shared by
    /// `Fx::add` and by `add.rs`'s own `add_opts` (Task 18's review, item 8).
    pub fn add_options(&self) -> AddOptions {
        AddOptions {
            provider: self.provider(),
            position: None,
            alias: None,
            yes: false,
        }
    }

    /// The `AddTokenOptions` every plain `add_token` call in these tests starts from.
    pub fn add_token_options(&self, token: &str) -> AddTokenOptions {
        AddTokenOptions {
            provider: self.provider(),
            token: token.into(),
            position: None,
            email: None,
            alias: None,
            yes: false,
        }
    }

    /// Stores an API-key account (§10.2) without touching the live login.
    pub fn add_api_key(&self, key: &str) -> AccountId {
        self.engine
            .add_token(self.add_token_options(key))
            .unwrap()
            .account
            .id
    }

    /// Logs a fresh account in and captures it (§10.1). Tasks 19-21 each need only the
    /// resulting id, so this is the one place that repeats `fx.login` + `add_live`.
    pub fn add(&self, email: &str, rt: &str) -> AccountId {
        self.login(email, rt);
        self.engine.add_live(self.add_options()).unwrap().account.id
    }

    /// A pending replacement whose vault write landed but whose metadata never did — the
    /// crash-recovery scenario `finish_replacement`/`rollback_replacement` exist for.
    /// `identity_json` is the raw `oauthAccount`-shaped object the replacement claims to be
    /// (Task 18's review, item 8: shared by every test that primes this scenario, instead of
    /// each one repeating the same four calls).
    pub fn begin_replacement(
        &self,
        id: &AccountId,
        new_bytes: &[u8],
        identity_json: &Value,
        kind: &str,
    ) {
        let identity = self.cc.parse_identity(identity_json).unwrap();
        let identity_key = format!(
            "{}\n{}",
            identity.email.as_deref().unwrap_or(&identity.label),
            identity.org_uuid
        );
        self.kc.put(SERVICE, id.as_str(), new_bytes);
        let meta = LoginMeta {
            identity_key: &identity_key,
            identity: &identity,
            kind,
            login_expires_at: None,
        };
        self.engine
            .store()
            .unwrap()
            .begin_replacement(id, self.cc.fingerprint(new_bytes).unwrap().as_str(), &meta)
            .unwrap();
    }

    /// Directly quarantines an account row. There is no public writer for this yet (a later
    /// task adds one); a second connection to the same on-disk store, mirroring
    /// `tests/store.rs`, is the only way a fixture can prime this state today.
    pub fn quarantine(&self, id: &AccountId, reason: &str, fp: &str) {
        self.engine.store().unwrap(); // ensures the db file exists and is migrated
        let path = self.env.data_dir().join("tagteam.db");
        rusqlite::Connection::open(path)
            .unwrap()
            .execute(
                "UPDATE accounts SET quarantine_reason = ?2, quarantine_fp = ?3, quarantine_at = 1 WHERE id = ?1",
                rusqlite::params![id.as_str(), reason, fp],
            )
            .unwrap();
    }

    /// A second engine over this fixture's provider and clock, as another tagteam process.
    fn engine_over(&self, env: Env, vault: Vault, oracle: Arc<dyn Oracle>) -> Engine {
        Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new().with(self.cc.clone()),
            vault,
            oracle,
            clock: self.clock.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
        })
    }

    fn keychain_vault(&self) -> Vault {
        Vault::new(Box::new(KeychainVault::new(self.kc.clone())))
    }

    /// An engine over the same Keychain, oracle and clock, but a different Env.
    pub fn engine_with_env(&self, env: Env) -> Engine {
        self.engine_over(env, self.keychain_vault(), self.oracle.clone())
    }

    /// An engine over the same Keychain, clock and Env, but a caller-supplied oracle — for
    /// exercising the oracle-call race between the pre-lock read and the locks (Task 18's
    /// review, item 2).
    pub fn engine_with_oracle(&self, oracle: Arc<dyn Oracle>) -> Engine {
        self.engine_over(self.env.clone(), self.keychain_vault(), oracle)
    }

    /// An engine over the same Env, Keychain, oracle and clock whose vault runs `on_read`
    /// before every read (see `ProbeVault`).
    pub fn engine_with_vault_probe(
        &self,
        on_read: impl Fn(&str) + Send + Sync + 'static,
    ) -> Engine {
        let vault = Vault::new(Box::new(ProbeVault {
            inner: KeychainVault::new(self.kc.clone()),
            on_read: Box::new(on_read),
        }));
        self.engine_over(self.env.clone(), vault, self.oracle.clone())
    }
}

/// A path's kind, for the snapshot comparison. A symlink records its `read_link` target
/// rather than following it: the walk never reads or descends through a link.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryKind {
    File(Vec<u8>),
    Dir,
    Symlink(PathBuf),
}

/// One path's kind, permission bits, and content (for a file) — everything the invariant
/// compares. Two snapshots' entries at the same path are equal only if all three match, so a
/// mode change, a kind swap (a symlink replaced by a regular file, say), or a new directory
/// are all differences, not just a changed file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    mode: u32,
    kind: EntryKind,
}

/// Every file, directory and symlink under HOME except tagteam's own data dir, plus every
/// Keychain item except the vault's.
pub struct HomeSnapshot {
    files: BTreeMap<PathBuf, Entry>,
    items: BTreeMap<(String, String), Vec<u8>>,
}

/// Records every entry under `dir` except under `skip`, without ever following a symlink into
/// its target. Any I/O failure here is a bug in the fixture or the walk, not a state to
/// tolerate silently: it panics rather than treating a path as absent or empty.
///
/// A bare ancestor directory of `skip` (`~/.local`, say, above `~/.local/share/tagteam`) is
/// walked through — so a sibling of the excluded subtree is still found — but never recorded
/// itself: it is created lazily as a side effect of creating the excluded subtree, not state
/// the invariant should have an opinion on.
fn walk(dir: &Path, skip: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.starts_with(skip) {
            continue;
        }
        let meta = fs::symlink_metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o7777;
        let is_plain_dir = meta.is_dir() && !meta.file_type().is_symlink();
        if is_plain_dir {
            walk(&path, skip, out);
            if skip.starts_with(&path) {
                continue;
            }
        }
        let kind = if meta.file_type().is_symlink() {
            EntryKind::Symlink(fs::read_link(&path).unwrap())
        } else if is_plain_dir {
            EntryKind::Dir
        } else {
            EntryKind::File(fs::read(&path).unwrap())
        };
        out.insert(path, Entry { mode, kind });
    }
}

/// §3: `customApiKeyResponses.approved` may only grow by appending; nothing else in that
/// object may change.
fn check_api_key_responses(
    before: Option<&Vec<u8>>,
    after: Option<&Vec<u8>>,
    step: &str,
    path: &Path,
) {
    let get = |d: Option<&Vec<u8>>| {
        d.and_then(|d| get_top_level(d, "customApiKeyResponses").unwrap())
            .unwrap_or_else(|| json!({}))
    };
    let (mut b, mut a) = (get(before), get(after));
    let approved = |v: &mut Value| {
        v.as_object_mut()
            .and_then(|o| o.remove("approved"))
            .and_then(|x| x.as_array().cloned())
            .unwrap_or_default()
    };
    let (b_list, a_list) = (approved(&mut b), approved(&mut a));
    assert!(
        a_list.starts_with(&b_list),
        "{step}: customApiKeyResponses.approved lost or reordered entries in {}",
        path.display()
    );
    assert_eq!(
        b,
        a,
        "{step}: customApiKeyResponses changed beyond appending to approved in {}",
        path.display()
    );
}

fn shared_keys(bytes: Option<&Vec<u8>>, keys: &[&str]) -> Map<String, Value> {
    let v: Value = bytes
        .and_then(|b| serde_json::from_slice(b).ok())
        .unwrap_or(Value::Null);
    v.as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The bytes of `path`'s entry, for a path declared in the identity surface. `None` when it
/// does not exist yet (a credential file created on first use). Panics if it exists as
/// something other than a plain file: a declared surface path swapping kind is itself a
/// violation this comparison must not silently wave through.
fn surface_file_bytes<'e>(
    entry: Option<&'e Entry>,
    step: &str,
    path: &Path,
) -> Option<&'e Vec<u8>> {
    match entry {
        None => None,
        Some(Entry {
            kind: EntryKind::File(bytes),
            ..
        }) => Some(bytes),
        Some(_) => panic!(
            "{step}: {} is declared in the identity surface but is no longer a plain file",
            path.display()
        ),
    }
}

/// The real path a declared surface path's writes actually land at: the pre-mutation
/// snapshot's `read_link` target when the surface path is itself a symlink (§9.5's
/// write-through), or the path itself otherwise. Resolving from `before` — never `after` —
/// means the literal surface path is then left to the default byte-for-byte rule below, so a
/// link that gets replaced, or repointed, is still caught: it is no longer a surface path once
/// resolved away from, so any change to it at all is a violation.
fn resolve(snapshot: &HomeSnapshot, path: &Path) -> PathBuf {
    match snapshot.files.get(path) {
        Some(Entry {
            kind: EntryKind::Symlink(target),
            ..
        }) => target.clone(),
        _ => path.to_path_buf(),
    }
}

impl Fx {
    pub fn snapshot(&self) -> HomeSnapshot {
        let mut files = BTreeMap::new();
        walk(&self.env.home, &self.env.data_dir(), &mut files);
        let items = self
            .kc
            .items()
            .into_iter()
            .filter(|((svc, _), _)| svc != SERVICE)
            .collect();
        HomeSnapshot { files, items }
    }

    /// §15.3: every byte, mode, and kind outside the identity surface is identical; inside it,
    /// only the declared keys moved, and the machine-shared credential keys kept their values.
    /// A declared path's rules apply to its resolved target, not its literal name, so a
    /// symlinked surface file is checked correctly while the link itself is held to the same
    /// byte-for-byte rule as everything else.
    pub fn assert_only_surface_changed(
        &self,
        before: &HomeSnapshot,
        after: &HomeSnapshot,
        step: &str,
    ) {
        let surface = self.cc.identity_surface(&self.env);
        let json_keys: BTreeMap<PathBuf, Vec<String>> = surface
            .json_keys
            .iter()
            .map(|(p, keys)| (resolve(before, p), keys.clone()))
            .collect();
        let cred_files: BTreeSet<PathBuf> = surface
            .credential_files
            .iter()
            .map(|p| resolve(before, p))
            .collect();
        let paths: BTreeSet<&PathBuf> = before.files.keys().chain(after.files.keys()).collect();
        for path in paths {
            let (b, a) = (before.files.get(path), after.files.get(path));
            if let Some(keys) = json_keys.get(path) {
                let (bb, ab) = (
                    surface_file_bytes(b, step, path),
                    surface_file_bytes(a, step, path),
                );
                if let (Some(bm), Some(am)) = (b.map(|e| e.mode), a.map(|e| e.mode)) {
                    assert_eq!(bm, am, "{step}: {} changed mode", path.display());
                }
                if keys.iter().any(|k| k == "customApiKeyResponses") {
                    check_api_key_responses(bb, ab, step, path);
                }
                let strip = |doc: Option<&Vec<u8>>| {
                    doc.map(|d| {
                        keys.iter()
                            .fold(d.clone(), |acc, k| remove_top_level(&acc, k).unwrap())
                    })
                };
                assert_eq!(
                    strip(bb),
                    strip(ab),
                    "{step}: {} changed outside {keys:?}",
                    path.display()
                );
            } else if cred_files.contains(path) {
                let (bb, ab) = (
                    surface_file_bytes(b, step, path),
                    surface_file_bytes(a, step, path),
                );
                if let (Some(bm), Some(am)) = (b.map(|e| e.mode), a.map(|e| e.mode)) {
                    assert_eq!(bm, am, "{step}: {} changed mode", path.display());
                }
                assert_eq!(
                    shared_keys(bb, &surface.machine_shared_keys),
                    shared_keys(ab, &surface.machine_shared_keys),
                    "{step}: machine-shared keys changed in {}",
                    path.display()
                );
            } else {
                assert_eq!(b, a, "{step}: {} changed", path.display());
            }
        }
        let owned: BTreeSet<(String, String)> = surface.owned_items.iter().cloned().collect();
        let creds: BTreeSet<(String, String)> = surface.credential_items.iter().cloned().collect();
        let keys: BTreeSet<&(String, String)> =
            before.items.keys().chain(after.items.keys()).collect();
        for key in keys {
            let (b, a) = (before.items.get(key), after.items.get(key));
            if owned.contains(key) {
                continue;
            }
            if creds.contains(key) {
                assert_eq!(
                    shared_keys(b, &surface.machine_shared_keys),
                    shared_keys(a, &surface.machine_shared_keys),
                    "{step}: machine-shared keys changed in Keychain item {key:?}"
                );
            } else {
                assert_eq!(b, a, "{step}: Keychain item {key:?} changed");
            }
        }
    }
}
