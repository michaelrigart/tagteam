//! §13.3's export file (Task 6): the envelope, its age encryption, and the formats import
//! detects, cswap's version 1 export among them (Decision 14). Pure: no store, vault or lock.

use std::collections::BTreeMap;
use std::sync::Arc;

use age::secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::live::Platform;
use tagteam_core::{CLAUDE_CODE, ProviderId};
use tagteam_engine::transfer::{
    Decoded, Encryption, EnvelopeAccount, IdentityFile, Need, TransferError, decode, encrypt,
    envelope, parse_identity_file, parse_recipient, parse_recipients, read_cswap_v1, read_envelope,
};
use tagteam_provider::{FakeKeychain, Provider};

/// age's own ssh-ed25519 test key (age 0.12.1, `src/ssh/recipient.rs` and
/// `src/ssh/identity.rs`): the public key, the private key unencrypted, and the same key
/// encrypted (aes256-ctr) with the passphrase "passphrase".
const SSH_PK: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uObz5dvMgjz1OxfM/XXUB+VHtZ6isGN alice@rust";
const SSH_SK: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACB7Ci6nqZYaVvrjm8+XbzII89TsXzP111AflR7WeorBjQAAAJCfEwtqnxML
agAAAAtzc2gtZWQyNTUxOQAAACB7Ci6nqZYaVvrjm8+XbzII89TsXzP111AflR7WeorBjQ
AAAEADBJvjZT8X6JRJI8xVq/1aU8nMVgOtVnmdwqWwrSlXG3sKLqeplhpW+uObz5dvMgjz
1OxfM/XXUB+VHtZ6isGNAAAADHN0cjRkQGNhcmJvbgE=
-----END OPENSSH PRIVATE KEY-----";
const SSH_SK_ENCRYPTED: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABBSs0SUhQ
958xWERf6ibyf2AAAAEAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uOb
z5dvMgjz1OxfM/XXUB+VHtZ6isGNAAAAkLvH9UsJa+ulewsZT2YtEkme1y9UZKI/vUbTms
LVqWdLprBQIm3IClfGso6IPW7+imkwYRHPKYfBYGYuexzO8b+LRiZU5/lDQmsvZA3asNxp
KjW7kUOJnI8dAeaqJa18P7XkAuzcuZmVoCTurqEOSeb5Ww9Nq0csB0zkF22/PeWy3+BZW5
hDsL1OfQl4WbakZQ==
-----END OPENSSH PRIVATE KEY-----";
const SSH_RSA_PK: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDE7nIXTGNuaRBN9toI/wNALuQec8mvlt0iJ7o3OaD2UvoKHJ7S8rmIn4FiQDUed/Vac3OhUibei1k+TBmm16u2Rj3klgWZOIDgi8d4vXKI5N3YBhxr3jsQ+kz1c+iZ4z/tTtz306+4K46XViVMWwyyg9j82Jn41mOAy9vdeDIfQ5fLeaGqn5KwlT61GNkZ+ozWK/ZNlQIlNCcoXxhJULIs9XrtczWyVBAea1nlDo0WHODePxoJjmsNHrpQXn5mf9O83xs10qfTUjnRUt48jRmedFy4tcra3QGmSTQ3KZne+wXXSb0cIpXLGvZjQSPHgG1hc4r3uBpiSzvesGLv79XL alice@rust";

const NOW_MS: i64 = 1_790_000_000_000;

fn oauth_account(email: &str) -> Value {
    json!({"emailAddress": email, "accountUuid": format!("uuid-{email}"), "organizationUuid": "", "organizationName": null})
}

fn credential(rt: &str) -> Value {
    json!({"claudeAiOauth": {"accessToken": format!("at-{rt}"), "refreshToken": rt, "expiresAt": 1_790_003_600_000i64}})
}

fn account(position: u32, email: &str, rt: &str) -> EnvelopeAccount {
    EnvelopeAccount {
        provider: ProviderId::new(CLAUDE_CODE),
        position,
        kind: "oauth".into(),
        label: email.into(),
        alias: (position == 2).then(|| "dev".to_owned()),
        disabled: position == 3,
        added_at: 1_756_713_600_123,
        identity: json!({"email": email, "accountUuid": format!("uuid-{email}"), "organizationUuid": "", "organizationName": null, "oauthAccount": oauth_account(email)}),
        credential: credential(rt),
    }
}

