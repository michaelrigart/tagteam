mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use common::Fx;
use serde_json::{Value, json};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::oracle::Oracle;
use tagteam_provider::{Credential, Identity, Provider};

fn add_opts(fx: &Fx) -> AddOptions {
    fx.add_options()
}

/// An oracle that always resolves to `None`, but first runs a side effect against the live
/// state it was built with — proving the §10.1 step 3.3 recheck under the lock actually
/// re-verifies the login just captured, not just the identity read before the locks were
/// taken (Task 18's review, item 2).
struct Racing<F>(F);

impl<F: Fn() + Send + Sync> Oracle for Racing<F> {
    fn resolve(&self, _p: &dyn Provider, _c: &Credential) -> Option<Identity> {
        (self.0)();
        None
    }
}

fn racing_engine(
    fx: &Fx,
    side_effect: impl Fn() + Send + Sync + 'static,
) -> tagteam_engine::Engine {
    fx.engine_with_oracle(Arc::new(Racing(side_effect)))
}

fn token_opts(fx: &Fx, token: &str) -> AddTokenOptions {
    fx.add_token_options(token)
}

#[test]
fn add_captures_the_live_login() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let out = fx
        .engine
        .add_live(AddOptions {
            alias: Some("Work".into()),
            ..add_opts(&fx)
        })
        .unwrap();
    let a = &out.account;
    assert!(out.created);
    assert_eq!(
        (a.position, a.label.as_str(), a.kind.as_str()),
        (1, "me@work.co", "oauth")
    );
    assert_eq!(a.alias.as_deref(), Some("work"));
    assert_eq!(a.account_uuid.as_deref(), Some("uuid-me@work.co"));
    assert_eq!(a.login_expires_at, Some(1_797_000_000_000));
    assert_eq!(fx.vault_refresh_token(&a.id).as_deref(), Some("rt-1"));
    assert_eq!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        Some(a.id.clone())
    );
    assert!(
        out.notices.iter().any(|n| n.contains("could not verify")),
        "M1 has no oracle"
    );
}

#[test]
fn adding_the_same_login_again_refreshes_it_in_place() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let first = fx.engine.add_live(add_opts(&fx)).unwrap().account;
    fx.rotate_live("rt-2");
    let again = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(!again.created);
    assert_eq!(again.account.id, first.id);
    assert_eq!(again.account.login_epoch, first.login_epoch + 1);
    assert_eq!(fx.vault_refresh_token(&first.id).as_deref(), Some("rt-2"));
    let prev = fx.kc.get("tagteam", &format!("{}.prev", first.id)).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-1"));
}

#[test]
fn a_failed_vault_write_during_a_rotation_is_reconciled_in_process() {
    // Task 18's review, item 6: a vault write that fails after `begin_replacement` is
    // reconciled immediately, rather than left dangling for the next lock holder to find.
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let first = fx.engine.add_live(add_opts(&fx)).unwrap().account;
    fx.rotate_live("rt-2");
    fx.kc.set_fail_write("tagteam", true);
    assert!(fx.engine.add_live(add_opts(&fx)).is_err());
    fx.kc.set_fail_write("tagteam", false);
    let row = fx
        .engine
        .store()
        .unwrap()
        .account(&first.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.login_epoch, first.login_epoch);
    assert!(row.replacing_fp.is_none());
    assert_eq!(fx.vault_refresh_token(&first.id).as_deref(), Some("rt-1"));
}

#[test]
fn add_refuses_what_it_cannot_safely_capture() {
    let fx = Fx::new();
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::NoLiveLogin)
    ));
    assert!(!fx.env.data_dir().exists());

    fx.login("me@work.co", "rt-1");
    let (svc, acct) = (
        keychain_service(&fx.env, ItemKind::OAuth),
        keychain_account(&fx.env),
    );
    fx.kc.set_unreadable(&svc, &acct, true);
    std::fs::write(
        fx.paths().credentials_file,
        Fx::credential_json("me@work.co", "rt-0").to_string(),
    )
    .unwrap();
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::DegradedRead)
    ));
    fx.kc.set_unreadable(&svc, &acct, false);

    fx.kc.put(
        &keychain_service(&fx.env, ItemKind::ManagedKey),
        &acct,
        b"sk-ant-api03-live",
    );
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::LiveApiKey)
    ));
    assert!(
        fx.engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
}

