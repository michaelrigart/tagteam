//! §13.3's export file (Decision 10): the envelope inside the encryption, its age encryption,
//! and the formats `import` detects: armored or binary age, tagteam's plaintext envelope, and a
//! cswap version 1 export (Decision 14). Pure: nothing here reads the store, the vault, a lock
//! or the network. The engine owns the envelope's own fields; each account's `identity` and
//! `credential` are its provider's (`Provider::export_login`, B.45).

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::str::FromStr;

use age::armor::{ArmoredReader, ArmoredWriter, Format};
/// A passphrase, which `Debug` never shows; the CLI builds one from what the person typed.
pub use age::secrecy::SecretString;
use serde_json::{Map, Value, json};
use tagteam_core::time::{format_iso8601, parse_iso8601};
use tagteam_core::{CLAUDE_CODE, ProviderId};

/// The envelope's `format` and `version` (§13.3).
pub const FORMAT: &str = "tagteam-export";
pub const VERSION: u64 = 1;

/// How an armored age file starts, and a binary one (age-encryption.org/v1).
const ARMOR_BEGIN: &[u8] = b"-----BEGIN AGE ENCRYPTED FILE-----";
const AGE_MAGIC: &[u8] = b"age-encryption.org/";

const NOT_AN_EXPORT: &str =
    "the file is not a tagteam export, an age-encrypted one, or a cswap version 1 export";
const WRONG_PASSPHRASE: &str = "the passphrase is wrong, or the file is damaged";
const NO_MATCHING_KEY: &str = "none of the --identity keys can decrypt this file";
const KEY_DID_NOT_OPEN: &str = "the file could not be opened with the given key, or it is damaged";
const ED25519_ONLY: &str = "only age X25519 and ssh-ed25519 keys can decrypt an export";
const BAD_RECIPIENT: &str = "a recipient must be an age1… or ssh-ed25519 public key";

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// Not a format import reads, or one of them malformed. The message names the account and
    /// the field, never a value, which may be a secret.
    #[error("{0}")]
    Format(String),
    /// A file encrypted with a passphrase, and no passphrase to try: there is no terminal, or
    /// `--json` forbids prompting (§13.3).
    #[error(
        "the file is encrypted with a passphrase, which needs a terminal to type it; run the import on one, without --json"
    )]
    NeedsPassphrase,
    /// A file encrypted to age or SSH keys, and no `--identity`.
    #[error(
        "the file is encrypted to age or SSH keys; pass --identity with the matching private key"
    )]
    NeedsIdentity,
    /// An encrypted SSH key whose passphrase could not be asked for; it names the file.
    #[error("{0} is an encrypted SSH key, and its passphrase needs a terminal to type it")]
    NeedsKeyPassphrase(String),
    /// A wrong passphrase, no key that matches, or a damaged file.
    #[error("{0}")]
    Decrypt(String),
    /// Not an `age1…` or `ssh-ed25519` public key. Never quotes the text, which may be a
    /// private key pasted by mistake.
    #[error("{0}")]
    Recipient(String),
    /// An `--identity` file holding nothing that can decrypt.
    #[error("{file}: {detail}")]
    Identity { file: String, detail: String },
    #[error("the export could not be encrypted: {0}")]
    Encrypt(String),
}

impl TransferError {
    /// Stable `error.type` for `--json` output (§14).
    pub fn kind(&self) -> &'static str {
        match self {
            TransferError::Format(_)
            | TransferError::Recipient(_)
            | TransferError::Identity { .. } => "invalid-input",
            TransferError::NeedsPassphrase | TransferError::NeedsKeyPassphrase(_) => {
                "needs-passphrase"
            }
            TransferError::NeedsIdentity => "needs-identity",
            TransferError::Decrypt(_) => "decrypt-failed",
            TransferError::Encrypt(_) => "encrypt-failed",
        }
    }
}

fn format_error(message: impl Into<String>) -> TransferError {
    TransferError::Format(message.into())
}

/// One account as the envelope carries it (§13.3): the engine's fields, then its provider's
/// payload. It holds a credential, so it has no `Debug`.
pub struct EnvelopeAccount {
    pub provider: ProviderId,
    pub position: u32,
    pub kind: String,
    pub label: String,
    pub alias: Option<String>,
    pub disabled: bool,
    /// Epoch ms, as the store keeps it.
    pub added_at: i64,
    pub identity: Value,
    pub credential: Value,
}

