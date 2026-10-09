mod common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use common::{LOCKED, login, seed_home};
use serde_json::{Value, json};
use tagteam::app::{self, Context, Io};
use tagteam::cli::Cli;
use tagteam::prompt::Prompter;
use tagteam_cc::live::Platform;
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
use tagteam_core::{CLAUDE_CODE, ProviderId, WindowKind};
use tagteam_engine::settings::Settings;
use tagteam_engine::store::Store;
use tagteam_engine::transfer;
use tagteam_provider::{Cancel, Env, FakeKeychain};

const UNLOCK: &str = "The login keychain is locked (common over SSH). Unlock it now?";

struct Scripted {
    interactive: bool,
    answers: VecDeque<&'static str>,
    asked: Vec<String>,
    /// The signal "sent" at every prompt, into this token, as a terminal's Ctrl-C would be.
    interrupt: Option<(Cancel, i32)>,
}

impl Scripted {
    fn none() -> Self {
        Self {
            interactive: false,
            answers: VecDeque::new(),
            asked: Vec::new(),
            interrupt: None,
        }
    }
    fn answering(a: &[&'static str]) -> Self {
        Self {
            interactive: true,
            answers: a.iter().copied().collect(),
            asked: Vec::new(),
            interrupt: None,
        }
    }
    /// A person who presses Ctrl-C at every prompt: `signal` lands in `cancel` while the prompt
    /// is up. The scripted answer is still given, where `TtyPrompter` would decline: whatever a
    /// prompt answers, the command must stop on the signal (Decision 5).
    fn interrupted_by(cancel: &Cancel, signal: i32, a: &[&'static str]) -> Self {
        Self {
            interrupt: Some((cancel.clone(), signal)),
            ..Self::answering(a)
        }
    }
    /// Notes the question, and sends the signal if this person interrupts.
    fn record(&mut self, q: &str) {
        self.asked.push(q.to_owned());
        if let Some((cancel, signal)) = &self.interrupt {
            cancel.request(*signal);
        }
    }
}

impl Prompter for Scripted {
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn confirm(&mut self, q: &str, default_yes: bool) -> bool {
        self.record(q);
        match self.answers.pop_front().expect("unexpected prompt") {
            "" => default_yes,
            a => a.starts_with('y'),
        }
    }
    fn choose(&mut self, q: &str, _o: &[String]) -> Option<usize> {
        self.record(q);
        self.answers
            .pop_front()
            .expect("unexpected prompt")
            .parse()
            .ok()
    }
    fn secret(&mut self, q: &str) -> Option<String> {
        self.record(q);
        Some(
            self.answers
                .pop_front()
                .expect("unexpected prompt")
                .to_owned(),
        )
    }
}

struct H {
    _dir: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
}

impl H {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        seed_home(&env);
        H {
            _dir: dir,
            env,
            kc: Arc::new(FakeKeychain::new()),
        }
    }

    fn login(&self, email: &str, rt: &str) {
        self.login_in(email, "", rt);
    }

    fn login_in(&self, email: &str, org: &str, rt: &str) {
        login(&self.env, &*self.kc, email, org, rt);
    }

    /// `a@x.co` stored at position 1 and live, and an API key at position 2.
    fn with_login_and_key() -> Self {
        let h = H::new();
        h.login("a@x.co", "rt-a");
        h.ok(&["add"]);
        h.ok(&["add-token", "sk-ant-api03-key"]);
        h
    }

    /// Runs `tagteam <args>` in-process, its output not a terminal and no colour variable set,
    /// whatever the test process's own environment holds.
    fn run(&self, args: &[&str], prompter: &mut Scripted) -> (i32, String, String) {
        self.run_in(args, prompter, |_| {})
    }

    /// `run` with `cancel` as the process's token, which a signal handler would set (§14.1).
    fn run_with_cancel(
        &self,
        args: &[&str],
        prompter: &mut Scripted,
        cancel: &Cancel,
    ) -> (i32, String, String) {
        self.run_in(args, prompter, |ctx| ctx.env.cancel = cancel.clone())
    }

    /// `run`, with `adjust` applied to the context first.
    fn run_in(
        &self,
        args: &[&str],
        prompter: &mut Scripted,
        adjust: impl FnOnce(&mut Context),
    ) -> (i32, String, String) {
        let argv = std::iter::once("tagteam").chain(args.iter().copied());
        let cli = Cli::try_parse_from(argv).unwrap();
        let mut ctx = Context {
            env: self.env.clone(),
            keychain: self.kc.clone(),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: Some(common::OFFLINE_API_BASE.into()),
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        };
        adjust(&mut ctx);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = app::run(
            cli,
            ctx,
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter,
            },
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (code, out, err) = self.run(args, &mut Scripted::none());
        assert_eq!(code, 0, "{args:?}: {err}");
        out
    }

    /// A successful `--json` command's object, and its stderr.
    fn switch_json(&self, args: &[&str]) -> (Value, String) {
        let (code, out, err) = self.run(args, &mut Scripted::none());
        assert_eq!(code, 0, "{args:?}: {err}");
        (serde_json::from_str(&out).unwrap(), err)
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut v: Value = serde_json::from_str(&self.ok(args)).unwrap();
        normalize(&mut v);
        v
    }
}

/// `a@x.co`'s row as §13.2 renders it, at `position`.
fn a_row(position: u32, active: bool) -> Value {
    json!({"number": position, "position": position, "id": "[id]", "provider": "claude-code", "email": "a@x.co",
           "organizationName": null, "organizationUuid": "", "isOrganization": false, "active": active,
           "usageStatus": "unavailable", "usage": null, "lastGoodUsage": null, "lastGoodFetchedAt": null,
           "lastGoodAgeSeconds": null, "usageError": "no-data", "usageRetryAt": null,
           "loginExpiresAt": 1_797_000_000_000i64})
}

/// The API key `add-token sk-ant-api03-key` stores second (its default email names that
/// position), at `position`. It is never the live login.
fn key_row(position: u32) -> Value {
    json!({"number": position, "position": position, "id": "[id]", "provider": "claude-code", "email": "api-key-2@token.local",
           "organizationName": null, "organizationUuid": "", "isOrganization": false, "active": false,
           "usageStatus": "api_key", "usage": null, "lastGoodUsage": null, "lastGoodFetchedAt": null,
           "lastGoodAgeSeconds": null, "usageError": null, "usageRetryAt": null})
}

/// `v` with `extra`'s fields set.
fn with(mut v: Value, extra: Value) -> Value {
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

/// An account command's success object.
fn done(account: Value) -> Value {
    json!({"schemaVersion": 1, "ok": true, "account": account})
}

/// Ids, and the retry time a collection records, as placeholders, so rows compare exactly.
fn normalize(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                match k.as_str() {
                    "id" if x.is_string() => *x = json!("[id]"),
                    "usageRetryAt" if x.is_string() => *x = json!("[time]"),
                    _ => normalize(x),
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize),
        _ => {}
    }
}

/// `a@x.co`'s row once `list` or `status` has tried to collect it with every endpoint offline
/// (§8.3): the request never left, so it is `unavailable` for `pre-send`, retried later.
fn a_row_offline(position: u32, active: bool) -> Value {
    with(
        a_row(position, active),
        json!({"usageError": "pre-send", "usageRetryAt": "[time]"}),
    )
}

#[test]
fn colour_under_auto_follows_the_output_the_command_writes_to() {
    // §13.1: `ui.color = auto` colours when the output is a terminal. That is the context's
    // say, not the test process's own stdout.
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    let store = Store::open_existing(&h.env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let id = store.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap()[0]
        .id
        .as_str()
        .to_owned();
    let now = common::now_epoch_s();
    let seven = common::usage_window(
        "7d",
        "7d",
        WindowKind::Long,
        77.0,
        Some(now + 300_000),
        Some(604_800),
    );
    common::record_reading(h._dir.path(), &id, now, &[seven]);
    const YELLOW_77: &str = "\x1b[33m77%\x1b[0m";
    let list = |args: &[&str], adjust: fn(&mut Context)| {
        let (code, out, err) = h.run_in(args, &mut Scripted::none(), adjust);
        assert_eq!((code, err.as_str()), (0, ""));
        out
    };
    assert!(!list(&["list"], |_| {}).contains('\x1b'), "not a terminal");
    let terminal: fn(&mut Context) = |c| c.stdout_terminal = true;
    assert!(list(&["list"], terminal).contains(YELLOW_77), "a terminal");
    assert!(!list(&["list", "--no-color"], terminal).contains('\x1b'));
    assert!(
        !list(&["list"], |c| {
            c.stdout_terminal = true;
            c.no_color_env = true;
        })
        .contains('\x1b'),
        "NO_COLOR wins"
    );
    assert!(
        list(&["list"], |c| c.force_color_env = true).contains(YELLOW_77),
        "FORCE_COLOR colours a pipe"
    );
}

#[test]
fn a_vault_keychain_of_its_own_holds_the_vault_and_nothing_else() {
    // Decision 16: `cargo xtask compat` gives the vault, and only the vault, a keychain of its
    // own; Claude Code's items stay where they are.
    let h = H::new();
    h.login("a@x.co", "rt-a");
    let vault = Arc::new(FakeKeychain::new());
    let own = vault.clone();
    let (code, _, err) = h.run_in(&["add"], &mut Scripted::none(), move |ctx| {
        ctx.vault_keychain = Some(own);
    });
    assert_eq!(code, 0, "{err}");
    let services: Vec<String> = vault.items().into_keys().map(|(s, _)| s).collect();
    assert!(
        !services.is_empty() && services.iter().all(|s| s == "tagteam"),
        "{services:?}"
    );
    let login = h.kc.items();
    assert!(
        login.keys().all(|(s, _)| s != "tagteam"),
        "no vault item in the login keychain"
    );
    let live = (
        keychain_service(&h.env, ItemKind::OAuth),
        keychain_account(&h.env),
    );
    assert!(login.contains_key(&live), "Claude Code's item is untouched");
}

#[test]
fn an_empty_list_says_how_to_start() {
    let h = H::new();
    assert_eq!(
        h.ok(&["list"]),
        "No accounts yet. Log in with `claude`, then run `tagteam add`.\n"
    );
    assert_eq!(
        h.ok(&[]),
        "No accounts yet. Log in with `claude`, then run `tagteam add`.\n"
    );
    // Review Focus 4: a command that changes nothing creates nothing (§5).
    assert!(!h.env.data_dir().exists());
}

#[test]
fn add_list_status_and_switch_read_like_this() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    assert_eq!(
        h.ok(&["add", "--alias", "work"]),
        "Added work (a@x.co) at position 1.\n"
    );
    h.login("b@x.co", "rt-b");
    assert_eq!(h.ok(&["add"]), "Added b@x.co at position 2.\n");
    // Every endpoint is offline: each account's fetch fails before sending (§8.3).
    assert_eq!(
        h.ok(&["list"]),
        concat!(
            "    #  ACCOUNT\n",
            "    1  work (a@x.co)  unavailable (pre-send, retry <1m)\n",
            " *  2  b@x.co         unavailable (pre-send, retry <1m)\n",
        )
    );
    assert_eq!(
        h.ok(&["status"]),
        "Live: b@x.co (position 2 of 2)\n  unavailable (pre-send, retry <1m)\n"
    );
    assert_eq!(
        h.ok(&["switch", "work"]),
        "Switched to work (a@x.co) (position 1).\nClaude Code picks this up within about 30 s; restart it to apply now.\n"
    );
    // The switch re-planned nothing: neither account has a reading, so both stay as the
    // `list` above left them, inside the 30 s backoff of their failed fetch.
    assert_eq!(
        h.ok(&["ls"]),
        concat!(
            "    #  ACCOUNT\n",
            " *  1  work (a@x.co)  unavailable (pre-send, retry <1m)\n",
            "    2  b@x.co         unavailable (pre-send, retry <1m)\n",
        )
    );
    assert_eq!(
        h.ok(&["switch"]),
        "Switched to b@x.co (position 2).\nClaude Code picks this up within about 30 s; restart it to apply now.\n"
    );
    assert_eq!(h.ok(&["switch", "2"]), "b@x.co is already active\n");
}

#[test]
fn list_json_is_cswap_compatible() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.ok(&["add-token", "sk-ant-api03-key", "--alias", "ci"]);
    h.ok(&["disable", "ci"]);
    assert_eq!(
        h.json(&["list", "--json"]),
        json!({
            "schemaVersion": 1,
            "activeAccountNumber": 1,
            "activeByProvider": {"claude-code": 1},
            "accounts": [a_row_offline(1, true), with(key_row(2), json!({"alias": "ci", "disabled": true}))]
        })
    );
}

