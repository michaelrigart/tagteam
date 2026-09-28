mod common;

use std::collections::VecDeque;
use std::sync::Arc;

use clap::Parser;
use common::{LOCKED, login, seed_home};
use serde_json::{Value, json};
use tagteam::app::{self, Context, Io};
use tagteam::cli::Cli;
use tagteam::prompt::Prompter;
use tagteam_cc::live::Platform;
use tagteam_cc::{ItemKind, keychain_service};
use tagteam_provider::{Env, FakeKeychain};

const UNLOCK: &str = "The login keychain is locked (common over SSH). Unlock it now?";
/// A switch on macOS whose Keychain write fell back to Claude Code's credentials file.
const FELL_BACK: &str = "warning: the Keychain could not be written, so the credential went to Claude Code's credentials file instead\n";

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

    fn run(&self, args: &[&str], prompter: &mut Scripted) -> (i32, String, String) {
        let cli =
            Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied())).unwrap();
        let ctx = Context {
            env: self.env.clone(),
            keychain: self.kc.clone(),
            platform: Platform::MacOs,
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

    fn json(&self, args: &[&str]) -> Value {
        let mut v: Value = serde_json::from_str(&self.ok(args)).unwrap();
        normalize_ids(&mut v);
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
           "lastGoodAgeSeconds": null})
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

fn normalize_ids(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if k == "id" && x.is_string() {
                    *x = json!("[id]");
                } else {
                    normalize_ids(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize_ids),
        _ => {}
    }
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
    assert_eq!(h.ok(&["list"]), "  1  work (a@x.co)\n* 2  b@x.co\n");
    assert_eq!(h.ok(&["status"]), "Live: b@x.co (position 2 of 2)\n");
    assert_eq!(
        h.ok(&["switch", "work"]),
        "Switched to work (a@x.co) (position 1).\nClaude Code picks this up within about 30 s; restart it to apply now.\n"
    );
    assert_eq!(h.ok(&["ls"]), "* 1  work (a@x.co)\n  2  b@x.co\n");
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
            "accounts": [a_row(1, true), with(key_row(2), json!({"alias": "ci", "disabled": true}))]
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
    assert_eq!(
        h.json(&["status", "--json"]),
        json!({"schemaVersion": 1, "provider": "claude-code",
               "active": with(a_row(1, true), json!({"managed": true})), "totalManagedAccounts": 2})
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

#[test]
fn a_keychain_write_that_falls_back_to_the_file_is_reported() {
    // Appendix A.3: the credential went to Claude Code's credentials file instead.
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("b@x.co", "rt-b");
    h.ok(&["add"]);
    let live_item = keychain_service(&h.env, ItemKind::OAuth);
    h.kc.set_fail_write(&live_item, true);
    let (code, out, err) = h.run(&["switch", "1"], &mut Scripted::none());
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "Switched to a@x.co (position 1).\nActive on your next message.\n",
            FELL_BACK
        )
    );
    let (code, out, err) = h.run(&["switch", "2", "--json"], &mut Scripted::none());
    assert_eq!((code, err.as_str()), (0, FELL_BACK));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (v["switched"].clone(), v["credentialStore"].clone()),
        (json!(true), json!("file"))
    );
    // Once the Keychain takes the write again, nothing is reported.
    h.kc.set_fail_write(&live_item, false);
    let (code, out, err) = h.run(&["switch", "1", "--json"], &mut Scripted::none());
    assert_eq!((code, err.as_str()), (0, ""));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["credentialStore"], "keychain");
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
    assert_eq!(h.ok(&["list"]), "  1  a@x.co\n");
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
    assert_eq!(h.ok(&["list"]), "* 1  stranger@x.co\n");
    let (code, _, err) = h.run(&["add-token"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert!(err.contains("`-`"), "{err}");
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
    assert_eq!(h.ok(&["list"]), "* 1  a@x.co\n");
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
