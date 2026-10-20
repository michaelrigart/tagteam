mod common;

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{
    Fx, block_rescue, capture_logs, credential, due, quarantine_of, rescue_files, token_requests,
    unblock_rescue, vault_fp,
};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::refresh::GateOutcome;
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault, VaultBackend, VaultError};
use tagteam_provider::http::Method;
use tagteam_provider::{Clock, Read, ReadError};

fn refresh(fx: &Fx, id: &AccountId, snapshot: &[u8]) -> GateOutcome {
    fx.engine
        .refresh_stored(fx.cc.as_ref(), id, snapshot)
        .unwrap()
}

/// Every rescue envelope on disk (§6.3), parsed. None when `rescue/` does not exist or is
/// not a directory.
fn rescues(fx: &Fx) -> Vec<Value> {
    let Ok(dir) = fs::read_dir(fx.env.data_dir().join("rescue")) else {
        return vec![];
    };
    dir.map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap())
        .collect()
}

/// The refresh token inside each rescue envelope's credential.
fn rescued_refresh_tokens(fx: &Fx) -> Vec<String> {
    rescues(fx)
        .iter()
        .map(|envelope| {
            let cred: Value =
                serde_json::from_str(envelope["credential"].as_str().unwrap()).unwrap();
            cred["claudeAiOauth"]["refreshToken"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// A vault that stores `{}` whenever `target`'s current generation is written, as a Keychain
/// that silently kept something else would: the read-back never matches.
struct GarblingVault {
    inner: KeychainVault,
    target: String,
}

impl VaultBackend for GarblingVault {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        self.inner.read(key)
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        if key == self.target {
            self.inner.write(key, b"{}")
        } else {
            self.inner.write(key, bytes)
        }
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.inner.delete(key)
    }
}

fn garbling_vault(fx: &Fx, id: &AccountId) -> Vault {
    Vault::new(Box::new(GarblingVault {
        inner: KeychainVault::new(fx.kc.clone()),
        target: id.to_string(),
    }))
}

#[test]
fn a_vault_that_refuses_the_write_leaves_the_successor_in_rescue_for_the_next_pass() {
    let fx = Fx::new();
    let a = due(&fx);
    let sent = vault_fp(&fx, &a);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    let out = refresh(&fx, &a, &snapshot);
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: true } if kind == "vault-write"),
        "{out:?}"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    let envelopes = rescues(&fx);
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0]["predecessorFp"], json!(sent));
    assert_eq!(rescued_refresh_tokens(&fx), ["rt-a2"]);

    // The next pass adopts it without a second request (§7.3 step 3).
    fx.kc.set_fail_write(SERVICE, false);
    let out = refresh(&fx, &a, &snapshot);
    assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert!(rescues(&fx).is_empty(), "an adopted rescue is deleted");
    assert_eq!(fx.http.count(Method::Post, &Fx::endpoints().token), 1);
}

#[test]
fn a_vault_that_stores_something_else_leaves_the_successor_in_rescue() {
    let fx = Fx::new();
    let a = due(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    let engine = fx.engine_with_vault(garbling_vault(&fx, &a));
    let out = engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: true } if kind == "vault-write"),
        "{out:?}"
    );
    assert_eq!(rescued_refresh_tokens(&fx), ["rt-a2"]);
}

#[test]
fn both_writes_failing_is_reported_as_unpersisted_and_quarantines() {
    let fx = Fx::new();
    let a = due(&fx);
    let sent = vault_fp(&fx, &a);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    block_rescue(&fx);
    assert!(matches!(
        refresh(&fx, &a, &snapshot),
        GateOutcome::Unpersisted
    ));
    unblock_rescue(&fx);
    assert_eq!(
        fx.http.count(Method::Post, &Fx::endpoints().token),
        1,
        "the request was sent"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("successor_lost".into()), Some(sent)),
        "the vault's generation is consumed (§7.4 successor_lost)"
    );
}