#[test]
fn status_and_switch_json() {
    let h = H::new();
    // Every status shape names its provider at the top level, and in `active` too (§13.2).
    assert_eq!(
        h.json(&["status", "--json"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "active": null})
    );
    h.login("stranger@x.co", "rt-s");
    assert_eq!(
        h.json(&["status", "--json"]),
        json!({"schemaVersion": 1, "provider": "claude-code",
               "active": {"email": "stranger@x.co", "provider": "claude-code", "managed": false}})
    );
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("b@x.co", "rt-b");
    h.ok(&["add"]);
    assert_eq!(
        h.json(&["switch", "1", "--json"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": true, "from": 2, "to": 1, "strategy": "direct",
               "reason": "switched", "message": "Switched to a@x.co", "credentialStore": "keychain", "warnings": []})
    );
    // a has no reading, so the switch left it without a plan (§8.3) and `status` fetches it on
    // demand: offline, that fails before it is sent.
    assert_eq!(
        h.json(&["status", "--json"]),
        json!({"schemaVersion": 1, "provider": "claude-code",
               "active": with(a_row_offline(1, true), json!({"managed": true})),
               "totalManagedAccounts": 2})
    );
}

#[test]
fn errors_are_one_json_object_with_a_stable_type() {
    let h = H::new();
    let (code, out, _) = h.run(&["switch", "9", "--json"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "no-such-account", "message": "no account matches \"9\""}})
    );
    let (code, out, err) = h.run(&["switch", "9"], &mut Scripted::none());
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(err, "tagteam: no account matches \"9\"\n");
}

const ADD_STRANGER_FIRST: &str = "Add the current login (stranger@x.co) first?";

/// `a@x.co` stored, and a login no account holds live.
fn with_unmanaged_login() -> H {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("stranger@x.co", "rt-s");
    h
}

#[test]
fn an_unmanaged_login_is_offered_for_adding_on_a_terminal() {
    let h = with_unmanaged_login();
    let mut yes = Scripted::answering(&[""]);
    let (code, out, _) = h.run(&["switch", "1"], &mut yes);
    assert_eq!(code, 0);
    assert_eq!(yes.asked, [ADD_STRANGER_FIRST]);
    assert!(
        out.starts_with("Added stranger@x.co at position 2.\nSwitched to a@x.co (position 1).\n"),
        "{out}"
    );
}

