//! §15.2 provider neutrality for sessions: FakeAgent's profiles go through the same engine as
//! Claude Code's, with its own config-dir variable (`FAKEAGENT_HOME`), session records
//! (`procs/`), credential file and share policy. Session records are judged by the fixture's
//! `FakeProcessProbe`, never by a real pid (§15.1).
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use common::{FakeFx, LSTART, Recorded, record_json, record_reading, usage_window};
use serde_json::{Value, json};
use tagteam_core::autoswitch::{AutoConfig, Decision, Strategy, Trigger};
use tagteam_core::{AccountId, Window, WindowKind};
use tagteam_engine::auto::TickOutcome;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::session::SessionState;
use tagteam_engine::vault::SERVICE;
use tagteam_fake::{FakePaths, credential_json};
use tagteam_provider::http::Method;
use tagteam_provider::liveness::{FakeProcess, parse_lstart};
use tagteam_provider::{Clock, IdentitySurface, LinksRecord, Provider, Seed};

/// The fixture clock's start, in seconds.
const NOW_S: i64 = 1_790_000_000;

/// FakeAgent's fingerprint of `secret`'s generation (§2): its renew token's.
fn fp(ffx: &FakeFx, secret: &[u8]) -> String {
    ffx.fake.fingerprint(secret).unwrap().as_str().to_owned()
}

/// A FakeAgent credential's access token and renew token.
fn tokens(secret: &[u8]) -> (String, String) {
    let v: Value = serde_json::from_slice(secret).unwrap();
    (
        v["fa"]["token"].as_str().unwrap().to_owned(),
        v["fa"]["renew"].as_str().unwrap().to_owned(),
    )
}

fn pair(token: &str, renew: &str) -> (String, String) {
    (token.to_owned(), renew.to_owned())
}

/// `alice` (stored, not live) and `bob`, FakeAgent's live login.
fn alice_and_bob(ffx: &FakeFx) -> (AccountId, AccountId) {
    (
        ffx.fake_add("alice", "tok-a", "renew-a"),
        ffx.fake_add("bob", "tok-b", "renew-b"),
    )
}

/// `id`'s FakeAgent profile as a quiescent `run` leaves it after bootstrapping it from the
/// vault: the marker, the seed (the vault's generation under the account's login epoch, §12.3
/// step 6), and the profile's own login, `handle` with `token`/`renew`, written where FakeAgent
/// writes a login when `FAKEAGENT_HOME` names the profile (its run shell's environment). With
/// the vault's own tokens it is in step; with others, it rotated since its seed (§12.5).
fn fake_profile(ffx: &FakeFx, id: &AccountId, handle: &str, token: &str, renew: &str) -> PathBuf {
    let profile = ffx.fx.make_profile_for(ffx.fake.as_ref(), id);
    let row = ffx.engine.store().unwrap().account(id).unwrap().unwrap();
    let vault = ffx.fx.vault_bytes(id).unwrap();
    Seed {
        login_epoch: row.login_epoch,
        seed_fp: fp(ffx, &vault),
        needs_bootstrap: false,
    }
    .write(&profile)
    .unwrap();
    let mut at = ffx.fx.env.clone();
    at.vars
        .insert("FAKEAGENT_HOME".into(), profile.as_os_str().to_owned());
    tagteam_fake::login(&at, handle, "ws", token, renew);
    profile
}

/// A FakeAgent session record for `pid` in FakeAgent's own records directory
/// (`<profile>/procs`), as a `run` writes it, with `LSTART` as its `procStart`. The pid's
/// process is whatever `ffx.fx.process` says; an unknown pid is dead.
/// (`Fx::plant_record` writes Claude Code's directory, so it does not fit.)
fn plant_fake_record(ffx: &FakeFx, profile: &Path, pid: u32) {
    let dir = ffx.fake.session_records_dir(profile);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{pid}.json")),
        record_json(pid, "interactive"),
    )
    .unwrap();
}

/// A FakeAgent session running in `profile`: its record in FakeAgent's own records directory,
/// and its process alive, started exactly at the record's `procStart`.
fn start_session(ffx: &FakeFx, profile: &Path) {
    plant_fake_record(ffx, profile, 4242);
    ffx.fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART),
            ..FakeProcess::default()
        },
    );
}

fn renew_requests(ffx: &FakeFx) -> usize {
    ffx.fx.http.count(Method::Post, &ffx.fake.renew_url())
}

