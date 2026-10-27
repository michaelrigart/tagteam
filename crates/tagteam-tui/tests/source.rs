//! `EngineSource` over a real engine (spec §4.4, §11 "Source over a real engine"): a fixture
//! home with Claude Code and FakeAgent registered, a fake Keychain, a scripted HTTP port and a
//! fake clock, and FakeAgent's transcripts for the token report (§9). Nothing reaches the real
//! HOME, the network or the Keychain (core §15.1).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tagteam_cc::CLAUDE_CODE;
use tagteam_cc::ClaudeCode;
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{Eligibility, EventRow, NewAccount, Reserve, Store};
use tagteam_engine::switch::LiveLogin;
use tagteam_engine::timeline::{AccountRef, EventView, Scope};
use tagteam_engine::tokens::{
    PeriodKey, RefreshOutcome, RowAccount, TokenGrouping, TokenPeriod, TokenQuery, tokens_lock_path,
};
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig, EventKind, EventSource};
use tagteam_fake::{FAKE_AGENT, FakeAgent, FakeUsage, append_transcript, usage_line_json};
use tagteam_provider::http::Method;
use tagteam_provider::liveness::FakeProcessProbe;
use tagteam_provider::process::ScriptedSpawner;
use tagteam_provider::{
    Cancel, Env, FakeClock, FakeKeychain, FlockGuard, Identity, ProcessStamp, ProfileMarker,
    RunShell, ScriptedHttp,
};
use tagteam_tui::source::{EngineSource, Source};

/// The fixture clock's start, in ms: every reading below is placed relative to it.
const NOW_MS: i64 = 1_790_000_000_000;
const NOW_S: i64 = NOW_MS / 1000;