#[test]
fn add_refuses_a_wiped_live_credential() {
    // Task 13's review: `shape::classify` falls through to `setup_token` for any bytes it
    // does not recognise, wiped ones included, so a wiped live credential must be refused
    // before it ever reaches `classify` — and it must create nothing (§5).
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    fx.set_live_credential(
        json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}})
            .to_string()
            .as_bytes(),
    );
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::NoLiveLogin)
    ));
    assert!(
        fx.engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn an_oracle_naming_someone_else_refuses_the_add() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let other = fx
        .cc
        .parse_identity(&json!({"emailAddress": "other@x.co", "accountUuid": "uuid-other"}))
        .unwrap();
    fx.oracle.set(Some(other));
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::OwnerMismatch { .. })
    ));
    let me = fx
        .cc
        .parse_identity(&Fx::oauth_account("me@work.co"))
        .unwrap();
    fx.oracle.set(Some(me));
    let out = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(out.notices.is_empty());
}

#[test]
fn an_oracle_answer_with_no_uuid_of_its_own_is_treated_as_unresolved() {
    // §7.6, Task 18's review item 3: an oracle answer carries no attribution signal at all
    // without a non-empty account_uuid, so it takes the "could not verify" notice path —
    // never `OwnerMismatch`, even though the email disagrees.
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let other = fx
        .cc
        .parse_identity(&json!({"emailAddress": "other@x.co"}))
        .unwrap();
    assert_eq!(other.account_uuid, None);
    fx.oracle.set(Some(other));
    let out = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(out.notices.iter().any(|n| n.contains("could not verify")));
}

#[test]
fn an_oracle_uuid_that_conflicts_with_the_stored_one_is_refused() {
    // Task 18's review, item 3: the oracle's positive uuid feeds the identity-conflict check
    // even when the live login's own self-reported identity carries none of its own.
    let fx = Fx::new();
    let a = fx.add("me@work.co", "rt-1"); // stored account_uuid: uuid-me@work.co
    let no_uuid =
        json!({"emailAddress": "me@work.co", "organizationUuid": "", "organizationName": null});
    common::splice_oauth_account(&fx.paths().global_config, &no_uuid);
    fx.set_live_credential(
        Fx::credential_json("me@work.co", "rt-2")
            .to_string()
            .as_bytes(),
    );
    let owner = fx
        .cc
        .parse_identity(&json!({"emailAddress": "me@work.co", "organizationUuid": "", "accountUuid": "uuid-different"}))
        .unwrap();
    fx.oracle.set(Some(owner));
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::IdentityConflict { .. })
    ));
    let store = fx.engine.store().unwrap();
    assert_eq!(
        store.account(&a).unwrap().unwrap().account_uuid.as_deref(),
        Some("uuid-me@work.co")
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-1"));
}

#[test]
fn an_occupied_position_needs_confirmation_and_then_replaces_its_occupant() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("b@x.co", "rt-b");
    let at1 = AddOptions {
        position: Some(1),
        ..add_opts(&fx)
    };
    assert!(matches!(
        fx.engine.add_live(at1),
        Err(EngineError::NeedsConfirmation { position: 1, .. })
    ));
    let b = fx
        .engine
        .add_live(AddOptions {
            position: Some(1),
            yes: true,
            ..add_opts(&fx)
        })
        .unwrap()
        .account;
    assert_eq!(b.position, 1);
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
    assert!(fx.vault_bytes(&a).is_none());
}

#[test]
fn a_replacement_that_cannot_be_written_keeps_its_occupant() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("b@x.co", "rt-b");
    fx.engine
        .add_live(AddOptions {
            alias: Some("work".into()),
            ..add_opts(&fx)
        })
        .unwrap();
    fx.login("c@x.co", "rt-c");
    // The alias belongs to someone else: refused before position 1 is touched.
    let clash = AddOptions {
        position: Some(1),
        yes: true,
        alias: Some("work".into()),
        ..add_opts(&fx)
    };
    assert!(matches!(
        fx.engine.add_live(clash),
        Err(EngineError::InvalidInput(_))
    ));
    // The vault refuses the new credential: the occupant survives.
    fx.kc.set_fail_write("tagteam", true);
    assert!(
        fx.engine
            .add_live(AddOptions {
                position: Some(1),
                yes: true,
                ..add_opts(&fx)
            })
            .is_err()
    );
    fx.kc.set_fail_write("tagteam", false);
    let store = fx.engine.store().unwrap();
    assert_eq!(store.account(&a).unwrap().unwrap().position, 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(store.accounts(&fx.provider()).unwrap().len(), 2);
}