/// Two Claude Code accounts at positions 2 (live, aliased) and 3 (disabled).
fn plaintext() -> Vec<u8> {
    let active = BTreeMap::from([(CLAUDE_CODE.to_owned(), 2)]);
    envelope(
        NOW_MS,
        &active,
        &[
            account(2, "a@x.co", "rt-a-SENTINEL"),
            account(3, "b@x.co", "rt-b"),
        ],
    )
}

/// `ask` for a file that must not need anything.
fn ask_nothing(need: &Need) -> Option<SecretString> {
    panic!("nothing should be asked, but {need:?} was")
}

fn decoded(input: &[u8], keys: &[IdentityFile]) -> Decoded {
    decode(input, keys, &mut ask_nothing).unwrap()
}

/// The two records `plaintext` holds, as import reads them back.
fn assert_round_trip(d: &Decoded) {
    let r = &d.records;
    assert_eq!(r.len(), 2);
    assert_eq!(
        (r[0].provider.as_str(), r[0].position, r[0].kind.as_deref()),
        (CLAUDE_CODE, 2, Some("oauth"))
    );
    assert_eq!((r[0].alias.as_deref(), r[0].disabled), (Some("dev"), false));
    assert_eq!((r[1].alias.as_deref(), r[1].disabled), (None, true));
    assert_eq!(r[0].label.as_deref(), Some("a@x.co"));
    assert_eq!(r[0].added_at, Some(1_756_713_600_000), "to the second");
    assert_eq!(r[0].identity["oauthAccount"], oauth_account("a@x.co"));
    assert_eq!(r[0].credential, credential("rt-a-SENTINEL"));
    assert!(!d.cswap);
}

