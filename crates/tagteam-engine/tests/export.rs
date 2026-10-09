//! §13.3's export in the engine (Task 7, Decision 12): which generation each account exports,
//! which accounts are broken, and that export writes neither the live store nor a profile.

mod common;

use std::fs;
use std::time::{Duration, Instant};

use common::{FakeFx, Fx, crashed_switch, credential, fp, quiescent, rescue_files, two_accounts};
use serde_json::{Value, json};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::export::{ExportRequest, ExportResult, Source};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Keychain, Provider};

fn all(fx: &Fx) -> ExportResult {
    fx.engine.export(&ExportRequest::default()).unwrap()
}

fn only(fx: &Fx, id: &AccountId) -> Result<ExportResult, EngineError> {
    fx.engine.export(&ExportRequest {
        accounts: Some(vec![id.clone()]),
        ..ExportRequest::default()
    })
}

/// The envelope's accounts.
fn envelope(r: &ExportResult) -> Vec<Value> {
    let v: Value = serde_json::from_slice(&r.envelope).unwrap();
    v["accounts"].as_array().unwrap().clone()
}

/// The refresh token of the envelope account at `position`.
fn exported_rt(r: &ExportResult, position: u32) -> String {
    envelope(r)
        .iter()
        .find(|a| a["position"] == position)
        .unwrap_or_else(|| panic!("position {position} was not exported"))["credential"]
        ["claudeAiOauth"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `(position, source, in use)` of each exported account.
fn sources(r: &ExportResult) -> Vec<(u32, Source, bool)> {
    r.accounts
        .iter()
        .map(|e| (e.row.position, e.source, e.in_use))
        .collect()
}

/// `(position, reason)` of each skipped account.
fn skipped(r: &ExportResult) -> Vec<(u32, String)> {
    r.skipped
        .iter()
        .map(|s| (s.row.position, s.reason.clone()))
        .collect()
}

#[test]
fn an_idle_account_is_exported_slim_from_the_vault_and_the_live_one_from_where_it_advances() {
    let fx = Fx::new();
    two_accounts(&fx); // a at 1, b at 2 and live
    let r = all(&fx);
    assert_eq!(
        sources(&r),
        [(1, Source::Vault, false), (2, Source::Vault, true)],
        "b's live credential is the vault's generation"
    );
    assert!(r.skipped.is_empty());
    let accounts = envelope(&r);
    assert_eq!(accounts[0]["identity"]["email"], "a@x.co");
    assert_eq!(
        accounts[0]["credential"],
        json!({"claudeAiOauth": Fx::credential_json("a@x.co", "rt-a")["claudeAiOauth"]}),
        "slim: no machine-shared key"
    );
    let v: Value = serde_json::from_slice(&r.envelope).unwrap();
    assert_eq!(v["active"], json!({"claude-code": 2}));
    assert!(r.accounts.iter().all(|e| e.refreshes));

    let full = fx
        .engine
        .export(&ExportRequest {
            full: true,
            ..ExportRequest::default()
        })
        .unwrap();
    assert_eq!(
        envelope(&full)[0]["credential"],
        Fx::credential_json("a@x.co", "rt-a")
    );
}

#[test]
fn claude_code_s_rotation_of_the_live_login_is_exported_and_nothing_is_written() {
    let fx = Fx::new();
    let b = {
        two_accounts(&fx);
        fx.engine.resolve("2", None).unwrap().id
    };
    fx.rotate_live("rt-b2");
    let items = fx.kc.items();

    let r = all(&fx);

    assert_eq!(sources(&r)[1], (2, Source::Live, true));
    assert_eq!(exported_rt(&r, 2), "rt-b2");
    assert_eq!(fx.kc.items(), items, "no vault, live or profile write");
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    assert_eq!(rescue_files(&fx), 0);
}

#[test]
fn a_live_credential_at_the_vault_s_prev_exports_the_vault_s_generation() {
    // §7.5 step 3's second row: an earlier pass reached the vault but not the live store.
    let fx = Fx::new();
    two_accounts(&fx);
    let b = fx.engine.resolve("2", None).unwrap().id;
    fx.kc
        .put(SERVICE, &format!("{b}.prev"), &credential("b@x.co", "rt-b"));
    fx.put_vault(&b, &credential("b@x.co", "rt-b2"));
    let r = all(&fx);
    assert_eq!(sources(&r)[1], (2, Source::Vault, true));
    assert_eq!(exported_rt(&r, 2), "rt-b2");
}

#[test]
fn a_pending_rescue_is_the_live_login_s_newest_generation_and_stays_where_it_is() {
    // §13.3: "the vault's generation, or the pending rescue that succeeds it", and a live
    // credential that is a rescue's generation is that rescue's.
    for live_is_the_rescue in [false, true] {
        let fx = Fx::new();
        two_accounts(&fx);
        let b = fx.engine.resolve("2", None).unwrap().id;
        fx.plant_rescue(
            &b,
            &fp_of(&fx, "b@x.co", "rt-b"),
            &credential("b@x.co", "rt-b3"),
        );
        if live_is_the_rescue {
            fx.rotate_live("rt-b3");
        }
        let r = all(&fx);
        assert_eq!(exported_rt(&r, 2), "rt-b3", "{live_is_the_rescue}");
        assert_eq!(sources(&r)[1], (2, Source::Vault, true));
        assert_eq!(
            rescue_files(&fx),
            1,
            "the live login's rescue is left alone"
        );
        assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    }
}

#[test]
fn a_stale_marked_live_store_exports_the_replacement() {
    let fx = Fx::new();
    two_accounts(&fx);
    let b = fx.engine.resolve("2", None).unwrap().id;
    fx.replace_login(&b, &credential("b@x.co", "rt-b-new"), "oauth");
    fx.rotate_live("rt-b2");
    assert!(fx.live_store_stale(&b));
    let r = all(&fx);
    assert_eq!(sources(&r)[1], (2, Source::Vault, true));
    assert_eq!(exported_rt(&r, 2), "rt-b-new");
}

#[test]
fn an_idle_account_s_pending_rescue_is_settled_first() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.plant_rescue(
        &a,
        &fp_of(&fx, "a@x.co", "rt-a"),
        &credential("a@x.co", "rt-a2"),
    );
    let r = all(&fx);
    assert_eq!(exported_rt(&r, 1), "rt-a2");
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a2"),
        "settled (§6.2)"
    );
    assert_eq!(rescue_files(&fx), 0);
}

