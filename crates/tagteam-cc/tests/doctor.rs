//! §13.6's Claude Code checks (`Provider::doctor_checks`): every spawn scripted, every
//! Keychain a `FakeKeychain`, so no test runs a binary or touches the login keychain (§15.1).

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_provider::{
    Cancel, Captured, Check, CheckStatus, Env, FakeKeychain, Keychain, KeychainError, LockState,
    Provider, Read, ScriptedSpawner,
};

struct Fx {
    d: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
    cc: ClaudeCode,
    spawner: ScriptedSpawner,
}

fn fx_on(platform: Platform) -> Fx {
    let d = tempfile::tempdir().unwrap();
    let mut env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    // `PATH` as the CLI captures it (M4b Decision 15): a directory with no `claude` in it.
    let bin = d.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    env.vars.insert("PATH".into(), bin.into_os_string());
    let kc = Arc::new(FakeKeychain::new());
    let cc = ClaudeCode::with_store(LiveStore::new(kc.clone(), platform));
    Fx {
        d,
        env,
        kc,
        cc,
        spawner: ScriptedSpawner::new(),
    }
}

fn fx() -> Fx {
    fx_on(Platform::MacOs)
}

impl Fx {
    /// An executable `claude` on the fixture's `PATH`. The spawner is scripted, so it never
    /// runs; `find_on_path` only needs it to be an executable file.
    fn install_claude(&self) -> PathBuf {
        let path = self.d.path().join("bin/claude");
        fs::write(&path, "#!/bin/sh\nexit 99\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn checks(&self) -> Vec<Check> {
        self.cc
            .doctor_checks(&self.env, &self.spawner, &Cancel::new())
    }

    /// A home Claude Code has been started in: its global config exists (§13.6 runs
    /// `claude auth status` only there).
    fn started(&self) {
        let global = CcPaths::resolve(&self.env).global_config;
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        fs::write(global, "{}").unwrap();
    }

    /// What `claude /login` leaves in `~/.claude.json`.
    fn login(&self, email: &str) {
        fs::write(
            CcPaths::resolve(&self.env).global_config,
            json!({"oauthAccount": {"emailAddress": email, "organizationUuid": "org-1", "accountUuid": "u"}})
                .to_string(),
        )
        .unwrap();
    }
}

fn exited(code: i32, stdout: &str) -> Captured {
    Captured::Exited {
        code: Some(code),
        signal: None,
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

fn status(v: serde_json::Value) -> Captured {
    exited(0, &v.to_string())
}

fn logged_in(email: &str, org: &str) -> Captured {
    status(
        json!({"loggedIn": true, "authMethod": "claude.ai", "configDirectory": "/h/.claude",
        "email": email, "orgId": org, "apiProvider": "firstParty"}),
    )
}

fn one<'c>(checks: &'c [Check], id: &str) -> &'c Check {
    let all: Vec<&Check> = checks.iter().filter(|c| c.id == id).collect();
    assert_eq!(all.len(), 1, "{id}: {checks:#?}");
    all[0]
}

fn none(checks: &[Check], id: &str) {
    assert!(checks.iter().all(|c| c.id != id), "{id}: {checks:#?}");
}

#[test]
fn without_claude_on_path_the_binary_warns_and_nothing_is_spawned() {
    let f = fx();
    let checks = f.checks();
    let c = one(&checks, "cc.binary");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.fix.as_deref().unwrap().contains("PATH"));
    none(&checks, "cc.version");
    none(&checks, "cc.auth");
    assert!(f.spawner.specs().is_empty());
}

#[test]
fn a_newer_claude_warns_and_the_tested_one_or_an_older_one_passes() {
    for (printed, want) in [
        ("99.0.0 (Claude Code)\n", CheckStatus::Warn),
        ("2.1.286 (Claude Code)\n", CheckStatus::Ok),
        ("2.0.1 (Claude Code)\n", CheckStatus::Ok),
        ("Claude Code, some version\n", CheckStatus::Warn),
    ] {
        let f = fx();
        let bin = f.install_claude();
        f.spawner.push(exited(0, printed));
        f.spawner.push(logged_in("a@x.co", "org-1"));
        let checks = f.checks();
        assert_eq!(one(&checks, "cc.version").status, want, "{printed}");
        let spec = &f.spawner.specs()[0];
        assert_eq!(spec.program, bin);
        assert_eq!(spec.args, vec![OsString::from("--version")]);
    }
}

