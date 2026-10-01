mod common;

use std::collections::VecDeque;
use std::path::Path;
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
use tagteam_provider::{Env, FakeKeychain};

const UNLOCK: &str = "The login keychain is locked (common over SSH). Unlock it now?";

struct Scripted {
    interactive: bool,
    answers: VecDeque<&'static str>,
    asked: Vec<String>,
}

impl Scripted {
    fn none() -> Self {
        Self {
            interactive: false,
            answers: VecDeque::new(),
            asked: Vec::new(),
        }
    }
    fn answering(a: &[&'static str]) -> Self {
        Self {
            interactive: true,
            answers: a.iter().copied().collect(),
            asked: Vec::new(),
        }
    }
}

impl Prompter for Scripted {
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn confirm(&mut self, q: &str, default_yes: bool) -> bool {
        self.asked.push(q.to_owned());
        match self.answers.pop_front().expect("unexpected prompt") {
            "" => default_yes,
            a => a.starts_with('y'),
        }
    }
    fn choose(&mut self, q: &str, _o: &[String]) -> Option<usize> {
        self.asked.push(q.to_owned());
        self.answers
            .pop_front()
            .expect("unexpected prompt")
            .parse()
            .ok()
    }
    fn secret(&mut self, q: &str) -> Option<String> {
        self.asked.push(q.to_owned());
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

    /// Runs `tagteam <args>` in-process, its output not a terminal. `--no-color` is always
    /// added: a `FORCE_COLOR` set around this test process must not colour the output the
    /// tests compare.
    fn run(&self, args: &[&str], prompter: &mut Scripted) -> (i32, String, String) {
        self.run_as(args, prompter, false, true)
    }

    /// `run`, with the output a terminal or not and `--no-color` or not.
    fn run_as(
        &self,
        args: &[&str],
        prompter: &mut Scripted,
        stdout_terminal: bool,
        no_color: bool,
    ) -> (i32, String, String) {
        let argv = std::iter::once("tagteam")
            .chain(args.iter().copied())
            .chain(no_color.then_some("--no-color"));
        let cli = Cli::try_parse_from(argv).unwrap();
        let ctx = Context {
            env: self.env.clone(),
            keychain: self.kc.clone(),
            platform: Platform::MacOs,
            api_base: Some(common::OFFLINE_API_BASE.into()),
            stdout_terminal,
        };
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
    let list = |terminal: bool, no_color: bool| {
        let (code, out, err) = h.run_as(&["list"], &mut Scripted::none(), terminal, no_color);
        assert_eq!((code, err.as_str()), (0, ""));
        out
    };
    assert!(!list(false, false).contains('\x1b'), "not a terminal");
    assert!(list(true, false).contains(YELLOW_77), "a terminal");
    assert!(!list(true, true).contains('\x1b'), "--no-color wins");
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