#[test]
fn a_quiescent_profile_that_rotated_is_captured_first() {
    // Lazy capture (§12.5), as every holder of the account lock does it.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    let r = all(&fx);
    assert_eq!(sources(&r)[0], (1, Source::Vault, false));
    assert_eq!(exported_rt(&r, 1), "rt-a2");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
}

/// The fingerprint of `email`'s credential with refresh token `rt`.
fn fp_of(fx: &Fx, email: &str, rt: &str) -> String {
    fx.cc
        .fingerprint(&credential(email, rt))
        .unwrap()
        .as_str()
        .to_owned()
}

/// `a`'s profile, bootstrapped at seed `seed_rt`, holding `profile_rt`, with a session running
/// in it.
fn running(
    fx: &Fx,
    a: &AccountId,
    seed_rt: &str,
    profile_rt: &str,
) -> tagteam_provider::FlockGuard {
    let dir = quiescent(fx, a, seed_rt, &credential("a@x.co", profile_rt));
    fx.hold_reservation(&dir)
}

#[test]
fn a_session_owned_account_exports_by_its_profile_s_provenance_and_writes_nothing() {
    // §13.3, §12.5's table: (seed, profile, vault) → what is exported.
    let cases = [
        ("rt-a", "rt-a2", None, Source::Profile, "rt-a2"), // P ≠ V, V = S: the profile rotated
        ("rt-a", "rt-a", None, Source::Vault, "rt-a"),     // in step
        ("rt-a", "rt-a", Some("rt-a3"), Source::Vault, "rt-a3"), // the vault moved on
    ];
    for (seed, profile, vault, source, want) in cases {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        if let Some(v) = vault {
            fx.put_vault(&a, &credential("a@x.co", v));
        }
        let _session = running(&fx, &a, seed, profile);
        let dir = fx.profile_dir(&a);
        let before = (fx.kc.items(), fx.profile_credential(&dir));

        let r = all(&fx);

        assert_eq!(
            sources(&r)[0],
            (1, source, true),
            "{seed} {profile} {vault:?}"
        );
        assert_eq!(exported_rt(&r, 1), want);
        assert_eq!((fx.kc.items(), fx.profile_credential(&dir)), before);
    }
}

