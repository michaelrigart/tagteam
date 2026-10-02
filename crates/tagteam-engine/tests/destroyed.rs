//! §9.4 step 7: every live entry holding a secret that a switch's write, or its clear of the
//! other axis, overwrites or deletes is saved to `displaced/` first, unless a vault already
//! holds its generation. One test per kind of entry; each checks both halves.

mod common;

use common::{
    API_KEY, Fx, OTHER_API_KEY, STRAY_API_KEY, assert_journal_cleared, crashed_switch,
    splice_config_key, write_target_credential,
};
use serde_json::json;

/// An OAuth login no vault holds.
fn stray_login() -> Vec<u8> {
    Fx::credential_json("stray@x.co", "rt-stray")
        .to_string()
        .into_bytes()
}

/// What `displaced/` must hold afterwards: nothing when a vault held the planted secret.
fn expected(held: bool, planted: &[u8]) -> Vec<Vec<u8>> {
    if held { vec![] } else { vec![planted.to_vec()] }
}

#[test]
fn the_primary_item() {
    // The OAuth item under an API-key login, overwritten by activating OAuth.
    for held in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let k = fx.add_api_key(API_KEY);
        fx.switch_to(&k, false).unwrap();
        let planted = if held {
            fx.vault_bytes(&a).unwrap() // the target's own generation
        } else {
            stray_login()
        };
        fx.set_live_credential(&planted);
        fx.switch_to(&a, false).unwrap();
        assert_eq!(fx.displaced(), expected(held, &planted), "held={held}");
    }
    // The managed-key item under an OAuth login, overwritten by activating an API key.
    for held in [false, true] {
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let k = fx.add_api_key(API_KEY);
        let planted = if held { API_KEY } else { STRAY_API_KEY };
        fx.put_managed_key(planted.as_bytes());
        fx.switch_to(&k, false).unwrap();
        assert_eq!(
            fx.displaced(),
            expected(held, planted.as_bytes()),
            "held={held}"
        );
    }
}

#[test]
fn a_credentials_file_the_keychain_shadows() {
    for held in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let planted = if held {
            fx.vault_bytes(&a).unwrap()
        } else {
            stray_login()
        };
        std::fs::write(fx.paths().credentials_file, &planted).unwrap();
        fx.switch_to(&a, false).unwrap();
        assert_eq!(fx.displaced(), expected(held, &planted), "held={held}");
    }
}

#[test]
fn a_primary_api_key_a_managed_key_item_hides() {
    // Readers take the Keychain item first, so `primaryApiKey` beside it is hidden; activating
    // OAuth drops it with the item.
    for held in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let k = fx.add_api_key(API_KEY);
        fx.switch_to(&k, false).unwrap();
        let planted = if held { API_KEY } else { STRAY_API_KEY };
        let config = fx.paths().global_config;
        splice_config_key(&config, "primaryApiKey", &json!(planted));
        fx.switch_to(&a, false).unwrap();
        assert_eq!(
            fx.displaced(),
            expected(held, planted.as_bytes()),
            "held={held}"
        );
        assert!(
            !std::fs::read_to_string(&config)
                .unwrap()
                .contains("primaryApiKey")
        );
    }
}

#[test]
fn recovery_saves_what_clearing_the_managed_key_axis_destroys() {
    // §9.6 finishing forward to OAuth clears the managed-key axis: the managed-key item and a
    // hidden `primaryApiKey` go, and are saved first unless a vault holds them.
    for held in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let k = fx.add_api_key(API_KEY);
        fx.switch_to(&k, false).unwrap();
        crashed_switch(&fx, &k, &a);
        write_target_credential(&fx, &a);
        let planted = if held { API_KEY } else { STRAY_API_KEY };
        fx.put_managed_key(planted.as_bytes());
        splice_config_key(
            &fx.paths().global_config,
            "primaryApiKey",
            &json!(if held { API_KEY } else { OTHER_API_KEY }),
        );
        fx.engine.set_disabled(&a, false).unwrap(); // a mutation: it recovers the row
        assert_journal_cleared(&fx);
        let want = if held {
            vec![]
        } else {
            vec![
                STRAY_API_KEY.as_bytes().to_vec(),
                OTHER_API_KEY.as_bytes().to_vec(),
            ]
        };
        let mut got = fx.displaced();
        got.sort();
        assert_eq!(got, want, "held={held}");
        assert_eq!(fx.managed_key(), None);
    }
}
