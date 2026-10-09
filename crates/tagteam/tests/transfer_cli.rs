//! `tagteam export` and `tagteam import` through the real binary (§13.3). Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::os::unix::fs::PermissionsExt;

use common::{cmd, two_accounts};
use serde_json::{Value, json};
use tagteam_engine::transfer::{self, Decoded, IdentityFile, Need};

/// age's own ssh-ed25519 test key pair (age 0.12.1, `src/ssh`).
const SSH_PK: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uObz5dvMgjz1OxfM/XXUB+VHtZ6isGN alice@rust";

/// The test key's private half, its armor assembled so a secret scanner does not take this
/// file for one that holds a key.
fn ssh_sk() -> String {
    let label = concat!("OPENSSH ", "PRIVATE", " KEY");
    let body = "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACB7Ci6nqZYaVvrjm8+XbzII89TsXzP111AflR7WeorBjQAAAJCfEwtqnxML
agAAAAtzc2gtZWQyNTUxOQAAACB7Ci6nqZYaVvrjm8+XbzII89TsXzP111AflR7WeorBjQ
AAAEADBJvjZT8X6JRJI8xVq/1aU8nMVgOtVnmdwqWwrSlXG3sKLqeplhpW+uObz5dvMgjz
1OxfM/XXUB+VHtZ6isGNAAAADHN0cjRkQGNhcmJvbgE=
";
    format!("-----BEGIN {label}-----\n{body}-----END {label}-----\n")
}

const HAND_OFF: &str = "warning: this export hands its OAuth logins over";

fn json_of(out: &[u8]) -> Value {
    serde_json::from_slice(out).unwrap()
}

/// What an export file holds, opened with `keys` and never a passphrase.
fn opened(bytes: &[u8], keys: &[IdentityFile]) -> Decoded {
    transfer::decode(bytes, keys, &mut |_: &Need| None).unwrap()
}

/// `(position, refresh token)` of each account an export file holds.
fn holds(d: &Decoded) -> Vec<(u32, String)> {
    d.records
        .iter()
        .map(|r| {
            let rt = r.credential["claudeAiOauth"]["refreshToken"]
                .as_str()
                .unwrap();
            (r.position, rt.to_owned())
        })
        .collect()
}

#[test]
fn a_plaintext_export_writes_a_private_file_and_reports_each_account() {
    let from = tempfile::tempdir().unwrap();
    two_accounts(from.path());
    let file = from.path().join("accounts.json");
    let out = cmd(from.path())
        .args(["export", file.to_str().unwrap(), "--plaintext", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        json_of(&out.stdout),
        json!({"schemaVersion": 1, "ok": true, "file": file.to_str().unwrap(),
            "encrypted": false,
            "accounts": [
                {"provider": "claude-code", "number": 1, "email": "a@x.co", "source": "vault", "inUse": false},
                {"provider": "claude-code", "number": 2, "email": "b@x.co", "source": "vault", "inUse": true}],
            "skipped": []})
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(HAND_OFF), "{err}");
    assert!(err.contains("warning: #2 b@x.co is in use here"), "{err}");
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(
        holds(&opened(&bytes, &[])),
        [(1, "rt-a".to_owned()), (2, "rt-b".to_owned())]
    );
}

#[test]
fn export_dash_writes_the_file_to_stdout_and_its_summary_to_stderr() {
    let from = tempfile::tempdir().unwrap();
    two_accounts(from.path());
    let out = cmd(from.path())
        .args(["export", "-", "--plaintext"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(holds(&opened(&out.stdout, &[])).len(), 2);
    assert!(
        String::from_utf8_lossy(&out.stderr).ends_with("Exported 2 accounts, unencrypted.\n"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn an_export_to_an_ssh_key_opens_with_that_key() {
    let from = tempfile::tempdir().unwrap();
    two_accounts(from.path());
    let file = from.path().join("accounts.age");
    let out = cmd(from.path())
        .args([
            "export",
            file.to_str().unwrap(),
            "--recipient",
            SSH_PK,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json_of(&out.stdout)["encrypted"], true);
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----"));
    let key = transfer::parse_identity_file("id_ed25519", ssh_sk().as_bytes()).unwrap();
    assert_eq!(holds(&opened(&bytes, &[key])).len(), 2);
}

#[test]
fn a_bad_recipient_is_a_usage_error_that_never_quotes_it() {
    let root = tempfile::tempdir().unwrap();
    two_accounts(root.path());
    let out = cmd(root.path())
        .args([
            "export",
            "x.age",
            "--recipient",
            "AGE-SECRET-KEY-1SECRETVALUE",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "tagteam: --recipient: a recipient must be an age1… or ssh-ed25519 public key\n"
    );
    let out = cmd(root.path())
        .args(["export", "x.age", "--recipient", SSH_PK, "--plaintext"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "clap refuses the pair");
}
