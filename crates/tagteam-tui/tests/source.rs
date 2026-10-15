//! `EngineSource` over a real engine (spec §4.4, §11 "Source over a real engine"): a fixture
//! home with Claude Code and FakeAgent registered, a fake Keychain, a scripted HTTP port and a
//! fake clock. Nothing reaches the real HOME, the network or the Keychain (core §15.1).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{Eligibility, NewAccount, Reserve, Store};
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FAKE_AGENT, FakeAgent};
use tagteam_provider::http::Method;
use tagteam_provider::liveness::FakeProcessProbe;
use tagteam_provider::process::ScriptedSpawner;
use tagteam_provider::{
    Env, FakeClock, FakeKeychain, Identity, ProfileMarker, RunShell, ScriptedHttp,
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
        Engine::new(EngineConfig {
            env: self.env.clone(),
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
        })
    }

    fn source(&self) -> EngineSource {
        EngineSource::new(self.engine_in(RunShell::Outside))
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

    /// A usable stored credential for `id`, and one usage reply queued for it.
    fn fetchable(&self, id: &AccountId) {
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
            snap.unmanaged_live,
            snap.in_run_shell,
            snap.read_at_s
        ),
        (None, None, false, NOW_S)
    );
}

#[test]
fn a_live_login_tagteam_does_not_manage_is_named() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    home.login("s@x.co");
    let snap = home.source().accounts(None).unwrap();
    assert_eq!(snap.unmanaged_live.as_deref(), Some("s@x.co"));
    assert_eq!(snap.providers[0].active_position, None);
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
fn history_is_the_samples_since_the_range_starts() {
    let home = Home::new();
    let a = home.add(CLAUDE_CODE, 1, "a@x.co");
    home.reading(&a, 3 * 86_400, 40.0);
    home.reading(&a, 3_600, 60.0);
    let view = home.source().history(&a, NOW_S - 86_400).unwrap();
    assert_eq!(view.account.row.id, a);
    assert_eq!(view.windows.len(), 1);
    let pcts: Vec<f64> = view.windows[0].samples.iter().map(|s| s.pct).collect();
    assert_eq!(pcts, [60.0], "only the day's sample");
    let week = home.source().history(&a, NOW_S - 7 * 86_400).unwrap();
    assert_eq!(week.windows[0].samples.len(), 2);
}

#[test]
fn a_failed_read_carries_the_engines_kind_and_words() {
    let home = Home::new();
    home.add(CLAUDE_CODE, 1, "a@x.co");
    let e = home
        .source()
        .history(&AccountId::from_string("gone"), 0)
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
    let snap = source.accounts(None).unwrap();
    let a = &snap.providers[0].accounts[0];
    assert_eq!(a.usage.fetched_at, Some(NOW_S));
    // Nothing is due a moment later: a reading is fresh, a failure backs off.
    let again = source.collect_due(&[read, missing, fresh]).unwrap();
    assert_eq!(again.fetched, 0);
}