#[test]
fn the_envelope_holds_section_13_3_s_fields_and_nothing_machine_local() {
    let v: Value = serde_json::from_slice(&plaintext()).unwrap();
    assert_eq!(v["format"], "tagteam-export");
    assert_eq!(v["version"], 1);
    assert_eq!(v["exportedAt"], "2026-09-21T14:13:20Z");
    assert!(["macos", "linux"].contains(&v["exportedFrom"].as_str().unwrap()));
    assert_eq!(v["tagteamVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["active"], json!({"claude-code": 2}));
    let keys: Vec<&str> = v["accounts"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "provider",
            "position",
            "kind",
            "label",
            "alias",
            "disabled",
            "addedAt",
            "identity",
            "credential"
        ],
        "no id, epoch, quarantine or usage (§13.3)"
    );
    assert_eq!(v["accounts"][0]["addedAt"], "2025-09-01T08:00:00Z");
    assert!(plaintext().ends_with(b"\n"));
}

#[test]
fn a_plaintext_export_round_trips() {
    let d = decoded(&plaintext(), &[]);
    assert!(!d.encrypted);
    assert_round_trip(&d);
}

#[test]
fn a_passphrase_export_is_armored_ascii_and_opens_with_that_passphrase_only() {
    let pt = plaintext();
    let file = encrypt(
        &pt,
        &Encryption::Passphrase(SecretString::from("correct horse")),
    )
    .unwrap();
    assert!(file.is_ascii(), "ASCII armor");
    assert!(file.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----\n"));
    assert!(!String::from_utf8_lossy(&file).contains("SENTINEL"));

    let mut asked = Vec::new();
    let d = decode(&file, &[], &mut |need: &Need| {
        asked.push(need.clone());
        Some(SecretString::from("correct horse"))
    })
    .unwrap();
    assert_eq!(asked, [Need::Passphrase]);
    assert!(d.encrypted);
    assert_round_trip(&d);

    let wrong = decode(&file, &[], &mut |_: &Need| {
        Some(SecretString::from("wrong"))
    })
    .unwrap_err();
    assert_eq!(wrong.kind(), "decrypt-failed");
    assert_eq!(
        wrong.to_string(),
        "the passphrase is wrong, or the file is damaged"
    );
    let none = decode(&file, &[], &mut |_: &Need| None).unwrap_err();
    assert!(matches!(none, TransferError::NeedsPassphrase), "{none}");
}

#[test]
fn an_x25519_export_opens_with_its_identity_file_and_never_asks() {
    let key = age::x25519::Identity::generate();
    let to = parse_recipient(&key.to_public().to_string()).unwrap();
    let file = encrypt(&plaintext(), &Encryption::Recipients(vec![to])).unwrap();
    assert!(file.is_ascii());
    let text = format!(
        "# created: by the test\n{}\n",
        key.to_string().expose_secret()
    );
    let keys = parse_identity_file("key.txt", text.as_bytes()).unwrap();
    assert!(
        !format!("{keys:?}").contains("AGE-SECRET-KEY"),
        "Debug names the file only"
    );
    assert_round_trip(&decoded(&file, &[keys]));

    let other = age::x25519::Identity::generate();
    let other =
        parse_identity_file("other.txt", other.to_string().expose_secret().as_bytes()).unwrap();
    let err = decode(&file, &[other], &mut ask_nothing).unwrap_err();
    assert_eq!(
        (err.kind(), err.to_string().as_str()),
        (
            "decrypt-failed",
            "none of the --identity keys can decrypt this file"
        )
    );
    let err = decode(&file, &[], &mut ask_nothing).unwrap_err();
    assert!(matches!(err, TransferError::NeedsIdentity), "{err}");
}

#[test]
fn an_ssh_ed25519_export_opens_with_the_key_and_asks_only_an_encrypted_key_s_passphrase() {
    let file = encrypt(
        &plaintext(),
        &Encryption::Recipients(vec![parse_recipient(SSH_PK).unwrap()]),
    )
    .unwrap();
    let plain = parse_identity_file("id_plain", SSH_SK.as_bytes()).unwrap();
    assert_round_trip(&decoded(&file, &[plain]));

    let locked = || parse_identity_file("id_ed25519", SSH_SK_ENCRYPTED.as_bytes()).unwrap();
    let mut asked = Vec::new();
    let d = decode(&file, &[locked()], &mut |need: &Need| {
        asked.push(need.clone());
        Some(SecretString::from("passphrase"))
    })
    .unwrap();
    assert_eq!(asked, [Need::KeyPassphrase("id_ed25519".into())]);
    assert_round_trip(&d);

    let wrong = decode(&file, &[locked()], &mut |_: &Need| {
        Some(SecretString::from("nope"))
    })
    .unwrap_err();
    assert_eq!(wrong.kind(), "decrypt-failed");
    assert!(wrong.to_string().starts_with("id_ed25519: "), "{wrong}");
    let none = decode(&file, &[locked()], &mut |_: &Need| None).unwrap_err();
    assert_eq!(
        none.to_string(),
        "id_ed25519 is an encrypted SSH key, and its passphrase needs a terminal to type it"
    );
}

#[test]
fn a_binary_age_file_is_read_too() {
    let key = age::x25519::Identity::generate();
    let file = age::encrypt(&key.to_public(), &plaintext()).unwrap();
    assert!(file.starts_with(b"age-encryption.org/v1\n"));
    let keys = parse_identity_file("k", key.to_string().expose_secret().as_bytes()).unwrap();
    assert_round_trip(&decoded(&file, &[keys]));
}

#[test]
fn recipients_are_age_or_ssh_ed25519_keys_and_a_bad_one_is_never_quoted() {
    assert!(parse_recipient(SSH_PK).is_ok());
    let age_pk = age::x25519::Identity::generate().to_public().to_string();
    assert!(parse_recipient(&format!("  {age_pk}  ")).is_ok());
    let secret = age::x25519::Identity::generate().to_string();
    for bad in [SSH_RSA_PK, secret.expose_secret(), "age1nope", ""] {
        let err = parse_recipient(bad).unwrap_err();
        assert_eq!(err.kind(), "invalid-input");
        assert_eq!(
            err.to_string(),
            "a recipient must be an age1… or ssh-ed25519 public key"
        );
    }
    let file = format!("# the laptop\n\n{SSH_PK}\n   \n{age_pk}\n");
    assert_eq!(parse_recipients(&file).unwrap().len(), 2);
    let err = parse_recipients(&format!("{age_pk}\n# ok\n{SSH_RSA_PK}\n")).unwrap_err();
    assert_eq!(
        err.to_string(),
        "line 3: a recipient must be an age1… or ssh-ed25519 public key"
    );
    let err = parse_recipients("# nothing here\n\n").unwrap_err();
    assert_eq!(err.to_string(), "it holds no recipient");
}

#[test]
fn an_identity_file_that_holds_no_usable_key_is_refused_by_name_only() {
    for (bytes, detail) in [
        (&b"AGE-SECRET-KEY-NOT-REALLY\n"[..], "it is not an age identity file"),
        (&b"# only a comment\n"[..], "it holds no key"),
        (
            &b"-----BEGIN OPENSSH PRIVATE KEY-----\nnot base64\n-----END OPENSSH PRIVATE KEY-----\n"[..],
            "it is not a private key tagteam can read",
        ),
    ] {
        let err = parse_identity_file("keys/mine", bytes).unwrap_err();
        assert_eq!(err.kind(), "invalid-input");
        assert_eq!(err.to_string(), format!("keys/mine: {detail}"));
    }
}

/// An edit of an envelope's first account.
type Edit = Box<dyn FnOnce(&mut Value)>;

/// The plaintext envelope with `edit` applied to its first account.
fn edited(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut v: Value = serde_json::from_slice(&plaintext()).unwrap();
    edit(&mut v["accounts"][0]);
    serde_json::to_vec(&v).unwrap()
}

#[test]
fn every_field_is_type_checked_and_a_refusal_names_it_without_quoting_a_value() {
    let cases: [(Edit, &str); 9] = [
        (
            Box::new(|a| a["position"] = json!(0)),
            "account 1: position must be an integer of at least 1",
        ),
        (
            Box::new(|a| a["position"] = json!("2")),
            "account 1: position must be an integer of at least 1",
        ),
        (
            Box::new(|a| a["position"] = json!(4_294_967_296u64)),
            "account 1: position must be an integer of at least 1",
        ),
        (
            Box::new(|a| a["provider"] = json!(7)),
            "account 1: provider must be a provider's name",
        ),
        (
            Box::new(|a| a["kind"] = Value::Null),
            "account 1: kind must be a credential kind",
        ),
        (
            Box::new(|a| a["alias"] = json!(["dev"])),
            "account 1: alias must be a string or null",
        ),
        (
            Box::new(|a| a["disabled"] = json!("no")),
            "account 1: disabled must be true or false",
        ),
        (
            Box::new(|a| a["addedAt"] = json!("yesterday")),
            "account 1: addedAt must be an ISO 8601 time",
        ),
        (
            Box::new(|a| {
                a.as_object_mut().unwrap().remove("identity");
            }),
            "account 1: identity must be present",
        ),
    ];
    for (edit, want) in cases {
        let err = decode(&edited(edit), &[], &mut ask_nothing).unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            ("invalid-input", want)
        );
        assert!(!err.to_string().contains("SENTINEL"));
    }
}

#[test]
fn an_id_in_the_file_is_never_read() {
    let d = decoded(
        &edited(|a| a["id"] = json!("0192-from-another-machine")),
        &[],
    );
    assert!(!format!("{:?}", d.records).contains("0192-from-another"));
}

#[test]
fn a_file_that_is_no_export_is_refused() {
    let not = "the file is not a tagteam export, an age-encrypted one, or a cswap version 1 export";
    for input in [
        &b"not json"[..],
        b"{\"x\": 1}",
        b"{\"format\": \"other\"}",
        b"",
    ] {
        let err = decode(input, &[], &mut ask_nothing).unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            ("invalid-input", not)
        );
    }
    let mut v: Value = serde_json::from_slice(&plaintext()).unwrap();
    v["version"] = json!(2);
    let err = read_envelope(&v).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the export's version is 2; this tagteam reads version 1"
    );
}

