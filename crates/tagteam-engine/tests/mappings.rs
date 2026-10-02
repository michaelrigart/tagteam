//! §12.7's mappings in the store: one per provider per path, found by the nearest mapped
//! ancestor, and removed with their account.

mod common;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use common::{Fx, add, cc};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{Mapping, Store, StoreError};

fn store() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    (d, s)
}

fn other() -> ProviderId {
    ProviderId::new("fake-agent")
}

/// The account `nearest_mapping` finds for `dir`, if any.
fn nearest(s: &Store, dir: &str, provider: &ProviderId) -> Option<AccountId> {
    s.nearest_mapping(Path::new(dir), provider)
        .unwrap()
        .map(|m| m.account_id)
}

#[test]
fn a_directory_holds_one_mapping_per_provider_and_a_new_one_replaces_it() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let f = add(&s, &other(), "f", "a@x.co", 1);
    s.set_mapping("/w/app", &cc(), &a, 10).unwrap();
    s.set_mapping("/w/app", &cc(), &b, 20).unwrap();
    s.set_mapping("/w/app", &other(), &f, 30).unwrap();
    assert_eq!(
        s.mappings().unwrap(),
        vec![
            Mapping {
                path: "/w/app".into(),
                provider: cc(),
                account_id: b,
                added_at: 20,
            },
            Mapping {
                path: "/w/app".into(),
                provider: other(),
                account_id: f,
                added_at: 30,
            },
        ]
    );
}

#[test]
fn the_nearest_mapped_ancestor_is_found_by_whole_path_components() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.set_mapping("/w/a/b", &cc(), &a, 1).unwrap();
    s.set_mapping("/w/a/b/c/d", &cc(), &b, 1).unwrap();
    for (dir, found) in [
        ("/w/a/b", Some(&a)),
        ("/w/a/b/", Some(&a)),
        ("/w/a/b/./c", Some(&a)),
        ("/w/a/b/c", Some(&a)),
        ("/w/a/b/c/d", Some(&b)),
        ("/w/a/b/c/d/e/f", Some(&b)),
        ("/w/a/b/c/dd", Some(&a)),
        // Sibling prefixes: a string prefix would match all of these.
        ("/w/a/bc", None),
        ("/w/a/b c", None),
        ("/w/a/b-x/c", None),
        ("/w/a", None),
        ("/", None),
    ] {
        assert_eq!(nearest(&s, dir, &cc()).as_ref(), found, "{dir}");
    }
}

#[test]
fn a_mapping_of_the_root_covers_everything_below_it() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_mapping("/", &cc(), &a, 1).unwrap();
    assert_eq!(nearest(&s, "/x/y/z", &cc()), Some(a.clone()));
    assert_eq!(nearest(&s, "/", &cc()), Some(a));
}

#[test]
fn nearest_mapping_is_per_provider() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let f = add(&s, &other(), "f", "a@x.co", 1);
    s.set_mapping("/w", &cc(), &a, 1).unwrap();
    s.set_mapping("/w/deep", &other(), &f, 1).unwrap();
    assert_eq!(
        nearest(&s, "/w/deep/x", &cc()),
        Some(a),
        "claude-code's own mapping"
    );
    assert_eq!(nearest(&s, "/w/deep/x", &other()), Some(f));
    assert_eq!(nearest(&s, "/w/x", &other()), None);
    assert_eq!(nearest(&s, "/w/x", &ProviderId::new("none")), None);
}

#[test]
fn a_path_that_is_not_utf8_still_finds_its_mapped_ancestor() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_mapping("/w", &cc(), &a, 1).unwrap();
    let dir = Path::new(OsStr::from_bytes(b"/w/caf\xe9/src"));
    assert_eq!(
        s.nearest_mapping(dir, &cc()).unwrap().map(|m| m.path),
        Some("/w".to_owned())
    );
}

#[test]
fn unmapping_removes_one_provider_s_mapping_or_every_one() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let f = add(&s, &other(), "f", "a@x.co", 1);
    for p in ["/w", "/x"] {
        s.set_mapping(p, &cc(), &a, 1).unwrap();
        s.set_mapping(p, &other(), &f, 1).unwrap();
    }
    assert_eq!(s.remove_mappings("/w", Some(&cc())).unwrap(), 1);
    assert_eq!(nearest(&s, "/w", &cc()), None);
    assert_eq!(
        nearest(&s, "/w", &other()),
        Some(f.clone()),
        "the other provider's stays"
    );
    assert_eq!(s.remove_mappings("/x", None).unwrap(), 2);
    assert_eq!(
        s.remove_mappings("/x", None).unwrap(),
        0,
        "nothing left to remove"
    );
    assert_eq!(
        s.remove_mappings("/w/sub", None).unwrap(),
        0,
        "never an ancestor's"
    );
    let left: Vec<_> = s
        .mappings()
        .unwrap()
        .into_iter()
        .map(|m| (m.path, m.provider))
        .collect();
    assert_eq!(left, vec![("/w".to_owned(), other())]);
}

#[test]
fn a_mapping_names_an_account_of_its_own_provider() {
    let (_d, s) = store();
    let f = add(&s, &other(), "f", "a@x.co", 1);
    assert!(matches!(
        s.set_mapping("/w", &cc(), &AccountId::from_string("nobody"), 1),
        Err(StoreError::NoSuchAccount)
    ));
    assert!(
        matches!(
            s.set_mapping("/w", &cc(), &f, 1),
            Err(StoreError::NoSuchAccount)
        ),
        "fake-agent's account cannot hold claude-code's mapping"
    );
    assert!(s.mappings().unwrap().is_empty());
}

#[test]
fn deleting_an_account_deletes_its_mappings() {
    let (_d, s) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.set_mapping("/w", &cc(), &a, 1).unwrap();
    s.set_mapping("/x", &cc(), &a, 1).unwrap();
    s.set_mapping("/y", &cc(), &b, 1).unwrap();
    s.delete_account(&a).unwrap();
    let left: Vec<String> = s.mappings().unwrap().into_iter().map(|m| m.path).collect();
    assert_eq!(left, ["/y"]);
}

#[test]
fn remove_unmaps_the_account_everywhere() {
    // §10.3: `remove` deletes the row last, and the row cascades to its mappings.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let store = fx.engine.store().unwrap();
    store.set_mapping("/w", &fx.provider(), &a, 1).unwrap();
    store.set_mapping("/x", &fx.provider(), &b, 1).unwrap();
    fx.engine.remove(&a).unwrap();
    let left: Vec<AccountId> = store
        .mappings()
        .unwrap()
        .into_iter()
        .map(|m| m.account_id)
        .collect();
    assert_eq!(left, vec![b]);
}
