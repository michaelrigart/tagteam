//! §15.2: kill the process at each switch step, then prove that the next command recovers it.
//!
//! Fix round 1: the first version of this file only ever looked at the live login and
//! `list --json`'s `activeAccountNumber`, both of which are derived straight from the live
//! state (`views.rs`), so a kill that landed with no recovery at all could still make most of
//! these assertions pass by accident. Every scenario here now also opens the store directly
//! (`Home::assert_settled`/`assert_journal_present`) to check what recovery is actually
//! responsible for: the journal row and the store's active pointer.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, SystemTime};

use assert_cmd::Command;
use common::{login, seed_home};
use serde_json::{Value, json};
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
use tagteam_core::{CLAUDE_CODE, ProviderId};
use tagteam_engine::store::Store;
use tagteam_provider::splice::{get_top_level, remove_top_level};
use tagteam_provider::{Env, FileKeychain, Keychain};

/// The exact top-level keys Claude Code's identity surface may touch in `~/.claude.json`
/// (`tagteam_cc::provider::ClaudeCode::identity_surface`, §3): everything else must be
/// byte-for-byte untouched by any switch or recovery.
const CC_JSON_KEYS: [&str; 3] = ["oauthAccount", "primaryApiKey", "customApiKeyResponses"];

struct Home {
    dir: tempfile::TempDir,
    /// `~/.claude.json` right after `seed_home`, before any account exists: the §3 baseline
    /// every test compares back against once every key is stripped.
    pristine: Vec<u8>,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        seed_home(&Env::for_test(dir.path()));
        let pristine = fs::read(dir.path().join("home/.claude.json")).unwrap();
        Home { dir, pristine }
    }

    fn env(&self) -> Env {
        Env::for_test(self.dir.path())
    }

    fn kc(&self) -> FileKeychain {
        FileKeychain::new(self.dir.path().join("keychain"))
    }

    fn provider(&self) -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn store(&self) -> Store {
        Store::open_existing(&self.env().data_dir().join("tagteam.db"))
            .unwrap()
            .expect("the store must exist by the time a switch or an add has run")
    }

    /// Fix round 1, item 1: what recovery is actually responsible for, not just what the live
    /// login happens to show. The journal is cleared and the store's active pointer names the
    /// account at `position`.
    fn assert_settled(&self, position: u32) {
        let store = self.store();
        let provider = self.provider();
        assert!(
            store.journal(&provider).unwrap().is_none(),
            "the journal row must be cleared once recovery settles the switch"
        );
        let expected = store
            .find_by_position(&provider, position)
            .unwrap()
            .unwrap_or_else(|| panic!("no account stored at position {position}"))
            .id;
        assert_eq!(
            store.active(&provider).unwrap(),
            Some(expected),
            "the store's active account must be position {position}"
        );
    }

    fn assert_journal_present(&self) {
        assert!(
            self.store().journal(&self.provider()).unwrap().is_some(),
            "the journal row must survive an undecidable refusal"
        );
    }

    fn oauth_item(&self) -> (String, String) {
        (
            keychain_service(&self.env(), ItemKind::OAuth),
            keychain_account(&self.env()),
        )
    }

    fn cmd(&self) -> Command {
        common::cmd(self.dir.path())
    }

    fn login(&self, email: &str, rt: &str) {
        login(&self.env(), &self.kc(), email, "", rt);
    }

    /// Simulates Claude Code rotating the live credential on its own, without a `login` (a
    /// new `oauthAccount`): only the Keychain item changes.
    fn set_live_rt(&self, rt: &str) {
        let (svc, acct) = self.oauth_item();
        let cred = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt}});
        self.kc()
            .upsert(&svc, &acct, cred.to_string().as_bytes())
            .unwrap();
    }

    /// Simulates Claude Code logging out: no `oauthAccount`, and both Keychain items gone.
    fn logout(&self) {
        let path = self.env().home.join(".claude.json");
        let doc = fs::read(&path).unwrap();
        fs::write(&path, remove_top_level(&doc, "oauthAccount").unwrap()).unwrap();
        let (oauth_svc, acct) = self.oauth_item();
        self.kc().delete(&oauth_svc, &acct).unwrap();
        self.kc()
            .delete(&keychain_service(&self.env(), ItemKind::ManagedKey), &acct)
            .unwrap();
    }

    fn live_email(&self) -> Option<String> {
        let doc = fs::read(self.env().home.join(".claude.json")).unwrap();
        get_top_level(&doc, "oauthAccount")
            .unwrap()
            .map(|v| v["emailAddress"].as_str().unwrap().to_owned())
    }

    fn keychain_bytes(&self, kind: ItemKind) -> Option<Vec<u8>> {
        let svc = keychain_service(&self.env(), kind);
        let acct = keychain_account(&self.env());
        self.kc().find(&svc, &acct).present()
    }

    /// The OAuth-specific reading the original version of this file used: the live email and
    /// the live refresh token, both from the credential axis.
    fn live_oauth(&self) -> (String, String) {
        let email = self.live_email().expect("a live oauthAccount");
        let bytes = self
            .keychain_bytes(ItemKind::OAuth)
            .expect("a live credential entry");
        let cred: Value = serde_json::from_slice(&bytes).unwrap();
        (
            email,
            cred["claudeAiOauth"]["refreshToken"]
                .as_str()
                .unwrap()
                .to_owned(),
        )
    }

    /// A kind-agnostic version of `live_oauth`: the live email, and that `needle` (the secret
    /// or token text itself) appears in whichever axis it should be on. Works for OAuth and
    /// setup-token entries (JSON-wrapped) and for a raw API key (`write_managed_key` stores the
    /// trimmed key string as-is) alike.
    fn assert_live_on_axis(&self, kind: ItemKind, needle: &str, email: &str) {
        assert_eq!(self.live_email().as_deref(), Some(email));
        let bytes = self
            .keychain_bytes(kind)
            .unwrap_or_else(|| panic!("the {kind:?} axis must hold the live secret"));
        assert!(
            String::from_utf8_lossy(&bytes).contains(needle),
            "expected {needle:?} live on the {kind:?} axis"
        );
    }

    /// A killed process leaves CC's mkdir locks behind; age them past their staleness.
    fn age_cc_locks(&self) {
        let p = CcPaths::resolve(&self.env());
        for lock in [
            p.refresh_lock.clone(),
            p.legacy_lock(),
            p.config_lock.clone(),
        ] {
            if lock.exists() {
                fs::File::open(&lock)
                    .unwrap()
                    .set_modified(SystemTime::now() - Duration::from_secs(120))
                    .unwrap();
            }
        }
    }

    fn two_accounts(&self) {
        self.login("a@x.co", "rt-a");
        self.cmd().arg("add").assert().success();
        self.login("b@x.co", "rt-b");
        self.cmd().arg("add").assert().success();
    }

    /// Two setup-token accounts (§7.1: anything that isn't an API key or a refresh-token-bearing
    /// OAuth blob), both on the credential axis. `add-token` never touches the live login, so a
    /// plain `switch 2` establishes the live baseline the matrix needs, the way `add`'s capture
    /// of an already-live login does for `two_accounts`.
    fn two_setup_tokens(&self) {
        self.cmd()
            .args(["add-token", "sk-ant-oat01-alpha"])
            .assert()
            .success();
        self.cmd()
            .args(["add-token", "sk-ant-oat01-beta"])
            .assert()
            .success();
        self.cmd().args(["switch", "2"]).assert().success();
    }

    /// Two API-key accounts, both on the managed-key axis. See `two_setup_tokens`.
    fn two_api_keys(&self) {
        self.cmd()
            .args(["add-token", "sk-ant-api03-alpha"])
            .assert()
            .success();
        self.cmd()
            .args(["add-token", "sk-ant-api03-beta"])
            .assert()
            .success();
        self.cmd().args(["switch", "2"]).assert().success();
    }

    /// Position 1 is OAuth (credential axis) and already live, exactly like `two_accounts`'s
    /// first account; position 2 is an API key (managed-key axis), never live until switched to.
    fn oauth_then_api_key(&self) {
        self.login("a@x.co", "rt-a");
        self.cmd().arg("add").assert().success();
        self.cmd()
            .args(["add-token", "sk-ant-api03-beta"])
            .assert()
            .success();
    }

    /// Fix round 1, item 6 / §3: nothing outside Claude Code's identity surface in
    /// `~/.claude.json` may change, ever. Strips every key that surface owns from both the
    /// pristine snapshot and the current file and compares the rest byte-for-byte, which is
    /// strictly stronger than the "at minimum `userID` survives" floor the review asked for.
    fn assert_no_foreign_config_writes(&self) {
        let strip = |doc: &[u8]| {
            let mut out = doc.to_vec();
            for key in CC_JSON_KEYS {
                out = remove_top_level(&out, key).unwrap();
            }
            out
        };
        let after = fs::read(self.env().home.join(".claude.json")).unwrap();
        let (before_stripped, after_stripped) = (strip(&self.pristine), strip(&after));
        assert_eq!(
            before_stripped, after_stripped,
            "§3: bytes outside Claude Code's identity surface must never change"
        );
        assert!(
            String::from_utf8_lossy(&after_stripped).contains("\"userID\": \"u\""),
            "the untouched userID must still be present"
        );
    }
}

