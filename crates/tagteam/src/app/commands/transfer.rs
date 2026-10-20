use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;

use tagteam_engine::EngineError;
use tagteam_engine::export::ExportRequest;
use tagteam_engine::transfer::{self, Encryption, Need, Recipient, SecretString};

use crate::{prompt, transfer_cmd};

use crate::app::App;
use crate::app::failure::{
    EXIT_ERROR, Failure, KIND_INVALID_INPUT, KIND_IO, KIND_NEEDS_PASSPHRASE, KIND_NOTHING_EXPORTED,
    cancelled,
};

const TOKEN_MISSING: &str = "pass the token as an argument, or `-` to read it from stdin";

const EXPORT_TO_STDOUT_UNDER_JSON: &str =
    "export - writes the export to stdout, which --json needs for its result; name a file";
const EXPORT_NEEDS_PASSPHRASE: &str = "an export is encrypted with a passphrase typed on a terminal; pass --recipient or --recipient-file to encrypt it to a key, or --plaintext to write it unencrypted";
const EMPTY_PASSPHRASE: &str =
    "an empty passphrase protects nothing; pass --plaintext to write the export unencrypted";
const PASSPHRASES_DIFFER: &str = "the two passphrases differ";
const PASSPHRASE: &str = "Passphrase for the export: ";
const PASSPHRASE_AGAIN: &str = "The same passphrase again: ";
const NOTHING_EXPORTED: &str = "no account was exported";
const IMPORT_FROM_TERMINAL: &str =
    "import - reads the export from stdin, which is a terminal; pipe the file in, or name it";

/// `--recipient` and `--recipient-file` (§13.3), each public key parsed before anything else
/// runs. A bad key is a usage error naming the flag, and never quoted: it may be a private key
/// pasted by mistake.
fn recipients(flags: &[String], files: &[PathBuf]) -> Result<Vec<Recipient>, Failure> {
    let mut out = Vec::new();
    for r in flags {
        out.push(
            transfer::parse_recipient(r)
                .map_err(|e| Failure::Usage(format!("--recipient: {e}")))?,
        );
    }
    for f in files {
        let named = |e: &dyn std::fmt::Display| {
            Failure::Usage(format!("--recipient-file {}: {e}", f.display()))
        };
        let text = fs::read_to_string(f).map_err(|e| named(&e))?;
        out.extend(transfer::parse_recipients(&text).map_err(|e| named(&e))?);
    }
    Ok(out)
}

