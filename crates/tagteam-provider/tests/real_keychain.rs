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
fn an_unlocked_keychain_file_passes_the_lock_check() {
    // Only the unlocked case: `unlock` would prompt, so it is never run here.
    let kc = TempKeychain::new();
    let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
    assert_eq!(k.lock_state(), LockState::Unlocked);
}