fn error_kind(json_stdout: &[u8]) -> String {
    let v: Value = serde_json::from_slice(json_stdout).unwrap();
    v["error"]["type"]
        .as_str()
        .unwrap_or_else(|| panic!("no error.type in {v}"))
        .to_owned()
}

/// §15.2's core matrix: crash the binary right after the journal row is inserted, right after
/// the credential write, and right after the identity write, then prove the next mutating
/// command recovers it — through the store (fix round 1, item 1), not only through whatever
/// `verify` can see live, and expecting the exact crash exit code (item 2) so an ordinary
/// pre-journal failure could never pass as a kill.
///
/// A fresh `Home` per point, built by `setup`, keeps the three points independent: reusing one
/// `Home` across points would let an earlier point's closing rotation turn a later point's
/// switch into a self-switch no-op that never reaches the crash at all.
///
/// `setup` must leave `other` live and active (as `two_accounts`, `two_setup_tokens` and
/// `two_api_keys` all do for position 2) and `target` merely stored; the matrix then crashes
/// `switch <target>`, landing on `target` or staying on `other` depending on the point.
///
/// Each point runs under `catch_unwind` so one point's failure doesn't hide the others' — the
/// mutation check (recovery disabled) showed every point's own assertion, not just the first.
fn run_three_point_matrix(
    setup: impl Fn(&Home),
    target: u32,
    other: u32,
    verify: impl Fn(&Home, u32),
) {
    let mut failures = Vec::new();
    for (point, lands) in [
        ("after-journal", false),
        ("after-credential", true),
        ("after-identity", true),
    ] {
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let h = Home::new();
            setup(&h);
            h.cmd()
                .args(["switch", &target.to_string()])
                .env("TAGTEAM_TEST_CRASH_AT", point)
                .assert()
                .code(137);
            h.age_cc_locks();
            // Any mutating command recovers; toggling `other` (never the crash's target) never
            // itself changes which account a fresh rotation would fall back to.
            h.cmd()
                .args(["disable", &other.to_string()])
                .assert()
                .success();
            let expect_position = if lands { target } else { other };
            h.assert_settled(expect_position);
            verify(&h, expect_position);
            h.cmd()
                .args(["enable", &other.to_string()])
                .assert()
                .success();
            h.cmd().arg("switch").assert().success(); // no interrupted switch is left behind
            h.assert_no_foreign_config_writes();
        }));
        if let Err(e) = outcome {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "non-string panic payload".to_owned());
            failures.push(format!("{point}: {msg}"));
        }
    }
    assert!(
        failures.is_empty(),
        "kill matrix failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn a_kill_at_any_step_is_recovered_by_the_next_command() {
    run_three_point_matrix(
        |h| h.two_accounts(),
        1,
        2,
        |h, position| {
            let (email, rt) = h.live_oauth();
            let expected = if position == 1 {
                ("a@x.co", "rt-a")
            } else {
                ("b@x.co", "rt-b")
            };
            assert_eq!((email.as_str(), rt.as_str()), expected);
            let list: Value = serde_json::from_slice(
                &h.cmd()
                    .args(["list", "--json"])
                    .assert()
                    .success()
                    .get_output()
                    .stdout,
            )
            .unwrap();
            assert_eq!(list["activeAccountNumber"], json!(position));
        },
    );
}