#[test]
fn a_version_check_that_times_out_or_cannot_spawn_warns_naming_the_cause() {
    for (reply, words) in [
        (Captured::TimedOut, "did not answer within 10 s"),
        (
            Captured::SpawnFailed("ENOEXEC".into()),
            "could not be run: ENOEXEC",
        ),
    ] {
        let f = fx();
        f.install_claude();
        f.spawner.push(reply);
        f.spawner.push(logged_in("a@x.co", "org-1"));
        let checks = f.checks();
        let c = one(&checks, "cc.version");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.message.contains(words), "{}", c.message);
    }
}

#[test]
fn auth_status_runs_in_the_outer_home_s_environment() {
    let mut f = fx();
    f.install_claude();
    let outer = f.env.home.join("elsewhere");
    f.env.claude_config_dir = Some(outer.clone().into_os_string());
    f.started();
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    f.spawner.push(logged_in("a@x.co", "org-1"));
    f.checks();
    let spec = &f.spawner.specs()[1];
    assert_eq!(
        spec.args,
        ["auth", "status", "--json"].map(OsString::from).to_vec()
    );
    assert_eq!(
        spec.set,
        vec![(OsString::from("CLAUDE_CONFIG_DIR"), outer.into_os_string())]
    );
    assert_eq!(
        spec.remove,
        vec![OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR")],
        "a run shell's own variables never reach the check (§12.8)"
    );
}

#[test]
fn a_home_with_no_global_config_is_not_asked_and_is_reported_as_never_started() {
    // §13.6, Appendix A.7: `claude auth status` creates the global config where it is missing,
    // so doctor does not run it there.
    let f = fx();
    f.install_claude();
    let global = CcPaths::resolve(&f.env).global_config;
    assert!(!global.exists());
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    let checks = f.checks();
    let c = one(&checks, "cc.auth");
    assert_eq!(c.status, CheckStatus::Info);
    assert!(
        c.message.contains("has not been started in this home"),
        "{}",
        c.message
    );
    let specs = f.spawner.specs();
    assert_eq!(specs.len(), 1, "only `claude --version`: {specs:?}");
    assert_eq!(specs[0].args, vec![OsString::from("--version")]);
    assert!(!global.exists(), "doctor creates nothing");
}

#[test]
fn a_global_config_that_cannot_be_told_to_exist_warns_and_nothing_is_spawned() {
    // A dangling link, and a home whose directory cannot be searched: unreadable input, a
    // warning, and still no `claude auth status`.
    let f = fx();
    f.install_claude();
    let global = CcPaths::resolve(&f.env).global_config;
    std::os::unix::fs::symlink(f.d.path().join("nowhere"), &global).unwrap();
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    let checks = f.checks();
    let c = one(&checks, "cc.auth");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("cannot be told"), "{}", c.message);
    assert_eq!(f.spawner.specs().len(), 1, "only `claude --version`");

    let f = fx();
    f.install_claude();
    f.started();
    let before = fs::metadata(&f.env.home).unwrap().permissions();
    fs::set_permissions(&f.env.home, fs::Permissions::from_mode(0o000)).unwrap();
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    let checks = f.checks();
    fs::set_permissions(&f.env.home, before).unwrap();
    let c = one(&checks, "cc.auth");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(f.spawner.specs().len(), 1, "only `claude --version`");
}

#[test]
fn a_login_by_another_method_warns_that_a_switch_changes_nothing() {
    let f = fx();
    f.install_claude();
    f.login("a@x.co");
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    f.spawner.push(status(
        json!({"loggedIn": true, "authMethod": "api_key_helper",
        "configDirectory": "/h/.claude", "apiKeySource": "apiKeyHelper"}),
    ));
    let checks = f.checks();
    let c = one(&checks, "cc.auth");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("api_key_helper from apiKeyHelper")
            && c.message.contains("a switch changes nothing"),
        "{}",
        c.message
    );
}