const API_KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
const SETUP_TOKEN: &str = "sk-ant-oat01-setup";

/// A cswap version 1 export (inventory §8.1) of `accounts`.
fn cswap(accounts: Value) -> Value {
    json!({"version": 1, "exportedAt": "2026-09-20T10:00:00Z", "exportedFrom": "macos",
        "swapVersion": "0.27.0b1", "encrypted": false, "activeAccountNumber": 1,
        "accounts": accounts})
}

/// The token-account identity cswap records (inventory §2.2).
fn token_account(email: &str) -> Value {
    json!({"emailAddress": email, "accountUuid": "", "organizationUuid": null, "organizationName": null})
}

/// An OAuth account, an API-key account and a setup-token account, as cswap exports them.
fn cswap_three() -> Value {
    cswap(json!([
        {"number": 1, "email": "a@x.co", "uuid": "uuid-a", "organizationUuid": "org-1",
         "organizationName": "Acme", "added": "2026-09-01T08:00:00.123456+00:00",
         "credentials": credential("rt-a"),
         "config": {"oauthAccount": {"emailAddress": "a@x.co", "accountUuid": "uuid-a",
            "organizationUuid": "org-1", "organizationName": "Acme"}},
         "alias": "main"},
        {"number": 2, "email": "api-key-2@token.local", "uuid": "", "organizationUuid": null,
         "organizationName": null, "added": "2026-09-02T08:00:00", "credentials": API_KEY,
         "config": {"oauthAccount": token_account("api-key-2@token.local")}, "kind": "api_key"},
        {"number": 5, "email": "setup-token-5@token.local", "uuid": "", "organizationUuid": null,
         "organizationName": null,
         "credentials": {"claudeAiOauth": {"accessToken": SETUP_TOKEN, "scopes": ["user:inference"]}},
         "config": {"oauthAccount": token_account("setup-token-5@token.local")}}
    ]))
}