#[test]
fn an_unreadable_managed_key_refuses_the_add() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let (svc, acct) = (
        keychain_service(&fx.env, ItemKind::ManagedKey),
        keychain_account(&fx.env),
    );
    fx.kc.put(&svc, &acct, b"sk-ant-api03-live");
    fx.kc.set_unreadable(&svc, &acct, true);
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::Unreadable(_))
    ));
}

#[test]
fn adding_a_login_updates_an_account_of_another_kind_in_place() {
    // §10.1: an existing (email, org) is refreshed in place, its kind included.
    let fx = Fx::new();
    let token = AddTokenOptions {
        email: Some("me@work.co".into()),
        ..token_opts(&fx, "sk-ant-api03-k")
    };
    let before = fx.engine.add_token(token).unwrap().account;
    fx.login("me@work.co", "rt-1");
    let after = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(!after.created);
    assert_eq!(
        (after.account.id, after.account.kind.as_str()),
        (before.id, "oauth")
    );
    assert_eq!(
        after.account.account_uuid.as_deref(),
        Some("uuid-me@work.co")
    );
}

#[test]
fn add_refuses_a_live_login_whose_account_uuid_conflicts_with_the_stored_one() {
    // `update_login`/`finish_replacement` COALESCE a new account_uuid over a known one, so a
    // stored uuid that would be overwritten by a *different* one must be refused rather than
    // silently clobbered.
    let fx = Fx::new();
    let a = fx.add("me@work.co", "rt-1");
    let identity = json!({
        "emailAddress": "me@work.co", "organizationUuid": "", "organizationName": null,
        "accountUuid": "uuid-different"
    });
    common::splice_oauth_account(&fx.paths().global_config, &identity);
    fx.set_live_credential(
        Fx::credential_json("me@work.co", "rt-2")
            .to_string()
            .as_bytes(),
    );
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::IdentityConflict { .. })
    ));
    let store = fx.engine.store().unwrap();
    assert_eq!(
        store.account(&a).unwrap().unwrap().account_uuid.as_deref(),
        Some("uuid-me@work.co")
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-1"));
}

#[test]
fn add_rechecks_the_account_uuid_after_a_pending_replacement_lands() {
    // Task 18's review, item 1: the pre-lock check in `prepare` cannot see a uuid that a
    // pending replacement's metadata only installs once the account lock reconciles it, so
    // the check must run again after the lock is taken.
    let fx = Fx::new();
    let x = fx
        .engine
        .add_token(AddTokenOptions {
            email: Some("me@work.co".into()),
            ..token_opts(&fx, "sk-ant-api03-first")
        })
        .unwrap()
        .account; // a NULL-uuid row
    // An `add` over x landed its vault write but died before its metadata did; the metadata
    // it left behind claims account_uuid U1.
    let cred = Fx::credential_json("me@work.co", "rt-1")
        .to_string()
        .into_bytes();
    let u1 = json!({
        "emailAddress": "me@work.co", "organizationUuid": "", "organizationName": null,
        "accountUuid": "U1"
    });
    fx.begin_replacement(&x.id, &cred, &u1, "oauth");
    // The live login now claims a *different* uuid, U2.
    fx.login("me@work.co", "rt-2");
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::IdentityConflict { .. })
    ));
    let store = fx.engine.store().unwrap();
    let row = store.account(&x.id).unwrap().unwrap();
    assert_eq!(
        row.account_uuid.as_deref(),
        Some("U1"),
        "U1 landed and was not overwritten by U2"
    );
    assert_eq!(fx.vault_refresh_token(&x.id).as_deref(), Some("rt-1"));
}

#[test]
fn add_token_rechecks_the_kind_after_a_pending_replacement_lands() {
    let fx = Fx::new();
    let token = |t: &str| AddTokenOptions {
        email: Some("me@work.co".into()),
        ..token_opts(&fx, t)
    };
    let x = fx
        .engine
        .add_token(token("sk-ant-api03-first"))
        .unwrap()
        .account;
    // An `add` over x with an OAuth login wrote the vault and died before its metadata landed.
    let cred = Fx::credential_json("me@work.co", "rt-1")
        .to_string()
        .into_bytes();
    fx.begin_replacement(&x.id, &cred, &Fx::oauth_account("me@work.co"), "oauth");
    assert!(
        matches!(fx.engine.add_token(token("sk-ant-api03-second")), Err(EngineError::InvalidInput(m)) if m.contains("oauth"))
    );
    assert_eq!(
        fx.vault_refresh_token(&x.id).as_deref(),
        Some("rt-1"),
        "the recovered OAuth credential stays"
    );
}

