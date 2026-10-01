//! §9.3's usage strategies, `best` and `next-available`: the release of quarantines that no
//! longer bind (§7.4, Decision 9), on-demand collection (§8.3, Decision 8), ranking from
//! decision-grade readings (§8.4), lazy vault reads, freshening (§7.2), and what stands under
//! the locks (Decision 11, Review Focus 4). Readings are recorded through the store's own
//! reserve-and-record calls, as the collector records them.
mod common;

use common::{Fx, credential, quarantine_of, vault_fp};
use tagteam_engine::vault::SERVICE;

#[test]
fn nothing_is_released_or_created_without_a_store() {
    let fx = Fx::new();
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(!fx.env.data_dir().join("tagteam.db").exists());
}

#[test]
fn a_quarantine_the_vault_has_moved_past_is_released_and_recorded_with_its_source() {
    // M3b's tick passes "auto"; the event says who released it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "auto")
            .unwrap(),
        [a.clone()]
    );
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    let events = fx.engine.store().unwrap().events().unwrap();
    let last = events.last().unwrap();
    assert_eq!(
        (
            last.kind.as_str(),
            last.to_id.as_ref(),
            last.source.as_str()
        ),
        ("unquarantine", Some(&a), "auto")
    );
}

#[test]
fn a_quarantine_the_vault_still_holds_stays() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(bound))
    );
}

#[test]
fn an_unreadable_vault_leaves_the_quarantine() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(quarantine_of(&fx, &a).0.is_some());
}

#[test]
fn the_live_account_s_quarantine_holds_while_the_live_credential_is_its_generation() {
    // §7.4: the active account's quarantine holds while either copy matches `quarantine_fp`.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: rt-b
    let bound = vault_fp(&fx, &b);
    fx.quarantine(&b, "invalid_grant", &bound);
    // The vault moves on; the live credential is still the generation the strike is bound to.
    fx.put_vault(&b, &credential("b@x.co", "rt-b2"));
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert_eq!(quarantine_of(&fx, &b).1, Some(bound));
    // Claude Code rotates the live credential too: neither copy is bound any more.
    fx.rotate_live("rt-b3");
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap(),
        [b.clone()]
    );
    assert_eq!(quarantine_of(&fx, &b), (None, None));
}

#[test]
fn a_busy_account_lock_leaves_the_quarantine_for_the_next_caller() {
    // Decision 9: try-only. Whoever holds the lock may be writing this very account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let held = fx.engine.lock_account(&a).unwrap();
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(quarantine_of(&fx, &a).0.is_some());
    drop(held);
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap(),
        [a]
    );
}
