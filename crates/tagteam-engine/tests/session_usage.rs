//! §8.1 "Session-owned account": an account a `tagteam run` session owns is collected from its
//! profile, read as Claude Code in the session reads it, with no lock, and never refreshed,
//! written or retried. A session is a held launch reservation (§12.5) or an unreadable session
//! record (§12.6); no test reaches a real pid.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{
    Fx, access_fp, credential, failed, refused, token_requests, two_accounts, usage_bearers,
    usage_fixture, usage_requests,
};
use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::store::UsageStateRow;
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker};
use tagteam_provider::{FlockGuard, Keychain, MutationGuard, Provider, Read};

/// The fixture clock's start, in the seconds the usage tables hold.
const NOW_S: i64 = 1_790_000_000;

/// An account a running session owns: its profile, the credential the profile holds, and the
/// session's reservation, held while this lives.
struct Session {
    id: AccountId,
    profile: PathBuf,
    bytes: Vec<u8>,
    _reservation: FlockGuard,
}

/// Puts `bytes` where Claude Code keeps `profile`'s credential: its hashed Keychain item on
/// macOS (`Fx::profile_item`, the item the profile's marker names), `<profile>/.credentials.json`
/// on Linux. Private to this file: no other test needs to write the item.
fn set_credential(fx: &Fx, profile: &Path, bytes: &[u8]) {
    match fx.platform {
        Platform::MacOs => {
            let (svc, acct) = fx.profile_item(profile);
            fx.kc.put(&svc, &acct, bytes);
        }
        Platform::Linux => fx.set_profile_credential(profile, bytes),
    }
}

/// Gives the stored account `id` a profile (`make_profile`: its marker, and its own login as the
/// profile's identity) holding `bytes`, and a running session: a held launch reservation.
fn owned(fx: &Fx, id: &AccountId, bytes: Vec<u8>) -> Session {
    let profile = fx.make_profile(id);
    set_credential(fx, &profile, &bytes);
    let reservation = fx.hold_reservation(&profile);
    Session {
        id: id.clone(),
        profile,
        bytes,
        _reservation: reservation,
    }
}

/// `a` inactive and session-owned, its profile holding `rt-p` while the vault still holds
/// `rt-a`; `b` is the live login.
fn session(fx: &Fx) -> Session {
    let a = two_accounts(fx);
    owned(fx, &a, credential("a@x.co", "rt-p"))
}

/// A setup-token account, inactive and session-owned (`b` is live). Its profile holds another
/// setup token than the vault's, so a request shows which one was sent.
fn setup_token_session(fx: &Fx) -> Session {
    fx.add("b@x.co", "rt-b");
    let s = fx
        .engine
        .add_token(fx.add_token_options("sk-ant-oat01-vault"))
        .unwrap()
        .account;
    fx.http.clear();
    let (_, bytes) = fx.cc.token_secret("sk-ant-oat01-profile");
    owned(fx, &s.id, bytes)
}

/// `rt`'s credential, with an access token that has expired.
fn expired_credential(rt: &str) -> Vec<u8> {
    let mut v = Fx::credential_json("a@x.co", rt);
    v["claudeAiOauth"]["expiresAt"] = json!(NOW_S * 1000 - 1);
    v.to_string().into_bytes()
}

fn state(fx: &Fx, id: &AccountId) -> UsageStateRow {
    fx.usage_state(id)
        .expect("the collection wrote a usage_state row")
}

/// Where Claude Code reads the profile's identity.
fn profile_config(s: &Session) -> PathBuf {
    s.profile.join(".claude.json")
}

type Breakage = fn(&Fx, &Session);

#[test]
fn a_session_owned_account_is_read_from_its_profile_and_planned_as_a_candidate() {
    for platform in [Platform::MacOs, Platform::Linux] {
        let fx = Fx::with_platform(platform);
        let s = session(&fx);
        let marker = fs::read(s.profile.join(MARKER_FILE)).unwrap();
        let identity = fs::read(profile_config(&s)).unwrap();
        let held = fx.profile_credential(&s.profile);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&s.id]);

        assert_eq!(
            report.outcomes,
            [(s.id.clone(), Collected::Recorded)],
            "{platform:?}"
        );
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(
            usage_bearers(&fx),
            ["at-rt-p"],
            "{platform:?}: the profile's token, not the vault's"
        );
        assert_eq!(token_requests(&fx), 0);
        assert_eq!(usage_requests(&fx), 1);
        // §8.6: not the default home's live login, so the candidate policy (300 s, grown to 450
        // by an idle first reading), never the active one (180 s, grown to 270).
        assert_eq!(state(&fx, &s.id).poll_interval_s, Some(450));
        // Read only: the profile and the vault are as they were.
        assert_eq!(fs::read(s.profile.join(MARKER_FILE)).unwrap(), marker);
        assert_eq!(fs::read(profile_config(&s)).unwrap(), identity);
        assert_eq!(fx.profile_credential(&s.profile), held);
        assert_eq!(fx.vault_refresh_token(&s.id).as_deref(), Some("rt-a"));
    }
}