/// The tokens FakeAgent's usage requests carried (`authorization: Fake <token>`).
fn fake_bearers(ffx: &FakeFx) -> Vec<String> {
    ffx.fx
        .http
        .requests()
        .iter()
        .filter(|r| r.url == ffx.fake.usage_url())
        .filter_map(|r| {
            r.headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .and_then(|(_, v)| v.strip_prefix("Fake "))
                .map(str::to_owned)
        })
        .collect()
}

fn script_meters(ffx: &FakeFx) {
    ffx.fx.http.push_json(
        Method::Get,
        &ffx.fake.usage_url(),
        200,
        json!({"meters": [{"id": "daily", "used": 0.42, "renews": NOW_S + 3_600}]}),
    );
}

/// `id`'s stored access token moved inside §7.2's buffer: the inactive branch would renew it
/// before fetching.
fn make_due(ffx: &FakeFx, id: &AccountId) {
    let mut v: Value = serde_json::from_slice(&ffx.fx.vault_bytes(id).unwrap()).unwrap();
    v["fa"]["expires"] = json!(ffx.fx.clock.now_ms() + 60_000);
    ffx.fx.put_vault(id, v.to_string().as_bytes());
}

/// A reading of `short` and `long` percent in FakeAgent's own windows, taken now and not due.
fn fake_reading(ffx: &FakeFx, id: &AccountId, short: f64, long: f64) {
    let windows: Vec<Window> = vec![
        usage_window("daily", WindowKind::Short, short, NOW_S + 9_630),
        usage_window("monthly", WindowKind::Long, long, NOW_S + 291_630),
    ];
    record_reading(&ffx.engine, id, &windows, NOW_S, NOW_S + 300);
}

/// Every entry under `root`, never following a link: `dir`, a link's target, or a file's text.
fn tree(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let shown = if meta.file_type().is_symlink() {
                format!("link {}", fs::read_link(&path).unwrap().display())
            } else if meta.is_dir() {
                dirs.push(path.clone());
                "dir".to_owned()
            } else {
                String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned()
            };
            out.insert(path, shown);
        }
    }
    out
}

#[test]
fn fake_agent_session_state_comes_from_its_own_records() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let row = ffx
        .engine
        .store()
        .unwrap()
        .account(&alice)
        .unwrap()
        .unwrap();
    let state = || ffx.engine.session_state(ffx.fake.as_ref(), &row).unwrap();
    assert_eq!(state(), SessionState::NoProfile);
    let profile = ffx.fx.make_profile_for(ffx.fake.as_ref(), &alice);
    assert!(
        matches!(state(), SessionState::Quiescent { .. }),
        "{:?}",
        state()
    );

    // Where Claude Code keeps its records is nothing to FakeAgent: a live Claude Code record
    // for the same pid, in Claude Code's directory, owns nothing here.
    let procs = ffx.fake.session_records_dir(&profile);
    let cc_records = ffx.fx.cc.session_records_dir(&profile);
    assert_ne!(procs, cc_records, "the two agents keep their records apart");
    ffx.fx.live_record(&profile, 4242, "interactive");
    assert!(cc_records.join("4242.json").exists());
    assert!(
        matches!(state(), SessionState::Quiescent { .. }),
        "{:?}",
        state()
    );

    plant_fake_record(&ffx, &profile, 4242);
    assert!(
        matches!(state(), SessionState::Owned { .. }),
        "{:?}",
        state()
    );
    let listed = ffx
        .engine
        .accounts(Some(&ffx.fake_provider()))
        .unwrap()
        .remove(0)
        .accounts;
    assert!(listed.iter().any(|v| v.row.id == alice && v.in_session));

    // §12.6: its pid recycled (it runs again, started an hour later, and is not a FakeAgent
    // process: Decision 16), then a malformed record, which counts as owned.
    ffx.fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART).map(|s| s + 3_600),
            mentions_launch: Some(false),
            ..FakeProcess::default()
        },
    );
    assert!(
        matches!(state(), SessionState::Quiescent { .. }),
        "{:?}",
        state()
    );
    fs::write(procs.join("7.json"), b"[1, 2").unwrap();
    let unreadable = state();
    assert!(
        matches!(unreadable, SessionState::Unreadable { .. }) && unreadable.owned(),
        "{unreadable:?}"
    );
}