#[test]
fn with_no_store_adding_from_a_switch_checks_the_keychain_first() {
    // No store, so `switch` itself ran no lock check; adding the live login reads the live
    // credential and writes the vault, so the offer runs Appendix A.3's check before it.
    let h = H::new();
    h.login("stranger@x.co", "rt-s");
    h.kc.set_locked(true);
    let mut yes = Scripted::answering(&["", ""]);
    let (code, out, err) = h.run(&["switch"], &mut yes);
    assert_eq!(code, 0, "{err}");
    assert_eq!(yes.asked, [ADD_STRANGER_FIRST, UNLOCK]);
    assert_eq!(h.kc.unlock_attempts(), 1);
    assert_eq!(
        out,
        "Added stranger@x.co at position 1.\nthere is only one switchable account\n"
    );
    // Declining the unlock refuses as the check always does, and adds nothing.
    let h = H::new();
    h.login("stranger@x.co", "rt-s");
    h.kc.set_locked(true);
    let mut no_unlock = Scripted::answering(&["", "n"]);
    let (code, out, err) = h.run(&["switch"], &mut no_unlock);
    assert_eq!(
        (code, out.as_str(), err),
        (1, "", format!("tagteam: {LOCKED}\n"))
    );
    assert_eq!(no_unlock.asked, [ADD_STRANGER_FIRST, UNLOCK]);
    assert_eq!(h.kc.unlock_attempts(), 0);
    assert!(!h.env.data_dir().exists());
}

/// The stderr notice for a write the Keychain refused, which went to `path` instead.
fn fell_back(path: &Path) -> String {
    format!(
        "warning: the Keychain refused the write, so the credential was stored in {} instead\n",
        path.display()
    )
}

#[test]
fn an_oauth_write_the_keychain_refuses_is_reported_where_it_went() {
    // Appendix A.3: the credential falls back to Claude Code's credentials file.
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("b@x.co", "rt-b");
    h.ok(&["add"]);
    let oauth_item = keychain_service(&h.env, ItemKind::OAuth);
    let notice = fell_back(&CcPaths::resolve(&h.env).credentials_file);
    h.kc.set_fail_write(&oauth_item, true);
    let (code, out, err) = h.run(&["switch", "1"], &mut Scripted::none());
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "Switched to a@x.co (position 1).\nActive on your next message.\n",
            notice.as_str()
        )
    );
    let (v, err) = h.switch_json(&["switch", "2", "--json"]);
    assert_eq!(
        (v["switched"].clone(), v["credentialStore"].clone(), err),
        (json!(true), json!("file"), notice)
    );
    // Once the Keychain takes the write again, nothing is reported.
    h.kc.set_fail_write(&oauth_item, false);
    let (v, err) = h.switch_json(&["switch", "1", "--json"]);
    assert_eq!(
        (v["credentialStore"].clone(), err.as_str()),
        (json!("keychain"), "")
    );
}

#[test]
fn an_api_key_write_the_keychain_refuses_is_reported_where_it_went() {
    // Appendix A.3: the managed key falls back to `primaryApiKey` in the global config.
    let h = H::with_login_and_key();
    let key_item = keychain_service(&h.env, ItemKind::ManagedKey);
    let config = CcPaths::resolve(&h.env).global_config;
    h.kc.set_fail_write(&key_item, true);
    let (v, err) = h.switch_json(&["switch", "2", "--json"]);
    assert_eq!(
        (v["switched"].clone(), v["credentialStore"].clone(), err),
        (json!(true), json!("file"), fell_back(&config))
    );
    assert!(
        std::fs::read_to_string(&config)
            .unwrap()
            .contains("\"primaryApiKey\"")
    );
    assert_eq!(h.kc.get(&key_item, &keychain_account(&h.env)), None);
    // Back to the OAuth login with a working Keychain: nothing to report.
    h.kc.set_fail_write(&key_item, false);
    let (v, err) = h.switch_json(&["switch", "1", "--json"]);
    assert_eq!(
        (v["credentialStore"].clone(), err.as_str()),
        (json!("keychain"), "")
    );
    // And in human mode.
    h.kc.set_fail_write(&key_item, true);
    let (code, out, err) = h.run(&["switch", "2"], &mut Scripted::none());
    assert_eq!((code, err), (0, fell_back(&config)));
    assert!(
        out.starts_with("Switched to api-key-2@token.local (position 2).\n"),
        "{out}"
    );
}

#[test]
fn declining_the_unmanaged_login_offer_cancels_and_adds_nothing() {
    let h = with_unmanaged_login();
    let mut no = Scripted::answering(&["n"]);
    let (code, out, err) = h.run(&["switch", "1"], &mut no);
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (1, "", "tagteam: cancelled\n")
    );
    assert_eq!(no.asked, [ADD_STRANGER_FIRST]);
    assert_eq!(
        h.ok(&["list"]),
        "    #  ACCOUNT\n    1  a@x.co   unavailable (pre-send, retry <1m)\n"
    );
    assert_eq!(
        h.ok(&["status"]),
        "Live: stranger@x.co (not managed by tagteam)\n"
    );
}