struct Home {
    _dir: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
    clock: Arc<FakeClock>,
    http: Arc<ScriptedHttp>,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        fs::write(env.home.join(".claude.json"), "{\n  \"userID\": \"u\"\n}\n").unwrap();
        Home {
            _dir: dir,
            env,
            kc: Arc::new(FakeKeychain::new()),
            clock: Arc::new(FakeClock::new(NOW_MS)),
            http: Arc::new(ScriptedHttp::new()),
        }
    }

    /// An engine as the binary builds one for `list` (`build_engine`), over the fixture's ports.
    fn engine_in(&self, run_shell: RunShell) -> Engine {
        self.engine_with(self.env.clone(), run_shell)
    }

    /// `engine_in`, over `env`.
    fn engine_with(&self, env: Env, run_shell: RunShell) -> Engine {
        Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new()
                .with(Arc::new(ClaudeCode::new(self.kc.clone(), Platform::MacOs)))
                .with(Arc::new(FakeAgent::new())),
            vault: Vault::new(Box::new(KeychainVault::new(self.kc.clone()))),
            oracle: Arc::new(NoOracle),
            clock: self.clock.clone(),
            http: self.http.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
            process: Arc::new(FakeProcessProbe::new()),
            spawner: Arc::new(ScriptedSpawner::new()),
            run_shell,
            source: EventSource::Cli,
        })
    }

    fn source(&self) -> EngineSource {
        EngineSource::new(self.engine_in(RunShell::Outside))
    }

    /// A source whose engine takes `shutdown` as its cancel token, as every lane's does (§4.3).
    fn source_under(&self, shutdown: &Cancel) -> EngineSource {
        let mut env = self.env.clone();
        env.cancel = shutdown.clone();
        EngineSource::new(self.engine_with(env, RunShell::Outside))
    }

    /// `usage`, appended to FakeAgent's transcript `name`, a line each.
    fn transcript(&self, name: &str, usage: &[FakeUsage]) {
        let text: String = usage
            .iter()
            .map(|u| format!("{}\n", usage_line_json(u)))
            .collect();
        append_transcript(&self.env, name, &text);
    }

    fn db(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }

    fn store(&self) -> Store {
        Store::open(&self.db()).unwrap()
    }

    /// `email`'s account at `position` under `provider`, as `add` stores it.
    fn add(&self, provider: &str, position: u32, email: &str) -> AccountId {
        let id = AccountId::from_string(format!("{provider}-{position}"));
        self.store()
            .insert_account(&NewAccount {
                id: &id,
                provider: &ProviderId::new(provider),
                position,
                identity_key: &format!("{email}\n"),
                identity: &Identity {
                    label: email.into(),
                    email: Some(email.into()),
                    org_uuid: String::new(),
                    org_name: None,
                    account_uuid: None,
                    raw: json!({"emailAddress": email}),
                },
                kind: "oauth",
                alias: None,
                login_expires_at: None,
                added_at: 1,
            })
            .unwrap();
        id
    }

    /// What `claude /login` as `email` leaves in `~/.claude.json`.
    fn login(&self, email: &str) {
        let config = json!({
            "userID": "u",
            "oauthAccount": {"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")},
        });
        fs::write(
            self.env.home.join(".claude.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }

    /// A reading of `id` taken `ago_s` before now, through the store's own write path.
    fn reading(&self, id: &AccountId, ago_s: i64, pct: f64) {
        let store = self.store();
        let row = store.account(id).unwrap().unwrap();
        let at_s = NOW_S - ago_s;
        let Reserve::Reserved(r) = store
            .reserve_usage(
                &row,
                at_s * 1000,
                Eligibility::OnDemand,
                &PollBudget::STANDARD,
            )
            .unwrap()
        else {
            panic!("no reservation for a reading {ago_s} s ago");
        };
        let window = Window {
            key: "5h".into(),
            label: "5h".into(),
            kind: WindowKind::Short,
            pct,
            resets_at: Some(NOW_S + 3_600),
            period_s: None,
            detail: None,
        };
        let plan = PollPlan {
            interval_s: 300,
            next_poll_at: at_s + 300,
        };
        assert!(store.record_usage(&r, &[window], at_s, &plan, 180).unwrap());
    }

    /// A usable stored credential for `id`.
    fn credential(&self, id: &AccountId) {
        let credential = json!({"claudeAiOauth": {
            "accessToken": format!("at-{id}"),
            "refreshToken": format!("rt-{id}"),
            "expiresAt": NOW_MS + 3_600_000,
            "refreshTokenExpiresAt": 1_797_000_000_000i64,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max",
        }});
        self.kc
            .put(SERVICE, id.as_str(), credential.to_string().as_bytes());
    }

    /// A usable stored credential for `id`, and one usage reply queued for it.
    fn fetchable(&self, id: &AccountId) {
        self.credential(id);
        let recorded: Value = serde_json::from_str(include_str!(
            "../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
        ))
        .unwrap();
        self.http.push_json(
            Method::Get,
            &Endpoints::production().usage,
            200,
            recorded["body"].clone(),
        );
    }

    /// A usable stored credential for `id`, and the usage endpoint's refusal queued for it.
    fn rate_limited(&self, id: &AccountId) {
        self.credential(id);
        let body = json!({"error": {"type": "rate_limit_error"}});
        self.http
            .push_json(Method::Get, &Endpoints::production().usage, 429, body);
    }
}

#[test]
fn accounts_is_lists_view_read_at_the_engine_clock() {
    let home = Home::new();
    let a = home.add(CLAUDE_CODE, 1, "a@x.co");
    let b = home.add(CLAUDE_CODE, 2, "b@x.co");
    home.login("a@x.co");
    let snap = home.source().accounts(None).unwrap();
    assert_eq!(snap.providers.len(), 1);
    let p = &snap.providers[0];
    assert_eq!(p.provider, ProviderId::new(CLAUDE_CODE));
    assert_eq!(p.active_position, Some(1));
    let ids: Vec<&AccountId> = p.accounts.iter().map(|v| &v.row.id).collect();
    assert_eq!(ids, [&a, &b]);
    assert_eq!(
        (
            snap.this,
            &snap.providers[0].live,
            snap.in_run_shell,
            snap.read_at_s
        ),
        (None, &LiveLogin::Managed { position: 1 }, false, NOW_S)
    );
}

#[test]
fn a_live_login_tagteam_does_not_manage_is_named() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    home.login("s@x.co");
    let snap = home.source().accounts(None).unwrap();
    assert_eq!(
        snap.providers[0].live,
        LiveLogin::Unmanaged {
            email: "s@x.co".into()
        }
    );
    assert_eq!(snap.providers[0].active_position, None);
}