#[test]
fn a_cswap_export_maps_its_oauth_api_key_and_setup_token_accounts_to_claude_code() {
    let records = read_cswap_v1(&cswap_three()).unwrap();
    assert_eq!(records.len(), 3);
    for r in &records {
        assert_eq!(r.provider.as_str(), CLAUDE_CODE);
    }
    let positions: Vec<u32> = records.iter().map(|r| r.position).collect();
    assert_eq!(positions, [1, 2, 5]);
    let kinds: Vec<Option<&str>> = records.iter().map(|r| r.kind.as_deref()).collect();
    assert_eq!(kinds, [None, Some("api_key"), None]);
    assert_eq!(records[0].alias.as_deref(), Some("main"));
    assert_eq!(records[0].added_at, Some(1_788_249_600_000));
    assert_eq!(
        records[1].added_at, None,
        "a time with no offset is dropped"
    );
    assert_eq!(
        records[0].identity,
        json!({"email": "a@x.co", "accountUuid": "uuid-a", "organizationUuid": "org-1",
            "organizationName": "Acme", "oauthAccount": {"emailAddress": "a@x.co",
            "accountUuid": "uuid-a", "organizationUuid": "org-1", "organizationName": "Acme"}})
    );
    assert_eq!(records[1].credential, json!(API_KEY));

    // Claude Code's own validation tells the three kinds apart (§7.1).
    let cc = ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs);
    let imported: Vec<String> = records
        .iter()
        .map(|r| cc.import_login(&r.identity, &r.credential).unwrap().kind)
        .collect();
    assert_eq!(imported, ["oauth", "api_key", "setup_token"]);

    let d = decoded(&serde_json::to_vec(&cswap_three()).unwrap(), &[]);
    assert!(d.cswap && !d.encrypted);
    assert_eq!(d.records.len(), 3);
}