#[test]
fn prompts_never_block_a_non_interactive_caller() {
    // Review Focus 2.
    let h = with_unmanaged_login();
    let (code, _, err) = h.run(&["switch", "1"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert!(
        err.contains("tagteam add") && err.contains("--force"),
        "{err}"
    );
    let noop = h.json(&["switch", "1", "--json"]);
    assert_eq!(
        (noop["switched"].clone(), noop["reason"].clone()),
        (json!(false), json!("unmanaged-account"))
    );
    let (code, _, err) = h.run(&["add", "--position", "1"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert!(err.contains("--yes"), "{err}");
    let (code, _, _) = h.run(
        &["add", "--position", "1"],
        &mut Scripted::answering(&["y"]),
    );
    assert_eq!(code, 0);
    assert_eq!(
        h.ok(&["list"]),
        "    #  ACCOUNT\n *  1  stranger@x.co  unavailable (pre-send, retry <1m)\n"
    );
    let (code, _, err) = h.run(&["add-token"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert!(err.contains("`-`"), "{err}");
}

const REPLACE_A: &str = "Position 1 holds a@x.co. Replace it?";

/// `list` with `a@x.co` alone and live, its fetch failed offline.
const LIVE_A_OFFLINE: &str = "    #  ACCOUNT\n *  1  a@x.co   unavailable (pre-send, retry <1m)\n";

/// `a@x.co` stored at position 1, and live.
fn with_one_login() -> H {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h
}

#[test]
fn add_token_over_an_occupied_position_asks_on_a_terminal_and_keeps_the_token() {
    // §10.2, as §10.1: the token is read once, before the question, and kept for the retry.
    let h = with_one_login();
    let mut yes = Scripted::answering(&["sk-ant-api03-key", "y"]);
    let (code, out, err) = h.run(&["add-token", "--position", "1"], &mut yes);
    assert_eq!(
        (code, out.as_str()),
        (0, "Added api-key-1@token.local at position 1.\n"),
        "{err}"
    );
    assert_eq!(yes.asked, ["Token: ", REPLACE_A]);
    assert_eq!(
        h.ok(&["list"]),
        "    #  ACCOUNT\n    1  api-key-1@token.local  api key\n"
    );
}

#[test]
fn declining_add_token_over_an_occupied_position_cancels_and_keeps_the_occupant() {
    let h = with_one_login();
    let mut no = Scripted::answering(&["n"]);
    let (code, out, err) = h.run(
        &["add-token", "sk-ant-api03-key", "--position", "1"],
        &mut no,
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (1, "", "tagteam: cancelled\n")
    );
    assert_eq!(no.asked, [REPLACE_A]);
    assert_eq!(h.ok(&["list"]), LIVE_A_OFFLINE);
}

#[test]
fn add_token_over_an_occupied_position_never_asks_off_a_terminal() {
    let h = with_one_login();
    let args = ["add-token", "sk-ant-api03-key", "--position", "1"];
    let (code, _, err) = h.run(&args, &mut Scripted::none());
    assert_eq!(code, 1);
    assert!(err.contains("--yes"), "{err}");
    // `--json` never prompts, even on a terminal: Scripted panics on any prompt it has no answer for.
    let (code, out, _) = h.run(
        &[&args[..], &["--json"]].concat(),
        &mut Scripted::answering(&[]),
    );
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["error"]["type"],
        "needs-confirmation"
    );
    assert_eq!(h.ok(&["list"]), LIVE_A_OFFLINE);
}

#[test]
fn an_ambiguous_email_is_chosen_on_a_terminal_and_refused_elsewhere() {
    // §10.4: one email in two organizations.
    let h = H::new();
    h.login_in("a@x.co", "", "rt-personal");
    h.ok(&["add"]);
    h.login_in("a@x.co", "org-1", "rt-org");
    h.ok(&["add"]);
    let (code, out, err) = h.run(&["disable", "a@x.co"], &mut Scripted::none());
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.contains("matches several accounts"), "{err}");
    let (code, out, _) = h.run(
        &["disable", "a@x.co", "--json"],
        &mut Scripted::answering(&[]),
    );
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["error"]["type"],
        "ambiguous-account"
    );
    let mut pick_second = Scripted::answering(&["1"]);
    let (code, out, err) = h.run(&["disable", "a@x.co"], &mut pick_second);
    assert_eq!((code, out.as_str()), (0, "a@x.co is disabled.\n"), "{err}");
    assert_eq!(pick_second.asked, ["Which account?"]);
    let list = h.json(&["list", "--json"]);
    assert_eq!(list["accounts"][0].get("disabled"), None);
    assert_eq!(list["accounts"][1]["disabled"], true);
}

#[test]
fn a_locked_keychain_is_offered_for_unlocking_on_a_terminal() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.kc.set_locked(true);
    let mut yes = Scripted::answering(&[""]);
    let (code, out, err) = h.run(&["add"], &mut yes);
    assert_eq!(
        (code, out.as_str()),
        (0, "Added a@x.co at position 1.\n"),
        "{err}"
    );
    assert_eq!(yes.asked, [UNLOCK]);
    assert_eq!(h.kc.unlock_attempts(), 1);
    // Declined: no unlock is attempted, and the command does nothing.
    h.kc.set_locked(true);
    let (code, out, err) = h.run(&["remove", "1"], &mut Scripted::answering(&["n"]));
    assert_eq!(
        (code, out.as_str(), err),
        (1, "", format!("tagteam: {LOCKED}\n"))
    );
    assert_eq!(h.kc.unlock_attempts(), 1);
    // The unlock failed (a wrong password): the same error.
    h.kc.set_refuse_unlock(true);
    let (code, _, err) = h.run(&["remove", "1"], &mut Scripted::answering(&["y"]));
    assert_eq!((code, err), (1, format!("tagteam: {LOCKED}\n")));
    assert_eq!(h.kc.unlock_attempts(), 2);
    h.kc.set_locked(false);
    assert_eq!(h.ok(&["list"]), LIVE_A_OFFLINE);
}

#[test]
fn the_unlock_offer_never_blocks_a_non_interactive_caller() {
    // Review Focus 2: no terminal fails at once, before anything is touched or created.
    let h = H::new();
    let refused = |commands: &[&[&str]]| {
        for args in commands {
            let (code, out, err) = h.run(args, &mut Scripted::none());
            assert_eq!(
                (code, out.as_str(), err),
                (1, "", format!("tagteam: {LOCKED}\n")),
                "{args:?}"
            );
        }
    };
    h.kc.set_locked(true);
    // With no store, only the two adds would touch a Keychain item.
    refused(&[&["add"], &["add-token", "sk-ant-api03-key"]]);
    // `--json` never prompts, even on a terminal: Scripted panics on any prompt it has no answer for.
    let (code, out, _) = h.run(&["add", "--json"], &mut Scripted::answering(&[]));
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "keychain-locked", "message": LOCKED}})
    );
    assert!(!h.env.data_dir().exists());
    // With a store, `switch` and `remove` reach its accounts' items too.
    h.kc.set_locked(false);
    h.ok(&["add-token", "sk-ant-api03-key"]);
    h.kc.set_locked(true);
    refused(&[&["switch"], &["remove", "1"]]);
    assert_eq!(h.kc.unlock_attempts(), 0);
}

#[test]
fn auto_checks_the_keychain_before_its_first_tick_a_dry_run_too() {
    // §11.1 and Appendix A.3. With no store there is nothing to switch between, so no check
    // runs and nothing prompts (Scripted panics on a prompt it has no answer for).
    let h = H::new();
    h.kc.set_locked(true);
    let (code, out, err) = h.run(&["auto", "--once"], &mut Scripted::answering(&[]));
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            1,
            "",
            "tagteam: auto-switch needs two switchable accounts on a provider; add another with `tagteam add`\n"
        )
    );
    h.kc.set_locked(false);
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("b@x.co", "rt-b");
    h.ok(&["add"]);
    h.kc.set_locked(true);
    for args in [&["auto", "--once"][..], &["auto", "--once", "--dry-run"]] {
        let (code, out, err) = h.run(args, &mut Scripted::none());
        assert_eq!(
            (code, out.as_str(), err),
            (1, "", format!("tagteam: {LOCKED}\n")),
            "{args:?}"
        );
    }
    assert_eq!(h.kc.unlock_attempts(), 0);
}

#[test]
fn with_no_store_switch_and_remove_run_no_lock_check() {
    // A fresh machine: there is nothing to activate or delete, so a locked keychain neither
    // refuses nor prompts (Scripted panics on a prompt it has no answer for).
    let h = H::new();
    h.kc.set_locked(true);
    let (code, out, err) = h.run(&["switch", "--json"], &mut Scripted::answering(&[]));
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": false, "from": null, "to": null,
               "strategy": "rotation", "reason": "no-valid-target",
               "message": "there are no stored accounts; add one with `tagteam add`",
               "credentialStore": null, "warnings": []})
    );
    let (code, out, err) = h.run(&["remove", "1"], &mut Scripted::answering(&[]));
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (1, "", "tagteam: no account matches \"1\"\n")
    );
    assert_eq!(h.kc.unlock_attempts(), 0);
    assert!(!h.env.data_dir().exists());
}

// One test per account command: its success JSON, `active` as the engine sees it.

#[test]
fn add_json_reports_the_captured_live_login_as_active() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    let added = done(a_row(1, true));
    assert_eq!(
        h.json(&["add", "--json"]),
        with(added.clone(), json!({"created": true}))
    );
    // The same login again is refreshed in place.
    assert_eq!(
        h.json(&["add", "--json"]),
        with(added, json!({"created": false}))
    );
}

