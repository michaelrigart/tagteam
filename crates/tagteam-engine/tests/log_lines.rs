//! §14.2's lines from the engine's central sites (Decision 10): every `events` row, every
//! vault generation written, a rescue, a displacement, a recovery decision, a switch's
//! rollback and every refresh outcome, and the status bar's DEBUG ceiling. Each names its
//! accounts by ID, never by email (B.35).
mod common;

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use common::{Fx, capture_logs, crashed_switch, due, vault_fp, write_target_credential};
use serde_json::json;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::active::{ActiveOutcome, ActiveTrigger};
use tagteam_engine::export::ExportRequest;
use tagteam_engine::transfer::ImportRecord;
use tagteam_engine::vault::SERVICE;
use tagteam_engine::views::StatuslineView;
use tagteam_provider::Clock;
use tagteam_provider::http::{HttpError, Method};

/// One test at a time: each captures through a subscriber of its own, and `tracing` caches
/// whether a call site is enabled across every subscriber alive in the process, so a test
/// running beside another can find a call site cached as disabled and capture nothing.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The lines among `logs` at `level` whose message contains `message`.
fn at<'a>(logs: &'a [String], level: &str, message: &str) -> Vec<&'a str> {
    logs.iter()
        .map(|l| l.trim_start())
        .filter(|l| l.starts_with(level) && l.contains(message))
        .collect()
}

/// The one INFO line whose message contains `message`.
fn one<'a>(logs: &'a [String], message: &str) -> &'a str {
    let found = at(logs, "INFO", message);
    assert_eq!(found.len(), 1, "one {message:?} line in {logs:#?}");
    found[0]
}

/// The value of `key` in `line`, as written: a string field keeps its quotes.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(' ')
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn no_email(logs: &[String]) {
    assert!(
        logs.iter().all(|l| !l.contains("@x.co")),
        "an email was logged: {logs:#?}"
    );
}

#[test]
fn an_add_logs_the_generation_it_stored_and_its_event() {
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let (a, logs) = capture_logs(|| fx.add("a@x.co", "rt-a"));
    let stored = one(&logs, "stored a credential in the vault");
    let fp = vault_fp(&fx, &a); // `sha256:` and 64 hex digits; the line keeps the first 12
    assert_eq!(
        [
            field(stored, "account"),
            field(stored, "fp"),
            field(stored, "new_generation"),
        ],
        [Some(a.as_str()), Some(&fp[7..19]), Some("true")],
        "{stored}"
    );
    let event = one(&logs, "event recorded");
    assert_eq!(
        [
            field(event, "kind"),
            field(event, "to_account"),
            field(event, "from_account"),
            field(event, "source"),
        ],
        [Some("\"add\""), Some(a.as_str()), None, Some("\"cli\"")],
        "{event}"
    );
    no_email(&logs);
    no_token(&logs);
}

#[test]
fn a_switch_logs_its_event_naming_both_accounts() {
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let (out, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap());
    assert!(out.switched, "{}", out.message);
    let event = one(&logs, "event recorded");
    assert_eq!(
        [
            field(event, "kind"),
            field(event, "from_account"),
            field(event, "to_account"),
            field(event, "trigger"),
            field(event, "source"),
        ],
        [
            Some("\"switch\""),
            Some(b.as_str()),
            Some(a.as_str()),
            Some("\"manual\""),
            Some("\"cli\""),
        ],
        "{event}"
    );
    no_email(&logs);
    no_token(&logs);
}

#[test]
fn a_rescue_is_logged_by_account_and_fingerprint() {
    // The vault refuses the refreshed successor, so `rescue/` keeps it (§6.3).
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = due(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    let (_, logs) = capture_logs(|| {
        fx.engine
            .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
            .unwrap()
    });
    fx.kc.set_fail_write(SERVICE, false);
    let line = one(&logs, "kept a refreshed token in rescue/");
    assert_eq!(field(line, "account"), Some(a.as_str()), "{line}");
    let fp = field(line, "fp").unwrap_or_default();
    assert!(
        fp.len() == 12 && fp.bytes().all(|b| b.is_ascii_hexdigit()),
        "{line}"
    );
    no_email(&logs);
    no_token(&logs);
}

#[test]
fn a_displacement_is_logged_without_the_login_it_displaced() {
    // A forced switch over an unmanaged login saves that login's credential (§6.3, §9.2).
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    let (out, logs) = capture_logs(|| fx.switch_to(&a, true).unwrap());
    assert!(out.switched, "{}", out.message);
    let line = one(&logs, "saved a credential to displaced/");
    let id = field(line, "displaced").unwrap_or_default();
    assert!(
        fx.env
            .data_dir()
            .join("displaced")
            .join(format!("{id}.json"))
            .exists(),
        "{line}"
    );
    assert_eq!(
        field(line, "reason"),
        Some("\"forced-activation\""),
        "{line}"
    );
    assert!(logs.iter().all(|l| !l.contains("stranger")), "{logs:#?}");
    no_email(&logs);
    no_token(&logs);
}

#[test]
fn a_recovery_logs_which_way_it_went() {
    // §9.6: the live credential decides. Landed, the switch finishes forward and commits a
    // `switch-recovered` row; not landed, it finishes backward and records none.
    let _serial = one_at_a_time();
    for (landed, direction) in [(true, "\"forward\""), (false, "\"backward\"")] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        crashed_switch(&fx, &b, &a);
        if landed {
            write_target_credential(&fx, &a);
        }
        let (_, logs) = capture_logs(|| drop(fx.engine.mutation_guard().unwrap()));
        let line = one(&logs, "recovering an interrupted switch");
        assert_eq!(
            [
                field(line, "direction"),
                field(line, "from_account"),
                field(line, "to_account"),
            ],
            [Some(direction), Some(b.as_str()), Some(a.as_str())],
            "{line}"
        );
        let recovered = at(&logs, "INFO", "event recorded")
            .iter()
            .any(|l| field(l, "kind") == Some("\"switch-recovered\""));
        assert_eq!(recovered, landed, "{logs:#?}");
        no_email(&logs);
        no_token(&logs);
    }
}

