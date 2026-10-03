//! §14.2's INFO lines from the engine's central sites (Decision 10): every `events` row, every
//! vault generation written, a rescue, a displacement and a recovery decision. Each names its
//! accounts by ID, never by email (B.35).
mod common;

use std::sync::{Mutex, MutexGuard, PoisonError};

use common::{Fx, capture_logs, crashed_switch, due, vault_fp, write_target_credential};
use tagteam_engine::vault::SERVICE;

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
    assert!(logs.iter().all(|l| !l.contains("rt-a2")), "{logs:#?}");
    no_email(&logs);
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
    assert!(
        logs.iter()
            .all(|l| !l.contains("stranger") && !l.contains("rt-s")),
        "{logs:#?}"
    );
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
    }
}
