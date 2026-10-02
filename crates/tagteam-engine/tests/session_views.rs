//! The views in and around sessions (§12.8, §13.1, §13.2): which accounts are in a session, and
//! the run shell's own account. Session records are judged by the fixture's `FakeProcessProbe`,
//! never by a real pid (§15.1).
mod common;

use std::fs;
use std::path::Path;

use common::{FakeFx, Fx, LSTART};
use serde_json::json;
use tagteam_core::AccountId;
use tagteam_engine::Engine;
use tagteam_engine::views::{AccountView, ShellAccount, StatusView, StatuslineView};
use tagteam_provider::{FakeProcess, MARKER_FILE, Provider, parse_lstart};

/// `id`'s row as `engine`'s list shows it.
fn listed(engine: &Engine, id: &AccountId) -> AccountView {
    engine
        .accounts(None)
        .unwrap()
        .into_iter()
        .flat_map(|l| l.accounts)
        .find(|v| &v.row.id == id)
        .expect("the account is listed")
}

/// The engine of a tagteam command run inside `profile`'s Claude Code session: in its tools,
/// hooks or statusline.
fn inside(fx: &Fx, profile: &Path) -> Engine {
    fx.engine_located(fx.shell_env(profile))
}

#[test]
fn an_account_is_in_session_while_its_profile_holds_a_live_reservation() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    assert!(!listed(&fx.engine, &a).in_session, "no profile");
    let profile = fx.make_profile(&a);
    assert!(!listed(&fx.engine, &a).in_session, "a quiescent profile");

    let launch = fx.hold_reservation(&profile);
    let (va, vb) = (listed(&fx.engine, &a), listed(&fx.engine, &b));
    assert_eq!((va.in_session, va.active), (true, false));
    assert_eq!((vb.in_session, vb.active), (false, true));
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert!(
        fx.engine.account_view(row, false).in_session,
        "an account command's row says so too"
    );

    drop(launch);
    assert!(!listed(&fx.engine, &a).in_session, "the session ended");
}

#[test]
fn a_session_record_counts_while_its_process_runs_and_an_unreadable_one_always() {
    // §12.6: a recycled pid no longer belongs to the record's writer; a malformed record
    // blocks destructive commands, so the list shows it in session.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    let records = fx.cc.session_records_dir(&profile);
    fx.dead_record(&profile, 4242);
    assert!(
        !listed(&fx.engine, &a).in_session,
        "pid 4242 is not running"
    );

    fx.live_record(&profile, 4242, "interactive");
    assert!(listed(&fx.engine, &a).in_session);

    // Decision 16: a start time that moved is no proof on its own. The pid was recycled only
    // when its new process does not mention the launch command either.
    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART).map(|s| s + 600),
            mentions_launch: Some(false),
            ..FakeProcess::default()
        },
    );
    assert!(!listed(&fx.engine, &a).in_session, "the pid was recycled");

    fs::write(records.join("7.json"), b"{\"pid\": ").unwrap();
    assert!(listed(&fx.engine, &a).in_session);
}