#[test]
fn a_fake_agent_account_in_a_session_is_owned_at_the_gate_and_never_renewed() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    // The session rotated its token: a running profile is still never captured (§12.5).
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    start_session(&ffx, &profile);
    let vault = ffx.fx.vault_bytes(&alice).unwrap();
    let before = ffx.fx.snapshot();

    let out = ffx
        .engine
        .refresh_stored(ffx.fake.as_ref(), &alice, &vault)
        .unwrap();

    assert!(
        matches!(out, GateOutcome::Owned(OwnedBy::Session)),
        "{out:?}"
    );
    assert_eq!(renew_requests(&ffx), 0);
    assert_eq!(ffx.fx.vault_bytes(&alice).unwrap(), vault);
    ffx.fx.assert_only_surface_changed_for(
        &IdentitySurface::default(),
        &before,
        &ffx.fx.snapshot(),
        "the gate on a FakeAgent account in a session",
    );
}

#[test]
fn a_quiescent_fake_agent_profile_that_rotated_is_captured_at_the_gate() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    let held = fs::read(profile.join("credential.json")).unwrap();
    let vault = ffx.fx.vault_bytes(&alice).unwrap();
    let before = ffx.fx.snapshot();

    let out = ffx
        .engine
        .refresh_stored(ffx.fake.as_ref(), &alice, &vault)
        .unwrap();

    // §7.3 step 3 adopted the profile's generation, so step 4 finds the vault already fresh.
    assert!(
        matches!(&out, GateOutcome::AlreadyFresh(bytes) if tokens(bytes) == pair("tok-a-2", "renew-a-2")),
        "{out:?}"
    );
    assert_eq!(
        tokens(&ffx.fx.vault_bytes(&alice).unwrap()),
        pair("tok-a-2", "renew-a-2")
    );
    let prev = ffx.fx.kc.get(SERVICE, &format!("{alice}.prev")).unwrap();
    assert_eq!(
        tokens(&prev),
        pair("tok-a", "renew-a"),
        "the vault keeps its own as .prev"
    );
    assert_eq!(
        Seed::read(&profile).present().unwrap().seed_fp,
        fp(&ffx, &held),
        "the seed moves to the captured generation"
    );
    assert_eq!(
        fs::read(profile.join("credential.json")).unwrap(),
        held,
        "a capture reads the profile and never writes it"
    );
    assert_eq!(renew_requests(&ffx), 0);
    ffx.fx.assert_only_surface_changed_for(
        &IdentitySurface::default(),
        &before,
        &ffx.fx.snapshot(),
        "a FakeAgent capture",
    );
}

#[test]
fn a_fake_agent_profile_and_vault_that_both_moved_conflict_and_nothing_moves() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    // The seed is a third generation: since they last agreed, the vault and the profile moved.
    let row = ffx
        .engine
        .store()
        .unwrap()
        .account(&alice)
        .unwrap()
        .unwrap();
    let older = credential_json("tok-a-0", Some("renew-a-0"), None).to_string();
    let seed = Seed {
        login_epoch: row.login_epoch,
        seed_fp: fp(&ffx, older.as_bytes()),
        needs_bootstrap: false,
    };
    seed.write(&profile).unwrap();
    let vault = ffx.fx.vault_bytes(&alice).unwrap();

    let out = ffx
        .engine
        .refresh_stored(ffx.fake.as_ref(), &alice, &vault)
        .unwrap();

    assert!(matches!(out, GateOutcome::Conflict), "{out:?}");
    assert_eq!(ffx.fx.vault_bytes(&alice).unwrap(), vault);
    assert_eq!(Seed::read(&profile).present().unwrap(), seed);
    assert_eq!(renew_requests(&ffx), 0);
}

#[test]
fn a_session_owned_fake_agent_account_is_read_with_the_profiles_token_and_never_renewed() {
    // §8.1: read-only, with the profile's token, no lock and no refresh. The vault's own token
    // is due, so the inactive branch would have renewed it first.
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    start_session(&ffx, &profile);
    make_due(&ffx, &alice);
    script_meters(&ffx);
    let vault = ffx.fx.vault_bytes(&alice).unwrap();
    let held = fs::read(profile.join("credential.json")).unwrap();

    let lock = ffx.engine.lock_account(&alice).unwrap();
    let report = ffx
        .engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![alice.clone()],
        })
        .unwrap();
    drop(lock);

    assert_eq!(report.outcomes, [(alice.clone(), Collected::Recorded)]);
    assert_eq!(fake_bearers(&ffx), ["tok-a-2"]);
    assert_eq!(renew_requests(&ffx), 0);
    assert_eq!(
        ffx.fx.vault_bytes(&alice).unwrap(),
        vault,
        "the vault is untouched"
    );
    assert_eq!(
        fs::read(profile.join("credential.json")).unwrap(),
        held,
        "so is the profile"
    );
    let windows = ffx
        .engine
        .store()
        .unwrap()
        .usage_state(&alice)
        .unwrap()
        .unwrap()
        .last_good
        .unwrap();
    assert_eq!(
        (windows[0].key.as_str(), windows[0].pct.round()),
        ("daily", 42.0)
    );
}