#[test]
fn a_kill_at_any_step_is_recovered_for_setup_token_accounts() {
    run_three_point_matrix(
        |h| h.two_setup_tokens(),
        1,
        2,
        |h, position| {
            let (needle, email) = if position == 1 {
                ("sk-ant-oat01-alpha", "setup-token-1@token.local")
            } else {
                ("sk-ant-oat01-beta", "setup-token-2@token.local")
            };
            h.assert_live_on_axis(ItemKind::OAuth, needle, email);
            assert!(
                h.keychain_bytes(ItemKind::ManagedKey).is_none(),
                "a setup-token pair never touches the managed-key axis"
            );
        },
    );
}

#[test]
fn a_kill_at_any_step_is_recovered_for_api_key_accounts() {
    run_three_point_matrix(
        |h| h.two_api_keys(),
        1,
        2,
        |h, position| {
            let (needle, email) = if position == 1 {
                ("sk-ant-api03-alpha", "api-key-1@token.local")
            } else {
                ("sk-ant-api03-beta", "api-key-2@token.local")
            };
            h.assert_live_on_axis(ItemKind::ManagedKey, needle, email);
            assert!(
                h.keychain_bytes(ItemKind::OAuth).is_none(),
                "an API-key pair never touches the credential axis"
            );
        },
    );
}