#[test]
fn a_cswap_export_that_is_encrypted_or_not_version_1_is_refused_by_name() {
    let mut encrypted = cswap_three();
    encrypted["encrypted"] = json!(true);
    let err = read_cswap_v1(&encrypted).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the cswap export is encrypted (encrypted: true), which tagteam cannot read"
    );
    let mut v2 = cswap_three();
    v2["version"] = json!(2);
    let err = decode(&serde_json::to_vec(&v2).unwrap(), &[], &mut ask_nothing).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the cswap export's version is 2; tagteam reads version 1"
    );
    let mut no_config = cswap_three();
    no_config["accounts"][1]
        .as_object_mut()
        .unwrap()
        .remove("config");
    let err = read_cswap_v1(&no_config).unwrap_err();
    assert_eq!(
        err.to_string(),
        "account 2: config.oauthAccount must be a JSON object"
    );
    assert!(!err.to_string().contains(API_KEY));
}

#[test]
fn an_armored_export_with_leading_whitespace_decrypts() {
    // The file is detected after its leading whitespace; it is decrypted from the same place.
    let file = encrypt(
        &plaintext(),
        &Encryption::Passphrase(SecretString::from("correct horse")),
    )
    .unwrap();
    let mut padded = b"\n \r\n".to_vec();
    padded.extend_from_slice(&file);
    let d = decode(&padded, &[], &mut |_: &Need| {
        Some(SecretString::from("correct horse"))
    })
    .unwrap();
    assert_round_trip(&d);
}

/// An age file (binary) encrypted to `SSH_PK`, its ssh-ed25519 stanza's body damaged: the key
/// matches the stanza's tag and then fails to open it.
fn damaged_ssh_stanza_file() -> Vec<u8> {
    use std::str::FromStr;
    let to = age::ssh::Recipient::from_str(SSH_PK).unwrap();
    let encryptor =
        age::Encryptor::with_recipients([&to as &dyn age::Recipient].into_iter()).unwrap();
    let mut file = Vec::new();
    let mut w = encryptor.wrap_output(&mut file).unwrap();
    std::io::Write::write_all(&mut w, &plaintext()).unwrap();
    w.finish().unwrap();
    let at = file
        .windows(14)
        .position(|w| w == b"-> ssh-ed25519")
        .unwrap();
    let body = at + file[at..].iter().position(|b| *b == b'\n').unwrap() + 1;
    file[body] = if file[body] == b'A' { b'B' } else { b'A' };
    file
}

#[test]
fn a_key_that_fails_to_open_a_file_does_not_blame_a_passphrase() {
    let plain = parse_identity_file("id_plain", SSH_SK.as_bytes()).unwrap();
    let err = decode(&damaged_ssh_stanza_file(), &[plain], &mut ask_nothing).unwrap_err();
    assert_eq!(err.kind(), "decrypt-failed");
    assert_eq!(
        err.to_string(),
        "the file could not be opened with the given key, or it is damaged"
    );
}

#[test]
fn an_encrypted_ssh_key_is_not_asked_for_when_the_file_has_no_ssh_ed25519_stanza() {
    let key = age::x25519::Identity::generate();
    let to = parse_recipient(&key.to_public().to_string()).unwrap();
    let file = encrypt(&plaintext(), &Encryption::Recipients(vec![to])).unwrap();
    let locked = parse_identity_file("id_ed25519", SSH_SK_ENCRYPTED.as_bytes()).unwrap();
    let err = decode(&file, &[locked], &mut ask_nothing).unwrap_err();
    assert_eq!(
        (err.kind(), err.to_string().as_str()),
        (
            "decrypt-failed",
            "none of the --identity keys can decrypt this file"
        )
    );
}

/// OpenSSH's wire `string`: a big-endian length, then the bytes.
fn wire_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_be_bytes());
    out.extend_from_slice(bytes);
}

fn take_wire_string(input: &[u8]) -> (&[u8], &[u8]) {
    let n = u32::from_be_bytes(input[..4].try_into().unwrap()) as usize;
    (&input[4..4 + n], &input[4 + n..])
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_decode(text: &str) -> Vec<u8> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for c in text.bytes().filter(|c| *c != b'=') {
        acc = acc << 6 | B64.iter().position(|b| *b == c).unwrap() as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
        out.push_str(&"=".repeat(3 - chunk.len()));
    }
    out
}

fn pem_blob(pem: &str) -> Vec<u8> {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    base64_decode(&body)
}

