//! The fixture the crate's tests share: a home with Claude Code and FakeAgent registered, on a
//! fake wall clock and a scripted HTTP port that answers nothing, and a sleeper that moves that
//! clock (§15.1: no real network, HOME or keychain).

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::json;
use tagteam_cc::CLAUDE_CODE;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::lifecycle::AddOptions;
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::{NotifyEvent, Settings};
use tagteam_engine::store::{Eligibility, Reserve};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FAKE_AGENT, FakeAgent};
use tagteam_provider::liveness::SystemProcessProbe;
use tagteam_provider::process::ScriptedSpawner;
use tagteam_provider::profile::RunShell;
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{Clock, Env, FakeClock, FakeKeychain, ScriptedHttp};

use crate::events::{DaemonEvent, DaemonSink, Mode, WaitingFor};
use crate::notify::Notice;
use crate::provider_job::ProviderJobCtx;
use crate::scheduler::Sleeper;
use crate::status::{DaemonInfo, NotifyInfo, ProviderStatus, StatusBoard};

/// The fixture clock's start, epoch seconds.
pub(crate) const T0: i64 = 1_790_000_000;
/// An access-token expiry no test reaches, so nothing is freshened unless a test says so.
pub(crate) const FAR_MS: i64 = 4_102_444_800_000;
/// Claude Code's usage endpoint (§8.1), as `ScriptedHttp` scripts it.
pub(crate) const CC_USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
/// When every fixture reading's 5h window resets.
pub(crate) const SHORT_RESETS: i64 = T0 + 9_630;
/// When every fixture reading's 7d window resets: 3d09h after `T0`.
pub(crate) const LONG_RESETS: i64 = T0 + 291_630;

pub(crate) fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

pub(crate) fn fake() -> ProviderId {
    ProviderId::new(FAKE_AGENT)
}

/// Claude Code's two windows as a fetch reports them: 5h at `short` and 7d at `long` percent,
/// resetting at `SHORT_RESETS` and `LONG_RESETS`.
pub(crate) fn windows(short: f64, long: f64) -> Vec<Window> {
    let window = |key: &str, kind, pct, resets_at, period_s| Window {
        key: key.into(),
        label: key.into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: Some(period_s),
        detail: None,
    };
    vec![
        window("5h", WindowKind::Short, short, SHORT_RESETS, 18_000),
        window("7d", WindowKind::Long, long, LONG_RESETS, 604_800),
    ]
}

/// An engine on `env` with Claude Code and FakeAgent registered, spawning through `spawner`
/// (which never starts a process, §15.1), inside or outside a run shell. Claude Code's live
/// Keychain reads are retried without a wait (Appendix A.3), so a locked keychain costs a test
/// no time.
fn engine_on(
    env: &Env,
    kc: &Arc<FakeKeychain>,
    clock: &Arc<FakeClock>,
    http: &Arc<ScriptedHttp>,
    spawner: &Arc<ScriptedSpawner>,
    run_shell: RunShell,
) -> Engine {
    let claude = ClaudeCode::with_store(
        LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    );
    Engine::new(EngineConfig {
        env: env.clone(),
        registry: ProviderRegistry::new()
            .with(Arc::new(claude))
            .with(Arc::new(FakeAgent::new())),
        vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
        oracle: Arc::new(NoOracle),
        clock: clock.clone(),
        http: http.clone(),
        default_provider: cc(),
        settings: Settings::default(),
        process: Arc::new(SystemProcessProbe),
        spawner: spawner.clone(),
        run_shell,
    })
}

/// A home with Claude Code and FakeAgent registered, on a fake wall clock that starts at
/// `T0` and a scripted HTTP port that answers nothing (every request is `PreSend`).
pub(crate) struct Fx {
    _dir: tempfile::TempDir,
    pub(crate) env: Env,
    pub(crate) kc: Arc<FakeKeychain>,
    pub(crate) clock: Arc<FakeClock>,
    pub(crate) http: Arc<ScriptedHttp>,
    /// Every engine of this home spawns through it: the notifier's deliveries among them.
    pub(crate) spawner: Arc<ScriptedSpawner>,
    pub(crate) engine: Engine,
}

impl Fx {
    pub(crate) fn new() -> Self {
        Self::with_run_shell(RunShell::Outside)
    }

    /// `new`, with the engine inside or outside a run shell (§12.8).
    pub(crate) fn with_run_shell(run_shell: RunShell) -> Self {
        Self::under(None, run_shell)
    }

    /// `new`, with the home and everything beside it under a directory called `marker`, so a
    /// test can tell whether a path under the home reached a log line.
    pub(crate) fn marked(marker: &str) -> Self {
        Self::under(Some(marker), RunShell::Outside)
    }