#[test]
fn a_session_profile_a_replacement_superseded_or_whose_identity_drifted_exports_the_vault() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let _session = running(&fx, &a, "rt-a0", "rt-a2");
    fx.replace_login(&a, &credential("a@x.co", "rt-a-new"), "oauth"); // stale-marks the profile
    assert_eq!(exported_rt(&all(&fx), 1), "rt-a-new");

    let fx = Fx::new();
    let a = two_accounts(&fx);
    let _session = running(&fx, &a, "rt-a", "rt-a2");
    fx.set_profile_identity(&fx.profile_dir(&a), "someone@else.co");
    let r = all(&fx);
    assert_eq!(sources(&r)[0], (1, Source::Vault, true));
    assert_eq!(exported_rt(&r, 1), "rt-a");
}

#[test]
fn a_session_that_rotated_but_names_no_identity_is_broken_never_exported_from_the_vault() {
    // B.64 and §13.3: an absent identity has not drifted. The profile rotated the vault's
    // generation (P ≠ V, V = S), so the vault's is consumed, and nothing says the rotation is
    // the account's: as for every holder of its lock (M4a's `apply_provenance`), the account is
    // broken. In step, the absent identity decides nothing.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let _session = running(&fx, &a, "rt-a", "rt-a2");
    fs::write(fx.profile_dir(&a).join(".claude.json"), "{}").unwrap();
    let r = all(&fx);
    assert!(
        envelope(&r).iter().all(|account| account["position"] != 1),
        "the consumed generation is never exported"
    );
    assert_eq!(
        skipped(&r),
        [(
            1,
            "its running session rotated the login but names no identity, so the rotation cannot be told to be the account's".to_owned()
        )]
    );
    assert_eq!(only(&fx, &a).unwrap_err().kind(), "account-broken");

    let fx = Fx::new();
    let a = two_accounts(&fx);
    let _session = running(&fx, &a, "rt-a", "rt-a");
    fs::write(fx.profile_dir(&a).join(".claude.json"), "{}").unwrap();
    let r = all(&fx);
    assert_eq!(sources(&r)[0], (1, Source::Vault, true));
    assert_eq!(exported_rt(&r, 1), "rt-a");
}

#[test]
fn a_replacement_over_a_session_exports_even_when_the_old_session_wiped_its_tokens() {
    // §13.3: a stale-marked profile exports the vault's generation, whatever its own copy
    // holds: the replacement wins, wiped tokens included.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let _session = running(&fx, &a, "rt-a", "rt-a2");
    fx.replace_login(&a, &credential("a@x.co", "rt-a-new"), "oauth");
    let wiped = json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}});
    fx.set_profile_credential(&fx.profile_dir(&a), wiped.to_string().as_bytes());
    let r = all(&fx);
    assert!(r.skipped.is_empty(), "{:?}", skipped(&r));
    assert_eq!(sources(&r)[0], (1, Source::Vault, true));
    assert_eq!(exported_rt(&r, 1), "rt-a-new");
    assert_eq!(exported_rt(&only(&fx, &a).unwrap(), 1), "rt-a-new");
}

#[test]
fn a_tokenless_generation_is_never_exported_so_import_never_refuses_the_file() {
    // B.64, §13.3: the generation exported is checked as `import_login` checks it.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let tokenless = json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}});
    fx.put_vault(&a, tokenless.to_string().as_bytes());
    let r = all(&fx);
    assert_eq!(
        skipped(&r),
        [(
            1,
            "its credential holds no token, so it could not be imported".to_owned()
        )]
    );
    assert_eq!(only(&fx, &a).unwrap_err().kind(), "account-broken");
    let accounts = envelope(&r);
    assert_eq!(accounts.len(), 1);
    for account in &accounts {
        assert!(
            fx.cc
                .import_login(&account["identity"], &account["credential"])
                .is_ok(),
            "import takes every account the file holds"
        );
    }
}