#[test]
fn a_degraded_profile_read_is_still_sent() {
    // §8.1: an access token sent to the usage endpoint consumes nothing.
    let fx = Fx::new();
    let s = session(&fx);
    let (svc, acct) = fx.profile_item(&s.profile);
    fx.kc.set_unreadable(&svc, &acct, true);
    // The file Claude Code falls back to when the item cannot be read.
    fx.set_profile_credential(&s.profile, &credential("a@x.co", "rt-file"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&s.id]);

    assert_eq!(report.outcomes, [(s.id.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-rt-file"]);
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_unreadable_profile_credential_is_keychain_unavailable_and_sends_nothing() {
    let fx = Fx::new();
    let s = session(&fx);
    let (svc, acct) = fx.profile_item(&s.profile);
    fx.kc.set_unreadable(&svc, &acct, true); // and no plaintext file covers it
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&s.id]);

    assert_eq!(
        report.outcomes,
        [(s.id.clone(), failed("keychain-unavailable"))]
    );
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0, "the slot went back");
}

#[test]
fn a_profile_without_an_access_token_is_no_access_token_and_sends_nothing() {
    let cases: [(&str, Breakage); 3] = [
        ("no credential at all", |fx, s| {
            let (svc, acct) = fx.profile_item(&s.profile);
            fx.kc.delete(&svc, &acct).unwrap();
        }),
        ("an empty item", |fx, s| set_credential(fx, &s.profile, b"")),
        ("a refresh token alone", |fx, s| {
            set_credential(
                fx,
                &s.profile,
                json!({"claudeAiOauth": {"refreshToken": "rt-p"}})
                    .to_string()
                    .as_bytes(),
            )
        }),
    ];
    for (case, break_it) in cases {
        let fx = Fx::new();
        let s = session(&fx);
        break_it(&fx, &s);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&s.id]);

        assert_eq!(
            report.outcomes,
            [(s.id.clone(), failed("no-access-token"))],
            "{case}"
        );
        assert!(fx.http.requests().is_empty(), "{case}");
        assert_eq!(usage_requests(&fx), 0, "{case}: the slot went back");
    }
}

#[test]
fn an_expired_profile_token_is_token_expired_without_a_request_and_gives_its_slot_back() {
    let fx = Fx::new();
    let s = session(&fx);
    set_credential(&fx, &s.profile, &expired_credential("rt-p"));
    fx.script_refresh(Some("rt-new"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&s.id]);

    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    assert!(
        fx.http.requests().is_empty(),
        "no refresh and no usage request: the agent refreshes it on its next call"
    );
    assert_eq!(usage_requests(&fx), 0);
    assert_eq!(
        state(&fx, &s.id).last_error.as_deref(),
        Some("token-expired")
    );
}

#[test]
fn a_401_stamps_the_profile_token_which_is_not_sent_again_until_it_changes() {
    let fx = Fx::new();
    let s = session(&fx);
    fx.script_usage(401, refused());
    fx.script_refresh(Some("rt-new"));

    let report = fx.collect(&[&s.id]);

    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    assert_eq!(usage_bearers(&fx), ["at-rt-p"], "no retry");
    assert_eq!(token_requests(&fx), 0, "no refresh");
    assert_eq!(usage_requests(&fx), 1);
    let fp = access_fp(&fx, &s.bytes);
    assert_eq!(state(&fx, &s.id).rejected_fp.as_deref(), Some(fp.as_str()));

    // Remembered: the same bytes are not sent, and the slot goes back.
    fx.http.clear();
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 1);

    // The agent in the session refreshed its token: the new bytes are sent.
    set_credential(&fx, &s.profile, &credential("a@x.co", "rt-p2"));
    fx.clock.advance_ms(91_000);
    fx.script_usage(200, usage_fixture());
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-rt-p2"]);
    assert_eq!(state(&fx, &s.id).rejected_fp, None, "a success clears it");
}