#[test]
fn add_token_json_reports_a_new_inactive_account() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    assert_eq!(
        h.json(&["add-token", "sk-ant-api03-key", "--json"]),
        with(done(key_row(2)), json!({"created": true}))
    );
}

#[test]
fn remove_json_says_whether_the_removed_login_was_live() {
    let h = H::with_login_and_key();
    assert_eq!(h.json(&["remove", "2", "--json"]), done(key_row(2)));
    assert_eq!(h.json(&["remove", "1", "--json"]), done(a_row(1, true)));
}

#[test]
fn alias_json_sets_lists_and_clears() {
    let h = H::with_login_and_key();
    assert_eq!(
        h.json(&["alias", "1", "work", "--json"]),
        done(with(a_row(1, true), json!({"alias": "work"})))
    );
    assert_eq!(
        h.json(&["alias", "--json"]),
        json!({"schemaVersion": 1, "aliases": [{"alias": "work", "number": 1, "provider": "claude-code"}]})
    );
    assert_eq!(
        h.json(&["alias", "work", "--unset", "--json"]),
        done(a_row(1, true))
    );
}

#[test]
fn move_json_reports_the_new_position() {
    let h = H::with_login_and_key();
    assert_eq!(h.json(&["move", "1", "2", "--json"]), done(a_row(2, true)));
}

#[test]
fn disable_json_marks_the_row() {
    let h = H::with_login_and_key();
    assert_eq!(
        h.json(&["disable", "1", "--json"]),
        done(with(a_row(1, true), json!({"disabled": true})))
    );
    assert_eq!(
        h.json(&["disable", "2", "--json"]),
        done(with(key_row(2), json!({"disabled": true})))
    );
}

#[test]
fn enable_json_clears_the_mark() {
    let h = H::with_login_and_key();
    h.ok(&["disable", "1"]);
    assert_eq!(h.json(&["enable", "1", "--json"]), done(a_row(1, true)));
}

#[test]
fn commands_that_touch_no_keychain_item_run_no_check() {
    let h = H::with_login_and_key();
    h.kc.set_locked(true);
    // A check would find the keychain locked and prompt; this prompter has no answers and panics.
    let mut p = Scripted::answering(&[]);
    let store_only: [&[&str]; 7] = [
        &[],
        &["list"],
        &["status"],
        &["alias", "1", "work"],
        &["disable", "work"],
        &["enable", "work"],
        &["move", "work", "2"],
    ];
    for args in store_only {
        let (code, _, err) = h.run(args, &mut p);
        assert_eq!(code, 0, "{args:?}: {err}");
    }
    assert!(p.asked.is_empty());
    assert_eq!(h.kc.unlock_attempts(), 0);
    // `list` and `status` do read Keychain items, to collect usage (§8.3), but run no lock
    // check: a locked keychain is the row's `keychain_unavailable`, never a prompt or an error.
    let v = h.json(&["list", "--json"]);
    let a = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["email"] == "a@x.co")
        .unwrap();
    assert_eq!(a["usageStatus"], "keychain_unavailable");
}

#[test]
fn alias_move_remove_and_usage_errors() {
    let h = H::with_login_and_key();
    assert_eq!(h.ok(&["alias", "1", "Work"]), "Position 1 is now work.\n");
    assert_eq!(h.ok(&["alias"]), "work  1  a@x.co\n");
    assert_eq!(
        h.ok(&["alias", "work", "--unset"]),
        "Position 1 has no alias now.\n"
    );
    assert_eq!(h.ok(&["move", "1", "2"]), "a@x.co is now at position 2.\n");
    assert_eq!(
        h.ok(&["remove", "api-key-2@token.local"]),
        "Removed api-key-2@token.local (position 1).\n"
    );
    let (code, _, _) = h.run(&["alias", "1"], &mut Scripted::none());
    assert_eq!(code, 2);
    let (code, out, _) = h.run(&["alias", "1", "--json"], &mut Scripted::none());
    assert_eq!(code, 2);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["error"]["type"],
        "usage"
    );
}

#[test]
fn token_accounts_list_their_kind_from_the_provider() {
    // M-5: the CLI names no kind strings. The provider's kind traits decide the label, the
    // JSON `usageStatus`, and the default email.
    let h = H::new();
    h.ok(&["add-token", "sk-ant-oat01-setup"]);
    h.ok(&["add-token", "sk-ant-api03-key"]);
    assert_eq!(
        h.ok(&["list"]),
        concat!(
            "    #  ACCOUNT\n",
            "    1  setup-token-1@token.local  unavailable (pre-send, retry <1m)  setup token\n",
            "    2  api-key-2@token.local      api key\n",
        )
    );
    let v = h.json(&["list", "--json"]);
    assert_eq!(v["accounts"][0]["usageStatus"], "unavailable");
    assert_eq!(v["accounts"][1]["usageStatus"], "api_key");
}

#[test]
fn settings_warnings_go_to_stderr_and_never_fail_the_command() {
    // §6.4: reads are forgiving. Each warning is one stderr line; stdout is unchanged, and
    // `--json` still prints exactly one object (§13.2).
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    let (code, plain, err) = h.run(&["list"], &mut Scripted::none());
    assert_eq!((code, err.as_str()), (0, ""), "no config.toml, no warning");

    let dir = h.env.config_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for text in ["[autoswitch]\nthreshold = 5\n", "[autoswitch\n"] {
        std::fs::write(dir.join("config.toml"), text).unwrap();
        let (_, warnings) = Settings::load(&h.env, &ProviderId::new(CLAUDE_CODE));
        assert_eq!(warnings.len(), 1, "{text:?}: {warnings:?}");
        let expected: String = warnings.iter().map(|w| format!("warning: {w}\n")).collect();
        let (code, out, err) = h.run(&["list"], &mut Scripted::none());
        assert_eq!((code, err.as_str()), (0, expected.as_str()), "{text:?}");
        assert_eq!(out, plain, "{text:?}: stdout is unchanged");
        let (code, out, err) = h.run(&["--json", "list"], &mut Scripted::none());
        assert_eq!((code, err.as_str()), (0, expected.as_str()), "{text:?}");
        serde_json::from_str::<Value>(&out).expect("stdout stays one JSON object");
    }
}

#[test]
fn settings_are_read_for_the_provider_the_command_resolves() {
    // §6.4: a provider's own table comes first, and it is the resolved provider's: `--provider`,
    // else the default. Only claude-code is registered, so another's table shows only in what
    // is read for it: its invalid key warns, before the command refuses the provider.
    let h = H::new();
    let dir = h.env.config_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "[provider.other.autoswitch]\nthreshold = 5\n",
    )
    .unwrap();
    let (code, _, err) = h.run(&["list"], &mut Scripted::none());
    assert_eq!(
        (code, err.as_str()),
        (0, ""),
        "the default provider's own tables only"
    );
    let (code, _, err) = h.run(&["list", "--provider", "other"], &mut Scripted::none());
    assert_eq!(code, 1);
    let mut lines = err.lines();
    let warning = lines.next().unwrap();
    assert!(
        warning.starts_with("warning: ")
            && warning.contains("`provider.other.autoswitch.threshold`"),
        "{err}"
    );
    assert_eq!(lines.next(), Some("tagteam: unknown provider \"other\""));
}

/// A token already set when the command starts: the signal came first.
fn signalled(signal: i32) -> Cancel {
    let cancel = Cancel::new();
    cancel.request(signal);
    cancel
}

/// `a@x.co` at position 1, and `b@x.co` at position 2 and live.
fn two_logins() -> H {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("b@x.co", "rt-b");
    h.ok(&["add"]);
    h
}