#[test]
fn a_live_engine_lock_holder_is_named_and_a_dead_one_is_not() {
    // C-16, T2-11: another process holds the provider's engine lock when its record names a live
    // pid with the start time it was written with, and is not the daemon.
    let home = Home::new();
    let locks = home.env.data_dir().join("locks");
    fs::create_dir_all(&locks).unwrap();
    let write = |name: &str, stamp: ProcessStamp| {
        fs::write(
            locks.join(name),
            format!("{{\"pid\":{},\"start\":{}}}\n", stamp.pid, stamp.start),
        )
        .unwrap();
    };
    let me = ProcessStamp::current().unwrap();
    let cc = ProviderId::new(CLAUDE_CODE);
    let holders = || home.source().accounts(None).unwrap().auto_holders;
    assert_eq!(holders(), [], "no record, no holder");

    write("autoswitch-claude-code.lock", me);
    assert_eq!(holders(), [(cc.clone(), me.pid)]);

    let recycled = ProcessStamp {
        pid: me.pid,
        start: me.start - 1,
    };
    write("autoswitch-claude-code.lock", recycled);
    assert_eq!(
        holders(),
        [],
        "the pid runs, but it started at another time"
    );

    write("autoswitch-claude-code.lock", me);
    fs::write(
        locks.join("daemon.lock"),
        format!(
            "{{\"pid\":{},\"start\":{},\"version\":\"1.0.0\"}}\n",
            me.pid, me.start
        ),
    )
    .unwrap();
    assert_eq!(
        holders(),
        [],
        "the daemon holding its own job's lock is not another terminal"
    );
}

#[test]
fn no_store_is_the_default_provider_with_no_accounts_and_creates_nothing() {
    let home = Home::new();
    let snap = home.source().accounts(None).unwrap();
    assert_eq!(snap.providers.len(), 1);
    assert_eq!(snap.providers[0].provider, ProviderId::new(CLAUDE_CODE));
    assert!(snap.providers[0].accounts.is_empty());
    assert!(!home.db().exists(), "a read never creates the store");
    let summary = home.source().collect_due(&[]).unwrap();
    assert_eq!((summary.fetched, summary.warnings.len()), (0, 0));
    assert!(!home.db().exists());
}

#[test]
fn accounts_carries_each_provider_s_last_auto_line_event() {
    // §5.3, §12.2, T4-30: each listed provider's `last_auto_event` of the last 30 days, its
    // accounts named at read time; an `add` is no auto-line kind.
    let home = Home::new();
    assert_eq!(
        home.source().accounts(None).unwrap().last_auto,
        [],
        "without a store"
    );
    let a = home.add(CLAUDE_CODE, 1, "a@x.co");
    let b = home.add(CLAUDE_CODE, 2, "b@x.co");
    let cc = ProviderId::new(CLAUDE_CODE);
    let detail = json!({"window": "5h", "label": "5h", "pct": 93.0, "threshold": 90.0});
    let event = |at: i64, kind: EventKind, from: Option<&AccountId>, to: &AccountId| EventRow {
        at,
        provider: cc.clone(),
        kind,
        from_id: from.cloned(),
        to_id: Some(to.clone()),
        trigger: (kind == EventKind::Switch).then(|| "proactive".to_owned()),
        source: EventSource::Auto,
        detail: (kind == EventKind::Switch).then(|| detail.clone()),
    };
    let switched_at = NOW_MS - 3_600_000;
    let store = home.store();
    store
        .insert_event(&event(switched_at, EventKind::Switch, Some(&a), &b), false)
        .unwrap();
    store
        .insert_event(&event(NOW_MS - 60_000, EventKind::Add, None, &b), false)
        .unwrap();
    let named = |id: &AccountId, position: u32, email: &str| AccountRef {
        id: id.clone(),
        position: Some(position),
        alias: None,
        email: Some(email.into()),
        removed: false,
    };
    let snap = home.source().accounts(None).unwrap();
    assert_eq!(
        snap.last_auto,
        [(
            cc,
            EventView {
                at_ms: switched_at,
                kind: EventKind::Switch,
                source: EventSource::Auto,
                trigger: Some("proactive".into()),
                from: Some(named(&a, 1, "a@x.co")),
                to: Some(named(&b, 2, "b@x.co")),
                detail: Some(detail),
                before_pct: None,
                after_pct: None,
            }
        )]
    );
}

