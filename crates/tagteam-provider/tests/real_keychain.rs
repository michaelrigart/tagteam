//! Runs `/usr/bin/security` against a throwaway keychain file; never the login keychain.
#![cfg(all(target_os = "macos", feature = "real_keychain"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tagteam_provider::Read;
use tagteam_provider::keychain::{Keychain, LockState};
use tagteam_provider::security::{
    LINE_LIMIT, ProcessRunner, RunResult, Runner, SECURITY, SecurityCli,
};

/// The user's keychain search list is shared by the whole session and cargo runs this file's tests
/// on parallel threads, so a fixture's whole life (snapshot, create, use, delete, restore) is
/// serialized behind this lock.
static SEARCH_LIST: Mutex<()> = Mutex::new(());

/// `security list-keychains -d user`, one quoted path per line, as
/// `xtask/src/compat/keychain.rs::parse_search_list` reads it.
fn search_list() -> Option<Vec<String>> {
    let out = Command::new(SECURITY)
        .args(["list-keychains", "-d", "user"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().trim_matches('"').to_owned())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

// Fields 1 and 3 are never read: they keep the directory alive (its `Drop` removes it) and the
// search-list lock held for as long as the keychain file is in use. The lock is the last field,
// so it drops after `Drop::drop` has restored the search list.
#[allow(dead_code)]
struct TempKeychain(
    PathBuf,
    tempfile::TempDir,
    Vec<String>,
    MutexGuard<'static, ()>,
);

impl TempKeychain {
    fn new() -> Self {
        let guard = SEARCH_LIST.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = search_list().expect("security list-keychains -d user failed");
        assert!(
            !snapshot.is_empty(),
            "an empty search list is a failed read, never a snapshot"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.keychain");
        let run = |args: &[&str]| {
            assert!(
                Command::new("/usr/bin/security")
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["create-keychain", "-p", "pw", path.to_str().unwrap()]);
        run(&["unlock-keychain", "-p", "pw", path.to_str().unwrap()]);
        Self(path, dir, snapshot, guard)
    }
}

impl Drop for TempKeychain {
    fn drop(&mut self) {
        let _ = Command::new("/usr/bin/security")
            .args(["delete-keychain", self.0.to_str().unwrap()])
            .status();
        // Best effort and no assertion: a panic in `Drop` while unwinding aborts.
        if search_list().as_ref() != Some(&self.2) {
            let _ = Command::new(SECURITY)
                .args(["list-keychains", "-d", "user", "-s"])
                .args(&self.2)
                .status();
        }
    }
}

const W_SERVICE: &str = "tagteam-test-w-text";

/// `security::escape_text`'s rule (H5.1), restated here because the function is crate-private.
fn escaped(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The `-i` line `upsert` writes for text from Task 7 on, with the test keychain as its last word.
fn text_line(keychain: &Path, service: &str, account: &str, text: &str) -> String {
    format!(
        "add-generic-password -U -a \"{account}\" -s \"{service}\" -w \"{}\" \"{}\"\n",
        escaped(text),
        keychain.display()
    )
}

/// Writes `text` through `security -i`. `security -i` exits 0 even when a command fails, so an
/// empty stderr is part of success. Returns the line's length.
fn write_text_line(keychain: &Path, service: &str, account: &str, text: &str) -> usize {
    let line = text_line(keychain, service, account, text);
    assert!(line.len() <= LINE_LIMIT, "{account}: {} bytes", line.len());
    match ProcessRunner.run(
        SECURITY,
        &["-i".to_owned()],
        Some(line.as_bytes()),
        Duration::from_secs(5),
    ) {
        RunResult::Exited {
            code: 0, stderr, ..
        } if stderr.is_empty() => line.len(),
        RunResult::Exited { code, stderr, .. } => panic!(
            "{account}: security -i rc {code}: {}",
            String::from_utf8_lossy(&stderr)
        ),
        other => panic!("{account}: {other:?}"),
    }
}

/// `(account, text)` pairs, each a JSON credential (Review Focus line 1).
fn text_cases() -> Vec<(&'static str, String)> {
    vec![
        (
            "quote-backslash",
            r#"{"claudeAiOauth":{"accessToken":"a\"b\\c"}}"#.to_owned(),
        ),
        // Backslash sequences as text.
        (
            "json-escapes",
            r#"{"n":"line\nbreak","e":"caf\u00e9","t":"\t"}"#.to_owned(),
        ),
        (
            "raw-non-ascii",
            "{\"e\":\"café\",\"cjk\":\"日本語\",\"emoji\":\"🔑\"}".to_owned(),
        ),
        (
            "shell-chars",
            r##"{"x":"#hash $HOME %s 'single' a  b"}"##.to_owned(),
        ),
        ("raw-tab", "{\"t\":\"a\tb\"}".to_owned()),
    ]
}

/// A credential whose `-i` line is exactly `LINE_LIMIT` bytes long.
fn full_line_text(keychain: &Path) -> String {
    let make = |pad: usize| {
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"sk-ant-oat01-\"q\"\\","refreshToken":"rt"}},"pad":"{}"}}"#,
            "x".repeat(pad)
        )
    };
    let empty = text_line(keychain, W_SERVICE, "full-line", &make(0)).len();
    let text = make(LINE_LIMIT - empty);
    assert_eq!(
        text_line(keychain, W_SERVICE, "full-line", &text).len(),
        LINE_LIMIT
    );
    text
}