#[test]
fn an_unverified_capture_names_the_account_by_id_and_position() {
    // §9.4 step 4's WARN line, in §14.2's one spelling: `account=<id> position=<n>`. Claude
    // Code rotated b in place, and the fixture's oracle has no answer.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2");
    let (out, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap());
    assert!(out.switched, "{}", out.message);
    let found = at(&logs, "WARN", "captured an unverified live credential");
    assert_eq!(found.len(), 1, "{logs:#?}");
    assert_eq!(
        (field(found[0], "account"), field(found[0], "position")),
        (Some(b.as_str()), Some("2")),
        "{}",
        found[0]
    );
    no_email(&logs);
    no_token(&logs);
}

/// The lines one gate call on `id` logs, with the vault's bytes as the caller's snapshot.
fn gate_logs(fx: &Fx, id: &AccountId) -> Vec<String> {
    let snapshot = fx.vault_bytes(id).unwrap();
    capture_logs(|| {
        fx.engine
            .refresh_stored(fx.cc.as_ref(), id, &snapshot)
            .unwrap()
    })
    .1
}

/// The one line among `logs` whose message contains `message`, at whichever level.
fn only<'a>(logs: &'a [String], message: &str) -> &'a str {
    let found: Vec<&str> = logs
        .iter()
        .map(|l| l.trim_start())
        .filter(|l| l.contains(message))
        .collect();
    assert_eq!(found.len(), 1, "one {message:?} line in {logs:#?}");
    found[0]
}

/// No line holds any part of the fixtures' tokens: refresh tokens are `rt-…` and access
/// tokens `at-…` (`at-rt-a2`, `at-same`).
fn no_token(logs: &[String]) {
    assert!(
        logs.iter()
            .all(|l| !l.contains("rt-") && !l.contains("at-")),
        "a token was logged: {logs:#?}"
    );
}

#[test]
fn a_gate_refresh_logs_its_outcome_once_at_its_level() {
    // §14.2: refresh outcomes are INFO once a request was sent or state changed, and a gate
    // that did neither logs at DEBUG. No line holds a token, or a systemic refusal's own words.
    let _serial = one_at_a_time();
    let gate = "refresh gate outcome";

    // The request was sent, and its successor stored.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let logs = gate_logs(&fx, &a);
    let line = only(&logs, gate);
    assert!(line.starts_with("INFO"), "{line}");
    assert_eq!(
        (field(line, "account"), field(line, "outcome")),
        (Some(a.as_str()), Some("\"refreshed\"")),
        "{line}"
    );
    no_token(&logs);

    // The request was sent and its reply never came.
    let fx = Fx::new();
    let a = due(&fx);
    fx.http.push(
        Method::Post,
        &Fx::endpoints().token,
        Err(HttpError::Ambiguous("timed out reading the reply".into())),
    );
    let logs = gate_logs(&fx, &a);
    let line = only(&logs, gate);
    assert!(line.starts_with("INFO"), "{line}");
    assert_eq!(
        [
            field(line, "outcome"),
            field(line, "kind"),
            field(line, "rescued"),
        ],
        [Some("\"transient\""), Some("\"ambiguous\""), Some("false")],
        "{line}"
    );
    assert!(!line.contains("timed out"), "{line}");
    no_token(&logs);

    // The endpoint refused the request itself, in words that name the account.
    let fx = Fx::new();
    let a = due(&fx);
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        400,
        json!({"error": "invalid_client", "error_description": "no client for a@x.co"}),
    );
    let logs = gate_logs(&fx, &a);
    let line = only(&logs, gate);
    assert!(line.starts_with("INFO"), "{line}");
    assert_eq!(field(line, "outcome"), Some("\"systemic\""), "{line}");
    assert!(logs.iter().all(|l| !l.contains("no client")), "{logs:#?}");
    no_email(&logs);
    no_token(&logs);

    // Another process holds the account lock: nothing is sent, nothing changes.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let _held = AccountLock::acquire(&fx.env, &a, Duration::ZERO).unwrap();
    let logs = gate_logs(&fx, &a);
    let line = only(&logs, gate);
    assert!(line.starts_with("DEBUG"), "{line}");
    assert_eq!(field(line, "outcome"), Some("\"busy\""), "{line}");
}