#[test]
fn a_response_without_a_refresh_token_keeps_the_lineage() {
    // Review Focus 3, persistence half: the old refresh token is kept, so the fingerprint is
    // unchanged and `.prev` does not rotate.
    let fx = Fx::new();
    let a = due(&fx);
    let before = vault_fp(&fx, &a);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(None);
    assert!(matches!(
        refresh(&fx, &a, &snapshot),
        GateOutcome::Refreshed(_)
    ));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(vault_fp(&fx, &a), before, "the lineage is unchanged");
    assert!(
        fx.kc.get(SERVICE, &format!("{a}.prev")).is_none(),
        ".prev rotates only on a lineage change (§6.2)"
    );
    let stored: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
    assert_eq!(stored["claudeAiOauth"]["accessToken"], "at-same");
}

/// A vault whose first read of `target` after writing it fails: a read-back that could not
/// confirm a write that landed.
struct ReadBackFailsOnce {
    inner: KeychainVault,
    target: String,
    armed: AtomicBool,
}

impl VaultBackend for ReadBackFailsOnce {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        if key == self.target && self.armed.swap(false, Ordering::SeqCst) {
            return Read::Unreadable(ReadError::new("vault", "the read-back failed"));
        }
        self.inner.read(key)
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        self.inner.write(key, bytes)?;
        if key == self.target {
            self.armed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.inner.delete(key)
    }
}