#[test]
fn round_trips_small_large_and_binary_items() {
    let kc = TempKeychain::new();
    let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
    assert!(matches!(k.find("tagteam", "id"), Read::Absent));
    for data in [
        b"{\"claudeAiOauth\":{}}".to_vec(),
        vec![b'x'; 5000],
        vec![0u8, 1, 2, 255],
    ] {
        k.upsert("tagteam", "id", &data).unwrap();
        assert_eq!(k.find("tagteam", "id").present().unwrap(), data);
        assert!(k.exists("tagteam", "id").is_present());
    }
    k.delete("tagteam", "id").unwrap();
    k.delete("tagteam", "id").unwrap();
    assert!(matches!(k.exists("tagteam", "id"), Read::Absent));
}

#[test]
fn round_trips_the_hex_rendering_ambiguity_cases() {
    // (a) printable text that is itself all lowercase hex digits, (b) genuinely binary
    // data with the identical hex rendering, and (c) printable text with a quote and a
    // backslash. `-w` renders (a) and (b) identically, so this exercises the real `-g`
    // disambiguation call, not just the fake `Runner` in security.rs's unit tests.
    let kc = TempKeychain::new();
    let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
    for (service, data) in [
        ("case-a", b"cafe".to_vec()),
        ("case-b", vec![0xca, 0xfe]),
        ("case-c", br#"{"a":"b\c"}"#.to_vec()),
    ] {
        k.upsert(service, "id", &data).unwrap();
        assert_eq!(k.find(service, "id").present().unwrap(), data);
        k.delete(service, "id").unwrap();
    }
}

#[test]
fn an_unlocked_keychain_file_passes_the_lock_check() {
    // Only the unlocked case: `unlock` would prompt, so it is never run here.
    let kc = TempKeychain::new();
    let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
    assert_eq!(k.lock_state(), LockState::Unlocked);
}

#[test]
fn deleting_by_service_deletes_every_item_of_that_service_and_no_other() {
    // Appendix A.3: `delete-generic-password -s` without `-a` deletes one item per call and
    // returns rc 44 once none is left (inferred until this test pins it). The service names
    // are this test's own, so even a call that missed the keychain file could touch nothing
    // of tagteam's or Claude Code's.
    const SERVICE: &str = "tagteam-test-delete-by-service";
    const OTHER: &str = "tagteam-test-delete-by-service-other";
    let kc = TempKeychain::new();
    let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
    for account in ["a", "a.prev", "b"] {
        k.upsert(SERVICE, account, b"{}").unwrap();
    }
    k.upsert(OTHER, "a", b"kept").unwrap();
    assert!(matches!(k.service_has_items(SERVICE), Read::Present(true)));
    assert_eq!(k.delete_service(SERVICE).unwrap(), 3);
    assert!(matches!(k.service_has_items(SERVICE), Read::Present(false)));
    assert_eq!(k.find(OTHER, "a").present().unwrap(), b"kept");
    assert!(matches!(k.service_has_items(OTHER), Read::Present(true)));
    assert_eq!(
        k.delete_service(SERVICE).unwrap(),
        0,
        "an empty service is done"
    );
}

#[test]
fn a_w_text_line_round_trips_every_h5_1_case_byte_for_byte() {
    let kc = TempKeychain::new();
    let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
    let mut cases = text_cases();
    cases.push(("full-line", full_line_text(&kc.0)));
    for (account, text) in &cases {
        write_text_line(&kc.0, W_SERVICE, account, text);
        // `find` applies A.3's `-g` disambiguation to whatever `-w` renders as hex.
        assert_eq!(
            k.find(W_SERVICE, account).present().unwrap(),
            text.as_bytes(),
            "{account}"
        );
    }
}

#[test]
fn a_w_text_line_with_dash_u_updates_an_existing_item_in_place() {
    let kc = TempKeychain::new();
    let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
    let (first, second) = (r#"{"g":"1 \"a\""}"#, r#"{"g":"2 \\b é"}"#);
    write_text_line(&kc.0, W_SERVICE, "id", first);
    write_text_line(&kc.0, W_SERVICE, "id", second);
    assert_eq!(
        k.find(W_SERVICE, "id").present().unwrap(),
        second.as_bytes()
    );
    assert_eq!(
        k.delete_service(W_SERVICE).unwrap(),
        1,
        "one item, updated in place"
    );
}