/// `macos` or `linux` (§1.2).
fn exported_from() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// §13.3's plaintext envelope, pretty-printed and ending in a newline. `active` is the live
/// login's position per provider, for information only: import ignores it. Nothing
/// machine-local is written: no account ID, login epoch, activation epoch, quarantine, usage
/// or `rejected_fp`.
pub fn envelope(
    now_ms: i64,
    active: &BTreeMap<String, u32>,
    accounts: &[EnvelopeAccount],
) -> Vec<u8> {
    let accounts: Vec<Value> = accounts
        .iter()
        .map(|a| {
            json!({
                "provider": a.provider.as_str(),
                "position": a.position,
                "kind": a.kind,
                "label": a.label,
                "alias": a.alias,
                "disabled": a.disabled,
                "addedAt": format_iso8601(a.added_at.div_euclid(1000)),
                "identity": a.identity,
                "credential": a.credential,
            })
        })
        .collect();
    let v = json!({
        "format": FORMAT,
        "version": VERSION,
        "exportedAt": format_iso8601(now_ms.div_euclid(1000)),
        "exportedFrom": exported_from(),
        "tagteamVersion": env!("CARGO_PKG_VERSION"),
        "active": active,
        "accounts": accounts,
    });
    let mut out = serde_json::to_vec_pretty(&v).expect("a Value always serializes");
    out.push(b'\n');
    out
}

/// One account an import file holds: read and type-checked, not yet validated by its provider
/// (§13.3 pass 1). An account ID is never read from a file (§6.1, B.65). It holds a
/// credential, so `Debug` shows neither payload.
#[derive(Clone)]
pub struct ImportRecord {
    pub provider: ProviderId,
    pub position: u32,
    /// The file's kind, held against the provider's classification of the credential. `None`
    /// for a cswap account whose kind cswap leaves to the credential's shape.
    pub kind: Option<String>,
    /// For information: the stored label comes from the identity the provider validates.
    pub label: Option<String>,
    pub alias: Option<String>,
    pub disabled: bool,
    /// Epoch ms.
    pub added_at: Option<i64>,
    pub identity: Value,
    pub credential: Value,
}

impl fmt::Debug for ImportRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportRecord")
            .field("provider", &self.provider)
            .field("position", &self.position)
            .field("kind", &self.kind)
            .field("alias", &self.alias)
            .field("disabled", &self.disabled)
            .field("added_at", &self.added_at)
            .finish_non_exhaustive()
    }
}

/// What one account of a file failed, naming the field and the type it must have.
fn field_error(account: usize, field: &str, must_be: &str) -> TransferError {
    format_error(format!("account {account}: {field} must be {must_be}"))
}

/// A string, or nothing when absent or null; any other type is refused.
fn optional_string(
    o: &Map<String, Value>,
    account: usize,
    field: &str,
) -> Result<Option<String>, TransferError> {
    match o.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(field_error(account, field, "a string or null")),
    }
}

/// An integer position ≥ 1 (§13.3 pass 1: a path-traversal defence, B.34).
fn position(o: &Map<String, Value>, account: usize, field: &str) -> Result<u32, TransferError> {
    o.get(field)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n >= 1)
        .ok_or_else(|| field_error(account, field, "an integer of at least 1"))
}

fn disabled(o: &Map<String, Value>, account: usize) -> Result<bool, TransferError> {
    match o.get("disabled") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(field_error(account, "disabled", "true or false")),
    }
}

/// A payload the provider validates (§13.3): present and not null.
fn payload(o: &Map<String, Value>, account: usize, field: &str) -> Result<Value, TransferError> {
    match o.get(field) {
        None | Some(Value::Null) => Err(field_error(account, field, "present")),
        Some(v) => Ok(v.clone()),
    }
}

/// A file's accounts, each numbered from 1 as the errors name them.
type Numbered = Vec<(usize, Map<String, Value>)>;