#[test]
fn a_fake_agent_profile_shares_by_fake_agents_own_allowlist() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let outer = FakePaths::resolve(&ffx.fx.env).dir;
    fs::create_dir_all(outer.join("notes")).unwrap();
    fs::write(outer.join("notes/today.md"), "note\n").unwrap();
    fs::write(outer.join("prefs.json"), "{\"theme\": \"x\"}\n").unwrap();
    fs::write(outer.join("mystery.db"), "?").unwrap();
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a", "renew-a");
    let surface = ffx.fake.identity_surface(&ffx.fx.env);
    let before = ffx.fx.snapshot();

    let first = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();

    let shared = BTreeSet::from(["journal.log", "notes", "prefs.json"]);
    let created: BTreeSet<&str> = first.created.iter().map(String::as_str).collect();
    assert_eq!(created, shared, "{first:?}");
    for name in &shared {
        assert_eq!(
            fs::read_link(profile.join(name)).unwrap(),
            fs::canonicalize(outer.join(name)).unwrap(),
            "{name} links to the fully resolved source"
        );
    }
    assert_eq!(
        fs::read(outer.join("journal.log")).unwrap(),
        b"",
        "the must-share entry is created empty in the outer home"
    );
    for private in ["credential.json", "identity.json"] {
        let meta = fs::symlink_metadata(profile.join(private)).unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "{private} stays the profile's own"
        );
    }
    assert!(
        fs::symlink_metadata(profile.join("mystery.db")).is_err(),
        "an unknown entry stays private"
    );
    let noted = first.warnings.iter().filter(|w| w.contains("mystery.db"));
    assert_eq!(noted.count(), 1, "{:?}", first.warnings);
    let links = LinksRecord::read(&profile).present().unwrap();
    let linked: BTreeSet<&str> = links.links.keys().map(String::as_str).collect();
    assert_eq!(linked, shared);
    assert_eq!(
        links.noted_unknown,
        BTreeSet::from(["mystery.db".to_owned()])
    );
    for cc_entry in ["projects", "history.jsonl"] {
        assert!(
            !outer.join(cc_entry).exists() && !profile.join(cc_entry).exists(),
            "Claude Code's must-share {cc_entry} is nothing to FakeAgent"
        );
    }
    // FakeAgent's surface allows `journal.log` created empty where there was none (§3), and
    // nothing else outside tagteam's own data.
    ffx.fx.assert_only_surface_changed_for(
        &surface,
        &before,
        &ffx.fx.snapshot(),
        "FakeAgent's sync",
    );

    // Again: nothing new, and the unknown entry is not noted twice.
    let second = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();
    assert!(
        second.created.is_empty() && second.removed.is_empty(),
        "{second:?}"
    );
    assert!(
        !second.warnings.iter().any(|w| w.contains("mystery.db")),
        "{:?}",
        second.warnings
    );

    // A shared file replaced by a regular one in the profile: a warning, and both copies stay.
    fs::remove_file(profile.join("prefs.json")).unwrap();
    fs::write(profile.join("prefs.json"), "{\"theme\": \"mine\"}\n").unwrap();
    let third = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();
    assert!(
        third.warnings.iter().any(|w| w.contains("prefs.json")),
        "{:?}",
        third.warnings
    );
    assert_eq!(
        fs::read_to_string(profile.join("prefs.json")).unwrap(),
        "{\"theme\": \"mine\"}\n"
    );
    assert_eq!(
        fs::read_to_string(outer.join("prefs.json")).unwrap(),
        "{\"theme\": \"x\"}\n"
    );

    // The must-share entry split: the sync refuses, naming it, and both copies stay.
    fs::remove_file(profile.join("journal.log")).unwrap();
    fs::write(profile.join("journal.log"), "a private copy\n").unwrap();
    let err = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap_err();
    assert_eq!(err.kind(), "profile-split", "{err}");
    assert!(err.to_string().contains("journal.log"), "{err}");
    assert_eq!(
        fs::read_to_string(profile.join("journal.log")).unwrap(),
        "a private copy\n"
    );
    assert_eq!(fs::read(outer.join("journal.log")).unwrap(), b"");
}