#[test]
fn broken_accounts_are_skipped_with_their_reason_and_refused_when_named() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let b = fx.engine.resolve("2", None).unwrap().id;
    let c = fx.add("c@x.co", "rt-c"); // live now: position 3
    let d = fx.add("d@x.co", "rt-d"); // live now: position 4
    fx.add("e@x.co", "rt-e"); // live: position 5
    fx.quarantine(&a, "invalid_grant", &fp_of(&fx, "a@x.co", "rt-a"));
    fx.kc.delete(SERVICE, b.as_str()).unwrap();
    let rescue = fx.env.data_dir().join("rescue");
    fs::create_dir_all(&rescue).unwrap();
    fs::write(rescue.join(format!("{c}-0-abcdef012345.json")), b"not json").unwrap();
    // d's profile and the vault both moved since they last agreed, with no session running.
    quiescent(&fx, &d, "rt-d0", &credential("d@x.co", "rt-d2"));

    let r = all(&fx);

    assert_eq!(sources(&r), [(5, Source::Vault, true)]);
    let reasons = skipped(&r);
    assert_eq!(
        reasons[0],
        (
            1,
            "it is quarantined: its login was rejected, so log in again".into()
        )
    );
    assert_eq!(reasons[1], (2, "it has no stored credential".into()));
    assert_eq!(reasons[2].0, 3);
    assert!(
        reasons[2]
            .1
            .contains("has a refreshed token that is not in the vault yet"),
        "{}",
        reasons[2].1
    );
    assert_eq!(
        reasons[3],
        (
            4,
            "its session profile and the vault both moved since they last agreed".into()
        )
    );

    let err = only(&fx, &a).unwrap_err();
    assert_eq!(err.kind(), "account-broken");
    assert_eq!(
        err.to_string(),
        "position 1 cannot be exported: it is quarantined: its login was rejected, so log in again"
    );
}

#[test]
fn a_live_copy_that_was_wiped_or_lacks_the_refresh_token_is_broken() {
    for (live, reason) in [
        (
            json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}}),
            "Claude Code wiped the copy in use after its login was rejected",
        ),
        (
            json!({"claudeAiOauth": {"accessToken": "at-other"}}),
            "its live credential has no refresh token, so the newest generation cannot be told",
        ),
    ] {
        let fx = Fx::new();
        two_accounts(&fx);
        fx.set_live_credential(live.to_string().as_bytes());
        let r = all(&fx);
        assert_eq!(skipped(&r), [(2, reason.to_owned())]);
        assert_eq!(sources(&r), [(1, Source::Vault, false)]);
    }
}

#[test]
fn the_accounts_an_undecidable_interrupted_switch_names_are_broken() {
    // §9.6: recovery cannot decide a live credential that is neither side's, and export asks
    // no oracle; both accounts the row names are broken, the others export.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let b = fx.engine.resolve("2", None).unwrap().id;
    fx.add("c@x.co", "rt-c");
    crashed_switch(&fx, &b, &a);
    fx.set_live_credential(&credential("c@x.co", "rt-c2"));
    let r = all(&fx);
    let why = "an interrupted switch that recovery cannot decide names it".to_owned();
    assert_eq!(skipped(&r), [(1, why.clone()), (2, why)]);
    assert_eq!(r.accounts.len(), 1);
    assert_eq!(fx.http.requests().len(), 0, "export sends no request");
}

#[test]
fn exporting_the_live_login_while_claude_code_refreshes_it_waits_for_the_new_generation() {
    // Review Focus 1: CC holds its refresh lock while it rotates the live login. Export waits
    // on the credential locks, then reads: the generation CC wrote, never the consumed one.
    let fx = Fx::new();
    two_accounts(&fx);
    let lock = fx.paths().refresh_lock;
    fs::create_dir(&lock).unwrap();
    let (kc, svc, acct) = (
        fx.kc.clone(),
        keychain_service(&fx.env, ItemKind::OAuth),
        keychain_account(&fx.env),
    );
    let rotated = credential("b@x.co", "rt-b2");
    let started = Instant::now();
    let cc = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        kc.put(&svc, &acct, &rotated);
        fs::remove_dir(&lock).unwrap();
    });

    let r = all(&fx);
    cc.join().unwrap();

    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_eq!(exported_rt(&r, 2), "rt-b2");
    assert_eq!(sources(&r)[1], (2, Source::Live, true));
    assert!(!fx.paths().refresh_lock.exists(), "released");
}