/// The `accounts` array, each entry an object.
fn accounts(v: &Value, what: &str) -> Result<Numbered, TransferError> {
    let list = v
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| format_error(format!("{what} has no accounts list")))?;
    list.iter()
        .enumerate()
        .map(|(i, a)| match a {
            Value::Object(o) => Ok((i + 1, o.clone())),
            _ => Err(format_error(format!(
                "account {}: it is not a JSON object",
                i + 1
            ))),
        })
        .collect()
}

/// §13.3's envelope, read back: `format` and `version` first, then every account's fields,
/// each of its type. Unknown keys, an `id` included, are never read.
pub fn read_envelope(v: &Value) -> Result<Vec<ImportRecord>, TransferError> {
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(format_error(NOT_AN_EXPORT));
    }
    match v.get("version").and_then(Value::as_u64) {
        Some(VERSION) => {}
        Some(n) => {
            return Err(format_error(format!(
                "the export's version is {n}; this tagteam reads version {VERSION}"
            )));
        }
        None => return Err(format_error("the export's version is not a number")),
    }
    accounts(v, "the export")?
        .into_iter()
        .map(|(i, o)| {
            let provider = match o.get("provider") {
                Some(Value::String(p)) if !p.is_empty() => ProviderId::new(p.as_str()),
                _ => return Err(field_error(i, "provider", "a provider's name")),
            };
            let kind = match o.get("kind") {
                Some(Value::String(k)) if !k.is_empty() => k.clone(),
                _ => return Err(field_error(i, "kind", "a credential kind")),
            };
            let added_at = match optional_string(&o, i, "addedAt")? {
                Some(s) => Some(
                    parse_iso8601(&s)
                        .ok_or_else(|| field_error(i, "addedAt", "an ISO 8601 time"))?
                        * 1000,
                ),
                None => None,
            };
            Ok(ImportRecord {
                provider,
                position: position(&o, i, "position")?,
                kind: Some(kind),
                label: optional_string(&o, i, "label")?,
                alias: optional_string(&o, i, "alias")?,
                disabled: disabled(&o, i)?,
                added_at,
                identity: payload(&o, i, "identity")?,
                credential: payload(&o, i, "credential")?,
            })
        })
        .collect()
}

/// Decision 14: a cswap version 1 export (cswap inventory §8.1), as records for Claude Code,
/// the provider cswap drives. `version` must be 1 and `encrypted` false; either is refused by
/// name otherwise.
///
/// Each account maps `number` to the position, `email`, `uuid`, `organizationUuid`,
/// `organizationName` and `config.oauthAccount` to Claude Code's identity payload (§13.3's
/// example), and `credentials` to its credential payload: the credential object, or an
/// `sk-ant-api…` string for an API-key account. cswap marks only API-key accounts
/// (`kind: "api_key"`); its OAuth logins and setup tokens carry no kind, which Claude Code
/// tells apart from the credential (§7.1). An `added` time tagteam cannot read is dropped.
pub fn read_cswap_v1(v: &Value) -> Result<Vec<ImportRecord>, TransferError> {
    match v.get("version").and_then(Value::as_u64) {
        Some(1) => {}
        Some(n) => {
            return Err(format_error(format!(
                "the cswap export's version is {n}; tagteam reads version 1"
            )));
        }
        None => return Err(format_error("the cswap export's version is not a number")),
    }
    if v.get("encrypted").and_then(Value::as_bool) == Some(true) {
        return Err(format_error(
            "the cswap export is encrypted (encrypted: true), which tagteam cannot read",
        ));
    }
    accounts(v, "the cswap export")?
        .into_iter()
        .map(|(i, o)| {
            let email = match o.get("email") {
                Some(Value::String(e)) if !e.is_empty() => e.clone(),
                _ => return Err(field_error(i, "email", "an email address")),
            };
            let oauth_account = match o.get("config").and_then(|c| c.get("oauthAccount")) {
                Some(a @ Value::Object(_)) => a.clone(),
                _ => return Err(field_error(i, "config.oauthAccount", "a JSON object")),
            };
            let credentials = match o.get("credentials") {
                Some(c @ (Value::Object(_) | Value::String(_))) => c.clone(),
                _ => {
                    return Err(field_error(i, "credentials", "a JSON object or an API key"));
                }
            };
            let kind = match optional_string(&o, i, "kind")?.as_deref() {
                None | Some("oauth") => None,
                Some(other) => Some(other.to_owned()),
            };
            let identity = json!({
                "email": email,
                "accountUuid": optional_string(&o, i, "uuid")?,
                "organizationUuid": optional_string(&o, i, "organizationUuid")?,
                "organizationName": optional_string(&o, i, "organizationName")?,
                "oauthAccount": oauth_account,
            });
            Ok(ImportRecord {
                provider: ProviderId::new(CLAUDE_CODE),
                position: position(&o, i, "number")?,
                kind,
                label: Some(email),
                alias: optional_string(&o, i, "alias")?,
                disabled: disabled(&o, i)?,
                added_at: optional_string(&o, i, "added")?
                    .as_deref()
                    .and_then(parse_iso8601)
                    .map(|s| s * 1000),
                identity,
                credential: credentials,
            })
        })
        .collect()
}