#[test]
fn fake_agent_sessions_write_nothing_outside_its_surface_and_its_profile() {
    // The negative surface test (§15.2, carried over). FakeAgent's profile is synced, then
    // lazily captured at the gate while quiescent; then a session starts and its usage is
    // read from the profile. Together these move only FakeAgent's declared surface (its
    // create-only must-share entry, §3) and its own profile. Claude Code's fixture home, its
    // Keychain items and its own profile stay byte-identical: the snapshot walks all of HOME
    // and every non-vault Keychain item, and the tree walks every profile.
    let ffx = FakeFx::new();
    let cc = ffx.fx.add("cc@b.co", "rt-cc");
    ffx.fx.add("cc2@b.co", "rt-cc2"); // Claude Code's live login
    let cc_profile = ffx.fx.make_profile(&cc);
    fs::write(
        cc_profile.join(".credentials.json"),
        b"{\"claudeAiOauth\": {}}",
    )
    .unwrap();
    let (alice, _bob) = alice_and_bob(&ffx);
    let outer = FakePaths::resolve(&ffx.fx.env).dir;
    fs::create_dir_all(outer.join("notes")).unwrap();
    fs::write(outer.join("notes/today.md"), "note\n").unwrap();
    fs::write(outer.join("prefs.json"), "{}\n").unwrap();
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    let surface = ffx.fake.identity_surface(&ffx.fx.env);
    let sessions = ffx.fx.env.data_dir().join("sessions");
    let (before, profiles_before) = (ffx.fx.snapshot(), tree(&sessions));

    ffx.engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();
    let vault = ffx.fx.vault_bytes(&alice).unwrap();
    let gated = ffx
        .engine
        .refresh_stored(ffx.fake.as_ref(), &alice, &vault)
        .unwrap();
    assert!(matches!(gated, GateOutcome::AlreadyFresh(_)), "{gated:?}");
    start_session(&ffx, &profile);
    script_meters(&ffx);
    let report = ffx
        .engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![alice.clone()],
        })
        .unwrap();
    assert_eq!(report.outcomes, [(alice.clone(), Collected::Recorded)]);

    let (after, profiles_after) = (ffx.fx.snapshot(), tree(&sessions));
    ffx.fx.assert_only_surface_changed_for(
        &surface,
        &before,
        &after,
        "a FakeAgent profile's sync, capture and session-owned usage",
    );
    let changed: Vec<&PathBuf> = profiles_before
        .keys()
        .chain(profiles_after.keys())
        .filter(|p| profiles_before.get(*p) != profiles_after.get(*p))
        .collect();
    assert!(
        !changed.is_empty(),
        "the sync and the capture wrote alice's profile"
    );
    assert!(
        changed.iter().all(|p| p.starts_with(&profile)),
        "written outside alice's profile: {changed:?}"
    );
}

#[test]
fn a_tick_never_targets_a_fakeagent_candidate_a_session_owns() {
    // Task 11 fills `AccountSnapshot.session_owned` from `session_state`, which reads
    // FakeAgent's own records directory (`procs/`), not Claude Code's. `alice` would win `best`
    // (the most headroom) were she not in a session; `bob` is live and over the threshold.
    let ffx = FakeFx::new();
    let carol = ffx.fake_add("carol", "tok-c", "renew-c");
    let (alice, bob) = alice_and_bob(&ffx);
    fake_reading(&ffx, &alice, 10.0, 20.0);
    fake_reading(&ffx, &carol, 10.0, 50.0);
    fake_reading(&ffx, &bob, 20.0, 95.0);
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a", "renew-a");
    start_session(&ffx, &profile);
    assert_eq!(ffx.fake_live_label().as_deref(), Some("bob@ws"));
    let config = AutoConfig {
        threshold: 90.0,
        hysteresis_pct: 10.0,
        cooldown_s: 300,
        interval_s: 60,
        unhealthy_ticks: 3,
        strategy: Strategy::Best,
        include_api_key_accounts: false,
        models: Vec::new(),
        long_window: ffx.fake.primary_long_window().map(str::to_owned),
    };
    let sink = Recorded::default();
    let mut engine = ffx
        .engine
        .auto(&ffx.fake_provider(), config, false)
        .unwrap()
        .unwrap();

    let (outcome, decision) = engine.tick(&sink).unwrap();

    assert_eq!(outcome, TickOutcome::Switched, "{decision:?}");
    assert_eq!(
        decision,
        Decision::Switch {
            trigger: Trigger::Proactive,
            targets: vec![carol.clone()],
            recheck: false,
        }
    );
    assert_eq!(ffx.fake_live_label().as_deref(), Some("carol@ws"));
}