#[test]
fn a_login_as_another_account_or_organization_than_the_live_one_warns() {
    for (email, org, other) in [
        ("b@x.co", "org-1", "account"),
        ("a@x.co", "org-2", "organization"),
    ] {
        let f = fx();
        f.install_claude();
        f.login("a@x.co");
        f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
        f.spawner.push(logged_in(email, org));
        let checks = f.checks();
        let c = one(&checks, "cc.auth");
        assert_eq!(c.status, CheckStatus::Warn, "{email} {org}");
        assert!(
            c.message.contains(&format!("another {other}")),
            "{}",
            c.message
        );
        assert!(
            !c.message.contains("b@x.co"),
            "doctor names no email (§4.4)"
        );
    }
    let f = fx();
    f.install_claude();
    f.login("a@x.co");
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    f.spawner.push(logged_in("a@x.co", "org-1"));
    assert_eq!(one(&f.checks(), "cc.auth").status, CheckStatus::Ok);
}

#[test]
fn an_auth_status_that_times_out_or_says_logged_out_is_reported_as_such() {
    let f = fx();
    f.install_claude();
    f.started();
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    f.spawner.push(Captured::TimedOut);
    let c = one(&f.checks(), "cc.auth").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("within 10 s"), "{}", c.message);

    let f = fx();
    f.install_claude();
    f.started();
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    f.spawner.push(exited(
        1,
        &json!({"loggedIn": false, "authMethod": "none", "configDirectory": "/h/.claude"})
            .to_string(),
    ));
    let c = one(&f.checks(), "cc.auth").clone();
    assert_eq!(c.status, CheckStatus::Info);
}

#[test]
fn a_locked_keychain_is_reported_and_its_secrets_are_not_read() {
    let f = fx();
    let acct = keychain_account(&f.env);
    f.kc.put(&keychain_service(&f.env, ItemKind::ManagedKey), &acct, b"");
    f.kc.set_locked(true);
    let checks = f.checks();
    let c = one(&checks, "cc.keychain");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.fix
            .as_deref()
            .unwrap()
            .contains("security unlock-keychain")
    );
    none(&checks, "cc.managed-key");
    assert_eq!(f.kc.unlock_attempts(), 0, "doctor never unlocks (§13.6)");
}

#[test]
fn an_empty_managed_key_item_fails_naming_the_command_that_deletes_it() {
    let f = fx();
    let acct = keychain_account(&f.env);
    let svc = keychain_service(&f.env, ItemKind::ManagedKey);
    f.kc.put(&svc, &acct, b"");
    let checks = f.checks();
    let c = one(&checks, "cc.managed-key");
    assert_eq!(c.status, CheckStatus::Fail);
    assert_eq!(
        c.fix.as_deref(),
        Some(format!("security delete-generic-password -s '{svc}' -a '{acct}'").as_str())
    );
    assert_eq!(one(&checks, "cc.keychain").status, CheckStatus::Ok);
}

#[test]
fn the_account_name_fallback_is_information() {
    let mut f = fx();
    f.env.user = Some("not a plain name".into());
    let c = one(&f.checks(), "cc.keychain-account").clone();
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.contains("claude-code-user"));
}

#[test]
fn items_under_a_former_fallback_name_are_information_found_by_attributes_alone() {
    let mut f = fx();
    let default_home = f.env.home.join(".claude");
    f.env.claude_config_dir = Some(default_home.into_os_string());
    let acct = keychain_account(&f.env);
    f.kc.put("Claude Code-credentials", &acct, b"{}");
    f.kc.set_locked(true);
    let checks = f.checks();
    let c = one(&checks, "cc.former-item");
    assert_eq!(c.status, CheckStatus::Info);
    assert_eq!(
        c.fix.as_deref(),
        Some(
            format!("security delete-generic-password -s 'Claude Code-credentials' -a '{acct}'")
                .as_str()
        )
    );
    let warned = one(&checks, "cc.env.config-dir");
    assert_eq!(warned.status, CheckStatus::Warn);
    assert_eq!(warned.fix.as_deref(), Some("unset CLAUDE_CONFIG_DIR"));
}