/// A public key an export is encrypted to (§13.3).
#[derive(Clone)]
pub enum Recipient {
    X25519(age::x25519::Recipient),
    Ssh(age::ssh::Recipient),
}

impl Recipient {
    fn as_age(&self) -> &dyn age::Recipient {
        match self {
            Recipient::X25519(r) => r,
            Recipient::Ssh(r) => r,
        }
    }
}

/// A public key, so it may be shown.
impl fmt::Debug for Recipient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Recipient::X25519(r) => write!(f, "Recipient({r})"),
            Recipient::Ssh(r) => write!(f, "Recipient({r})"),
        }
    }
}

/// One recipient: `age1…`, or `ssh-ed25519 <key> [comment]` (§13.3). An `ssh-rsa` key, or
/// anything else, is refused without being quoted.
pub fn parse_recipient(text: &str) -> Result<Recipient, TransferError> {
    let t = text.trim();
    let refused = || TransferError::Recipient(BAD_RECIPIENT.into());
    if t.starts_with("age1") {
        return age::x25519::Recipient::from_str(t)
            .map(Recipient::X25519)
            .map_err(|_| refused());
    }
    match age::ssh::Recipient::from_str(t) {
        Ok(r @ age::ssh::Recipient::SshEd25519(..)) => Ok(Recipient::Ssh(r)),
        _ => Err(refused()),
    }
}

/// `--recipient-file`'s contents: one recipient per line; blank lines and `#` comments are
/// skipped. A bad line is named by its number, and a file with no recipient is refused.
pub fn parse_recipients(text: &str) -> Result<Vec<Recipient>, TransferError> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(
            parse_recipient(line).map_err(|_| {
                TransferError::Recipient(format!("line {}: {BAD_RECIPIENT}", n + 1))
            })?,
        );
    }
    if out.is_empty() {
        return Err(TransferError::Recipient("it holds no recipient".into()));
    }
    Ok(out)
}

/// How `encrypt` protects an export (§13.3). It holds a passphrase, so `Debug` shows only
/// which.
pub enum Encryption {
    Passphrase(SecretString),
    Recipients(Vec<Recipient>),
    Plaintext,
}

impl fmt::Debug for Encryption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Encryption::Passphrase(_) => f.write_str("Passphrase(<redacted>)"),
            Encryption::Recipients(r) => f.debug_tuple("Recipients").field(r).finish(),
            Encryption::Plaintext => f.write_str("Plaintext"),
        }
    }
}

impl Encryption {
    pub fn is_encrypted(&self) -> bool {
        !matches!(self, Encryption::Plaintext)
    }
}

fn encrypt_error(e: impl fmt::Display) -> TransferError {
    TransferError::Encrypt(e.to_string())
}

/// The export as written (§13.3): age, ASCII-armored, to a passphrase (scrypt) or to every
/// recipient; `Plaintext` returns it unchanged.
pub fn encrypt(plaintext: &[u8], to: &Encryption) -> Result<Vec<u8>, TransferError> {
    let encryptor = match to {
        Encryption::Plaintext => return Ok(plaintext.to_vec()),
        Encryption::Passphrase(p) => age::Encryptor::with_user_passphrase(p.clone()),
        Encryption::Recipients(rs) => {
            age::Encryptor::with_recipients(rs.iter().map(Recipient::as_age))
                .map_err(encrypt_error)?
        }
    };
    let mut out = Vec::new();
    let armored =
        ArmoredWriter::wrap_output(&mut out, Format::AsciiArmor).map_err(encrypt_error)?;
    let mut writer = encryptor.wrap_output(armored).map_err(encrypt_error)?;
    writer.write_all(plaintext).map_err(encrypt_error)?;
    writer
        .finish()
        .and_then(|armored| armored.finish())
        .map_err(encrypt_error)?;
    Ok(out)
}