#[test]
fn in_a_run_shell_the_default_login_stays_live_and_the_marker_names_the_sessions_account() {
    // §12.8, §15.2 "Run shell".
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    assert!(matches!(
        fx.engine.shell_account().unwrap(),
        ShellAccount::NotInShell
    ));
    let profile = fx.make_profile(&a);
    let _launch = fx.hold_reservation(&profile);
    let engine = inside(&fx, &profile);

    match engine.shell_account().unwrap() {
        ShellAccount::Managed(row) => assert_eq!(row.id, a),
        other => panic!("{other:?}"),
    }
    let (va, vb) = (listed(&engine, &a), listed(&engine, &b));
    assert_eq!((va.in_session, va.active), (true, false));
    assert_eq!((vb.in_session, vb.active), (false, true));
    match engine.status(&fx.provider()).unwrap() {
        StatusView::Managed { account, total } => assert_eq!((account.row.id, total), (b, 2)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_statusline_view_never_computes_session_state() {
    // Decision 17: `list` marks a session-owned account, the status bar's view of the same
    // account does not, so the status bar never looks inside a profile directory (§13.5).
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let profile = fx.make_profile(&b);
    let _launch = fx.hold_reservation(&profile);
    assert!(listed(&fx.engine, &b).in_session, "list says so");

    match fx.engine.statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => {
            assert_eq!(
                (account.row.id, account.active, account.in_session),
                (b, true, false)
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_marker_naming_an_account_the_store_lacks_is_unmanaged() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("0192-gone"));
    assert!(matches!(
        inside(&fx, &profile).shell_account().unwrap(),
        ShellAccount::Unmanaged
    ));
}

#[test]
fn a_run_shell_with_no_store_is_unmanaged_and_creates_none() {
    let fx = Fx::new();
    let profile = fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("0192"));
    assert!(matches!(
        inside(&fx, &profile).shell_account().unwrap(),
        ShellAccount::Unmanaged
    ));
    assert!(
        !fx.env.data_dir().join("tagteam.db").exists(),
        "§5: a read creates nothing"
    );
}

#[test]
fn an_unreadable_marker_is_the_refusal_that_names_it() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    fs::write(
        profile.join(MARKER_FILE),
        b"{\"format\": \"tagteam-profile\", ",
    )
    .unwrap();

    let err = inside(&fx, &profile).shell_account().unwrap_err();

    assert_eq!(err.kind(), "run-shell-unreadable", "{err}");
    // The run shell names its profile as the variable gives it (`shell_env` does not
    // canonicalize), and the refusal names the marker in that directory.
    let marker = profile.join(MARKER_FILE);
    assert!(
        err.to_string().contains(&marker.display().to_string()),
        "{err}"
    );
}

#[test]
fn statusline_in_a_run_shell_is_the_markers_account_and_reads_no_live_identity() {
    // §13.5: the marker names the account; neither the profile's `.claude.json` nor the
    // default home's is parsed, and the live-identity cache is neither read nor written.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let profile = fx.make_profile(&a);
    // CC's own file in the profile, garbled: parsed, it would show nothing.
    fs::write(profile.join(".claude.json"), b"{\"oauthAccount\": ").unwrap();
    let _launch = fx.hold_reservation(&profile);

    match inside(&fx, &profile).statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => assert_eq!(
            (account.row.id, account.active, account.in_session),
            (a, false, false),
            "the session's account, not the live login, and no session state computed for it \
             (Decision 17) even with a reservation held"
        ),
        other => panic!("{other:?}"),
    }
    let cache = || {
        fx.engine
            .store()
            .unwrap()
            .live_identity_cache(&fx.provider())
            .unwrap()
    };
    assert!(
        cache().is_none(),
        "nothing went through the live-identity cache"
    );

    match fx.engine.statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => assert_eq!(account.row.id, b),
        other => panic!("{other:?}"),
    }
    assert!(cache().is_some(), "outside, the live login goes through it");
}

#[test]
fn statusline_in_a_run_shell_of_an_account_tagteam_does_not_manage_shows_nothing() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a"); // live: never shown in the session's place
    let profile = fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("0192-gone"));
    assert!(matches!(
        inside(&fx, &profile).statusline(&fx.provider()).unwrap(),
        StatuslineView::NoLogin
    ));
}

#[test]
fn statusline_under_an_unreadable_marker_shows_nothing_and_parses_no_profile() {
    // §12.8: the outer home is unknown. The environment still names the profile, whose own
    // `.claude.json` names a: a parse of it would show a's line.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    let login = json!({"oauthAccount": Fx::oauth_account("a@x.co")});
    fs::write(profile.join(".claude.json"), login.to_string()).unwrap();
    fs::write(profile.join(MARKER_FILE), b"[").unwrap();
    assert!(matches!(
        inside(&fx, &profile).statusline(&fx.provider()).unwrap(),
        StatuslineView::NoLogin
    ));
}

#[test]
fn statusline_for_another_provider_in_a_run_shell_is_that_providers_live_login() {
    // The marker names Claude Code's session: FakeAgent's line is still its own live login,
    // read in the outer home.
    let ffx = FakeFx::new();
    let a = ffx.fx.add("a@x.co", "rt-a");
    ffx.fx.add("b@x.co", "rt-b");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let profile = ffx.fx.make_profile(&a);
    let engine = ffx.engine_located(ffx.fx.shell_env(&profile));
    match engine.statusline(&ffx.fake_provider()).unwrap() {
        StatuslineView::Managed { account } => {
            assert_eq!((account.row.id, account.active), (alice, true))
        }
        other => panic!("{other:?}"),
    }
    match engine.statusline(&ffx.fx.provider()).unwrap() {
        StatuslineView::Managed { account } => assert_eq!(account.row.id, a),
        other => panic!("{other:?}"),
    }
}