/// An OpenSSH private key file holding `blob`.
fn pem(blob: &[u8]) -> String {
    let b64 = base64_encode(blob);
    let lines: Vec<&str> = b64
        .as_bytes()
        .chunks(70)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
        lines.join("\n")
    )
}

/// `rsa_blob` (an OpenSSH private key) with the public key in its header replaced by
/// `ed25519_blob`'s: an RSA private section under an ssh-ed25519 header.
fn splice(rsa_blob: &[u8], ed25519_blob: &[u8]) -> Vec<u8> {
    let magic = b"openssh-key-v1\0";
    let header_of = |blob: &[u8]| {
        let rest = &blob[magic.len()..];
        let (cipher, rest) = take_wire_string(rest);
        let (kdf, rest) = take_wire_string(rest);
        let (opts, rest) = take_wire_string(rest);
        let (public, rest) = take_wire_string(&rest[4..]);
        (
            cipher.to_vec(),
            kdf.to_vec(),
            opts.to_vec(),
            public.to_vec(),
            rest.to_vec(),
        )
    };
    let (cipher, kdf, opts, _, rest) = header_of(rsa_blob);
    let (_, _, _, ed25519_public, _) = header_of(ed25519_blob);
    let mut out = magic.to_vec();
    for s in [&cipher, &kdf, &opts] {
        wire_string(&mut out, s);
    }
    out.extend_from_slice(&1u32.to_be_bytes());
    wire_string(&mut out, &ed25519_public);
    out.extend_from_slice(&rest);
    out
}

/// A freshly generated OpenSSH RSA private key (`ssh-keygen`, test-only, never stored), or
/// `None` when this machine has no `ssh-keygen`.
fn rsa_private_key(passphrase: &str) -> Option<String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsa");
    let made = std::process::Command::new("ssh-keygen")
        .args([
            "-q",
            "-t",
            "rsa",
            "-b",
            "2048",
            "-C",
            "test-only",
            "-N",
            passphrase,
            "-f",
        ])
        .arg(&path)
        .status()
        .ok()?
        .success();
    let pem = made
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()?;
    pem.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----")
        .then_some(pem)
}

const ED25519_ONLY: &str = "keys/x: only age X25519 and ssh-ed25519 keys can decrypt an export";

#[test]
fn an_rsa_ssh_key_is_not_an_identity() {
    let Some(rsa) = rsa_private_key("") else {
        eprintln!("skipped: no ssh-keygen on this machine");
        return;
    };
    let err = parse_identity_file("keys/x", rsa.as_bytes()).unwrap_err();
    assert_eq!(
        (err.kind(), err.to_string().as_str()),
        ("invalid-input", ED25519_ONLY)
    );
}

#[test]
fn an_rsa_private_section_under_an_ed25519_header_is_not_an_identity() {
    // age reads the private section by its own type, so the header's public key alone does
    // not say which key decrypts.
    let Some(rsa) = rsa_private_key("") else {
        eprintln!("skipped: no ssh-keygen on this machine");
        return;
    };
    let spliced = pem(&splice(&pem_blob(&rsa), &pem_blob(SSH_SK)));
    let err = parse_identity_file("keys/x", spliced.as_bytes()).unwrap_err();
    assert_eq!(
        (err.kind(), err.to_string().as_str()),
        ("invalid-input", ED25519_ONLY)
    );
}

#[test]
fn an_encrypted_rsa_private_section_under_an_ed25519_header_is_refused_once_unlocked() {
    let Some(rsa) = rsa_private_key("pw") else {
        eprintln!("skipped: no ssh-keygen on this machine");
        return;
    };
    let spliced = pem(&splice(&pem_blob(&rsa), &pem_blob(SSH_SK)));
    let locked = parse_identity_file("keys/x", spliced.as_bytes()).unwrap();
    let file = encrypt(
        &plaintext(),
        &Encryption::Recipients(vec![parse_recipient(SSH_PK).unwrap()]),
    )
    .unwrap();
    let err = decode(&file, &[locked], &mut |_: &Need| {
        Some(SecretString::from("pw"))
    })
    .unwrap_err();
    assert_eq!(
        (err.kind(), err.to_string().as_str()),
        ("invalid-input", ED25519_ONLY)
    );
}