#[test]
fn a_landed_replacement_can_make_add_token_valid() {
    let fx = Fx::new();
    let x = fx.add("me@work.co", "rt-1"); // an OAuth account
    // An `add` over x with a setup-token login wrote the vault and died before its metadata.
    let setup =
        br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-old","scopes":["user:inference"]}}"#;
    fx.begin_replacement(&x, setup, &Fx::oauth_account("me@work.co"), "setup_token");
    let token = AddTokenOptions {
        email: Some("me@work.co".into()),
        ..token_opts(&fx, "sk-ant-oat01-new")
    };
    let out = fx.engine.add_token(token).unwrap();
    assert_eq!(
        (out.account.id, out.account.kind.as_str()),
        (x, "setup_token")
    );
}

#[test]
fn an_out_of_range_position_creates_nothing() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    assert!(
        fx.engine
            .add_live(AddOptions {
                position: Some(100),
                ..add_opts(&fx)
            })
            .is_err()
    );
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn invalid_token_input_creates_nothing() {
    let fx = Fx::new();
    let bad_email = AddTokenOptions {
        email: Some("nope".into()),
        ..token_opts(&fx, "sk-ant-api03-x")
    };
    assert!(fx.engine.add_token(bad_email).is_err());
    let zero = AddTokenOptions {
        position: Some(0),
        ..token_opts(&fx, "sk-ant-api03-x")
    };
    assert!(fx.engine.add_token(zero).is_err());
    assert!(
        !fx.env.data_dir().exists(),
        "a command that changes nothing creates nothing"
    );
}

#[test]
fn add_token_stores_api_keys_and_setup_tokens() {
    let fx = Fx::new();
    let k = fx
        .engine
        .add_token(token_opts(&fx, "  sk-ant-api03-abc\n"))
        .unwrap()
        .account;
    assert_eq!(
        (k.kind.as_str(), k.label.as_str()),
        ("api_key", "api-key-1@token.local")
    );
    assert_eq!(fx.vault_bytes(&k.id).unwrap(), b"sk-ant-api03-abc");
    let s = fx
        .engine
        .add_token(token_opts(&fx, "sk-ant-oat01-setup"))
        .unwrap()
        .account;
    assert_eq!(
        (s.kind.as_str(), s.label.as_str()),
        ("setup_token", "setup-token-2@token.local")
    );
    let v: serde_json::Value = serde_json::from_slice(&fx.vault_bytes(&s.id).unwrap()).unwrap();
    assert_eq!(
        v,
        json!({"claudeAiOauth": {"accessToken": "sk-ant-oat01-setup", "scopes": ["user:inference"]}})
    );
    assert_eq!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        None
    );
}

#[test]
fn a_defaulted_email_over_a_default_named_token_needs_confirmation() {
    // §10.1/§10.3: `--position 1` over the token already at 1 is an occupied position, even
    // though that token's default email is the one position 1 would default to.
    let fx = Fx::new();
    let first = fx.add_api_key("sk-ant-api03-first-key");
    let at1 = |yes| AddTokenOptions {
        position: Some(1),
        yes,
        ..token_opts(&fx, "sk-ant-api03-second-key")
    };
    assert!(matches!(
        fx.engine.add_token(at1(false)),
        Err(EngineError::NeedsConfirmation { position: 1, .. })
    ));
    assert_eq!(
        fx.vault_bytes(&first).unwrap(),
        b"sk-ant-api03-first-key",
        "the stored token is untouched"
    );
    let out = fx.engine.add_token(at1(true)).unwrap();
    assert!(out.created);
    assert_ne!(
        out.account.id, first,
        "a new account, not the old one rewritten"
    );
    assert_eq!(out.account.position, 1);
    assert_eq!(
        fx.vault_bytes(&out.account.id).unwrap(),
        b"sk-ant-api03-second-key"
    );
    let store = fx.engine.store().unwrap();
    assert!(
        store.account(&first).unwrap().is_none(),
        "the confirmed occupant is replaced"
    );
}