#[test]
fn an_active_token_refresh_logs_its_outcome_by_provider_and_account() {
    // §14.2 and §7.5: the live token's refresh is INFO once it sent a request; one that found
    // the token still fresh, and changed nothing, logs at DEBUG.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let mut live = fx.live_credential().unwrap();
    live["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
    fx.set_live_credential(live.to_string().as_bytes());
    fx.script_refresh(Some("rt-a2"));
    let active = "active-token refresh outcome";

    let (out, logs) = capture_logs(|| {
        fx.engine
            .refresh_active(&fx.provider(), ActiveTrigger::Expired)
            .unwrap()
    });
    assert_eq!(out, ActiveOutcome::Refreshed);
    let line = one(&logs, active);
    assert_eq!(
        [
            field(line, "provider"),
            field(line, "account"),
            field(line, "outcome"),
        ],
        [Some("claude-code"), Some(a.as_str()), Some("\"refreshed\"")],
        "{line}"
    );
    no_email(&logs);
    no_token(&logs);

    let (out, logs) = capture_logs(|| {
        fx.engine
            .refresh_active(&fx.provider(), ActiveTrigger::Expired)
            .unwrap()
    });
    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: false });
    let line = only(&logs, active);
    assert!(line.starts_with("DEBUG"), "{line}");
    assert_eq!(field(line, "outcome"), Some("\"not-needed\""), "{line}");
    no_email(&logs);
    no_token(&logs);
}

#[test]
fn an_unreadable_usage_row_is_a_warning_for_a_command_and_a_debug_line_for_the_status_bar() {
    // §14.2: everything `statusline` does logs at DEBUG at most, so a status bar that cannot
    // read the live account's usage never opens the log. An account command's result logs the
    // same failure at WARN (§14: a contained error is logged with its cause).
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO usage_state (account_id, fetched_at) VALUES (?1, 'soon')",
            [a.as_str()],
        )
        .unwrap();
    let usage = "could not read the account's usage";

    let (view, logs) = capture_logs(|| fx.engine.statusline(&fx.provider()).unwrap());
    assert!(matches!(view, StatuslineView::Managed { .. }));
    let line = only(&logs, usage);
    assert!(line.starts_with("DEBUG"), "{line}");
    assert_eq!(
        (field(line, "account"), field(line, "kind")),
        (Some(a.as_str()), Some("\"store\"")),
        "{line}"
    );
    assert!(
        logs.iter()
            .all(|l| l.trim_start().starts_with("DEBUG") || l.trim_start().starts_with("TRACE")),
        "the status bar logs at DEBUG at most: {logs:#?}"
    );

    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    let (_, logs) = capture_logs(|| fx.engine.account_view(row, true));
    assert!(only(&logs, usage).starts_with("WARN"), "{logs:#?}");
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use tagteam_cc::{ItemKind, keychain_service};
    use tagteam_engine::EngineError;

    use super::*;

    #[test]
    fn a_rolled_back_switch_names_its_accounts_and_its_cause_s_kind() {
        // §14.2: switches and their rollbacks. An error after the switch wrote a's credential
        // puts every byte back; the line names the cause by its kind, never by its text, which
        // can hold a label.
        let _serial = one_at_a_time();
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.engine.fail_at(Some("after-credential"));
        let (err, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap_err());
        assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
        let found = at(&logs, "WARN", "rolled back a switch");
        assert_eq!(found.len(), 1, "{logs:#?}");
        assert_eq!(
            [
                field(found[0], "provider"),
                field(found[0], "from_account"),
                field(found[0], "to_account"),
                field(found[0], "kind"),
            ],
            [
                Some("claude-code"),
                Some(b.as_str()),
                Some(a.as_str()),
                Some("\"invalid-input\""),
            ],
            "{}",
            found[0]
        );
        assert!(
            logs.iter().all(|l| !l.contains("injected failure")),
            "the cause's text: {logs:#?}"
        );
        no_email(&logs);
        no_token(&logs);
    }

    #[test]
    fn a_rollback_that_fails_too_is_an_error_naming_what_it_left() {
        // §9.4 step 10: the credential undo cannot write b's credential back, so the journal
        // row stays for recovery (§9.6), and the line says so.
        let _serial = one_at_a_time();
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let kc = fx.kc.clone();
        let svc = keychain_service(&fx.env, ItemKind::OAuth);
        fx.engine.on_point(
            "after-identity",
            Box::new(move || kc.set_fail_write(&svc, true)),
        );
        fx.engine.fail_at(Some("after-identity"));
        let (err, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap_err());
        assert!(matches!(err, EngineError::RollbackFailed { .. }), "{err}");
        let found = at(&logs, "ERROR", "a switch was not fully rolled back");
        assert_eq!(found.len(), 1, "{logs:#?}");
        // A count, never the entries (§14.2): each names a Keychain service or a file path,
        // and an error's text.
        assert_eq!(field(found[0], "failed"), Some("1"), "{}", found[0]);
        assert!(
            !found[0].contains("restore the live credential"),
            "{}",
            found[0]
        );
        assert_eq!(
            field(found[0], "kind"),
            Some("\"invalid-input\""),
            "{}",
            found[0]
        );
        assert!(
            logs.iter().all(|l| !l.contains("injected failure")),
            "the cause's text: {logs:#?}"
        );
        no_email(&logs);
        no_token(&logs);
    }
}