#[test]
fn a_symlinked_config_dir_s_target_item_is_information() {
    let mut f = fx();
    let target = f.d.path().join("real-claude");
    fs::create_dir_all(&target).unwrap();
    let link = f.d.path().join("link-claude");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    f.env.claude_config_dir = Some(link.into_os_string());
    let mut by_target = f.env.clone();
    by_target.claude_config_dir = Some(fs::canonicalize(&target).unwrap().into_os_string());
    let acct = keychain_account(&f.env);
    let former = keychain_service(&by_target, ItemKind::OAuth);
    f.kc.put(&former, &acct, b"{}");
    let checks = f.checks();
    assert!(one(&checks, "cc.former-item").message.contains(&former));
    none(&checks, "cc.env.config-dir");
}

#[test]
fn variables_that_redirect_the_login_warn_each_by_name() {
    let mut f = fx();
    f.env.claude_config_dir = Some(OsString::new());
    for name in [
        "ANTHROPIC_API_KEY",
        "USE_STAGING_OAUTH",
        "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
    ] {
        // M4b Decision 18: the boundary records each scrubbed variable's presence alone.
        f.env.vars.insert(name.into(), OsString::new());
    }
    f.env.vars.insert("CLAUDECODE".into(), "1".into());
    let checks = f.checks();
    assert_eq!(one(&checks, "cc.env.config-dir").status, CheckStatus::Warn);
    assert_eq!(
        one(&checks, "cc.env.oauth").fix.as_deref(),
        Some("unset USE_STAGING_OAUTH")
    );
    let scrubbed: Vec<&str> = checks
        .iter()
        .filter(|c| c.id == "cc.env.scrubbed")
        .map(|c| c.fix.as_deref().unwrap())
        .collect();
    assert_eq!(
        scrubbed,
        [
            "unset ANTHROPIC_API_KEY",
            "unset CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR"
        ]
    );
    none(&checks, "cc.env");
}

#[test]
fn a_clean_environment_passes() {
    let f = fx();
    assert_eq!(one(&f.checks(), "cc.env").status, CheckStatus::Ok);
}

#[test]
fn a_lock_directory_past_its_staleness_warns_and_a_fresh_one_does_not() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::create_dir(&paths.refresh_lock).unwrap();
    fs::create_dir(&paths.config_lock).unwrap();
    fs::File::open(&paths.refresh_lock)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();
    let checks = f.checks();
    let c = one(&checks, "cc.locks");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains(".oauth_refresh.lock"), "{}", c.message);
    assert!(paths.refresh_lock.exists(), "doctor removes nothing");
}

#[test]
fn a_stale_storage_write_lock_of_either_spelling_warns() {
    for pick in [
        (|p: &CcPaths| p.storage_write_lock.clone()) as fn(&CcPaths) -> std::path::PathBuf,
        |p: &CcPaths| p.storage_write_lock_v2.clone(),
    ] {
        let f = fx();
        let dir = pick(&CcPaths::resolve(&f.env));
        fs::create_dir(&dir).unwrap();
        fs::File::open(&dir)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(120))
            .unwrap();
        let checks = f.checks();
        let c = one(&checks, "cc.locks");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(
            c.message.contains(&dir.display().to_string()),
            "{}",
            c.message
        );
        assert!(dir.exists(), "doctor removes nothing");
    }
}

#[test]
fn linux_has_no_keychain_checks() {
    let f = fx_on(Platform::Linux);
    let checks = f.checks();
    for id in ["cc.keychain", "cc.keychain-account", "cc.managed-key"] {
        none(&checks, id);
    }
    assert!(!one(&checks, "cc.paths").message.contains("Keychain"));
}