#[test]
fn last_auto_is_each_provider_s_own_and_reaches_back_30_days() {
    // A switch 31 days old is past the read's 30 days; FakeAgent's event is in its own entry.
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    let b = home.add(CLAUDE_CODE, 2, "b@x.co");
    let f = home.add(FAKE_AGENT, 1, "f@x.co");
    let (cc, fake) = (ProviderId::new(CLAUDE_CODE), ProviderId::new(FAKE_AGENT));
    let switch = |provider: &ProviderId, at: i64, to: &AccountId| EventRow {
        at,
        provider: provider.clone(),
        kind: EventKind::Switch,
        from_id: None,
        to_id: Some(to.clone()),
        trigger: None,
        source: EventSource::Daemon,
        detail: None,
    };
    let store = home.store();
    let old = NOW_MS - 31 * 86_400_000;
    store.insert_event(&switch(&cc, old, &b), false).unwrap();
    let recent = NOW_MS - 600_000;
    store
        .insert_event(&switch(&fake, recent, &f), false)
        .unwrap();
    let snap = home.source().accounts(None).unwrap();
    assert!(snap.providers.iter().any(|p| p.provider == cc));
    let to = AccountRef {
        id: f,
        position: Some(1),
        alias: None,
        email: Some("f@x.co".into()),
        removed: false,
    };
    assert_eq!(
        snap.last_auto,
        [(
            fake,
            EventView {
                at_ms: recent,
                kind: EventKind::Switch,
                source: EventSource::Daemon,
                trigger: None,
                from: None,
                to: Some(to),
                detail: None,
                before_pct: None,
                after_pct: None,
            }
        )],
        "none for Claude Code: its switch is 31 days old"
    );
}

#[test]
fn last_auto_reaches_a_switch_29_days_old() {
    // A switch 29 days old is inside the read's 30 days and is the entry, not the 31-day one.
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    let b = home.add(CLAUDE_CODE, 2, "b@x.co");
    let cc = ProviderId::new(CLAUDE_CODE);
    let switch = |at: i64| EventRow {
        at,
        provider: cc.clone(),
        kind: EventKind::Switch,
        from_id: None,
        to_id: Some(b.clone()),
        trigger: None,
        source: EventSource::Daemon,
        detail: None,
    };
    let store = home.store();
    store
        .insert_event(&switch(NOW_MS - 31 * 86_400_000), false)
        .unwrap();
    let within = NOW_MS - 29 * 86_400_000;
    store.insert_event(&switch(within), false).unwrap();
    let snap = home.source().accounts(None).unwrap();
    let last: Vec<(&ProviderId, i64)> = snap.last_auto.iter().map(|(p, e)| (p, e.at_ms)).collect();
    assert_eq!(last, [(&cc, within)]);
}

#[test]
fn several_providers_are_listed_and_a_provider_narrows_them() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    home.add(FAKE_AGENT, 1, "f@x.co");
    let source = home.source();
    let all = source.accounts(None).unwrap();
    let names: Vec<&str> = all.providers.iter().map(|p| p.provider.as_str()).collect();
    assert_eq!(names, [CLAUDE_CODE, FAKE_AGENT]);
    let fake = source.accounts(Some(&ProviderId::new(FAKE_AGENT))).unwrap();
    assert_eq!(fake.providers.len(), 1);
    assert_eq!(fake.providers[0].accounts[0].row.position, 1);
}

#[test]
fn a_run_shell_names_its_own_account() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    let b = home.add(CLAUDE_CODE, 2, "b@x.co");
    let run_shell = RunShell::Inside {
        profile: home.env.home.join("profile-b"),
        marker: ProfileMarker {
            provider: ProviderId::new(CLAUDE_CODE),
            account_id: b.clone(),
            config_dir: "profile-b".into(),
            outer: Value::Null,
        },
    };
    let snap = EngineSource::new(home.engine_in(run_shell))
        .accounts(None)
        .unwrap();
    assert_eq!(snap.this, Some(b));
    assert!(snap.in_run_shell);
}