/// §13.2's envelope for §14.1's interruption (Decision 4).
fn interrupted_json() -> Value {
    json!({"schemaVersion": 1, "error": {"type": "interrupted", "message": "interrupted"}})
}

const INTERRUPTED: &str = "tagteam: interrupted\n";

/// Decision 6's notice for `command`.
fn too_late(command: &str) -> String {
    format!("tagteam: interrupted too late to stop: {command} had already finished\n")
}

#[test]
fn an_interrupted_command_exits_128_plus_the_signal() {
    // §13.1, §14.1, Decision 4: the switch stops at its first lock wait, before it writes.
    let h = two_logins();
    let item = (
        keychain_service(&h.env, ItemKind::OAuth),
        keychain_account(&h.env),
    );
    let entry = h.kc.get(&item.0, &item.1);
    for (signal, code) in [
        (libc::SIGINT, 130),
        (libc::SIGTERM, 143),
        (libc::SIGHUP, 129),
    ] {
        let (got, out, err) = h.run_with_cancel(
            &["switch", "1", "--json"],
            &mut Scripted::none(),
            &signalled(signal),
        );
        assert_eq!((got, err.as_str()), (code, ""), "signal {signal}");
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            interrupted_json(),
            "signal {signal}"
        );
    }
    let (code, out, err) = h.run_with_cancel(
        &["switch", "1"],
        &mut Scripted::none(),
        &signalled(libc::SIGINT),
    );
    assert_eq!((code, out.as_str(), err.as_str()), (130, "", INTERRUPTED));
    assert_eq!(common::live_email(h.env.home.parent().unwrap()), "b@x.co");
    assert_eq!(h.kc.get(&item.0, &item.1), entry, "nothing was written");
}

#[test]
fn an_interrupted_usage_collection_interrupts_list_and_status() {
    // §14.1: collecting is a cancellation point, and an interrupted collection records nothing.
    let h = with_one_login();
    for args in [["list", "--json"], ["status", "--json"]] {
        let (code, out, err) =
            h.run_with_cancel(&args, &mut Scripted::none(), &signalled(libc::SIGINT));
        assert_eq!((code, err.as_str()), (130, ""), "{args:?}");
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            interrupted_json(),
            "{args:?}"
        );
    }
    let store = Store::open_existing(&h.env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let a = store.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap()[0]
        .id
        .clone();
    assert_eq!(
        store.usage_state(&a).unwrap(),
        None,
        "nothing was recorded, not even a failure"
    );
}

#[test]
fn a_signal_that_meets_no_cancellation_point_is_too_late_and_changes_nothing() {
    // Decision 6: `list` on a fresh home collects nothing, so nothing could stop it.
    let h = H::new();
    let (code, out, err) = h.run_with_cancel(
        &["list", "--json"],
        &mut Scripted::none(),
        &signalled(libc::SIGINT),
    );
    assert_eq!((code, err), (0, too_late("list")));
    assert_eq!(
        out,
        h.ok(&["list", "--json"]),
        "stdout is one JSON object, as without the signal"
    );
    let (code, out, err) =
        h.run_with_cancel(&["list"], &mut Scripted::none(), &signalled(libc::SIGTERM));
    assert_eq!((code, err), (0, too_late("list")));
    assert_eq!(out, h.ok(&["list"]));
    // A command that failed for a reason of its own keeps its error and its exit code.
    let (code, out, err) = h.run_with_cancel(
        &["switch", "9"],
        &mut Scripted::none(),
        &signalled(libc::SIGINT),
    );
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(
        err,
        format!("tagteam: no account matches \"9\"\n{}", too_late("switch"))
    );
}

#[test]
fn a_broken_pipe_that_meets_no_cancellation_point_prints_no_late_notice() {
    // The reader of `auto --once`'s output left: SIGPIPE is recorded, the exit code stays, and
    // nobody is left to read a notice that the signal came too late.
    let h = H::new();
    let (code, out, err) =
        h.run_with_cancel(&["list"], &mut Scripted::none(), &signalled(libc::SIGPIPE));
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, h.ok(&["list"]));
}

#[test]
fn ctrl_c_at_the_offer_to_add_the_live_login_interrupts_and_adds_nothing() {
    // Review Focus 3. The scripted answer is a yes: whatever the prompt answered, the signal
    // stops the command.
    let h = with_unmanaged_login();
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGINT, &["y"]);
    let (code, out, err) = h.run_with_cancel(&["switch", "1"], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (130, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, [ADD_STRANGER_FIRST]);
    assert_eq!(
        h.ok(&["status"]),
        "Live: stranger@x.co (not managed by tagteam)\n"
    );
}

#[test]
fn ctrl_c_at_the_secret_prompt_interrupts_and_adds_nothing() {
    // Review Focus 3: `add-token`'s no-echo prompt. The terminal side (echo restored, typed
    // input discarded) is `read_secret`'s, pinned on a pty in `prompt.rs`.
    let h = with_one_login();
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGINT, &["sk-ant-api03-key"]);
    let (code, out, err) = h.run_with_cancel(&["add-token"], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (130, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, ["Token: "]);
    assert_eq!(h.ok(&["list"]), LIVE_A_OFFLINE);
}

#[test]
fn ctrl_c_at_the_unlock_question_never_runs_the_unlock() {
    // Appendix A.3's question is a prompt like any other: a yes typed as the signal lands does
    // not go on to `security unlock-keychain`.
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.kc.set_locked(true);
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGINT, &[""]);
    let (code, out, err) = h.run_with_cancel(&["add"], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (130, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, [UNLOCK]);
    assert_eq!(h.kc.unlock_attempts(), 0);
    assert!(!h.env.data_dir().exists(), "nothing was added");
}

#[test]
fn ctrl_c_at_a_choice_or_a_replacement_question_interrupts() {
    // §10.4's choice of account, then §10.1's replacement question, under SIGTERM.
    let h = H::new();
    h.login_in("a@x.co", "", "rt-personal");
    h.ok(&["add"]);
    h.login_in("a@x.co", "org-1", "rt-org");
    h.ok(&["add"]);
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGTERM, &["1"]);
    let (code, out, err) = h.run_with_cancel(&["disable", "a@x.co"], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (143, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, ["Which account?"]);
    let list = h.json(&["list", "--json"]);
    assert_eq!(
        (
            list["accounts"][0].get("disabled"),
            list["accounts"][1].get("disabled")
        ),
        (None, None),
        "nothing was disabled"
    );

    let h = with_one_login();
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGTERM, &["y"]);
    let (code, out, err) = h.run_with_cancel(
        &["add-token", "sk-ant-api03-key", "--position", "1"],
        &mut ctrl_c,
        &cancel,
    );
    assert_eq!((code, out.as_str(), err.as_str()), (143, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, [REPLACE_A]);
    assert_eq!(h.ok(&["list"]), LIVE_A_OFFLINE);
}

const DELETE_ONE: &str = "Delete 1 displaced credential? It cannot be recovered.";
const NEEDS_YES: &str =
    "displaced credentials cannot be recovered once deleted; pass --yes to delete them";

/// `a@x.co` stored, and a stranger's login that `switch 1 --force` displaced (§9.4 step 2).
/// Returns the entry's ID and its file.
fn with_a_displaced_login() -> (H, String, PathBuf) {
    let h = with_unmanaged_login();
    h.ok(&["switch", "1", "--force"]);
    let v: Value = serde_json::from_str(&h.ok(&["displaced", "--json"])).unwrap();
    let id = v["displaced"][0]["id"].as_str().unwrap().to_owned();
    let file = h
        .env
        .data_dir()
        .join("displaced")
        .join(format!("{id}.json"));
    (h, id, file)
}