#[test]
fn a_lock_directory_that_cannot_be_read_warns_and_is_never_ok() {
    let f = fx();
    let home = CcPaths::resolve(&f.env).config_home;
    let before = fs::metadata(&home).unwrap().permissions();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o000)).unwrap();
    let checks = f.checks();
    fs::set_permissions(&home, before).unwrap();
    let locks: Vec<&Check> = checks.iter().filter(|c| c.id == "cc.locks").collect();
    assert!(
        !locks.is_empty() && locks.iter().all(|c| c.status == CheckStatus::Warn),
        "{locks:#?}"
    );
    assert!(
        locks
            .iter()
            .any(|c| c.message.contains(".oauth_refresh.lock")
                && c.message.contains("cannot be read (permission denied)")),
        "{locks:#?}"
    );
}

#[test]
fn a_former_item_the_probe_cannot_read_warns() {
    let mut f = fx();
    let default_home = f.env.home.join(".claude");
    f.env.claude_config_dir = Some(default_home.into_os_string());
    let acct = keychain_account(&f.env);
    f.kc.put("Claude Code-credentials", &acct, b"{}");
    f.kc.set_unreadable("Claude Code-credentials", &acct, true);
    let c = one(&f.checks(), "cc.former-item").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("cannot be told"), "{}", c.message);
}

#[test]
fn a_live_login_that_cannot_be_read_leaves_the_auth_check_untold_with_a_warning() {
    let f = fx();
    f.install_claude();
    fs::write(
        CcPaths::resolve(&f.env).global_config,
        "{\"oauthAccount\": ",
    )
    .unwrap();
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    f.spawner.push(logged_in("a@x.co", "org-1"));
    let c = one(&f.checks(), "cc.auth").clone();
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("cannot be told"), "{}", c.message);
}

#[test]
fn auth_status_never_runs_while_the_keychain_is_locked() {
    // §13.6: `claude auth status` reads the stored login, which a locked keychain would ask
    // SecurityAgent to unlock. `cc.keychain`'s one warning covers the skip.
    let f = fx();
    f.install_claude();
    f.kc.set_locked(true);
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    let checks = f.checks();
    let specs = f.spawner.specs();
    assert_eq!(specs.len(), 1, "only `claude --version`: {specs:?}");
    assert_eq!(specs[0].args, vec![OsString::from("--version")]);
    none(&checks, "cc.auth");
    assert!(
        one(&checks, "cc.keychain")
            .message
            .contains("`claude auth status` included")
    );
}

#[test]
fn every_warning_and_failure_names_a_fix() {
    // §13.6 (Task 9's rule): a finding that reports a problem says what to do about it.
    let replies = |version: Captured, auth: Option<Captured>| {
        let f = fx();
        f.install_claude();
        f.login("a@x.co");
        f.spawner.push(version);
        if let Some(auth) = auth {
            f.spawner.push(auth);
        }
        f.checks()
    };
    let mut all = Vec::new();
    for version in [
        exited(0, "garbled\n"),
        exited(3, ""),
        Captured::TimedOut,
        Captured::SpawnFailed("ENOEXEC".into()),
        Captured::Interrupted(2),
        exited(0, "99.0.0 (Claude Code)\n"),
    ] {
        all.extend(replies(version, Some(Captured::TimedOut)));
    }
    for auth in [
        Captured::TimedOut,
        Captured::SpawnFailed("ENOEXEC".into()),
        Captured::Interrupted(2),
        exited(0, "not json"),
    ] {
        all.extend(replies(exited(0, "2.1.286 (Claude Code)\n"), Some(auth)));
    }
    let f = fx();
    let acct = keychain_account(&f.env);
    let svc = keychain_service(&f.env, ItemKind::ManagedKey);
    f.kc.put(&svc, &acct, b"x");
    f.kc.set_unreadable(&svc, &acct, true);
    all.extend(f.checks());
    f.kc.set_locked(true);
    all.extend(f.checks());
    let problems: Vec<&Check> = all
        .iter()
        .filter(|c| matches!(c.status, CheckStatus::Warn | CheckStatus::Fail))
        .collect();
    assert!(problems.len() >= 12, "{problems:#?}");
    for c in problems {
        assert!(c.fix.is_some(), "{} names no fix: {c:?}", c.id);
    }
}

