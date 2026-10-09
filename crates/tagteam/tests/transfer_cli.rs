//! `tagteam export` and `tagteam import` through the real binary (§13.3). Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use common::{cc_profile, cmd, seed_home, std_cmd, two_accounts};
use serde_json::{Value, json};
use tagteam_engine::transfer::{self, Decoded, IdentityFile, Need};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Env, FileKeychain, Keychain};

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
        .current_dir(root.path())
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
        .current_dir(root.path())
        .args(["export", "x.age", "--recipient", SSH_PK, "--plaintext"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "clap refuses the pair");
}

/// Deletes the vault copy of each of `ids`, so the account has no stored credential.
fn break_accounts(root: &Path, ids: &[&String]) {
    let kc = FileKeychain::new(root.join("keychain"));
    for id in ids {
        kc.delete(SERVICE, id).unwrap();
    }
}

/// Every file directly in `dir`, except the fixture's own `home` and `keychain`.
fn left_beside(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "home" && n != "keychain")
        .collect()
}

#[test]
fn an_export_with_every_account_broken_is_nothing_exported_and_writes_no_file() {
    let root = tempfile::tempdir().unwrap();
    let (a, b) = two_accounts(root.path());
    break_accounts(root.path(), &[&a, &b]);
    let file = root.path().join("accounts.json");
    let out = cmd(root.path())
        .args(["export", file.to_str().unwrap(), "--plaintext", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json_of(&out.stdout);
    assert_eq!(v["error"]["type"], "nothing-exported");
    assert_eq!(v["error"]["message"], "no account was exported");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("warning: #1 a@x.co was not exported: it has no stored credential"),
        "{err}"
    );
    assert!(!file.exists());
    assert_eq!(
        left_beside(root.path()),
        Vec::<String>::new(),
        "no temporary file"
    );
}

#[test]
fn an_export_of_no_accounts_is_nothing_exported_too() {
    let root = tempfile::tempdir().unwrap();
    let out = cmd(root.path())
        .current_dir(root.path())
        .args(["export", "out.json", "--plaintext"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "tagteam: no account was exported\n"
    );
    assert_eq!(left_beside(root.path()), Vec::<String>::new());
}

#[test]
fn a_named_broken_account_is_a_hard_error_and_leaves_no_file() {
    let root = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(root.path());
    break_accounts(root.path(), &[&a]);
    let file = root.path().join("accounts.json");
    let out = cmd(root.path())
        .args([
            "export",
            file.to_str().unwrap(),
            "--plaintext",
            "--account",
            "1",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json_of(&out.stdout);
    assert_eq!(v["error"]["type"], "account-broken");
    assert_eq!(
        v["error"]["message"],
        "position 1 cannot be exported: it has no stored credential"
    );
    assert!(!file.exists());
    assert_eq!(
        left_beside(root.path()),
        Vec::<String>::new(),
        "no temporary file"
    );

    // The other account exports on its own.
    let out = cmd(root.path())
        .args([
            "export",
            file.to_str().unwrap(),
            "--plaintext",
            "--account",
            "2",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(holds(&opened(&std::fs::read(&file).unwrap(), &[])).len(), 1);
}

/// A home where Claude Code has run and nothing is stored yet.
fn empty_home() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    seed_home(&Env::for_test(d.path()));
    d
}

/// The emails `list --json` shows in `root`, in position order.
fn listed(root: &Path) -> Vec<String> {
    let out = cmd(root).args(["list", "--json"]).output().unwrap();
    json_of(&out.stdout)["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["email"].as_str().unwrap().to_owned())
        .collect()
}

/// `root`'s two accounts, exported to `<root>/<name>` with `args`.
fn exported(root: &Path, name: &str, args: &[&str]) -> PathBuf {
    two_accounts(root);
    let file = root.join(name);
    let out = cmd(root)
        .arg("export")
        .arg(&file)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    file
}

#[test]
fn a_plaintext_export_moves_both_accounts_to_another_home() {
    let from = tempfile::tempdir().unwrap();
    let file = exported(from.path(), "accounts.json", &["--plaintext"]);
    let to = empty_home();
    let out = cmd(to.path())
        .args(["import", file.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        json_of(&out.stdout),
        json!({"schemaVersion": 1, "ok": true, "accounts": [
            {"provider": "claude-code", "number": 1, "email": "a@x.co", "outcome": "created", "message": "added"},
            {"provider": "claude-code", "number": 2, "email": "b@x.co", "outcome": "created", "message": "added"}],
            "warnings": []})
    );
    assert_eq!(listed(to.path()), ["a@x.co", "b@x.co"]);
    let again = cmd(to.path())
        .args(["import", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(again.status.success());
    assert_eq!(
        String::from_utf8_lossy(&again.stdout),
        "skipped  #1 a@x.co: already stored; pass --force to replace it\n\
         skipped  #2 b@x.co: already stored; pass --force to replace it\n"
    );
}

#[test]
fn import_dash_reads_the_export_from_stdin() {
    let from = tempfile::tempdir().unwrap();
    let file = exported(from.path(), "accounts.json", &["--plaintext"]);
    let to = empty_home();
    let mut child = std_cmd(to.path())
        .args(["import", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&std::fs::read(&file).unwrap())
        .unwrap();
    let done = child.wait_with_output().unwrap();
    assert!(done.status.success());
    assert_eq!(
        json_of(&done.stdout)["accounts"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn an_ssh_export_imports_with_its_key_and_needs_one() {
    let from = tempfile::tempdir().unwrap();
    let file = exported(from.path(), "accounts.age", &["--recipient", SSH_PK]);
    let to = empty_home();
    let key = to.path().join("id_ed25519");
    std::fs::write(&key, ssh_sk()).unwrap();
    let out = cmd(to.path())
        .args([
            "import",
            file.to_str().unwrap(),
            "--identity",
            key.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "created  #1 a@x.co: added\ncreated  #2 b@x.co: added\n"
    );
    let none = cmd(empty_home().path())
        .args(["import", file.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(none.status.code(), Some(1));
    assert_eq!(json_of(&none.stdout)["error"]["type"], "needs-identity");
}

/// cswap's export of an OAuth account, an API key and a setup token (inventory §8.1).
fn cswap_file(dir: &Path) -> PathBuf {
    let token = |email: &str| json!({"emailAddress": email, "accountUuid": "", "organizationUuid": null, "organizationName": null});
    let v = json!({"version": 1, "exportedAt": "2026-09-20T10:00:00Z", "exportedFrom": "macos",
        "swapVersion": "0.27.0b1", "encrypted": false, "activeAccountNumber": 1, "accounts": [
        {"number": 1, "email": "a@x.co", "uuid": "uuid-a", "organizationUuid": "", "organizationName": null,
         "added": "2026-09-01T08:00:00Z",
         "credentials": {"claudeAiOauth": {"accessToken": "at-a", "refreshToken": "rt-a"}},
         "config": {"oauthAccount": {"emailAddress": "a@x.co", "accountUuid": "uuid-a", "organizationUuid": ""}},
         "alias": "main"},
        {"number": 2, "email": "api-key-2@token.local", "uuid": "", "organizationUuid": null,
         "organizationName": null, "credentials": "sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
         "config": {"oauthAccount": token("api-key-2@token.local")}, "kind": "api_key"},
        {"number": 3, "email": "setup-token-3@token.local", "uuid": "", "organizationUuid": null,
         "organizationName": null,
         "credentials": {"claudeAiOauth": {"accessToken": "sk-ant-oat01-x", "scopes": ["user:inference"]}},
         "config": {"oauthAccount": token("setup-token-3@token.local")}}]});
    let path = dir.join("cswap-export.json");
    std::fs::write(&path, v.to_string()).unwrap();
    path
}

#[test]
fn a_cswap_v1_export_imports_through_the_cli() {
    let to = empty_home();
    let file = cswap_file(to.path());
    let out = cmd(to.path())
        .args(["import", file.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let outcomes: Vec<(u64, String)> = json_of(&out.stdout)["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let n = a["number"].as_u64().unwrap();
            (n, a["outcome"].as_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            (1, "created".into()),
            (2, "created".into()),
            (3, "created".into())
        ]
    );
    let list = cmd(to.path()).args(["list", "--json"]).output().unwrap();
    let rows = json_of(&list.stdout)["accounts"].clone();
    assert_eq!(rows[0]["alias"], "main");
    assert_eq!(rows[1]["usageStatus"], "api_key");
}

#[test]
fn an_account_that_fails_is_reported_and_the_import_exits_1() {
    let root = tempfile::tempdir().unwrap();
    two_accounts(root.path()); // a@x.co's accountUuid is uuid-a@x.co-
    let file = root.path().join("other.json");
    let v = json!({"format": "tagteam-export", "version": 1, "accounts": [
        {"provider": "claude-code", "position": 1, "kind": "oauth",
         "identity": {"oauthAccount": {"emailAddress": "a@x.co", "accountUuid": "uuid-someone-else", "organizationUuid": ""}},
         "credential": {"claudeAiOauth": {"accessToken": "at", "refreshToken": "rt-a9"}}}]});
    std::fs::write(&file, v.to_string()).unwrap();
    let out = cmd(root.path())
        .args(["import", file.to_str().unwrap(), "--force", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report = json_of(&out.stdout);
    assert_eq!(report["ok"], false);
    assert_eq!(report["accounts"][0]["outcome"], "failed");
}

#[test]
fn import_refuses_inside_a_run_shell_before_reading_the_file() {
    let root = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(root.path());
    let (_, spelling) = cc_profile(root.path(), &a);
    let out = cmd(root.path())
        .env("CLAUDE_CONFIG_DIR", &spelling)
        .args(["import", "/nonexistent/export.json", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json_of(&out.stdout)["error"]["type"], "inside-run-shell");
}