#[test]
fn a_kill_at_any_step_is_recovered_for_a_cross_kind_pair() {
    run_three_point_matrix(
        |h| h.oauth_then_api_key(),
        2,
        1,
        |h, position| {
            if position == 1 {
                h.assert_live_on_axis(ItemKind::OAuth, "rt-a", "a@x.co");
                assert!(h.keychain_bytes(ItemKind::ManagedKey).is_none());
            } else {
                h.assert_live_on_axis(
                    ItemKind::ManagedKey,
                    "sk-ant-api03-beta",
                    "api-key-2@token.local",
                );
                assert!(h.keychain_bytes(ItemKind::OAuth).is_none());
            }
        },
    );
}

/// Fix round 1, item 4's last bullet: `after-target-axis` sits inside `ClaudeCode`'s own
/// `write_credential`, between writing the target axis and clearing the other one — a boundary
/// the engine's own hooks never see, since the whole call is one atomic step from the engine's
/// side. Crashing there on a cross-kind switch (OAuth -> API key) leaves both axes genuinely
/// populated: the managed-key axis already holds the new key, and the credential axis still
/// holds the outgoing OAuth login untouched.
#[test]
fn a_kill_between_axes_on_a_cross_kind_switch_settles_coherently() {
    let h = Home::new();
    h.oauth_then_api_key();
    h.cmd()
        .args(["switch", "2"])
        .env("TAGTEAM_TEST_CRASH_AT", "after-target-axis")
        .assert()
        .code(137);

    let managed = h
        .keychain_bytes(ItemKind::ManagedKey)
        .expect("the target axis is written before the crash");
    assert!(String::from_utf8_lossy(&managed).contains("sk-ant-api03-beta"));
    let entry = h
        .keychain_bytes(ItemKind::OAuth)
        .expect("the other axis is not cleared yet");
    assert!(String::from_utf8_lossy(&entry).contains("rt-a"));

    h.age_cc_locks();
    h.cmd().args(["disable", "1"]).assert().success(); // any mutating command recovers

    h.assert_settled(2);
    h.assert_live_on_axis(
        ItemKind::ManagedKey,
        "sk-ant-api03-beta",
        "api-key-2@token.local",
    );
    assert!(
        h.keychain_bytes(ItemKind::OAuth).is_none(),
        "recovery must finish clearing the axis the crash left populated"
    );
    h.assert_no_foreign_config_writes();
}