/// One key of an `--identity` file.
enum Key {
    Age(Box<dyn age::Identity + Send + Sync>),
    Ssh(age::ssh::Identity),
    /// An OpenSSH ed25519 key encrypted with a passphrase, asked for only when it is needed.
    SshEncrypted(age::ssh::EncryptedKey),
}

/// The keys one `--identity` file holds (§13.3): age X25519 identities, one per line with `#`
/// comments, or one OpenSSH ed25519 private key, which may be encrypted. It holds private
/// keys, so `Debug` names only the file.
pub struct IdentityFile {
    file: String,
    keys: Vec<Key>,
}

impl fmt::Debug for IdentityFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentityFile")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

/// Parses `bytes`, the contents of the identity file `file` names. An RSA key, a hardware
/// key, or anything that holds no key is refused, naming the file, never its contents.
pub fn parse_identity_file(file: &str, bytes: &[u8]) -> Result<IdentityFile, TransferError> {
    let refused = |detail: &str| TransferError::Identity {
        file: file.to_owned(),
        detail: detail.to_owned(),
    };
    let keys = if bytes.trim_ascii_start().starts_with(b"-----BEGIN") {
        let key = age::ssh::Identity::from_buffer(bytes, Some(file.to_owned()))
            .map_err(|_| refused("it is not a private key tagteam can read"))?;
        // The header's public half says the type of an encrypted key, which is checked again
        // once it is unlocked. An unencrypted key's private section is read by its own type,
        // not the header's, so it must open what is encrypted to its own public half.
        let encrypted = matches!(key, age::ssh::Identity::Encrypted(_));
        if !is_ed25519_recipient(&key) || (!encrypted && !opens_its_own_recipient(&key)) {
            return Err(refused(ED25519_ONLY));
        }
        match key {
            age::ssh::Identity::Encrypted(key) => vec![Key::SshEncrypted(key)],
            key => vec![Key::Ssh(key)],
        }
    } else {
        age::IdentityFile::from_buffer(bytes)
            .map_err(|_| refused("it is not an age identity file"))?
            .into_identities()
            .map_err(|_| refused("it is not an age identity file"))?
            .into_iter()
            .map(Key::Age)
            .collect()
    };
    if keys.is_empty() {
        return Err(refused("it holds no key"));
    }
    Ok(IdentityFile {
        file: file.to_owned(),
        keys,
    })
}

/// Whether the key's public half, which names its type, is an ssh-ed25519 one.
fn is_ed25519_recipient(key: &age::ssh::Identity) -> bool {
    matches!(
        age::ssh::Recipient::try_from(key.clone()),
        Ok(age::ssh::Recipient::SshEd25519(..))
    )
}

/// Whether `key` decrypts what is encrypted to the ssh-ed25519 recipient derived from its
/// public half. A private section of another type (an RSA key under an ed25519 header) does
/// not, and so never reaches the `rsa` crate's decryption (RUSTSEC-2023-0071).
fn opens_its_own_recipient(key: &age::ssh::Identity) -> bool {
    let Ok(recipient) = age::ssh::Recipient::try_from(key.clone()) else {
        return false;
    };
    let probe = b"tagteam";
    let mut sealed = Vec::new();
    let sealed_ok =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .ok()
            .and_then(|e| e.wrap_output(&mut sealed).ok())
            .is_some_and(|mut w| w.write_all(probe).is_ok() && w.finish().is_ok());
    if !sealed_ok {
        return false;
    }
    let Ok(decryptor) = age::Decryptor::new_buffered(&sealed[..]) else {
        return false;
    };
    let Ok(mut reader) = decryptor.decrypt(std::iter::once(key as &dyn age::Identity)) else {
        return false;
    };
    let mut opened = Vec::new();
    reader.read_to_end(&mut opened).is_ok() && opened == probe
}