    fn under(marker: Option<&str>, run_shell: RunShell) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = marker.map_or_else(|| dir.path().to_owned(), |m| dir.path().join(m));
        let env = Env::for_test(&root);
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        let kc = Arc::new(FakeKeychain::new());
        let clock = Arc::new(FakeClock::new(T0 * 1000));
        let http = Arc::new(ScriptedHttp::new());
        let spawner = Arc::new(ScriptedSpawner::new());
        let engine = engine_on(&env, &kc, &clock, &http, &spawner, run_shell);
        Fx {
            _dir: dir,
            env,
            kc,
            clock,
            http,
            spawner,
            engine,
        }
    }

    /// `claude /login` as `email`: the live login's `oauthAccount` and its Keychain credential.
    pub(crate) fn login(&self, email: &str) {
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
    }

    /// `claude /login` as `email`, then `tagteam add`: the login stays live.
    pub(crate) fn add(&self, email: &str) -> AccountId {
        self.login(email);
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

/// `b` at position 2 is the live login, `a` at position 1 a candidate.
pub(crate) fn two(fx: &Fx) -> (AccountId, AccountId) {
    (fx.add("a@x.co"), fx.add("b@x.co"))
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

/// Every daemon event, with the fixture clock's time it was emitted at, in seconds after `T0`,
/// and every notice announced to it (`DaemonSink::notified`), in order.
pub(crate) struct Emitted<'a> {
    clock: &'a FakeClock,
    events: RefCell<Vec<(i64, DaemonEvent)>>,
    announced: RefCell<Vec<(ProviderId, Notice)>>,
}

impl<'a> Emitted<'a> {
    pub(crate) fn new(clock: &'a FakeClock) -> Self {
        Emitted {
            clock,
            events: RefCell::new(Vec::new()),
            announced: RefCell::new(Vec::new()),
        }
    }

    /// Each notice announced to this sink, with its provider.
    pub(crate) fn announced(&self) -> Vec<(ProviderId, Notice)> {
        self.announced.borrow().clone()
    }

    pub(crate) fn events(&self) -> Vec<(i64, DaemonEvent)> {
        self.events.borrow().clone()
    }

    /// Each `mode` event of `of`: (seconds after `T0`, mode, waiting for, holder pid).
    pub(crate) fn modes(
        &self,
        of: &ProviderId,
    ) -> Vec<(i64, Mode, Option<WaitingFor>, Option<u32>)> {
        self.events
            .borrow()
            .iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Mode {
                    provider,
                    mode,
                    waiting_for,
                    holder_pid,
                    ..
                } if provider == of => Some((*at, *mode, *waiting_for, *holder_pid)),
                _ => None,
            })
            .collect()
    }

    /// When `of` polled: one per tick that reached §11.2 step 3.
    pub(crate) fn polls(&self, of: &ProviderId) -> Vec<i64> {
        self.events
            .borrow()
            .iter()
            .filter_map(|(at, e)| match e {
                DaemonEvent::Auto(tagteam_engine::auto::AutoEvent::Poll { provider, .. })
                    if provider == of =>
                {
                    Some(*at)
                }
                _ => None,
            })
            .collect()
    }
}

impl DaemonSink for Emitted<'_> {
    fn emit(&self, e: &DaemonEvent) {
        let at = self.clock.now_ms().div_euclid(1000) - T0;
        self.events.borrow_mut().push((at, e.clone()));
    }

    fn notified(&self, provider: &ProviderId, notice: &Notice) {
        self.announced
            .borrow_mut()
            .push((provider.clone(), notice.clone()));
    }
}

/// A board for a daemon started at `T0`, with every notification on.
pub(crate) fn board() -> Arc<StatusBoard> {
    StatusBoard::new(
        DaemonInfo {
            pid: 4242,
            version: "20261014.2045.0".into(),
            started_at: T0,
            socket: None,
            log: PathBuf::from("/home/u/.local/state/tagteam/tagteam.log"),
        },
        NotifyInfo {
            enabled: true,
            backend: None,
            events: vec![
                NotifyEvent::Switch,
                NotifyEvent::Quarantine,
                NotifyEvent::Limit,
            ],
        },
    )
}

/// A provider job's context as Linux gives it: no Keychain counts, and a store this build
/// reads. Every test builds its context here, so a field added later is added once.
pub(crate) fn job_ctx(board: &Arc<StatusBoard>) -> ProviderJobCtx {
    ProviderJobCtx {
        board: board.clone(),
        notices: None,
        keychain_counts: Vec::new(),
        schema_newer: false,
    }
}