/// Fix round 1, item 4's logout case: nothing is live on either axis, so §9.6's table cannot
/// name either account and the row must stay undecidable until `--force` settles it. Also
/// folds in item 5: `disable` proceeds regardless, and `switch` refuses by JSON error kind.
#[test]
fn a_logout_between_kill_and_recovery_stays_undecidable() {
    let h = Home::new();
    h.two_accounts();
    h.cmd()
        .args(["switch", "1"])
        .env("TAGTEAM_TEST_CRASH_AT", "after-credential")
        .assert()
        .code(137);
    h.age_cc_locks();
    h.logout();

    // Position 1, not the baseline 2, so disabling it does not change which account a later
    // rotation would fall back to.
    h.cmd().args(["disable", "1"]).assert().success(); // proceeds even while undecidable
    h.assert_journal_present();

    let out = h
        .cmd()
        .args(["switch", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(error_kind(&out), "interrupted-switch");
    h.assert_journal_present(); // the refusal itself settles nothing

    h.cmd().args(["switch", "--force"]).assert().success();
    h.assert_settled(2);
    assert_eq!(h.live_oauth(), ("b@x.co".to_owned(), "rt-b".to_owned()));
    h.assert_no_foreign_config_writes();
}

/// Fix round 1, items 3 and 5: the rotated credential a forced switch displaces must actually
/// survive on disk, the superseded journal row must be visible while undecidable, and the
/// commands that refuse it must be told apart from the ones that don't by JSON error kind, not
/// by their human-readable text.
#[test]
fn a_cc_rotation_after_the_kill_needs_force() {
    let h = Home::new();
    h.two_accounts();
    h.cmd()
        .args(["switch", "1"])
        .env("TAGTEAM_TEST_CRASH_AT", "after-credential")
        .assert()
        .code(137);
    h.age_cc_locks();
    h.set_live_rt("rt-a-rotated-by-cc");

    let out = h
        .cmd()
        .args(["switch", "2", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(error_kind(&out), "interrupted-switch");
    h.assert_journal_present(); // item 3: an undecidable refusal never clears the row

    // Item 5: `disable`/`enable` proceed regardless of the undecidable row; `add-token` and
    // `remove` refuse exactly as `switch` did, by the same JSON error kind.
    h.cmd().args(["disable", "2"]).assert().success();
    h.cmd().args(["enable", "2"]).assert().success();
    let add_out = h
        .cmd()
        .args(["add-token", "sk-ant-api03-extra", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(error_kind(&add_out), "interrupted-switch");
    let remove_out = h
        .cmd()
        .args(["remove", "1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(error_kind(&remove_out), "interrupted-switch");
    h.assert_journal_present();

    h.cmd().args(["switch", "2", "--force"]).assert().success();
    h.assert_settled(2);
    assert_eq!(h.live_oauth(), ("b@x.co".to_owned(), "rt-b".to_owned()));

    // Item 3: the rotated credential the force displaced must actually be on disk, not just
    // named in a warning.
    let displaced_dir = h.env().data_dir().join("displaced");
    let survived = fs::read_dir(&displaced_dir).unwrap().any(|entry| {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        String::from_utf8_lossy(&bytes).contains("rt-a-rotated-by-cc")
    });
    assert!(
        survived,
        "the credential Claude Code rotated in must survive as a displaced file"
    );
    h.assert_no_foreign_config_writes();
}
