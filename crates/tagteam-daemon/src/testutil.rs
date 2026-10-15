//! The fixture the crate's tests share: a home with Claude Code and FakeAgent registered, on a
//! fake wall clock and a scripted HTTP port that answers nothing, and a sleeper that moves that
//! clock (§15.1: no real network, HOME or keychain).

use std::cell::RefCell;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_cc::{ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::lifecycle::AddOptions;
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{Eligibility, Reserve};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FAKE_AGENT, FakeAgent};
use tagteam_provider::liveness::SystemProcessProbe;
use tagteam_provider::profile::RunShell;
use tagteam_provider::{Env, FakeClock, FakeKeychain, ScriptedHttp};

use crate::scheduler::Sleeper;

/// The fixture clock's start, epoch seconds.
pub(crate) const T0: i64 = 1_790_000_000;
/// An access-token expiry no test reaches, so nothing is freshened unless a test says so.
pub(crate) const FAR_MS: i64 = 4_102_444_800_000;

pub(crate) fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

pub(crate) fn fake() -> ProviderId {
    ProviderId::new(FAKE_AGENT)
}

/// A home with Claude Code and FakeAgent registered, on a fake wall clock that starts at
/// `T0` and a scripted HTTP port that answers nothing (every request is `PreSend`).
pub(crate) struct Fx {
    _dir: tempfile::TempDir,
    pub(crate) env: Env,
    pub(crate) kc: Arc<FakeKeychain>,
    pub(crate) clock: Arc<FakeClock>,
    pub(crate) http: Arc<ScriptedHttp>,
    pub(crate) engine: Engine,
}

impl Fx {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        let kc = Arc::new(FakeKeychain::new());
        let clock = Arc::new(FakeClock::new(T0 * 1000));
        let http = Arc::new(ScriptedHttp::new());
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new()
                .with(Arc::new(ClaudeCode::new(kc.clone(), Platform::MacOs)))
                .with(Arc::new(FakeAgent::new())),
            vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            oracle: Arc::new(NoOracle),
            clock: clock.clone(),
            http: http.clone(),
            default_provider: cc(),
            settings: Settings::default(),
            process: Arc::new(SystemProcessProbe),
            // Nothing here validates, and a scripted spawner never starts a process (§15.1).
            spawner: Arc::new(tagteam_provider::process::ScriptedSpawner::new()),
            run_shell: RunShell::Outside,
        });
        Fx {
            _dir: dir,
            env,
            kc,
            clock,
            http,
            engine,
        }
    }

    /// `claude /login` as `email`, then `tagteam add`: the login stays live.
    pub(crate) fn add(&self, email: &str) -> AccountId {
        let account = json!({"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")});
        fs::write(
            self.env.home.join(".claude.json"),
            json!({"oauthAccount": account}).to_string(),
        )
        .unwrap();
        let credential = json!({"claudeAiOauth": {"accessToken": format!("at-{email}"), "refreshToken": format!("rt-{email}"), "expiresAt": FAR_MS, "refreshTokenExpiresAt": FAR_MS}});
        self.kc.put(
            &keychain_service(&self.env, ItemKind::OAuth),
            &keychain_account(&self.env),
            credential.to_string().as_bytes(),
        );
        self.add_live(cc())
    }

    /// A FakeAgent login as `handle`, then `tagteam add`: the login stays live.
    pub(crate) fn add_fake(&self, handle: &str) -> AccountId {
        let (token, renew) = (format!("tok-{handle}"), format!("renew-{handle}"));
        tagteam_fake::login(&self.env, handle, "ws", &token, &renew);
        self.add_live(fake())
    }

    fn add_live(&self, provider: ProviderId) -> AccountId {
        let opts = AddOptions {
            provider,
            position: None,
            alias: None,
            yes: false,
        };
        self.engine.add_live(opts).unwrap().account.id
    }

    /// `id`'s reading of `short` and `long` percent in its provider's two windows, taken at
    /// `at` with its next poll planned at `next_poll_at`, recorded as a fetch records one.
    pub(crate) fn read(&self, id: &AccountId, short: f64, long: f64, at: i64, next_poll_at: i64) {
        let store = self.engine.store().unwrap();
        let row = store.account(id).unwrap().unwrap();
        let (s, l) = if row.provider == cc() {
            ("5h", "7d")
        } else {
            ("daily", "monthly")
        };
        let window = |key: &str, kind, pct, resets_at, period_s| Window {
            key: key.into(),
            label: key.into(),
            kind,
            pct,
            resets_at: Some(resets_at),
            period_s: Some(period_s),
            detail: None,
        };
        let windows = [
            window(s, WindowKind::Short, short, T0 + 9_630, 18_000),
            window(l, WindowKind::Long, long, T0 + 291_630, 604_800),
        ];
        let budget = &PollBudget::STANDARD;
        let Reserve::Reserved(r) = store
            .reserve_usage(&row, at * 1000, Eligibility::Scheduled, budget)
            .unwrap()
        else {
            panic!("no reservation at {at}");
        };
        let plan = PollPlan {
            interval_s: next_poll_at - at,
            next_poll_at,
        };
        assert!(store.record_usage(&r, &windows, at, &plan, 180).unwrap());
    }

    /// `config.toml` as `text`, with an mtime of its own whatever the file system's
    /// granularity.
    pub(crate) fn settings(&self, text: &str, mtime_s: u64) {
        let path = self.env.config_dir().join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        let at = std::time::UNIX_EPOCH + Duration::from_secs(mtime_s);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }
}

/// The scheduler's sleep on the fixture's wall clock: each slice moves the clock on by its
/// length, then `then` runs with the slice's number (from 0), to jump the clock, rewrite the
/// settings or send a signal.
pub(crate) struct Slices<'a> {
    clock: &'a FakeClock,
    pub(crate) taken: RefCell<Vec<Duration>>,
    then: Box<dyn Fn(usize) + 'a>,
}

impl<'a> Slices<'a> {
    pub(crate) fn new(clock: &'a FakeClock, then: impl Fn(usize) + 'a) -> Self {
        Slices {
            clock,
            taken: RefCell::new(Vec::new()),
            then: Box::new(then),
        }
    }

    /// A sleep that must never be taken.
    pub(crate) fn none(clock: &'a FakeClock) -> Self {
        Self::new(clock, |_| panic!("nothing may sleep here"))
    }
}

impl Sleeper for Slices<'_> {
    fn sleep(&self, d: Duration) {
        let n = self.taken.borrow().len();
        self.taken.borrow_mut().push(d);
        self.clock.advance_ms(d.as_millis() as i64);
        (self.then)(n);
    }
}