/// What `decode` asks its caller for, only when the file needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    /// The file is encrypted with a passphrase (scrypt).
    Passphrase,
    /// An `--identity` file, named, is an encrypted SSH key.
    KeyPassphrase(String),
}

/// What an import file held (§13.3).
#[derive(Debug)]
pub struct Decoded {
    pub encrypted: bool,
    /// A cswap version 1 export rather than tagteam's own.
    pub cswap: bool,
    pub records: Vec<ImportRecord>,
}

/// §13.3's detection: armored or binary age, decrypted with `ask`'s passphrase when the file
/// is encrypted to one, else with `identities`; then tagteam's plaintext envelope, or a cswap
/// version 1 export. `ask` is called only when the file needs what it asks for, and `None`
/// from it refuses with the need named.
pub fn decode(
    input: &[u8],
    identities: &[IdentityFile],
    ask: &mut dyn FnMut(&Need) -> Option<SecretString>,
) -> Result<Decoded, TransferError> {
    let start = input.trim_ascii_start();
    let encrypted = start.starts_with(ARMOR_BEGIN) || start.starts_with(AGE_MAGIC);
    let plaintext = if encrypted {
        decrypt(start, identities, ask)?
    } else {
        input.to_vec()
    };
    let v: Value = serde_json::from_slice(&plaintext).map_err(|_| format_error(NOT_AN_EXPORT))?;
    let (cswap, records) = if v.get("format").is_some() {
        (false, read_envelope(&v)?)
    } else if v.get("swapVersion").is_some() {
        (true, read_cswap_v1(&v)?)
    } else {
        return Err(format_error(NOT_AN_EXPORT));
    };
    Ok(Decoded {
        encrypted,
        cswap,
        records,
    })
}

/// How one decryption attempt ended.
enum Attempt {
    Plaintext(Vec<u8>),
    /// No key the attempt tried matches the file's recipients.
    NoMatch,
}

fn damaged(e: impl fmt::Display) -> TransferError {
    TransferError::Decrypt(format!("the file could not be decrypted: {e}"))
}

/// One attempt with `keys`, over a fresh read of `input`: a `Decryptor` is spent by a try.
/// `passphrase` says whether the keys are a passphrase, which a failure then blames.
fn attempt(
    input: &[u8],
    keys: &[&dyn age::Identity],
    passphrase: bool,
) -> Result<Attempt, TransferError> {
    let decryptor = age::Decryptor::new_buffered(ArmoredReader::new(input)).map_err(damaged)?;
    match decryptor.decrypt(keys.iter().copied()) {
        Ok(mut reader) => {
            let mut out = Vec::new();
            reader.read_to_end(&mut out).map_err(damaged)?;
            Ok(Attempt::Plaintext(out))
        }
        Err(age::DecryptError::NoMatchingKeys) => Ok(Attempt::NoMatch),
        Err(age::DecryptError::DecryptionFailed) => Err(TransferError::Decrypt(
            if passphrase {
                WRONG_PASSPHRASE
            } else {
                KEY_DID_NOT_OPEN
            }
            .into(),
        )),
        Err(e) => Err(damaged(e)),
    }
}

/// The recipient stanza types of an age file's header, armored or binary, or `None` when the
/// header cannot be read.
fn stanza_types(input: &[u8]) -> Option<Vec<String>> {
    let mut head = Vec::new();
    ArmoredReader::new(input)
        .take(64 * 1024)
        .read_to_end(&mut head)
        .ok()?;
    let mut lines = head.split(|b| *b == b'\n');
    lines.next()?;
    let mut types = Vec::new();
    for line in lines {
        if line.starts_with(b"---") {
            return Some(types);
        }
        if let Some(stanza) = line.strip_prefix(b"-> ") {
            let name = stanza.split(|b| *b == b' ').next()?;
            types.push(String::from_utf8_lossy(name).into_owned());
        }
    }
    None
}

