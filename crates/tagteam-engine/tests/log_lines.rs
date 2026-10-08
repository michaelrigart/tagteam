//! §14.2's lines from the engine's central sites (Decision 10): every `events` row, every
//! vault generation written, a rescue, a displacement, a recovery decision, a switch's
//! rollback and every refresh outcome, and the status bar's DEBUG ceiling. Each names its
//! accounts by ID, never by email (B.35).
mod common;

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use common::{Fx, capture_logs, crashed_switch, due, vault_fp, write_target_credential};
use serde_json::json;
use tagteam_core::AccountId;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::active::{ActiveOutcome, ActiveTrigger};
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
        assert!(
            found[0].contains("its journal row stays for recovery: restore the live credential"),
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