impl App<'_, '_> {
    /// §13.3: `export`. The destination and the encryption are settled before any account is
    /// read: a destination that cannot be written refuses first, and the
    /// passphrase is asked for, and recipients parsed, before any lock is taken. The Keychain
    /// check comes next (Appendix A.3), then the engine. Encryption and the write run once the
    /// engine has released every lock.
    pub(in crate::app) fn export(
        &mut self,
        file: Option<String>,
        account: &[String],
        full: bool,
        recipient: &[String],
        recipient_file: &[PathBuf],
        plaintext: bool,
    ) -> Result<(), Failure> {
        let path = file.filter(|f| f != "-").map(PathBuf::from);
        if path.is_none() && self.json {
            return Err(Failure::Usage(EXPORT_TO_STDOUT_UNDER_JSON.into()));
        }
        let pending = match &path {
            Some(p) => Some(
                transfer_cmd::ExportFile::create(p)
                    .map_err(|e| Failure::Message(KIND_IO, e.to_string()))?,
            ),
            None => None,
        };
        let recipients = recipients(recipient, recipient_file)?;
        let accounts = if account.is_empty() {
            None
        } else {
            Some(
                account
                    .iter()
                    .map(|a| self.resolve(a).map(|r| r.id))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        };
        let encryption = if plaintext {
            Encryption::Plaintext
        } else if !recipients.is_empty() {
            Encryption::Recipients(recipients)
        } else {
            Encryption::Passphrase(self.new_passphrase()?)
        };
        if self.engine.existing_store()?.is_some() {
            self.ensure_unlocked()?;
        }
        let result = self.engine.export(&ExportRequest {
            accounts,
            provider: self.provider_flag.clone(),
            full,
        })?;
        for w in transfer_cmd::export_warnings(&result) {
            self.io.warn(&w);
        }
        if result.accounts.is_empty() {
            return Err(Failure::Message(
                KIND_NOTHING_EXPORTED,
                NOTHING_EXPORTED.into(),
            ));
        }
        let bytes = transfer::encrypt(&result.envelope, &encryption).map_err(EngineError::from)?;
        let encrypted = encryption.is_encrypted();
        let human = transfer_cmd::export_human(&result, path.as_deref(), encrypted);
        match pending {
            Some(f) => {
                f.publish(&bytes).map_err(EngineError::from)?;
                let json = transfer_cmd::export_json(&result, path.as_deref(), encrypted);
                self.print(&human, json);
            }
            None => {
                self.io.out.write_all(&bytes).map_err(EngineError::from)?;
                // stdout carries the export, so the summary goes to stderr.
                let _ = write!(self.io.err, "{human}");
            }
        }
        Ok(())
    }

    /// §13.3: the export's passphrase, asked twice without echo, only where a person can
    /// answer and never under `--json`.
    fn new_passphrase(&mut self) -> Result<SecretString, Failure> {
        if !self.can_prompt() {
            return Err(Failure::Message(
                KIND_NEEDS_PASSPHRASE,
                EXPORT_NEEDS_PASSPHRASE.into(),
            ));
        }
        let first = self.io.prompter.secret(PASSPHRASE);
        self.after_prompt()?;
        let first = first.ok_or_else(cancelled)?;
        if first.is_empty() {
            return Err(Failure::Message(
                KIND_INVALID_INPUT,
                EMPTY_PASSPHRASE.into(),
            ));
        }
        let again = self.io.prompter.secret(PASSPHRASE_AGAIN);
        self.after_prompt()?;
        if again.ok_or_else(cancelled)? != first {
            return Err(Failure::Message(
                KIND_INVALID_INPUT,
                PASSPHRASES_DIFFER.into(),
            ));
        }
        Ok(SecretString::from(first))
    }

    /// §13.3: `import`. Refused inside a run shell before anything is read (§12.8). The file is
    /// read and decoded first, a passphrase asked for only when the file needs one, and only
    /// where a person can answer; then the Keychain check (Appendix A.3), and the engine's two
    /// passes. It exits 1 if any account failed.
    pub(in crate::app) fn import(
        &mut self,
        file: &str,
        force: bool,
        identity: &[PathBuf],
    ) -> Result<i32, Failure> {
        self.refuse_in_run_shell()?;
        let input = if file == "-" {
            if std::io::stdin().is_terminal() {
                return Err(Failure::Usage(IMPORT_FROM_TERMINAL.into()));
            }
            let all = prompt::read_piped_all(self.engine.cancel()).map_err(EngineError::Io)?;
            self.after_prompt()?;
            all.ok_or_else(cancelled)?
        } else {
            fs::read(file).map_err(|e| Failure::Message(KIND_IO, format!("{file}: {e}")))?
        };
        let mut keys = Vec::new();
        for path in identity {
            let name = path.display().to_string();
            let bytes =
                fs::read(path).map_err(|e| Failure::Message(KIND_IO, format!("{name}: {e}")))?;
            keys.push(transfer::parse_identity_file(&name, &bytes).map_err(EngineError::from)?);
        }
        let can_ask = !self.json && self.io.prompter.reaches_terminal();
        let what = if file == "-" { "the export" } else { file };
        let prompter = &mut *self.io.prompter;
        let decoded =
            transfer::decode(&input, &keys, self.engine.registry(), &mut |need: &Need| {
                if !can_ask {
                    return None;
                }
                let question = match need {
                    Need::Passphrase => format!("Passphrase for {what}: "),
                    Need::KeyPassphrase(key) => format!("Passphrase for the SSH key {key}: "),
                };
                prompter.secret(&question).map(SecretString::from)
            });
        self.after_prompt()?;
        let decoded = decoded.map_err(EngineError::from)?;
        self.ensure_unlocked()?;
        let report = self.engine.import(decoded.records, force)?;
        for w in &report.warnings {
            self.io.warn(w);
        }
        self.print(
            &transfer_cmd::import_human(&report),
            transfer_cmd::import_json(&report),
        );
        Ok(if report.any_failed() { EXIT_ERROR } else { 0 })
    }

    /// §10.2's token source: `-` reads one line from stdin; none prompts without echo, and
    /// only when a person can answer.
    pub(super) fn token(&mut self, token: Option<String>) -> Result<String, Failure> {
        match token {
            Some(t) if t == "-" && std::io::stdin().is_terminal() => {
                // A plain read would sit through Ctrl-C until Enter (the handler restarts it).
                let line =
                    prompt::read_terminal_line(self.engine.cancel()).map_err(EngineError::Io)?;
                self.after_prompt()?;
                line.ok_or_else(cancelled)
            }
            Some(t) if t == "-" => {
                // Read in slices the token ends, and checked again after the read: a signal that
                // landed during it reports the interruption, never the input it cut short.
                let line =
                    prompt::read_piped_line(self.engine.cancel()).map_err(EngineError::Io)?;
                self.after_prompt()?;
                line.ok_or_else(cancelled)
            }
            Some(t) => Ok(prompt::note_argv_token(&mut *self.io.err, t)),
            None if self.can_prompt() => {
                let token = self.io.prompter.secret("Token: ");
                self.after_prompt()?;
                token.ok_or_else(cancelled)
            }
            None => Err(Failure::Message(KIND_INVALID_INPUT, TOKEN_MISSING.into())),
        }
    }
}