#[test]
fn export_works_inside_a_run_shell_where_the_live_login_is_the_default_home_s() {
    // §12.8: the session's own account is session-owned through its reservation; the live
    // login is the default home's.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a"));
    let _session = fx.hold_reservation(&dir);
    fx.rotate_live("rt-b2");
    let shell = fx.engine_located(fx.shell_env(&dir));
    let r = shell.export(&ExportRequest::default()).unwrap();
    assert_eq!(
        sources(&r),
        [(1, Source::Vault, true), (2, Source::Live, true)]
    );
}

#[test]
fn with_no_store_nothing_is_exported_and_nothing_is_created() {
    let fx = Fx::new();
    let r = all(&fx);
    assert!(r.accounts.is_empty() && r.skipped.is_empty());
    assert!(!fx.env.data_dir().exists());
    let err = only(&fx, &AccountId::from_string("0192")).unwrap_err();
    assert_eq!(err.kind(), "no-such-account");
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_fake_agent_account_exports_its_own_payload() {
    // §15.2: the engine names no Claude Code field (B.45).
    let ff = FakeFx::new();
    ff.fake_add("alice", "tok-a", "renew-a");
    let r = ff.engine.export(&ExportRequest::default()).unwrap();
    let v: Value = serde_json::from_slice(&r.envelope).unwrap();
    let account = &v["accounts"][0];
    assert_eq!(account["provider"], "fake-agent");
    assert_eq!(account["identity"]["handle"], "alice");
    assert_eq!(account["credential"]["fa"]["renew"], "renew-a");
    assert!(account["credential"].get("device").is_none(), "slim");
    assert_eq!(
        (r.accounts[0].source, r.accounts[0].in_use),
        (Source::Vault, true),
        "its live credential is the vault's generation"
    );
}

#[test]
fn a_profile_that_rotated_under_a_running_session_is_exported_whatever_its_expiry() {
    // B.52: provenance, never expiry, decides; the profile's rotation carries an older expiry.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &common::cred_at("rt-a2", 1));
    let _session = fx.hold_reservation(&dir);
    let r = all(&fx);
    assert_eq!(exported_rt(&r, 1), "rt-a2");
    assert_eq!(fp(&fx, "rt-a"), fp_of(&fx, "a@x.co", "rt-a"));
}

#[test]
fn a_session_credential_without_a_refresh_token_is_broken_while_the_vault_has_one() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let tokenless = json!({"claudeAiOauth": {"accessToken": "at-profile"}});
    let dir = quiescent(&fx, &a, "rt-a", tokenless.to_string().as_bytes());
    let _session = fx.hold_reservation(&dir);
    let r = all(&fx);
    assert_eq!(
        skipped(&r),
        [(
            1,
            "its session's credential has no refresh token, while the vault's has one".to_owned()
        )]
    );
    assert_eq!(only(&fx, &a).unwrap_err().kind(), "account-broken");
}

#[test]
fn a_session_with_no_seed_exports_the_vault_only_when_its_credential_is_the_vault_s() {
    // In step: the vault's generation. Otherwise nothing to compare the profile against.
    for (profile_rt, want) in [("rt-a", Ok("rt-a")), ("rt-a2", Err(()))] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", profile_rt));
        fs::remove_file(dir.join(tagteam_provider::profile::SEED_FILE)).unwrap();
        let _session = fx.hold_reservation(&dir);
        let r = all(&fx);
        match want {
            Ok(rt) => {
                assert_eq!(sources(&r)[0], (1, Source::Vault, true));
                assert_eq!(exported_rt(&r, 1), rt);
            }
            Err(()) => assert_eq!(
                skipped(&r),
                [(
                    1,
                    "its running session has no seed to compare its credential against".to_owned()
                )]
            ),
        }
    }
}