#[test]
fn a_purge_logs_each_entry_it_deleted_by_its_id() {
    // §14.2 and Decision 10: a purge is a state change a user may need to reconstruct. Its
    // line names the entry's own ID, never the identity the entry was attributed to.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    fx.switch_to(&a, true).unwrap();
    let id = fx.engine.displaced().unwrap().entries[0].id.clone();
    let (_, logs) = capture_logs(|| fx.engine.purge_displaced(&[id.clone()]).unwrap());
    let line = one(&logs, "deleted a displaced credential");
    assert_eq!(field(line, "displaced"), Some(id.as_str()), "{line}");
    assert!(logs.iter().all(|l| !l.contains("stranger")), "{logs:#?}");
}

#[test]
fn a_cleared_quarantine_is_logged_as_one_unquarantine_event() {
    // §7.4, §14.2: the switch's outgoing capture clears it, and its event is logged once.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    fx.quarantine(&b, "invalid_grant", &vault_fp(&fx, &b));
    fx.rotate_live("rt-b2");
    let (_, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap());
    let cleared: Vec<&str> = at(&logs, "INFO", "event recorded")
        .into_iter()
        .filter(|l| field(l, "kind") == Some("\"unquarantine\""))
        .collect();
    assert_eq!(cleared.len(), 1, "{logs:#?}");
    assert_eq!(field(cleared[0], "to_account"), Some(b.as_str()));
    no_email(&logs);
}

#[test]
fn a_purge_logs_each_account_and_its_totals() {
    // §14.2: INFO names each account purged by ID and position, then the totals.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let plan = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    let (report, logs) = capture_logs(|| fx.engine.purge(&plan).unwrap());
    assert_eq!(report.accounts.len(), 1);
    let purged = one(&logs, "purged an account");
    assert_eq!(
        [field(purged, "account"), field(purged, "position")],
        [Some(a.as_str()), Some("1")],
        "{purged}"
    );
    let done = one(&logs, "purge finished");
    assert_eq!(
        [field(done, "accounts"), field(done, "failures")],
        [Some("1"), Some("0")],
        "{done}"
    );
    no_email(&logs);
}

#[test]
fn an_export_logs_each_account_by_id_and_position_and_never_its_login() {
    // §14.2: an export is logged by position, never by its contents.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", "sha256:old");
    let (_, logs) = capture_logs(|| fx.engine.export(&ExportRequest::default()).unwrap());
    let line = one(&logs, "exported an account");
    assert_eq!(
        [
            field(line, "account"),
            field(line, "position"),
            field(line, "source")
        ],
        [Some(b.as_str()), Some("2"), Some("\"vault\"")],
        "{line}"
    );
    let skipped = at(&logs, "WARN", "an account was not exported");
    assert_eq!(skipped.len(), 1, "{logs:#?}");
    assert_eq!(
        [field(skipped[0], "account"), field(skipped[0], "kind")],
        [Some(a.as_str()), Some("\"account-broken\"")],
        "{}",
        skipped[0]
    );
    no_email(&logs);
    assert!(logs.iter().all(|l| !l.contains("rt-")), "{logs:#?}");
}

#[test]
fn an_import_logs_each_account_by_id_and_position_and_never_its_login() {
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let record = ImportRecord {
        provider: ProviderId::new(CLAUDE_CODE),
        position: 3,
        kind: Some("oauth".into()),
        label: None,
        alias: None,
        disabled: false,
        added_at: None,
        identity: json!({"oauthAccount": Fx::oauth_account("c@x.co")}),
        credential: json!({"claudeAiOauth": {"accessToken": "at-c", "refreshToken": "rt-c"}}),
    };
    let (_, logs) = capture_logs(|| fx.engine.import(vec![record], false).unwrap());
    let c = fx.engine.resolve("c@x.co", None).unwrap();
    let line = one(&logs, "imported an account");
    assert_eq!(
        [
            field(line, "account"),
            field(line, "position"),
            field(line, "outcome")
        ],
        [Some(c.id.as_str()), Some("3"), Some("\"created\"")],
        "{line}"
    );
    no_email(&logs);
    assert!(logs.iter().all(|l| !l.contains("rt-")), "{logs:#?}");
}

#[test]
fn a_keychain_delete_that_fails_is_logged_with_its_cause() {
    // §14: the probe after the delete decides, and `remove` reports the item left; the line
    // says why the delete failed, naming the item's service and no account.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let (svc, acct) = fx.profile_item(&dir);
    fx.kc
        .put(&svc, &acct, &common::credential("a@x.co", "rt-a2"));
    fx.kc.set_fail_delete(&svc, true);
    let (removed, logs) = capture_logs(|| fx.engine.remove(&a));
    assert!(removed.is_err());
    let failed = at(&logs, "WARN", "could not delete a Keychain item");
    assert_eq!(failed.len(), 1, "{logs:#?}");
    // By `security`'s exit status, never its message (§14.2).
    assert_eq!(field(failed[0], "rc"), Some("25"), "{}", failed[0]);
    assert!(
        failed[0].contains(&svc) && !failed[0].contains("injected"),
        "{}",
        failed[0]
    );
    no_email(&logs);
}

/// A fixture whose data directory sits under a name with an email in it (§14.2).
fn named_home() -> Fx {
    Fx::with(tagteam_cc::live::Platform::MacOs, |e| {
        e.xdg_data_home = Some(e.home.join("alice@example.com/data"));
    })
}