fn decrypt(
    input: &[u8],
    identities: &[IdentityFile],
    ask: &mut dyn FnMut(&Need) -> Option<SecretString>,
) -> Result<Vec<u8>, TransferError> {
    let decryptor = age::Decryptor::new_buffered(ArmoredReader::new(input)).map_err(damaged)?;
    if decryptor.is_scrypt() {
        let passphrase = ask(&Need::Passphrase).ok_or(TransferError::NeedsPassphrase)?;
        let key = age::scrypt::Identity::new(passphrase);
        return match attempt(input, &[&key], true)? {
            Attempt::Plaintext(p) => Ok(p),
            Attempt::NoMatch => Err(TransferError::Decrypt(WRONG_PASSPHRASE.into())),
        };
    }
    if identities.is_empty() {
        return Err(TransferError::NeedsIdentity);
    }
    let ready: Vec<&dyn age::Identity> = identities
        .iter()
        .flat_map(|f| &f.keys)
        .filter_map(|k| match k {
            Key::Age(k) => Some(k.as_ref() as &dyn age::Identity),
            Key::Ssh(k) => Some(k as &dyn age::Identity),
            Key::SshEncrypted(_) => None,
        })
        .collect();
    if !ready.is_empty() {
        if let Attempt::Plaintext(p) = attempt(input, &ready, false)? {
            return Ok(p);
        }
    }
    // An encrypted SSH key can only open an ssh-ed25519 stanza: without one, its passphrase is
    // not worth asking for. A header that cannot be read leaves the question to the key.
    let may_hold_ssh_stanza =
        stanza_types(input).is_none_or(|t| t.iter().any(|t| t == "ssh-ed25519"));
    for file in identities.iter().filter(|_| may_hold_ssh_stanza) {
        for key in &file.keys {
            let Key::SshEncrypted(encrypted) = key else {
                continue;
            };
            let passphrase = ask(&Need::KeyPassphrase(file.file.clone()))
                .ok_or_else(|| TransferError::NeedsKeyPassphrase(file.file.clone()))?;
            let unlocked = encrypted.decrypt(passphrase).map_err(|_| {
                TransferError::Decrypt(format!("{}: the SSH key's passphrase is wrong", file.file))
            })?;
            let unlocked = age::ssh::Identity::from(unlocked);
            if !is_ed25519_recipient(&unlocked) || !opens_its_own_recipient(&unlocked) {
                return Err(TransferError::Identity {
                    file: file.file.clone(),
                    detail: ED25519_ONLY.into(),
                });
            }
            if let Attempt::Plaintext(p) = attempt(input, &[&unlocked], false)? {
                return Ok(p);
            }
        }
    }
    Err(TransferError::Decrypt(NO_MATCHING_KEY.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_kinds_are_pinned() {
        let cases = [
            (TransferError::Format("x".into()), "invalid-input"),
            (TransferError::Recipient("x".into()), "invalid-input"),
            (
                TransferError::Identity {
                    file: "f".into(),
                    detail: "d".into(),
                },
                "invalid-input",
            ),
            (TransferError::NeedsPassphrase, "needs-passphrase"),
            (
                TransferError::NeedsKeyPassphrase("f".into()),
                "needs-passphrase",
            ),
            (TransferError::NeedsIdentity, "needs-identity"),
            (TransferError::Decrypt("x".into()), "decrypt-failed"),
            (TransferError::Encrypt("x".into()), "encrypt-failed"),
        ];
        for (e, want) in cases {
            assert_eq!(e.kind(), want, "{e:?}");
        }
    }

    #[test]
    fn debug_shows_no_payload_or_passphrase() {
        let r = ImportRecord {
            provider: ProviderId::new(CLAUDE_CODE),
            position: 1,
            kind: Some("oauth".into()),
            label: Some("who@example.com".into()),
            alias: None,
            disabled: false,
            added_at: None,
            identity: json!({"email": "who@example.com"}),
            credential: json!({"claudeAiOauth": {"refreshToken": "SENTINEL"}}),
        };
        let shown = format!("{r:?}");
        assert!(
            !shown.contains("SENTINEL") && !shown.contains("who@"),
            "{shown}"
        );
        let p = format!(
            "{:?}",
            Encryption::Passphrase(SecretString::from("pw-SENTINEL"))
        );
        assert!(!p.contains("SENTINEL"), "{p}");
    }
}