#[test]
fn a_stale_reading_is_shown_with_its_age_and_not_as_current() {
    let home = Home::new();
    let a = home.add(CLAUDE_CODE, 1, "a@x.co");
    home.reading(&a, 3_600, 50.0);
    let snap = home.source().accounts(None).unwrap();
    let usage = &snap.providers[0].accounts[0].usage;
    assert_eq!(usage.age_s, Some(3_600));
    assert!(
        !usage.decision_grade,
        "an hour old is past §8.4's five minutes"
    );
    assert_eq!(usage.windows.as_ref().unwrap()[0].0.pct, 50.0);
}

#[test]
fn timeline_is_the_engine_s_for_the_scope_and_range() {
    let home = Home::new();
    let a = home.add(CLAUDE_CODE, 1, "a@x.co");
    let b = home.add(CLAUDE_CODE, 2, "b@x.co");
    home.reading(&a, 3_600, 40.0);
    home.reading(&b, 7_200, 60.0);
    let cc = ProviderId::new(CLAUDE_CODE);
    let since = NOW_S - 86_400;
    let engine = home.engine_in(RunShell::Outside);
    assert_eq!(
        home.source().timeline(&cc, &Scope::All, since, 7).unwrap(),
        engine.timeline(&cc, Scope::All, since, 7).unwrap()
    );
    assert_eq!(
        home.source()
            .timeline(&cc, &Scope::Account(b.clone()), since, 7)
            .unwrap(),
        engine.timeline(&cc, Scope::Account(b), since, 7).unwrap()
    );
}

#[test]
fn a_failed_read_carries_the_engines_kind_and_words() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    let e = home
        .source()
        .timeline(
            &ProviderId::new(CLAUDE_CODE),
            &Scope::Account(AccountId::from_string("gone")),
            0,
            70,
        )
        .unwrap_err();
    assert_eq!(e.kind, "no-such-account");
    assert_eq!(e.message, "no account matches \"gone\"");
}

#[test]
fn collect_due_counts_the_readings_and_failures_it_recorded() {
    let home = Home::new();
    let read = home.add(CLAUDE_CODE, 1, "a@x.co");
    let missing = home.add(CLAUDE_CODE, 2, "b@x.co");
    let fresh = home.add(CLAUDE_CODE, 3, "c@x.co");
    home.fetchable(&read);
    home.reading(&fresh, 10, 5.0);
    let source = home.source();
    let summary = source
        .collect_due(&[read.clone(), missing.clone(), fresh.clone()])
        .unwrap();
    // `read` recorded a reading, `missing` a `vault-absent` failure; `fresh` was not due.
    assert_eq!(summary.fetched, 2, "{summary:?}");
    assert_eq!(summary.updated, 1, "only `read` recorded a reading");
    assert!(summary.rate_limited.is_empty(), "{summary:?}");
    let snap = source.accounts(None).unwrap();
    let a = &snap.providers[0].accounts[0];
    assert_eq!(a.usage.fetched_at, Some(NOW_S));
    // Nothing is due a moment later: a reading is fresh, a failure backs off.
    let again = source.collect_due(&[read, missing, fresh]).unwrap();
    assert_eq!(again.fetched, 0);
}

#[test]
fn collect_due_lists_the_accounts_the_endpoint_rate_limited() {
    let home = Home::new();
    let fresh = home.add(CLAUDE_CODE, 1, "a@x.co");
    let limited = home.add(CLAUDE_CODE, 2, "b@x.co");
    home.reading(&fresh, 10, 5.0);
    home.rate_limited(&limited);
    let summary = home
        .source()
        .collect_due(&[fresh.clone(), limited.clone()])
        .unwrap();
    // `fresh` was not due; the refusal is recorded, so it counts as fetched, but not as updated.
    assert_eq!(summary.fetched, 1, "{summary:?}");
    assert_eq!(summary.updated, 0, "{summary:?}");
    assert_eq!(summary.rate_limited, [limited], "{summary:?}");
}

fn fake() -> ProviderId {
    ProviderId::new(FAKE_AGENT)
}

/// FakeAgent's report by day and account, over all time, in UTC.
fn by_day() -> TokenQuery {
    TokenQuery {
        provider: fake(),
        period: TokenPeriod::Day,
        grouping: TokenGrouping::Account,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: jiff::tz::TimeZone::UTC,
    }
}

/// A second response, a minute after the first, in session `session`.
fn another(session: &str) -> FakeUsage {
    FakeUsage {
        message_id: "m2".into(),
        request_id: "r2".into(),
        session_id: session.into(),
        at_ms: FakeUsage::default().at_ms + 60_000,
        ..FakeUsage::default()
    }
}