/// Asserts that no line names the data directory's email, nor `bob@example.com`, which the
/// tests write into the files they break.
fn none_named(logs: &[String]) {
    for name in ["alice@example.com", "bob@example.com"] {
        assert!(
            logs.iter().all(|l| !l.contains(name)),
            "{name} in {logs:#?}"
        );
    }
    no_email(logs);
}

#[test]
fn a_keychain_write_that_falls_back_is_logged_by_its_exit_status() {
    // §14.2: `security`'s message is another program's text; its exit status is the cause.
    let _serial = one_at_a_time();
    let fx = named_home();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.kc.set_fail_write(
        &tagteam_cc::keychain_service(&fx.env, tagteam_cc::ItemKind::OAuth),
        true,
    );
    let (switched, logs) = capture_logs(|| fx.switch_to(&a, false));
    assert!(switched.unwrap().switched);
    let line = at(
        &logs,
        "WARN",
        "keychain write failed, falling back to the credentials file",
    );
    assert_eq!(line.len(), 1, "{logs:#?}");
    assert_eq!(field(line[0], "rc"), Some("25"), "{}", line[0]);
    assert!(logs.iter().all(|l| !l.contains("injected")), "{logs:#?}");
    none_named(&logs);
}

#[test]
fn a_replacement_whose_vault_write_and_rollback_both_fail_logs_the_rollback_by_kind() {
    // §14, §14.2: `add` reports the vault write's error; the rollback's own failure is logged
    // by its kind and the account's ID and position, never by an error text that may carry a
    // label.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("a@x.co", "rt-a2");
    fx.kc
        .set_unreadable(tagteam_engine::vault::SERVICE, a.as_str(), true);
    let (added, logs) = capture_logs(|| fx.engine.add_live(fx.add_options()));
    assert!(added.is_err());
    let line = at(&logs, "WARN", "could not roll back a replacement");
    assert_eq!(line.len(), 1, "{logs:#?}");
    assert_eq!(
        [
            field(line[0], "account"),
            field(line[0], "position"),
            field(line[0], "kind")
        ],
        [Some(a.as_str()), Some("1"), Some("\"unreadable\"")],
        "{}",
        line[0]
    );
    no_email(&logs);
}