#[test]
fn the_online_hosts_are_each_endpoint_host_once() {
    let f = fx();
    assert_eq!(
        f.cc.doctor_hosts(),
        ["https://platform.claude.com/", "https://api.anthropic.com/"]
    );
    let local = ClaudeCode::with_store(LiveStore::new(f.kc.clone(), Platform::MacOs))
        .with_endpoints(Endpoints::with_base("http://127.0.0.1:9"));
    assert_eq!(local.doctor_hosts(), ["http://127.0.0.1:9/"]);
}

/// A keychain whose lock state cannot be told (`security show-keychain-info` failing), over a
/// `FakeKeychain` for everything else.
struct UnknownLock(Arc<FakeKeychain>);

impl Keychain for UnknownLock {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.0.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.0.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.0.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.0.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        LockState::Unknown
    }
    fn unlock(&self) -> bool {
        self.0.unlock()
    }
}

#[test]
fn a_keychain_whose_state_cannot_be_told_skips_auth_status_and_says_so() {
    let mut f = fx();
    let inner = f.kc.clone();
    f.cc = ClaudeCode::with_store(LiveStore::new(
        Arc::new(UnknownLock(inner.clone())),
        Platform::MacOs,
    ));
    f.install_claude();
    inner.put(
        &keychain_service(&f.env, ItemKind::ManagedKey),
        &keychain_account(&f.env),
        b"",
    );
    f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
    let checks = f.checks();
    let specs = f.spawner.specs();
    assert_eq!(specs.len(), 1, "only `claude --version`: {specs:?}");
    assert_eq!(specs[0].args, vec![OsString::from("--version")]);
    none(&checks, "cc.auth");
    none(&checks, "cc.managed-key");
    let c = one(&checks, "cc.keychain");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("lock state cannot be told")
            && c.message.contains("`claude auth status` included"),
        "{}",
        c.message
    );
    assert!(c.fix.as_deref().unwrap().contains("show-keychain-info"));
    assert_eq!(inner.unlock_attempts(), 0);
}

#[test]
fn the_auth_fixes_say_where_to_run_claude_auth_status() {
    for reply in [
        Captured::TimedOut,
        Captured::SpawnFailed("x".into()),
        exited(0, "no"),
    ] {
        let f = fx();
        f.install_claude();
        f.started();
        f.spawner.push(exited(0, "2.1.286 (Claude Code)\n"));
        f.spawner.push(reply);
        let checks = f.checks();
        let c = one(&checks, "cc.auth");
        let home = CcPaths::resolve(&f.env).config_home;
        let fix = c.fix.as_deref().unwrap();
        assert!(
            fix.contains("outside any `tagteam run` session")
                && fix.contains(&home.display().to_string()),
            "{fix}"
        );
    }
}

#[test]
fn the_unreadable_item_fixes_do_not_claim_an_attributes_probe_shows_why() {
    let mut f = fx();
    let default_home = f.env.home.join(".claude");
    f.env.claude_config_dir = Some(default_home.into_os_string());
    let acct = keychain_account(&f.env);
    f.kc.put("Claude Code-credentials", &acct, b"{}");
    f.kc.set_unreadable("Claude Code-credentials", &acct, true);
    let svc = keychain_service(&f.env, ItemKind::ManagedKey);
    f.kc.put(&svc, &acct, b"x");
    f.kc.set_unreadable(&svc, &acct, true);
    for c in f.checks().iter().filter(|c| {
        matches!(c.id.as_str(), "cc.former-item" | "cc.managed-key")
            && c.status == CheckStatus::Warn
    }) {
        let fix = c.fix.as_deref().unwrap();
        assert!(!fix.contains("find-generic-password"), "{}: {fix}", c.id);
        assert!(fix.contains("unlock"), "{}: {fix}", c.id);
    }
}

#[test]
fn the_online_hosts_are_probed_once_each_even_when_a_duplicate_is_not_adjacent() {
    let f = fx();
    let cc = ClaudeCode::with_store(LiveStore::new(f.kc.clone(), Platform::MacOs)).with_endpoints(
        Endpoints {
            token: "https://a.example/token".into(),
            profile: "https://b.example/profile".into(),
            usage: "https://a.example/usage".into(),
        },
    );
    assert_eq!(
        cc.doctor_hosts(),
        ["https://a.example/", "https://b.example/"]
    );
}