/// `of`'s line of the board.
pub(crate) fn status(board: &StatusBoard, of: &ProviderId) -> ProviderStatus {
    board
        .snapshot()
        .providers
        .into_iter()
        .find(|p| &p.provider == of)
        .expect("the provider job has run")
}

impl Fx {
    /// Another process on the same home: an engine of its own, with its own store connection,
    /// over the same Keychain, clock and spawner. It is also what a restarted daemon starts
    /// with: no store open yet.
    pub(crate) fn other(&self) -> Engine {
        engine_on(
            &self.env,
            &self.kc,
            &self.clock,
            &self.http,
            &self.spawner,
            RunShell::Outside,
        )
    }

    /// `claude /login` as `email`, then `tagteam add` through `engine`.
    pub(crate) fn add_on(&self, engine: &Engine, email: &str) -> AccountId {
        self.login(email);
        let opts = AddOptions {
            provider: cc(),
            position: None,
            alias: None,
            yes: false,
        };
        engine.add_live(opts).unwrap().account.id
    }

    /// The store file and its sidecars removed, as a person deleting the store by hand would.
    pub(crate) fn remove_store(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.engine.store_file().into_os_string();
            name.push(suffix);
            match fs::remove_file(&name) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => panic!("{}: {e}", std::path::Path::new(&name).display()),
            }
        }
    }
}

impl Fx {
    /// `claude /login` as `email` in the organization named `org`: `login`, with an
    /// `oauthAccount` that also names the organization, spliced into `~/.claude.json` so the
    /// file's other keys stay. A notice must never carry an organization's name (D6.2), so the
    /// tests that look for one log in this way.
    pub(crate) fn login_as(&self, email: &str, org: &str) {
        let path = self.env.home.join(".claude.json");
        let before = fs::read(&path).unwrap_or_else(|_| b"{}\n".to_vec());
        self.login(email);
        let account = json!({"emailAddress": email, "organizationUuid": format!("org-{org}"),
                             "organizationName": org, "accountUuid": format!("uuid-{email}")});
        fs::write(
            &path,
            replace_top_level(&before, "oauthAccount", &account).unwrap(),
        )
        .unwrap();
    }

    /// `login_as`, then `tagteam add` with `alias`: the login stays live.
    pub(crate) fn add_as(&self, email: &str, org: &str, alias: Option<&str>) -> AccountId {
        self.login_as(email, org);
        let opts = AddOptions {
            provider: cc(),
            position: None,
            alias: alias.map(str::to_owned),
            yes: false,
        };
        self.engine.add_live(opts).unwrap().account.id
    }

    /// `windows` as `id`'s reading taken at `at`, its next poll planned at `next_poll_at`,
    /// recorded as a fetch records one (§8.3): reserve, then record.
    pub(crate) fn read_windows(
        &self,
        id: &AccountId,
        windows: &[Window],
        at: i64,
        next_poll_at: i64,
    ) {
        let store = self.engine.store().unwrap();
        let row = store.account(id).unwrap().unwrap();
        let Reserve::Reserved(r) = store
            .reserve_usage(
                &row,
                at * 1000,
                Eligibility::Scheduled,
                &PollBudget::STANDARD,
            )
            .unwrap()
        else {
            panic!("no reservation at {at}");
        };
        let plan = PollPlan {
            interval_s: next_poll_at - at,
            next_poll_at,
        };
        assert!(store.record_usage(&r, windows, at, &plan, 180).unwrap());
    }
}

/// A `tracing` writer that appends to a shared buffer.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> LogBuffer {
        self.clone()
    }
}

/// A dispatcher that wants nothing, kept alive for the whole test process. `tracing` caches
/// each call site's interest from every registered dispatcher. With one alone, a thread with
/// no capture that reaches a call site first would cache it as unwanted for every capture.
/// The engine's test `capture_logs` explains this in full.
static ALWAYS_TWO: OnceLock<tracing::Dispatch> = OnceLock::new();

/// Runs `f` with every `tracing` event this thread emits captured. The notifier's thread is
/// captured too, since it takes the dispatcher it starts under. Returns `f`'s result and the
/// lines: level first, then the message and its fields, with no colour, time or target.
pub(crate) fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    ALWAYS_TWO.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let result = tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    (result, text.lines().map(str::to_owned).collect())
}

/// Runs `f` on a thread of its own and waits 10 s for it: a timeout panics, and a panic in `f`
/// is resumed here. A thread that never wakes fails the test instead of stalling the run.
pub(crate) fn within<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(value) => {
            let _ = handle.join();
            value
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match handle.join() {
            Err(payload) => std::panic::resume_unwind(payload),
            Ok(()) => panic!("{what}: the thread ended without a result"),
        },
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("not within 10 s: {what}"),
    }
}
