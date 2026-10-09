//! Runs `/usr/bin/security` against a throwaway keychain file; never the login keychain.
#![cfg(all(target_os = "macos", feature = "real_keychain"))]

use std::process::Command;

use tagteam_provider::Read;
use tagteam_provider::keychain::{Keychain, LockState};
use tagteam_provider::security::{ProcessRunner, SecurityCli};

// The `TempDir` field is never read: it exists to keep the directory alive (its `Drop`
// removes it) for as long as the keychain file inside it is in use.
#[allow(dead_code)]
struct TempKeychain(std::path::PathBuf, tempfile::TempDir);

impl TempKeychain {
    fn new() -> Self {
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
        Self(path, dir)
    }
}

impl Drop for TempKeychain {
    fn drop(&mut self) {
        let _ = Command::new("/usr/bin/security")
            .args(["delete-keychain", self.0.to_str().unwrap()])
            .status();
    }
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