#[test]
fn the_index_refresh_reads_the_transcripts_and_the_report_shows_them() {
    let home = Home::new();
    home.transcript("s1.jsonl", &[FakeUsage::default(), another("s1")]);
    let source = home.source();
    let mut seen = Vec::new();
    let outcome = source
        .refresh_tokens(&fake(), &mut |p| seen.push(*p))
        .unwrap();
    let RefreshOutcome::Done(done) = outcome else {
        panic!("{outcome:?}");
    };
    assert!(done.complete);
    assert_eq!(done.responses_added, 2);
    assert_eq!(
        seen.last().map(|p| (p.files_done, p.files_total)),
        Some((1, 1)),
        "progress names the files"
    );
    let state = source.token_index_state(&fake()).unwrap();
    assert!(state.refreshed_at_ms.is_some());
    let report = source.token_report(&by_day()).unwrap();
    assert_eq!(report.index, state, "the report carries the index's state");
    assert_eq!(report.rows.len(), 1);
    let row = &report.rows[0];
    assert_eq!(row.account, Some(RowAccount::Unattributed));
    assert_eq!((row.responses, row.counts.total()), (2, 2 * 15));
}

#[test]
fn a_refresh_another_process_holds_is_busy_at_once() {
    // Spec §9: the TUI always tries (`Wait::Try`).
    let home = Home::new();
    home.transcript("s1.jsonl", &[FakeUsage::default()]);
    let _held = FlockGuard::try_lock(&tokens_lock_path(&home.env, &fake()))
        .unwrap()
        .unwrap();
    let source = home.source();
    let outcome = source.refresh_tokens(&fake(), &mut |_| {}).unwrap();
    assert_eq!(outcome, RefreshOutcome::Busy);
    let state = source.token_index_state(&fake()).unwrap();
    assert_eq!(
        (state.refreshed_at_ms, state.responses),
        (None, 0),
        "it read nothing"
    );
}

#[test]
fn a_refresh_under_the_shutdown_token_stops_and_says_it_was_interrupted() {
    // Spec §9: a refresh cancelled by the shutdown token stops at its next chunk.
    let home = Home::new();
    home.transcript("s1.jsonl", &[FakeUsage::default()]);
    let shutdown = Cancel::new();
    shutdown.request(2);
    let e = home
        .source_under(&shutdown)
        .refresh_tokens(&fake(), &mut |_| {})
        .unwrap_err();
    assert_eq!(e.kind, "interrupted");
    let state = home.source().token_index_state(&fake()).unwrap();
    assert_eq!(state.refreshed_at_ms, None, "no refresh completed");
}

#[test]
fn a_day_drills_to_its_sessions_and_a_session_to_its_models_whose_keys_hold() {
    // Spec §5.3: `TokenQuery::drill`, and `RowKey`s stable across a re-query.
    let home = Home::new();
    home.transcript("s1.jsonl", &[FakeUsage::default()]);
    home.transcript("s2.jsonl", &[another("s2")]);
    let source = home.source();
    source.refresh_tokens(&fake(), &mut |_| {}).unwrap();
    let days = source.token_report(&by_day()).unwrap();
    assert_eq!(days.rows.len(), 1);
    let sessions = source
        .token_report(&days.query.drill(&days.rows[0]).unwrap())
        .unwrap();
    let ids: Vec<&str> = sessions
        .rows
        .iter()
        .map(|r| match &r.period {
            PeriodKey::Session { session_id, .. } => session_id.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, ["s2", "s1"], "newest first");
    let models = source
        .token_report(&sessions.query.drill(&sessions.rows[1]).unwrap())
        .unwrap();
    assert_eq!(models.rows.len(), 1);
    assert_eq!(models.rows[0].model.as_deref(), Some("fake-1"));
    assert_eq!(models.query.drill(&models.rows[0]), None, "the bottom");
    let again = source.token_report(&sessions.query).unwrap();
    let keys = |r: &tagteam_engine::tokens::TokenReport| {
        r.rows.iter().map(|row| row.key.clone()).collect::<Vec<_>>()
    };
    assert_eq!(keys(&again), keys(&sessions));
}