#[test]
fn purging_asks_on_a_terminal_and_deletes_only_on_yes() {
    // §6.3: the question defaults to no, so Enter keeps the entry, and so does an explicit no.
    let (h, id, file) = with_a_displaced_login();
    for answer in ["n", ""] {
        let mut declined = Scripted::answering(&[answer]);
        let (code, out, err) = h.run(&["displaced", "--purge", id.as_str()], &mut declined);
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (1, "", "tagteam: cancelled\n"),
            "{answer:?}"
        );
        assert_eq!(declined.asked, [DELETE_ONE]);
        assert!(file.exists(), "{answer:?}");
    }
    let mut yes = Scripted::answering(&["y"]);
    let (code, out, err) = h.run(&["displaced", "--purge", id.as_str()], &mut yes);
    assert_eq!(
        (code, out, err),
        (0, format!("Deleted {id}.\n"), String::new())
    );
    assert_eq!(yes.asked, [DELETE_ONE]);
    assert!(!file.exists());
    assert_eq!(h.ok(&["displaced"]), "No displaced credentials.\n");
}

#[test]
fn purging_never_asks_off_a_terminal_or_under_json() {
    // Review Focus 2's rule for every prompt: a caller that cannot answer is never asked.
    let (h, id, file) = with_a_displaced_login();
    let (code, out, err) = h.run(
        &["displaced", "--purge", id.as_str()],
        &mut Scripted::none(),
    );
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(err, format!("tagteam: {NEEDS_YES}\n"));
    // `--json` never prompts, even on a terminal: Scripted panics on any prompt it has no answer for.
    let (code, out, _) = h.run(
        &["displaced", "--purge", id.as_str(), "--json"],
        &mut Scripted::answering(&[]),
    );
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "needs-confirmation", "message": NEEDS_YES}})
    );
    assert!(file.exists());
    // `--yes` needs nobody to answer.
    let (code, out, _) = h.run(
        &["displaced", "--purge", id.as_str(), "--yes", "--json"],
        &mut Scripted::answering(&[]),
    );
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "ok": true, "deleted": [id]})
    );
    assert!(!file.exists());
}

#[test]
fn an_unknown_id_is_refused_before_anyone_is_asked() {
    // §6.3: every ID is checked first. Scripted panics on any prompt it has no answer for.
    let (h, id, file) = with_a_displaced_login();
    let (code, out, err) = h.run(
        &["displaced", "--purge", id.as_str(), "../../tagteam.db"],
        &mut Scripted::answering(&[]),
    );
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(
        err,
        "tagteam: no displaced credential matches \"../../tagteam.db\"; `tagteam displaced` lists them\n"
    );
    assert!(file.exists());
}

#[test]
fn a_signal_at_the_purge_question_interrupts_and_deletes_nothing() {
    let (h, id, file) = with_a_displaced_login();
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGTERM, &["y"]);
    let (code, out, err) =
        h.run_with_cancel(&["displaced", "--purge", id.as_str()], &mut ctrl_c, &cancel);
    assert_eq!((code, out.as_str(), err.as_str()), (143, "", INTERRUPTED));
    assert_eq!(ctrl_c.asked, [DELETE_ONE]);
    assert!(file.exists());
    let v: Value = serde_json::from_str(&h.ok(&["displaced", "--json"])).unwrap();
    assert_eq!(v["displaced"][0]["id"], json!(id));
    assert_eq!(v["displaced"][0]["recorded"], json!(true));
}

#[test]
fn a_repeated_id_counts_once_in_the_question_and_is_deleted_once() {
    let (h, id, file) = with_a_displaced_login();
    let mut yes = Scripted::answering(&["y"]);
    let (code, out, err) = h.run(
        &["displaced", "--purge", id.as_str(), id.as_str()],
        &mut yes,
    );
    assert_eq!(
        (code, out, err),
        (0, format!("Deleted {id}.\n"), String::new())
    );
    assert_eq!(yes.asked, [DELETE_ONE]);
    assert!(!file.exists());
}

/// §10.5's question, after its summary.
const PURGE_QUESTION: &str = "Delete all of this?";

/// What `with_login_and_key`'s full purge summary says before the question.
const PURGE_SUMMARY: &str = "This deletes, for good:\n  #1  a@x.co\n  #2  api-key-2@token.local\n  the store and the log\nIt never deletes or changes a live login.\n";

/// The accounts `list --json` shows, by email.
fn stored_emails(h: &H) -> Vec<String> {
    h.json(&["list", "--json"])["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["email"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn purge_asks_on_a_terminal_and_only_a_yes_deletes() {
    // §10.5 step 2: the summary and the question, default no, before anything is locked.
    let h = H::with_login_and_key();
    let mut no = Scripted::answering(&[""]);
    let (code, out, err) = h.run(&["purge"], &mut no);
    assert_eq!(
        (code, out.as_str(), err),
        (1, "", format!("{PURGE_SUMMARY}tagteam: cancelled\n"))
    );
    assert_eq!(no.asked, [PURGE_QUESTION]);
    assert_eq!(stored_emails(&h), ["a@x.co", "api-key-2@token.local"]);

    let mut yes = Scripted::answering(&["y"]);
    let (code, out, err) = h.run(&["purge"], &mut yes);
    assert_eq!(code, 0, "{err}");
    assert_eq!(err, PURGE_SUMMARY);
    assert_eq!(
        out,
        "Purged a@x.co (position 1).\nPurged api-key-2@token.local (position 2).\nEmptied the store and deleted the log.\n"
    );
    assert!(stored_emails(&h).is_empty());
}

#[test]
fn purge_without_a_terminal_or_under_json_needs_yes() {
    let h = H::with_login_and_key();
    let (code, out, err) = h.run(&["purge"], &mut Scripted::none());
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            1,
            "",
            "tagteam: purge deletes tagteam's data for good; run it on a terminal to confirm, or pass --yes\n"
        )
    );
    // Even a person at a terminal is not asked under --json.
    let mut nobody_asked = Scripted::answering(&[]);
    let (code, out, _) = h.run(&["purge", "--json"], &mut nobody_asked);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (code, v["error"]["type"].as_str()),
        (1, Some("needs-confirmation"))
    );
    assert!(nobody_asked.asked.is_empty());
    assert_eq!(stored_emails(&h).len(), 2);
    h.ok(&["purge", "--yes"]);
    assert!(stored_emails(&h).is_empty());
}

#[test]
fn a_signal_at_the_purge_question_deletes_nothing() {
    // §14.1, Decision 5: a prompt is a cancellation point, whatever it answered.
    let h = H::with_login_and_key();
    let cancel = Cancel::new();
    let mut ctrl_c = Scripted::interrupted_by(&cancel, libc::SIGINT, &["y"]);
    let (code, out, err) = h.run_with_cancel(&["purge"], &mut ctrl_c, &cancel);
    assert_eq!(
        (code, out.as_str(), err),
        (130, "", format!("{PURGE_SUMMARY}{INTERRUPTED}"))
    );
    assert_eq!(stored_emails(&h).len(), 2);
}