#[test]
fn purge_logs_no_path_of_an_orphaned_profile_or_of_the_data_directory() {
    // §14.2: the data directory sits under a name the user chose, and an entry of `sessions/`
    // may carry any name. Purge's lines name an entry by its account's ID when its name is one
    // tagteam makes, and otherwise as an unrecognized entry: never a path. A provider is named
    // only when this build registers it: a marker or a row may say anything.
    let _serial = one_at_a_time();
    let fx = Fx::with(tagteam_cc::live::Platform::MacOs, |e| {
        e.xdg_data_home = Some(e.home.join("alice@example.com/data"));
    });
    let a = fx.add("a@x.co", "rt-a");
    let sessions = fx.env.data_dir().join("sessions");
    assert!(sessions.to_string_lossy().contains("alice@example.com"));
    let unrecognized = sessions.join("bob@example.com");
    fx.write_marker(
        &unrecognized,
        &AccountId::from_string("bob@example.com"),
        &fx.env,
    );
    let gone = uuid::Uuid::now_v7().to_string();
    let recognized = sessions.join(&gone);
    fx.write_marker(&recognized, &AccountId::from_string(gone.as_str()), &fx.env);
    let foreign = AccountId::from_string(uuid::Uuid::now_v7().to_string().as_str());
    mark(
        &sessions.join(foreign.as_str()),
        "alice@example.com",
        &foreign,
    );
    // An account whose row names that provider, with its profile, and a switch of it whose
    // holder died.
    let alien = ProviderId::new("alice@example.com");
    let ghost = common::add(
        &fx.engine.store().unwrap(),
        &alien,
        &uuid::Uuid::now_v7().to_string(),
        "g@x.co",
        1,
    );
    fx.put_vault(&ghost, b"a credential only its provider can read");
    mark(&fx.profile_dir(&ghost), alien.as_str(), &ghost);
    fx.engine
        .store()
        .unwrap()
        .insert_journal(&tagteam_engine::store::JournalRow {
            provider: alien.clone(),
            holder: common::dead_holder(),
            from_id: None,
            to_id: ghost.clone(),
            from_fp: None,
            from_identity: None,
            to_fp: "sha256:00".into(),
            to_epoch: None,
            started_at: 1,
            prior: None,
        })
        .unwrap();
    // A dangling entry: no marker, and no path to resolve.
    std::os::unix::fs::symlink(sessions.join("nowhere"), sessions.join("carol@example.com"))
        .unwrap();
    // A session record that cannot be read makes the first purge refuse, with its warning,
    // after the guard dealt with the dead switch.
    let record = fx.live_record(&unrecognized, 778, "interactive");
    std::fs::write(&record, "{\"pid\": ").unwrap();
    let plan = fx.engine.purge_plan(None).unwrap();
    let (refused, logs) = capture_logs(|| fx.engine.purge(&plan));
    assert_eq!(refused.unwrap_err().kind(), "session-owned");
    assert_eq!(
        at(
            &logs,
            "WARN",
            "of an unrecognized profile entry could not be read"
        )
        .len(),
        1,
        "{logs:#?}"
    );
    let unrecovered = at(&logs, "WARN", "could not recover an interrupted switch");
    assert_eq!(unrecovered.len(), 1, "{logs:#?}");
    assert_eq!(
        [
            field(unrecovered[0], "provider"),
            field(unrecovered[0], "kind")
        ],
        [None, Some("\"unknown-provider\"")],
        "{logs:#?}"
    );
    // The refused purge deleted nothing: the record goes only once nothing refuses.
    assert!(
        at(
            &logs,
            "WARN",
            "purge deleted an interrupted switch's record"
        )
        .is_empty(),
        "{logs:#?}"
    );
    let mut all = logs;
    std::fs::remove_file(&record).unwrap();
    let plan = fx.engine.purge_plan(None).unwrap();
    let (purged, logs) = capture_logs(|| fx.engine.purge(&plan));
    assert!(purged.unwrap().failures.is_empty());
    let dropped = at(
        &logs,
        "WARN",
        "purge deleted an interrupted switch's record",
    );
    assert_eq!(dropped.len(), 1, "{logs:#?}");
    assert_eq!(field(dropped[0], "provider"), None, "{logs:#?}");
    // The directory is deleted as a profile; the dangling link is only a link (R-final-M1),
    // and its line names no path either.
    assert_eq!(
        at(&logs, "INFO", "deleted an unrecognized profile entry").len(),
        1,
        "{logs:#?}"
    );
    assert_eq!(
        at(
            &logs,
            "WARN",
            "an unrecognized profile entry is a link, not a profile directory"
        )
        .len(),
        1,
        "{logs:#?}"
    );
    one(
        &logs,
        &format!("deleted the orphaned profile of account {gone}"),
    );
    one(
        &logs,
        &format!("deleted the orphaned profile of account {foreign}"),
    );
    assert_eq!(
        at(
            &logs,
            "WARN",
            "names a provider this build does not register"
        )
        .len(),
        1,
        "{logs:#?}"
    );
    assert_eq!(
        at(
            &logs,
            "WARN",
            "purged the session profile of an account whose provider this build does not register"
        )
        .len(),
        1,
        "{logs:#?}"
    );
    // Each account's line and its `remove` event name the provider only when it is registered.
    for (id, provider) in [(&a, Some(CLAUDE_CODE)), (&ghost, None)] {
        let account = at(&logs, "INFO", "purged an account")
            .into_iter()
            .filter(|l| field(l, "account") == Some(id.as_str()))
            .collect::<Vec<_>>();
        let event = at(&logs, "INFO", "event recorded")
            .into_iter()
            .filter(|l| field(l, "from_account") == Some(id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!((account.len(), event.len()), (1, 1), "{logs:#?}");
        assert_eq!(field(account[0], "provider"), provider, "{logs:#?}");
        assert_eq!(field(event[0], "provider"), provider, "{logs:#?}");
    }
    // The dangling link is no profile directory (R-final-M1): no marker is looked for.
    assert!(
        at(
            &logs,
            "WARN",
            "an unrecognized profile entry has no readable marker"
        )
        .is_empty(),
        "{logs:#?}"
    );
    all.extend(logs);
    for name in ["alice@example.com", "bob@example.com", "carol@example.com"] {
        assert!(all.iter().all(|l| !l.contains(name)), "{name} in {all:#?}");
    }
    no_email(&all);
}

/// Writes a profile marker into `dir`, created if absent, that names `provider` whatever this
/// build registers: a marker's provider may be any text.
fn mark(dir: &std::path::Path, provider: &str, id: &AccountId) {
    tagteam_provider::profile::ProfileMarker {
        provider: ProviderId::new(provider),
        account_id: id.clone(),
        config_dir: dir.display().to_string(),
        outer: json!({}),
    }
    .write(dir)
    .unwrap();
}

#[test]
fn a_profile_whose_state_cannot_be_read_is_logged_by_a_fixed_phrase() {
    // §14.2: what could not be read is named by the profile's path, under a data directory the
    // user may have named, and may quote the file. The lines say it in a fixed phrase.
    let _serial = one_at_a_time();
    let fx = Fx::with(tagteam_cc::live::Platform::MacOs, |e| {
        e.xdg_data_home = Some(e.home.join("alice@example.com/data"));
    });
    let s = fx.add("s@x.co", "rt-s");
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.expire_access(&a);
    // `remove` refuses an account whose session record does not parse.
    let profile = fx.make_profile(&s);
    let record = fx.live_record(&profile, 778, "interactive");
    std::fs::write(&record, r#"{"pid": "bob@example.com"}"#).unwrap();
    let (removed, mut logs) = capture_logs(|| fx.engine.remove(&s));
    assert_eq!(removed.unwrap_err().kind(), "session-owned");
    assert_eq!(
        at(
            &logs,
            "WARN",
            "a session reservation or record could not be read; the account counts as session-owned"
        )
        .len(),
        1,
        "{logs:#?}"
    );
    // The gate sends nothing for an account whose profile's seed does not parse.
    let dir = common::quiescent(&fx, &a, "rt-a", &fx.vault_bytes(&a).unwrap());
    std::fs::write(
        dir.join(tagteam_provider::profile::SEED_FILE),
        r#"{"login_epoch": "bob@example.com"}"#,
    )
    .unwrap();
    let snapshot = fx.vault_bytes(&a).unwrap();
    let (gated, more) = capture_logs(|| {
        fx.engine
            .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
            .unwrap()
    });
    assert!(
        matches!(&gated, tagteam_engine::refresh::GateOutcome::Transient { kind, .. } if kind == "profile-unreadable"),
        "{gated:?}"
    );
    assert_eq!(
        at(
            &more,
            "WARN",
            "the session profile could not be read; nothing is sent"
        )
        .len(),
        1,
        "{more:#?}"
    );
    logs.extend(more);
    for name in ["alice@example.com", "bob@example.com"] {
        assert!(
            logs.iter().all(|l| !l.contains(name)),
            "{name} in {logs:#?}"
        );
    }
    no_email(&logs);
}

#[test]
fn an_automatic_target_that_cannot_be_freshened_is_logged_by_a_fixed_phrase() {
    // §14.2, as above: freshening passes over a target whose pending rescue or profile cannot
    // be read, and its line names neither the file nor what the file says.
    let _serial = one_at_a_time();
    let rescue: fn(&Fx, &AccountId) = |fx, a| {
        let path = fx.plant_rescue(a, &vault_fp(fx, a), &common::credential("a@x.co", "rt-a2"));
        std::fs::write(path, r#"{"format": "bob@example.com"}"#).unwrap();
    };
    let seed: fn(&Fx, &AccountId) = |fx, a| {
        let dir = common::quiescent(fx, a, "rt-a", &common::credential("a@x.co", "rt-a2"));
        std::fs::write(
            dir.join(tagteam_provider::profile::SEED_FILE),
            r#"{"login_epoch": "bob@example.com"}"#,
        )
        .unwrap();
    };
    for (break_it, message) in [
        (
            rescue,
            "a pending rescue could not be settled; the target is passed over",
        ),
        (
            seed,
            "the session profile could not be read; the target is passed over",
        ),
    ] {
        let fx = Fx::with(tagteam_cc::live::Platform::MacOs, |e| {
            e.xdg_data_home = Some(e.home.join("alice@example.com/data"));
        });
        let a = fx.add("a@x.co", "rt-a");
        let c = fx.add("c@x.co", "rt-c"); // live, at its limit
        reading(&fx, &a, 10.0, 20.0);
        reading(&fx, &c, 100.0, 50.0);
        break_it(&fx, &a);
        let mut engine = fx
            .engine
            .auto(&fx.provider(), auto_config(&fx), false)
            .unwrap()
            .unwrap();
        let sink = common::Recorded::default();
        let (_, logs) = capture_logs(|| engine.tick(&sink).unwrap());
        assert_eq!(at(&logs, "WARN", message).len(), 1, "{logs:#?}");
        for name in ["alice@example.com", "bob@example.com"] {
            assert!(
                logs.iter().all(|l| !l.contains(name)),
                "{name} in {logs:#?}"
            );
        }
        no_email(&logs);
    }
}

/// A decision-grade reading of `short` and `long` percent in Claude Code's windows, taken at
/// the fixture clock's start with its next poll five minutes out, as `auto_tick.rs` records.
fn reading(fx: &Fx, id: &AccountId, short: f64, long: f64) {
    use tagteam_core::WindowKind::{Long, Short};
    let t0 = fx.clock.now_ms() / 1000;
    let windows = [
        common::usage_window("5h", Short, short, t0 + 9_630),
        common::usage_window("7d", Long, long, t0 + 291_630),
    ];
    common::record_reading(&fx.engine, id, &windows, t0, t0 + 300);
}

/// `auto_tick.rs`'s configuration: switch at 90%, the best strategy.
fn auto_config(fx: &Fx) -> tagteam_core::autoswitch::AutoConfig {
    use tagteam_provider::Provider;
    tagteam_core::autoswitch::AutoConfig {
        threshold: 90.0,
        hysteresis_pct: 10.0,
        cooldown_s: 300,
        interval_s: 60,
        unhealthy_ticks: 3,
        strategy: tagteam_core::autoswitch::Strategy::Best,
        include_api_key_accounts: false,
        models: Vec::new(),
        long_window: fx.cc.primary_long_window().map(str::to_owned),
    }
}

#[test]
fn remove_logs_a_profile_whose_marker_cannot_be_read_by_a_fixed_phrase() {
    // §14.2: a marker that does not parse is named by its path in its read error, under a data
    // directory the user may have named. `remove` says why it falls back in a fixed phrase.
    let _serial = one_at_a_time();
    let fx = named_home();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c");
    fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    std::fs::write(
        profile.join(".tagteam-profile.json"),
        r#"{"provider": "bob@example.com""#,
    )
    .unwrap();
    let (removed, mut logs) = capture_logs(|| fx.engine.remove(&a));
    removed.unwrap();
    assert_eq!(
        at(
            &logs,
            "WARN",
            "the session profile's marker could not be read (it cannot be read); deleting its Keychain item under its current spelling"
        )
        .len(),
        1,
        "{logs:#?}"
    );
    // A profile that is a link, here to nowhere: no item is named through it (Codex pre-merge
    // P1), so its path is neither resolved nor logged.
    std::os::unix::fs::symlink(fx.env.home.join("bob@example.com/gone"), fx.profile_dir(&c))
        .unwrap();
    let (removed, more) = capture_logs(|| fx.engine.remove(&c));
    removed.unwrap();
    assert_eq!(
        at(
            &more,
            "WARN",
            "the session profile is a link, not a directory; the link was removed and no Keychain item was deleted through it"
        )
        .len(),
        1,
        "{more:#?}"
    );
    logs.extend(more);
    none_named(&logs);
}

#[test]
fn a_new_account_whose_vault_entry_cannot_be_removed_is_logged_by_a_fixed_phrase() {
    // §14.2: a vault error carries the Keychain's message; the line names the account by ID.
    let _serial = one_at_a_time();
    let fx = named_home();
    fx.login("a@x.co", "rt-a");
    fx.kc.set_fail_write(tagteam_engine::vault::SERVICE, true);
    fx.kc.set_fail_delete(tagteam_engine::vault::SERVICE, true);
    let (added, logs) = capture_logs(|| fx.engine.add_live(fx.add_options()));
    assert!(added.is_err());
    let line = at(
        &logs,
        "ERROR",
        "could not remove the vault entry of an account that was never added",
    );
    assert_eq!(line.len(), 1, "{logs:#?}");
    assert!(logs.iter().all(|l| !l.contains("injected")), "{logs:#?}");
    none_named(&logs);
}

#[test]
fn a_refreshed_token_that_cannot_be_stored_is_logged_by_kind() {
    // §14.2: the gate's errors are named by `kind()`, the loss by a fixed phrase; neither by an
    // error's text, which may carry the Keychain's message or a path.
    let _serial = one_at_a_time();
    let fx = named_home();
    let a = due(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(tagteam_engine::vault::SERVICE, true);
    common::block_rescue(&fx);
    let (gated, logs) = capture_logs(|| {
        fx.engine
            .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
            .unwrap()
    });
    common::unblock_rescue(&fx);
    assert!(
        matches!(gated, tagteam_engine::refresh::GateOutcome::Unpersisted),
        "{gated:?}"
    );
    for message in [
        "the vault could not store a refreshed token",
        "neither the vault nor rescue/ could store a refreshed token",
    ] {
        let line = at(&logs, "ERROR", message);
        assert_eq!(line.len(), 1, "{logs:#?}");
        assert!(field(line[0], "kind").is_some(), "{}", line[0]);
    }
    let lost = at(&logs, "ERROR", "a refreshed token was lost");
    assert_eq!(lost.len(), 1, "{logs:#?}");
    assert!(
        lost[0].contains(r#"cause="neither the vault nor rescue/ could store it""#),
        "{}",
        lost[0]
    );
    assert!(logs.iter().all(|l| !l.contains("injected")), "{logs:#?}");
    none_named(&logs);
}

#[test]
fn an_adopted_rescue_that_cannot_be_deleted_is_logged_by_kind() {
    // §14.2: the delete's error is named by `kind()`, never by a text that may name its path.
    let _serial = one_at_a_time();
    let fx = named_home();
    let a = due(&fx);
    fx.plant_rescue(
        &a,
        &vault_fp(&fx, &a),
        &common::credential("a@x.co", "rt-a2"),
    );
    common::block_rescue(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    let (_, logs) = capture_logs(|| {
        fx.engine
            .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
            .unwrap()
    });
    common::unblock_rescue(&fx);
    let line = at(&logs, "WARN", "an adopted rescue file could not be deleted");
    assert_eq!(line.len(), 1, "{logs:#?}");
    assert_eq!(field(line[0], "kind"), Some("\"io\""), "{}", line[0]);
    none_named(&logs);
}

#[test]
fn a_live_identity_cache_that_cannot_be_written_is_logged_by_its_sqlite_code() {
    // §14.2: the store's error is SQLite's message, which can be the database's own text (here
    // a trigger's); the status bar's DEBUG line gives SQLite's result code instead.
    let _serial = one_at_a_time();
    let fx = named_home();
    fx.add("a@x.co", "rt-a");
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_cache BEFORE INSERT ON live_identity_cache \
               BEGIN SELECT RAISE(ABORT, 'the cache is read-only for bob@example.com'); END;",
        )
        .unwrap();
    let (view, logs) = capture_logs(|| fx.engine.statusline(&fx.provider()).unwrap());
    assert!(matches!(view, StatuslineView::Managed { .. }), "{view:?}");
    let line = at(&logs, "DEBUG", "the live identity cache was not written");
    assert_eq!(line.len(), 1, "{logs:#?}");
    assert_eq!(
        field(line[0], "code"),
        Some("ConstraintViolation"),
        "{}",
        line[0]
    );
    none_named(&logs);
}

#[test]
fn a_marker_whose_outer_home_cannot_be_applied_is_logged_by_a_fixed_phrase() {
    // §14: link sync falls back to tagteam's own environment, and says so once. §14.2: the
    // provider's error is not logged, and nothing names the user's data directory.
    let _serial = one_at_a_time();
    let fx = named_home();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    // The account's own marker, whose outer record names no home Claude Code can restore.
    mark(&profile, CLAUDE_CODE, &a);
    let (_removed, logs) = capture_logs(|| fx.engine.remove(&a));
    let line = at(
        &logs,
        "WARN",
        "the outer home a profile's marker records could not be applied; the profile is judged by tagteam's own environment",
    );
    assert_eq!(line.len(), 1, "{logs:#?}");
    // No account field (§14.2): purge's orphan path trusts any marker, whose ID is a free string.
    assert_eq!(field(line[0], "account"), None, "{}", line[0]);
    assert!(!line[0].contains("outer record"), "{}", line[0]);
    none_named(&logs);
}