#[test]
fn a_refused_setup_token_in_a_session_is_a_401_whether_first_or_remembered() {
    // §8.1's last bullet: a token that cannot be refreshed records `http-401` on every path.
    let fx = Fx::new();
    let s = setup_token_session(&fx);
    fx.script_usage(401, refused());

    let report = fx.collect(&[&s.id]);

    assert_eq!(report.outcomes, [(s.id.clone(), failed("http-401"))]);
    assert_eq!(
        usage_bearers(&fx),
        ["sk-ant-oat01-profile"],
        "the profile's token, once"
    );
    assert_eq!(token_requests(&fx), 0);
    let fp = access_fp(&fx, &s.bytes);
    assert_eq!(state(&fx, &s.id).rejected_fp.as_deref(), Some(fp.as_str()));

    fx.http.clear();
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("http-401"))]);
    assert!(
        fx.http.requests().is_empty(),
        "the refused token is not sent again"
    );
    assert_eq!(
        usage_requests(&fx),
        1,
        "the remembered refusal gave its slot back"
    );
}

#[test]
fn a_profile_whose_identity_drifted_is_not_used() {
    let cases: [(&str, Breakage); 3] = [
        ("another login", |fx, s| {
            fx.set_profile_identity(&s.profile, "other@x.co")
        }),
        ("no oauthAccount", |_, s| {
            fs::write(profile_config(s), "{}\n").unwrap()
        }),
        ("a torn .claude.json", |_, s| {
            fs::write(profile_config(s), "{\"oauthAccount\": ").unwrap()
        }),
    ];
    for (case, break_it) in cases {
        let fx = Fx::new();
        let s = session(&fx);
        break_it(&fx, &s);
        // The credential of a profile that is not the account's is never read: an unreadable
        // one would otherwise say `keychain-unavailable`.
        let (svc, acct) = fx.profile_item(&s.profile);
        fx.kc.set_unreadable(&svc, &acct, true);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&s.id]);

        assert_eq!(
            report.outcomes,
            [(s.id.clone(), failed("profile-drifted"))],
            "{case}"
        );
        assert!(fx.http.requests().is_empty(), "{case}");
        assert_eq!(usage_requests(&fx), 0, "{case}");
    }
}

#[test]
fn a_profile_whose_marker_cannot_name_its_credential_is_keychain_unavailable() {
    // §12.2: the credential is read under the recorded spelling, never one derived again. The
    // held reservation keeps the account session-owned whatever the marker says (Task 9's
    // `session_state` never reads it), so the branch itself reports the marker.
    let cases: [(&str, Breakage); 3] = [
        ("no marker", |_, s| {
            fs::remove_file(s.profile.join(MARKER_FILE)).unwrap()
        }),
        ("a corrupt marker", |_, s| {
            fs::write(s.profile.join(MARKER_FILE), "{").unwrap()
        }),
        ("another account's marker", |_, s| {
            let Read::Present(marker) = ProfileMarker::read(&s.profile) else {
                panic!("the marker reads");
            };
            ProfileMarker {
                account_id: AccountId::from_string("someone-else"),
                ..marker
            }
            .write(&s.profile)
            .unwrap();
        }),
    ];
    for (case, break_it) in cases {
        let fx = Fx::new();
        let s = session(&fx);
        break_it(&fx, &s);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&s.id]);

        assert_eq!(
            report.outcomes,
            [(s.id.clone(), failed("keychain-unavailable"))],
            "{case}"
        );
        assert!(fx.http.requests().is_empty(), "{case}");
        assert_eq!(usage_requests(&fx), 0, "{case}");
    }
}

#[test]
fn an_unreadable_session_record_makes_the_account_session_owned_too() {
    // §12.6: a malformed record counts as unreadable, and an unreadable one as live. The record
    // never parses, so no pid is ever looked up.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = fx.make_profile(&a);
    set_credential(&fx, &profile, &credential("a@x.co", "rt-p"));
    fx.plant_record(&profile, "4242", b"[1, 2");
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-rt-p"]);
}