#[test]
fn purge_checks_a_locked_keychain_before_the_question() {
    // §10.5 step 1, Appendix A.3: a person may unlock it; declining fails before the summary.
    let h = H::with_login_and_key();
    h.kc.set_locked(true);
    let mut no_unlock = Scripted::answering(&["n"]);
    let (code, out, err) = h.run(&["purge"], &mut no_unlock);
    assert_eq!(
        (code, out.as_str(), err),
        (1, "", format!("tagteam: {LOCKED}\n"))
    );
    assert_eq!(no_unlock.asked, [UNLOCK]);
    h.kc.set_locked(false);
    assert_eq!(stored_emails(&h).len(), 2);
}

#[test]
fn keychain_orphans_is_named_in_the_summary_only_where_there_is_a_keychain() {
    // §10.5: it deletes `tagteam` Keychain items; Linux keeps its vault in `vault/`.
    let h = H::new();
    let summary = |keychain: &str| {
        format!(
            "This deletes, for good:\n  no account\n  the store and the log\n{keychain}It never deletes or changes a live login.\ntagteam: cancelled\n"
        )
    };
    let mut no = Scripted::answering(&["n"]);
    let (code, _, err) = h.run(&["purge", "--keychain-orphans"], &mut no);
    assert_eq!(
        (code, err),
        (
            1,
            summary(
                "  every `tagteam` Keychain item no account names, for every tagteam data directory on this Mac\n"
            )
        )
    );
    let mut no = Scripted::answering(&["n"]);
    let (code, _, err) = h.run_in(&["purge", "--keychain-orphans"], &mut no, |c| {
        c.platform = Platform::Linux
    });
    assert_eq!((code, err), (1, summary("")));
}

/// `a@x.co` stored at position 1 and live, and the path `export` writes to in `h`'s root.
fn exporting() -> (H, String) {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    let path = h._dir.path().join("backup.age");
    (h, path.to_str().unwrap().to_owned())
}

/// Every file directly in `dir`.
fn files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn an_export_asks_its_passphrase_twice_and_seals_the_file_with_it() {
    let (h, path) = exporting();
    let mut asked = Scripted::answering(&["pw-1", "pw-1"]);
    let (code, out, err) = h.run(&["export", &path], &mut asked);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        asked.asked,
        ["Passphrase for the export: ", "The same passphrase again: "]
    );
    assert_eq!(out, format!("Exported 1 account to {path}, encrypted.\n"));
    assert!(err.contains("hands its OAuth logins over"), "{err}");
    assert!(err.contains("#1 a@x.co is in use here"), "{err}");
    let file = std::fs::read(&path).unwrap();
    assert!(file.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----"));
    assert!(!String::from_utf8_lossy(&file).contains("rt-a"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let opened = transfer::decode(&file, &[], &mut |_: &transfer::Need| {
        Some(transfer::SecretString::from("pw-1"))
    })
    .unwrap();
    assert_eq!(opened.records.len(), 1);
    assert_eq!(
        opened.records[0].credential["claudeAiOauth"]["refreshToken"],
        "rt-a"
    );
}

#[test]
fn a_destination_that_cannot_be_written_refuses_before_any_prompt_or_vault_read() {
    // Review Focus 5: a directory, a directory this user cannot write in, and `-` under
    // `--json` each refuse first. `Scripted::answering(&[])` fails the test on any prompt.
    let (h, _) = exporting();
    let root = h._dir.path();
    let dir = root.to_str().unwrap();
    let (code, _, err) = h.run(&["export", dir], &mut Scripted::answering(&[]));
    assert_eq!(code, 1);
    assert_eq!(
        err,
        format!("tagteam: {dir} is a directory; name a file to export to\n")
    );

    let theirs = root.join("theirs");
    std::fs::create_dir(&theirs).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&theirs, std::fs::Permissions::from_mode(0o500)).unwrap();
    let into = theirs.join("x.age");
    let (code, _, err) = h.run(
        &["export", into.to_str().unwrap()],
        &mut Scripted::answering(&[]),
    );
    std::fs::set_permissions(&theirs, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(code, 1);
    assert!(
        err.starts_with(&format!(
            "tagteam: cannot create the export in {}: ",
            theirs.display()
        )),
        "{err}"
    );

    for args in [&["export", "-", "--json"][..], &["export", "--json"]] {
        let (code, out, _) = h.run(args, &mut Scripted::answering(&[]));
        assert_eq!(code, 2, "{args:?}");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["type"], "usage");
        assert_eq!(
            v["error"]["message"],
            "export - writes the export to stdout, which --json needs for its result; name a file"
        );
    }
    assert!(
        files_in(root).iter().all(|f| !f.contains(".tagteam-")),
        "no temporary file is left: {:?}",
        files_in(root)
    );
}

#[test]
fn the_export_passphrase_must_be_typed_the_same_twice_and_not_be_empty() {
    let (h, path) = exporting();
    for (answers, message) in [
        (&["pw-1", "pw-2"][..], "the two passphrases differ"),
        (
            &[""][..],
            "an empty passphrase protects nothing; pass --plaintext to write the export unencrypted",
        ),
    ] {
        let (code, _, err) = h.run(&["export", &path], &mut Scripted::answering(answers));
        assert_eq!((code, err), (1, format!("tagteam: {message}\n")));
        assert!(!Path::new(&path).exists());
    }
    assert_eq!(
        files_in(h._dir.path()),
        ["home"],
        "no temporary file is left"
    );
}

#[test]
fn without_a_terminal_or_under_json_an_export_needs_a_key_or_plaintext() {
    let (h, path) = exporting();
    let (code, _, err) = h.run(&["export", &path], &mut Scripted::none());
    assert_eq!(code, 1);
    assert_eq!(
        err,
        "tagteam: an export is encrypted with a passphrase typed on a terminal; pass --recipient or --recipient-file to encrypt it to a key, or --plaintext to write it unencrypted\n"
    );
    let (code, out, _) = h.run(&["export", &path, "--json"], &mut Scripted::answering(&[]));
    assert_eq!(code, 1);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["type"], "needs-passphrase");
    assert!(!Path::new(&path).exists());
}

#[test]
fn another_home_imports_a_passphrase_export_asking_for_it_once() {
    let (from, path) = exporting();
    let (code, _, err) = from.run(
        &["export", &path],
        &mut Scripted::answering(&["pw-1", "pw-1"]),
    );
    assert_eq!(code, 0, "{err}");
    let to = H::new();
    let mut asked = Scripted::answering(&["pw-1"]);
    let (code, out, err) = to.run(&["import", &path], &mut asked);
    assert_eq!(code, 0, "{err}");
    assert_eq!(asked.asked, [format!("Passphrase for {path}: ")]);
    assert_eq!(out, "created  #1 a@x.co: added\n");
    assert_eq!(
        to.json(&["list", "--json"])["accounts"][0]["email"],
        "a@x.co"
    );
    let (code, _, err) = to.run(&["import", &path], &mut Scripted::answering(&["wrong"]));
    assert_eq!(
        (code, err.as_str()),
        (
            1,
            "tagteam: the passphrase is wrong, or the file is damaged\n"
        )
    );
}

#[test]
fn a_passphrase_file_is_never_prompted_for_under_json() {
    let (from, path) = exporting();
    let (code, _, err) = from.run(&["export", &path], &mut Scripted::answering(&["pw", "pw"]));
    assert_eq!(code, 0, "{err}");
    let to = H::new();
    let (code, out, _) = to.run(&["import", &path, "--json"], &mut Scripted::answering(&[]));
    assert_eq!(code, 1);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["type"], "needs-passphrase");
    assert!(!to.env.data_dir().exists(), "nothing was created");
}