#[test]
fn a_landed_write_whose_read_back_failed_loses_nothing_when_the_successor_has_machine_shared_keys()
{
    // H5.3: `persist_received` compares the vault form of what it received. The vault still
    // holds a pre-branch item, so the successor (built from it) carries `mcpOAuth`.
    let fx = Fx::new();
    let a = due(&fx);
    let mut old = Fx::credential_json("a@x.co", "rt-a");
    old["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms() + 60_000);
    fx.put_vault(&a, old.to_string().as_bytes());
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    let engine = fx.engine_with_vault(Vault::new(Box::new(ReadBackFailsOnce {
        inner: KeychainVault::new(fx.kc.clone()),
        target: a.to_string(),
        armed: AtomicBool::new(false),
    })));
    let out = engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    assert!(matches!(out, GateOutcome::Refreshed(_)), "{out:?}");
    let stored: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
    assert_eq!(stored["claudeAiOauth"]["refreshToken"], "rt-a2");
    assert!(stored.get("mcpOAuth").is_none(), "the vault form");
    assert_eq!(rescue_files(&fx), 0, "nothing was kept outside the vault");
}

#[test]
fn a_same_generation_rescue_is_deleted_and_never_adopted_over_a_newer_access_token() {
    let fx = Fx::new();
    let a = due(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(None); // no refresh token in the reply: the successor keeps the fingerprint
    fx.kc.set_fail_write(SERVICE, true);
    let out = refresh(&fx, &a, &snapshot);
    fx.kc.set_fail_write(SERVICE, false);
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: true } if kind == "vault-write"),
        "{out:?}"
    );
    assert_eq!(rescue_files(&fx), 1);
    // Another writer stores a newer access token of the same generation meanwhile.
    let mut newer: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
    newer["claudeAiOauth"]["accessToken"] = json!("at-newer");
    newer["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms() + 3_600_000);
    fx.put_vault(&a, newer.to_string().as_bytes());
    let out = refresh(&fx, &a, &snapshot);
    assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
    assert_eq!(rescue_files(&fx), 0);
    let stored: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
    assert_eq!(
        stored["claudeAiOauth"]["accessToken"], "at-newer",
        "the rescue's at-same never went over it"
    );
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn a_failed_write_ahead_is_logged_and_the_vault_still_takes_the_successor() {
    let fx = Fx::new();
    let a = due(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    block_rescue(&fx);
    let (out, logs) = capture_logs(|| refresh(&fx, &a, &snapshot));
    unblock_rescue(&fx);
    assert!(matches!(out, GateOutcome::Refreshed(_)), "{out:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(rescue_files(&fx), 0);
    assert!(
        logs.iter().any(|l| l.contains("WARN")
            && l.contains("could not write a refreshed token ahead to rescue/")),
        "{logs:#?}"
    );
}

#[test]
fn a_rescue_written_before_this_branch_still_settles_into_the_vault_form() {
    // Review Focus line 2, rescue half: a pre-branch envelope holds full bytes, `mcpOAuth` included.
    let fx = Fx::new();
    let a = due(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    let planted = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a2"));
    let envelope: Value = serde_json::from_slice(&fs::read(&planted).unwrap()).unwrap();
    assert!(
        envelope["credential"]
            .as_str()
            .unwrap()
            .contains("mcpOAuth"),
        "the envelope holds what a pre-branch gate kept: the full bytes"
    );
    let out = refresh(&fx, &a, &snapshot);
    assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
    let stored: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
    assert_eq!(stored["claudeAiOauth"]["refreshToken"], "rt-a2");
    assert!(
        stored.get("mcpOAuth").is_none(),
        "adopted in the vault form"
    );
    assert_eq!(rescue_files(&fx), 0);
    assert_eq!(token_requests(&fx), 0);
}

#[cfg(feature = "test-hooks")]
mod hooked {
    use std::os::unix::fs::MetadataExt;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};

    use tagteam_engine::Engine;

    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use tagteam_engine::EngineError;

    /// `(path, inode, envelope)` of every file in `rescue/`, by path (a type alias keeps clippy's
    /// `type_complexity` quiet).
    type Listing = Vec<(PathBuf, u64, Value)>;

    fn listing(dir: &Path) -> Listing {
        let Ok(entries) = fs::read_dir(dir) else {
            return vec![];
        };
        let mut out: Listing = entries
            .map(|e| {
                let path = e.unwrap().path();
                let inode = fs::metadata(&path).unwrap().ino();
                let envelope = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                (path, inode, envelope)
            })
            .collect();
        out.sort_by(|x, y| x.0.cmp(&y.0));
        out
    }

    /// What `rescue/` held each time the engine passed `point`.
    fn rescues_at(fx: &Fx, point: &'static str) -> Arc<Mutex<Vec<Listing>>> {
        let seen: Arc<Mutex<Vec<_>>> = Arc::default();
        let (slot, dir) = (seen.clone(), fx.env.data_dir().join("rescue"));
        fx.engine.on_point(
            point,
            Box::new(move || slot.lock().unwrap().push(listing(&dir))),
        );
        seen
    }

    #[test]
    fn an_own_successor_is_written_ahead_before_the_vault_re_read_and_deleted_once_stored() {
        let fx = Fx::new();
        let a = due(&fx);
        let sent = vault_fp(&fx, &a);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.script_refresh(Some("rt-a2"));
        let seen = rescues_at(&fx, "gate-after-response"); // before persist_successor's re-read
        let (out, logs) = capture_logs(|| refresh(&fx, &a, &snapshot));
        assert!(matches!(out, GateOutcome::Refreshed(_)), "{out:?}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].len(), 1, "written ahead");
        assert_eq!(seen[0][0].2["predecessorFp"], json!(sent));
        assert!(
            rescues(&fx).is_empty(),
            "deleted once the vault write verified"
        );
        assert!(
            logs.iter()
                .any(|l| l.contains("DEBUG")
                    && l.contains("wrote a refreshed token ahead to rescue/"))
        );
        assert!(
            !logs
                .iter()
                .any(|l| l.contains("kept a refreshed token in rescue/")),
            "routine: no INFO line"
        );
    }

    #[test]
    fn a_crash_between_the_write_ahead_and_the_vault_write_is_adopted_by_the_next_gate_pass() {
        let fx = Fx::new();
        let a = due(&fx);
        let sent = vault_fp(&fx, &a);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.script_refresh(Some("rt-a2"));
        let seen = rescues_at(&fx, "gate-after-response");
        fx.engine.fail_at(Some("gate-before-vault-write"));
        assert!(
            fx.engine
                .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
                .is_err()
        );
        fx.engine.fail_at(None);
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a"),
            "the vault write never started"
        );
        assert_eq!(
            listing(&fx.env.data_dir().join("rescue")),
            seen.lock().unwrap()[0],
            "no second copy"
        );
        assert_eq!(rescued_refresh_tokens(&fx), ["rt-a2"]);
        assert_eq!(rescues(&fx)[0]["predecessorFp"], json!(sent));
        let out = refresh(&fx, &a, &snapshot); // step 3 adopts it; no second request
        assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
        assert!(rescues(&fx).is_empty());
        assert_eq!(token_requests(&fx), 1);
    }

    #[test]
    fn a_crash_after_the_vault_write_leaves_a_rescue_the_landed_rule_deletes() {
        // Replayed by putting back the envelope `gate-before-vault-write` saw, once the vault holds it.
        let fx = Fx::new();
        let a = due(&fx);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.script_refresh(Some("rt-a2"));
        let seen = rescues_at(&fx, "gate-before-vault-write");
        assert!(matches!(
            refresh(&fx, &a, &snapshot),
            GateOutcome::Refreshed(_)
        ));
        let (landed, prev) = (
            fx.vault_bytes(&a).unwrap(),
            fx.kc.get(SERVICE, &format!("{a}.prev")),
        );
        for (path, _, envelope) in &seen.lock().unwrap()[0] {
            fs::write(path, envelope.to_string()).unwrap();
        }
        assert_eq!(rescue_files(&fx), 1);
        let out = refresh(&fx, &a, &snapshot);
        assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
        assert_eq!(rescue_files(&fx), 0, "deleted by the landed rule");
        assert_eq!(fx.vault_bytes(&a).unwrap(), landed, "never adopted again");
        assert_eq!(
            fx.kc.get(SERVICE, &format!("{a}.prev")),
            prev,
            ".prev did not rotate"
        );
        assert_eq!(token_requests(&fx), 1);
    }

    #[test]
    fn a_failed_vault_write_keeps_the_envelope_written_ahead_and_writes_no_second_copy() {
        let fx = Fx::new();
        let a = due(&fx);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.script_refresh(Some("rt-a2"));
        let seen = rescues_at(&fx, "gate-after-response");
        fx.kc.set_fail_write(SERVICE, true);
        let (out, logs) = capture_logs(|| refresh(&fx, &a, &snapshot));
        fx.kc.set_fail_write(SERVICE, false);
        assert!(
            matches!(&out, GateOutcome::Transient { kind, rescued: true } if kind == "vault-write"),
            "{out:?}"
        );
        assert_eq!(
            listing(&fx.env.data_dir().join("rescue")),
            seen.lock().unwrap()[0],
            "same file, same inode"
        );
        assert_eq!(
            logs.iter()
                .filter(|l| l.contains("kept a refreshed token in rescue/ until the vault takes it"))
                .count(),
            1,
            "{logs:#?}"
        );
    }

    #[test]
    fn a_failed_write_ahead_is_logged_only_after_the_vault_write() {
        // §7.3 step 6: while the write-ahead has failed, the successor is only in memory, so
        // nothing, its WARN included, comes before the vault write; whether the vault then
        // takes it or not, the WARN follows, once.
        for vault_fails in [false, true] {
            let fx = Fx::new();
            let a = due(&fx);
            let snapshot = fx.vault_bytes(&a).unwrap();
            fx.script_refresh(Some("rt-a2"));
            fx.engine.on_point(
                "gate-before-vault-write",
                Box::new(|| tracing::info!("marker: the vault write starts")),
            );
            block_rescue(&fx);
            fx.kc.set_fail_write(SERVICE, vault_fails);
            let (out, logs) = capture_logs(|| refresh(&fx, &a, &snapshot));
            fx.kc.set_fail_write(SERVICE, false);
            unblock_rescue(&fx);
            let lines = |text: &str| -> Vec<usize> {
                logs.iter()
                    .enumerate()
                    .filter(|(_, l)| l.contains(text))
                    .map(|(i, _)| i)
                    .collect()
            };
            let (marker, warn) = (
                lines("marker: the vault write starts"),
                lines("could not write a refreshed token ahead to rescue/"),
            );
            assert_eq!(
                (marker.len(), warn.len()),
                (1, 1),
                "{vault_fails}: {logs:#?}"
            );
            assert!(marker[0] < warn[0], "{vault_fails}: {logs:#?}");
            assert!(
                logs[warn[0]].contains("WARN"),
                "{vault_fails}: {}",
                logs[warn[0]]
            );
            if vault_fails {
                assert!(matches!(out, GateOutcome::Unpersisted), "{out:?}");
            } else {
                assert!(matches!(out, GateOutcome::Refreshed(_)), "{out:?}");
            }
        }
    }

    #[test]
    fn a_foreign_successor_is_never_written_ahead() {
        let fx = Fx::new();
        let a = due(&fx);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.http.push_json(
            Method::Post,
            &Fx::endpoints().token,
            200,
            json!({"access_token": "at-a2", "refresh_token": "rt-a2", "expires_in": 28800,
                   "account": {"uuid": "uuid-someone-else"}}),
        );
        let seen = rescues_at(&fx, "gate-after-response");
        let out = refresh(&fx, &a, &snapshot);
        assert!(matches!(out, GateOutcome::Dead(_)), "{out:?}");
        assert!(seen.lock().unwrap()[0].is_empty());
        assert!(rescues(&fx).is_empty());
        assert_eq!(fx.displaced().len(), 1);
    }

    #[test]
    fn the_vault_moving_during_the_request_keeps_the_newer_generation() {
        let fx = Fx::new();
        let a = due(&fx);
        let sent = vault_fp(&fx, &a);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.script_refresh(Some("rt-a2"));
        let mut newer: Value = serde_json::from_slice(&snapshot).unwrap();
        newer["claudeAiOauth"]["refreshToken"] = json!("rt-a-written-meanwhile");
        let newer = newer.to_string().into_bytes();
        let (kc, id, bytes) = (fx.kc.clone(), a.clone(), newer.clone());
        fx.engine.on_point(
            "gate-after-response",
            Box::new(move || kc.put(SERVICE, id.as_str(), &bytes)),
        );
        let GateOutcome::AlreadyFresh(returned) = refresh(&fx, &a, &snapshot) else {
            panic!("expected AlreadyFresh")
        };
        assert_eq!(returned, newer, "the vault's newer credential is returned");
        assert_eq!(rescued_refresh_tokens(&fx), ["rt-a2"]);
        assert_eq!(rescues(&fx)[0]["predecessorFp"], json!(sent));
    }

    /// The second engine's writer, running on its own thread.
    type Writer = thread::JoinHandle<Result<(), EngineError>>;

    #[derive(Debug, Clone, Copy)]
    enum Fault {
        VaultWrite,
        VaultVerify,
        VaultAndRescue,
        ErrorAfterResponse,
        PanicAfterResponse,
        ErrorBeforeVaultWrite,
        PanicBeforeVaultWrite,
        /// An error after the response while `rescue/` is unwritable: the successor cannot be
        /// kept, and the gate must say so rather than return the error.
        ErrorAfterResponseRescueBlocked,
        /// Panics while `rescue/` is unwritable: the guard's `Drop` can neither keep the
        /// successor nor return `Unpersisted`, so it records the loss (§7.3 step 6, amended).
        PanicAfterResponseRescueBlocked,
        PanicBeforeVaultWriteRescueBlocked,
    }

    const FAULTS: [Fault; 10] = [
        Fault::VaultWrite,
        Fault::VaultVerify,
        Fault::VaultAndRescue,
        Fault::ErrorAfterResponse,
        Fault::PanicAfterResponse,
        Fault::ErrorBeforeVaultWrite,
        Fault::PanicBeforeVaultWrite,
        Fault::ErrorAfterResponseRescueBlocked,
        Fault::PanicAfterResponseRescueBlocked,
        Fault::PanicBeforeVaultWriteRescueBlocked,
    ];

    /// §15.3 "Refresh tokens" and §1.1 criterion 2: for every fault injected after the token
    /// response is received, the successor ends up in the vault or in `rescue/`; when every
    /// write fails, the gate reports `Unpersisted`, never the injected error, and quarantines
    /// the account `successor_lost`. A panic that nothing can keep the successor through still
    /// records the loss: the same quarantine, and an ERROR log line (§7.3 step 6, §7.4).
    #[test]
    fn a_received_successor_is_never_discarded() {
        for fault in FAULTS {
            let fx = Fx::new();
            let a = due(&fx);
            let sent = vault_fp(&fx, &a);
            let snapshot = fx.vault_bytes(&a).unwrap();
            fx.script_refresh(Some("rt-a2"));
            let garbling;
            let engine: &Engine = match fault {
                Fault::VaultVerify => {
                    garbling = fx.engine_with_vault(garbling_vault(&fx, &a));
                    &garbling
                }
                _ => &fx.engine,
            };
            match fault {
                Fault::VaultWrite => fx.kc.set_fail_write(SERVICE, true),
                Fault::VaultVerify => {}
                Fault::VaultAndRescue => {
                    fx.kc.set_fail_write(SERVICE, true);
                    block_rescue(&fx);
                }
                Fault::ErrorAfterResponse => engine.fail_at(Some("gate-after-response")),
                Fault::PanicAfterResponse => engine.fail_at(Some("panic:gate-after-response")),
                Fault::ErrorBeforeVaultWrite => engine.fail_at(Some("gate-before-vault-write")),
                Fault::PanicBeforeVaultWrite => {
                    engine.fail_at(Some("panic:gate-before-vault-write"))
                }
                Fault::ErrorAfterResponseRescueBlocked => {
                    engine.fail_at(Some("gate-after-response"));
                    block_rescue(&fx);
                }
                Fault::PanicAfterResponseRescueBlocked => {
                    engine.fail_at(Some("panic:gate-after-response"));
                    block_rescue(&fx);
                }
                Fault::PanicBeforeVaultWriteRescueBlocked => {
                    engine.fail_at(Some("panic:gate-before-vault-write"));
                    block_rescue(&fx);
                }
            }
            let result = catch_unwind(AssertUnwindSafe(|| {
                engine.refresh_stored(fx.cc.as_ref(), &a, &snapshot)
            }));
            let unpersisted = matches!(result, Ok(Ok(GateOutcome::Unpersisted)));
            let kept = fx.vault_refresh_token(&a).as_deref() == Some("rt-a2")
                || rescued_refresh_tokens(&fx).iter().any(|rt| rt == "rt-a2");
            let loss_recorded =
                quarantine_of(&fx, &a) == (Some("successor_lost".into()), Some(sent.clone()));
            match fault {
                Fault::PanicAfterResponseRescueBlocked
                | Fault::PanicBeforeVaultWriteRescueBlocked => {
                    assert!(result.is_err(), "{fault:?}: the injected panic propagates");
                    assert!(!kept, "{fault:?}: nothing could keep the successor");
                    assert!(
                        loss_recorded,
                        "{fault:?}: the loss quarantines the account (§7.4 successor_lost)"
                    );
                }
                _ => {
                    assert!(
                        kept || unpersisted,
                        "{fault:?}: the successor was discarded ({result:?})"
                    );
                    if unpersisted {
                        assert!(
                            loss_recorded,
                            "{fault:?}: Unpersisted quarantines successor_lost"
                        );
                    }
                }
            }
            if matches!(
                fault,
                Fault::VaultAndRescue | Fault::ErrorAfterResponseRescueBlocked
            ) {
                assert!(unpersisted, "{fault:?}: {result:?}");
            }
            unblock_rescue(&fx);
        }
    }

    /// §7.3 step 6 and §7.4: a successor the response says belongs to another account is never
    /// an adoptable rescue, even when an error interrupts the gate before it could be
    /// displaced. It is displaced instead, the account quarantined, and the error returned.
    #[test]
    fn a_foreign_successor_is_displaced_never_rescued_even_when_interrupted() {
        let fx = Fx::new();
        let a = due(&fx);
        let sent = vault_fp(&fx, &a);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.http.push_json(
            Method::Post,
            &Fx::endpoints().token,
            200,
            json!({"access_token": "at-a2", "refresh_token": "rt-a2", "expires_in": 28800,
                   "account": {"uuid": "uuid-someone-else"}}),
        );
        fx.engine.fail_at(Some("gate-after-response"));
        assert!(
            fx.engine
                .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
                .is_err(),
            "the injected error is returned once the successor is kept"
        );
        assert!(rescues(&fx).is_empty(), "never an adoptable rescue");
        assert_eq!(fx.displaced().len(), 1, "displaced instead");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
        assert_eq!(
            quarantine_of(&fx, &a),
            (Some("identity_conflict".into()), Some(sent))
        );
    }

    /// §15.2 "every vault writer racing the refresh gate", for the writers of an inactive
    /// account that M2a has: a switch to it, and `remove`. Each runs in a second engine (a
    /// second tagteam process) while the gate holds the account lock across its request. It
    /// waits for that lock, then acts on the gate's successor, never on the generation the
    /// gate consumed (§6.2), and never makes a second request.
    #[test]
    fn a_vault_writer_waits_for_the_gate_then_builds_on_its_successor() {
        for writer in ["switch", "remove"] {
            let fx = Fx::new();
            let a = due(&fx);
            let snapshot = fx.vault_bytes(&a).unwrap();
            fx.script_refresh(Some("rt-a2"));
            let other = Mutex::new(Some(fx.engine_with_env(fx.env.clone())));
            let running: Arc<Mutex<Option<Writer>>> = Arc::default();
            let (slot, target, request) =
                (running.clone(), a.clone(), fx.switch_request(&a, false));
            fx.engine.on_point(
                "gate-before-request",
                Box::new(move || {
                    let engine = other.lock().unwrap().take().expect("one request per gate");
                    let (target, request) = (target.clone(), request.clone());
                    let handle = thread::spawn(move || match writer {
                        "switch" => engine.switch(request).map(drop),
                        _ => engine.remove(&target).map(drop),
                    });
                    thread::sleep(Duration::from_millis(300));
                    assert!(
                        !handle.is_finished(),
                        "the {writer} must wait for the account lock the gate holds (§6.2)"
                    );
                    *slot.lock().unwrap() = Some(handle);
                }),
            );
            assert!(
                matches!(refresh(&fx, &a, &snapshot), GateOutcome::Refreshed(_)),
                "{writer}"
            );
            let handle = running.lock().unwrap().take().expect("the writer ran");
            handle.join().unwrap().unwrap();
            match writer {
                "switch" => assert_eq!(
                    fx.live_refresh_token().as_deref(),
                    Some("rt-a2"),
                    "the switch activated the gate's successor, not the generation it consumed"
                ),
                _ => assert!(
                    fx.vault_bytes(&a).is_none(),
                    "remove ran after the gate, on what the gate stored"
                ),
            }
            assert!(
                rescues(&fx).is_empty(),
                "{writer}: the successor reached the vault"
            );
            assert_eq!(
                fx.http.count(Method::Post, &Fx::endpoints().token),
                1,
                "{writer}: exactly one refresh request"
            );
        }
    }
}