#[test]
fn the_gate_is_never_called_and_no_token_request_is_ever_made_for_a_session_owned_account() {
    // §8.1: the agent in the session owns the profile's token. The vault's token is due, so the
    // inactive path would go through the gate. The test holds the account lock and the mutation
    // lock throughout: the gate would return `Busy`, and a live read would stop, so any lock taken
    // would end as `refresh-failed` or `Dropped`, never as below.
    let fx = Fx::new();
    let s = session(&fx);
    fx.expire_access(&s.id);
    let _account = AccountLock::try_acquire(&fx.env, &s.id)
        .unwrap()
        .expect("the account lock is free");
    let _guard = MutationGuard::acquire(&fx.env, Duration::ZERO).unwrap();
    for _ in 0..3 {
        fx.script_refresh(Some("rt-new"));
    }

    // An expired profile token: nothing is sent at all.
    set_credential(&fx, &s.profile, &expired_credential("rt-p"));
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    // A current one that the server refuses: one usage request, then the stamp, and no retry.
    fx.clock.advance_ms(91_000);
    set_credential(&fx, &s.profile, &credential("a@x.co", "rt-p2"));
    fx.script_usage(401, refused());
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    // The same token, remembered: nothing is sent.
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);

    assert_eq!(token_requests(&fx), 0, "no token request, ever");
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-p2"],
        "one usage request, with the profile's token"
    );
    assert_eq!(
        fx.vault_refresh_token(&s.id).as_deref(),
        Some("rt-a"),
        "the vault is untouched"
    );
}

#[test]
fn the_live_login_s_account_is_collected_as_the_active_one_even_when_a_session_holds_it() {
    // §12.8: the live login is always the default home's, and usage collection sees it as it
    // would outside a run shell. The session's copy is another lineage, never sent here.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let _s = owned(&fx, &b, credential("b@x.co", "rt-p"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&b]);

    assert_eq!(report.outcomes, [(b.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-rt-b"], "the live token");
    assert_eq!(
        state(&fx, &b).poll_interval_s,
        Some(270),
        "the active policy"
    );
}

#[test]
fn a_scheduled_tick_picks_past_a_session_owned_candidate() {
    // §8.6: phase 2's candidates are the provider's switchable accounts (§9.3), and §11.2 step 7
    // never switches to a session-owned one, so the tick's one pick is not spent on it. Never
    // read and at position 1, `a` would otherwise be the stalest due candidate. On demand it is
    // still read from its profile (§8.1), so `list` and `status` keep showing its reading.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c");
    let b = fx.add("b@x.co", "rt-b"); // live
    let s = owned(&fx, &a, credential("a@x.co", "rt-p"));
    fx.script_usage(200, usage_fixture());

    // 77 % on the live account is below 99.9 − 15, so the tick does not escalate: one pick.
    let report = fx
        .engine
        .collect_usage(CollectMode::Scheduled {
            provider: ProviderId::new(CLAUDE_CODE),
            threshold: 99.9,
            models: Vec::new(),
        })
        .unwrap();

    assert_eq!(
        report.outcomes,
        [
            (b.clone(), Collected::Recorded),
            (c.clone(), Collected::Recorded)
        ]
    );
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-b", "at-rt-c"],
        "phase 1, then phase 2"
    );
    assert_eq!(fx.usage_state(&s.id), None, "never reserved");

    fx.http.clear();
    fx.script_usage(200, usage_fixture());
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), Collected::Recorded)]);
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-p"],
        "on demand, the profile's token"
    );
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use super::*;

    /// Stamps `id`'s `rejected_fp` as another process's 401 would, outside this collection's
    /// lease (`Store::set_rejected_fp` is fenced by one).
    fn stamp(db: &Path, id: &AccountId, fp: &str) {
        rusqlite::Connection::open(db)
            .unwrap()
            .execute(
                "INSERT INTO usage_state (account_id, rejected_fp) VALUES (?1, ?2) \
                 ON CONFLICT(account_id) DO UPDATE SET rejected_fp = excluded.rejected_fp",
                rusqlite::params![id.as_str(), fp],
            )
            .unwrap();
    }

    #[test]
    fn a_stamp_written_by_another_process_before_the_send_is_refused_as_the_session_s_kind() {
        // C10 for the session branch: the store's authorization refuses a token stamped after
        // this collection read its state. The agent in the session refreshes an OAuth token
        // itself, so that is `token-expired`. A setup token never refreshes: `http-401`.
        for setup in [false, true] {
            let fx = Fx::new();
            let s = if setup {
                setup_token_session(&fx)
            } else {
                session(&fx)
            };
            let (db, id, fp) = (
                fx.env.data_dir().join("tagteam.db"),
                s.id.clone(),
                access_fp(&fx, &s.bytes),
            );
            fx.engine
                .on_point("usage-before-send", Box::new(move || stamp(&db, &id, &fp)));
            fx.script_usage(200, usage_fixture());

            let report = fx.collect(&[&s.id]);

            let want = if setup { "http-401" } else { "token-expired" };
            assert_eq!(report.outcomes, [(s.id.clone(), failed(want))], "{want}");
            assert!(fx.http.requests().is_empty(), "{want}: nothing was sent");
            assert_eq!(usage_requests(&fx), 0, "{want}: the slot went back");
        }
    }
}
