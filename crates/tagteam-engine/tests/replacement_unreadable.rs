//! §12.5: a replacement that landed but whose `replacing_meta` cannot be read can be neither
//! installed nor undone. Every holder of the account's lock refuses with
//! `replacement-unreadable`, naming the account and `tagteam remove`; `remove` deletes it.
mod common;

use common::{Fx, credential};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::store::LoginMeta;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Provider;

/// Two shapes a `replacing_meta` cannot be read in: not JSON at all, and JSON without the
/// fields a login needs (as an incompatible version might write).
const UNREADABLE: [&str; 2] = ["{\"identity_key\": \"a@x.co", "{\"org_uuid\": \"\"}"];

/// Sets `id`'s recorded replacement metadata to `meta`, behind the store's back.
fn corrupt_meta(fx: &Fx, id: &AccountId, meta: &str) {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET replacing_meta = ?1 WHERE id = ?2",
            [meta, id.as_str()],
        )
        .unwrap();
}

/// `a@x.co` at position 1 and `b@x.co` at position 2 (live), and a replacement of `a` whose
/// vault write landed (`rt-a9`) and whose metadata reads as `meta`.
fn landed_unreadable(meta: &str) -> (Fx, AccountId) {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.begin_replacement(
        &a,
        &credential("a@x.co", "rt-a9"),
        &Fx::oauth_account("a@x.co"),
        "oauth",
    );
    corrupt_meta(&fx, &a, meta);
    (fx, a)
}

fn assert_refused(result: Result<impl std::fmt::Debug, EngineError>, what: &str) {
    let err = result.expect_err(what);
    assert_eq!(err.kind(), "replacement-unreadable", "{what}: {err}");
    assert!(
        matches!(&err, EngineError::ReplacementUnreadable { position: 1, label } if label == "a@x.co"),
        "{what}: {err:?}"
    );
    assert!(
        err.to_string().contains("`tagteam remove 1`"),
        "{what}: {err}"
    );
}

#[test]
fn every_lock_holder_refuses_a_landed_replacement_it_cannot_read() {
    for meta in UNREADABLE {
        let (fx, a) = landed_unreadable(meta);
        // The refresh gate.
        let snapshot = fx.vault_bytes(&a).unwrap();
        assert_refused(
            fx.engine.refresh_stored(fx.cc.as_ref(), &a, &snapshot),
            "gate",
        );
        // A switch to the account.
        assert_refused(fx.switch_to(&a, false), "switch");
        // An add of the same login.
        fx.login("a@x.co", "rt-a3");
        assert_refused(fx.engine.add_live(fx.add_options()), "add");
        // Nothing was installed or undone: the marker and the landed vault stand.
        let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
        assert!(row.replacing_fp.is_some(), "{meta}");
        assert_eq!(row.login_epoch, 1, "{meta}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a9"));
    }
}

#[test]
fn remove_deletes_an_account_whose_replacement_cannot_be_read() {
    for meta in UNREADABLE {
        let (fx, a) = landed_unreadable(meta);
        let removed = fx.engine.remove(&a).unwrap();
        assert_eq!(removed.id, a);
        assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
        assert!(fx.kc.get(SERVICE, a.as_str()).is_none(), "{meta}");
        assert!(fx.kc.get(SERVICE, &format!("{a}.prev")).is_none(), "{meta}");
    }
}

#[test]
fn a_replacement_that_never_landed_rolls_back_without_its_metadata() {
    // §12.5: the vault does not hold `replacing_fp`, so the metadata is never needed.
    for meta in UNREADABLE {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let store = fx.engine.store().unwrap();
        let identity = fx.cc.token_identity("a@x.co");
        store
            .begin_replacement(
                &a,
                "sha256:a-generation-never-written",
                &LoginMeta {
                    identity_key: "a@x.co\n",
                    identity: &identity,
                    kind: "oauth",
                    login_expires_at: None,
                    from_live: false,
                },
                false,
            )
            .unwrap();
        corrupt_meta(&fx, &a, meta);
        drop(fx.engine.lock_account(&a).unwrap());
        let row = store.account(&a).unwrap().unwrap();
        assert_eq!((row.replacing_fp, row.login_epoch), (None, 0), "{meta}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    }
}