#[test]
fn a_defaulted_email_never_names_an_existing_token_account() {
    // After a move and a remove, the next position's default email belongs to the account
    // moved away from it. A plain `add-token` must add, never rewrite that account's token.
    for (first, second, third) in [
        (
            "sk-ant-api03-first",
            "sk-ant-api03-second",
            "sk-ant-api03-third",
        ),
        (
            "sk-ant-oat01-first",
            "sk-ant-oat01-second",
            "sk-ant-oat01-third",
        ),
    ] {
        let fx = Fx::new();
        let k1 = fx.add_api_key(first); // <prefix>-1 at 1
        let k2 = fx.add_api_key(second); // <prefix>-2 at 2
        fx.engine.move_to(&k2, 1).unwrap(); // k2 at 1, k1 at 2
        fx.engine.remove(&k1).unwrap(); // the next position is 2 again
        let k2_secret = fx.vault_bytes(&k2).unwrap();
        let out = fx.engine.add_token(token_opts(&fx, third)).unwrap();
        assert!(out.created, "{third}");
        assert_ne!(out.account.id, k2, "{third}");
        assert_eq!(out.account.position, 2, "{third}");
        assert_eq!(fx.vault_bytes(&k2).unwrap(), k2_secret, "{third}");
        let store = fx.engine.store().unwrap();
        assert_eq!(store.accounts(&fx.provider()).unwrap().len(), 2, "{third}");
        let labels: std::collections::BTreeSet<String> = store
            .accounts(&fx.provider())
            .unwrap()
            .into_iter()
            .map(|a| a.label)
            .collect();
        assert_eq!(labels.len(), 2, "two distinct identities: {labels:?}");
    }
}

#[test]
fn an_explicit_email_still_replaces_that_token_account_in_place() {
    // §10.2: naming an existing token account's email is the way to replace its token.
    let fx = Fx::new();
    let first = fx.add_api_key("sk-ant-api03-first-key");
    let label = fx
        .engine
        .store()
        .unwrap()
        .account(&first)
        .unwrap()
        .unwrap()
        .label;
    let out = fx
        .engine
        .add_token(AddTokenOptions {
            email: Some(label),
            ..token_opts(&fx, "sk-ant-api03-second-key")
        })
        .unwrap();
    assert!(!out.created);
    assert_eq!(out.account.id, first);
    assert_eq!(fx.vault_bytes(&first).unwrap(), b"sk-ant-api03-second-key");
}

#[test]
fn add_token_validates_its_inputs() {
    let fx = Fx::new();
    assert!(matches!(
        fx.engine.add_token(token_opts(&fx, "  ")),
        Err(EngineError::InvalidInput(_))
    ));
    let bad_email = AddTokenOptions {
        email: Some("not-an-email".into()),
        ..token_opts(&fx, "sk-ant-api03-x")
    };
    assert!(matches!(
        fx.engine.add_token(bad_email),
        Err(EngineError::InvalidInput(_))
    ));
    let email = Some("shared@x.co".to_string());
    fx.engine
        .add_token(AddTokenOptions {
            email: email.clone(),
            ..token_opts(&fx, "sk-ant-api03-x")
        })
        .unwrap();
    let clash = AddTokenOptions {
        email,
        ..token_opts(&fx, "sk-ant-oat01-y")
    };
    assert!(
        matches!(fx.engine.add_token(clash), Err(EngineError::InvalidInput(m)) if m.contains("api_key"))
    );
    let bad_alias = AddTokenOptions {
        alias: Some("123".into()),
        ..token_opts(&fx, "sk-ant-api03-z")
    };
    assert!(matches!(
        fx.engine.add_token(bad_alias),
        Err(EngineError::InvalidInput(_))
    ));
    let zero = AddTokenOptions {
        position: Some(0),
        ..token_opts(&fx, "sk-ant-api03-z")
    };
    assert!(matches!(
        fx.engine.add_token(zero),
        Err(EngineError::InvalidInput(_))
    ));
}

#[test]
fn account_commands_refuse_inside_a_run_shell() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let mut env = fx.env.clone();
    env.claude_config_dir = Some(fx.env.data_dir().join("sessions/x").into_os_string());
    let engine = fx.engine_with_env(env);
    assert!(matches!(
        engine.add_live(add_opts(&fx)),
        Err(EngineError::InsideRunShell)
    ));
    assert!(matches!(
        engine.add_token(token_opts(&fx, "sk-ant-api03-x")),
        Err(EngineError::InsideRunShell)
    ));
}

#[test]
fn add_clears_a_quarantine() {
    // §10.1 step 4: capturing a fresh login clears any quarantine a previous failure left.
    let fx = Fx::new();
    let x = fx.add("me@work.co", "rt-1");
    fx.quarantine(&x, "refresh failed", "sha256:stale");
    assert!(
        fx.engine
            .store()
            .unwrap()
            .account(&x)
            .unwrap()
            .unwrap()
            .quarantine_reason
            .is_some()
    );
    fx.rotate_live("rt-2");
    let out = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert_eq!(out.account.id, x);
    assert!(out.account.quarantine_reason.is_none());
    assert!(out.account.quarantine_fp.is_none());
}

// Task 18's review, item 2: pinning the §10.1 step 3.3 rechecks under the lock. Each test
// below makes the oracle call between the pre-lock read and the locks mutate the live state,
// and checks that `add_live` still refuses (rather than committing what it read before the
// race) and creates no account row. The oracle itself always resolves to `None`, so nothing
// upstream of the recheck would otherwise stop the add.

#[test]
fn add_live_rechecks_the_credential_has_not_rotated_under_the_lock() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let (env, kc) = (fx.env.clone(), fx.kc.clone());
    let engine = racing_engine(&fx, move || {
        let svc = keychain_service(&env, ItemKind::OAuth);
        let acct = keychain_account(&env);
        let mut v: Value = serde_json::from_slice(&kc.get(&svc, &acct).unwrap()).unwrap();
        v["claudeAiOauth"]["refreshToken"] = json!("rt-race");
        kc.put(&svc, &acct, v.to_string().as_bytes());
    });
    assert!(matches!(
        engine.add_live(add_opts(&fx)),
        Err(EngineError::LiveMoved)
    ));
    assert!(
        engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
}

#[test]
fn add_live_rechecks_no_managed_key_appeared_under_the_lock() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let (env, kc) = (fx.env.clone(), fx.kc.clone());
    let engine = racing_engine(&fx, move || {
        kc.put(
            &keychain_service(&env, ItemKind::ManagedKey),
            &keychain_account(&env),
            b"sk-ant-api03-race",
        );
    });
    assert!(matches!(
        engine.add_live(add_opts(&fx)),
        Err(EngineError::LiveApiKey)
    ));
    assert!(
        engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
}

#[test]
fn add_live_rechecks_the_credential_is_still_readable() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let (env, kc) = (fx.env.clone(), fx.kc.clone());
    let engine = racing_engine(&fx, move || {
        kc.set_unreadable(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
            true,
        );
    });
    assert!(matches!(
        engine.add_live(add_opts(&fx)),
        Err(EngineError::Unreadable(_))
    ));
    assert!(
        engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
}

#[test]
fn add_live_rechecks_the_credential_is_still_fresh() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let (env, kc, paths) = (fx.env.clone(), fx.kc.clone(), fx.paths());
    let engine = racing_engine(&fx, move || {
        // A covering plaintext file plus a now-unreadable Keychain item is what makes the
        // next read `Degraded` rather than `Unreadable` (see `LiveStore::read_credential`).
        std::fs::write(
            &paths.credentials_file,
            Fx::credential_json("me@work.co", "rt-1").to_string(),
        )
        .unwrap();
        kc.set_unreadable(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
            true,
        );
    });
    assert!(matches!(
        engine.add_live(add_opts(&fx)),
        Err(EngineError::DegradedRead)
    ));
    assert!(
        engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
}

#[test]
fn add_live_rechecks_the_identity_has_not_changed_under_the_lock() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let path = fx.paths().global_config;
    let engine = racing_engine(&fx, move || {
        let raced = json!({
            "emailAddress": "me@work.co", "organizationUuid": "", "organizationName": null,
            "accountUuid": "uuid-raced"
        });
        common::splice_oauth_account(&path, &raced);
    });
    assert!(matches!(
        engine.add_live(add_opts(&fx)),
        Err(EngineError::LiveMoved)
    ));
    assert!(
        engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
}

#[test]
fn add_live_reports_an_identity_that_becomes_unreadable_under_the_lock_as_unreadable() {
    // Task 18's review, item 9: an `Unreadable` identity on the re-read must surface as
    // `Unreadable`, not collapse into the generic `LiveMoved`.
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let path = fx.paths().global_config;
    let engine = racing_engine(&fx, move || {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    });
    let result = engine.add_live(add_opts(&fx));
    // Restored before any assertion can panic and leave the tempdir unreadable for cleanup.
    std::fs::set_permissions(
        &fx.paths().global_config,
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(matches!(result, Err(EngineError::Unreadable(_))));
}
