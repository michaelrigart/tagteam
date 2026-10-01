//! `tagteam statusline` (§13.5): one line for Claude Code's status bar, from the stored
//! reading. It never fetches, never touches the Keychain, and never builds the HTTP adapter.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use tagteam_cc::{CcPaths, ClaudeCode};
use tagteam_core::{CLAUDE_CODE, ProviderId, Window, WindowKind};
use tagteam_engine::lazy_http::LazyHttp;
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::{STATUSLINE_MODEL_PREFIX, Settings, is_statusline_placeholder};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::views::{AccountView, StatuslineView};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_provider::{
    Capabilities, Env, Http, Keychain, KeychainError, LockState, NoHttp, Read, ReadError,
    SystemClock,
};

use crate::app::Context;
use crate::render::{self, MISSING, RESET};

/// §13.5: at most this much piped stdin is read, and none of it is used.
const STDIN_CAP: u64 = 64 * 1024;
/// A reading older than this says how old it is (`{stale}`).
const STALE_AFTER_S: i64 = 15 * 60;
const REFUSED: &str = "the statusline never uses the Keychain";
/// What Claude Code's `statusLine` setting runs.
const COMMAND: &str = "tagteam statusline";

/// Reads and discards up to `STDIN_CAP` bytes, so Claude Code's write of its session JSON never
/// blocks. A terminal is never read: someone running the command by hand would otherwise have
/// to press Ctrl-D. Returns how much was read.
pub(crate) fn drain(input: impl std::io::Read, terminal: bool) -> u64 {
    if terminal {
        return 0;
    }
    std::io::copy(&mut input.take(STDIN_CAP), &mut std::io::sink()).unwrap_or(0)
}

/// §4.5: the command refuses for a provider without the `statusline` capability.
pub(crate) fn unsupported(caps: Capabilities, provider_name: &str) -> Option<String> {
    (!caps.statusline).then(|| format!("{provider_name} has no statusline"))
}

/// §13.5: the managed live login's line in `format`, an unmanaged login's email alone, or
/// nothing without a live login. `now_s` dates the countdowns and `{stale}`.
pub(crate) fn line(view: &StatuslineView, format: &str, now_s: i64, colour: bool) -> String {
    match view {
        StatuslineView::NoLogin => String::new(),
        StatuslineView::Unmanaged { email } => format!("{email}\n"),
        StatuslineView::Managed { account } => {
            format!("{}\n", expand(format, account, now_s, colour))
        }
    }
}

/// Fills each known `{placeholder}`. An unknown or unclosed one stays as written.
fn expand(format: &str, account: &AccountView, now_s: i64, colour: bool) -> String {
    let windows: Vec<&Window> = account
        .usage
        .windows
        .iter()
        .flatten()
        .map(|(w, _)| w)
        .collect();
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|c| open + c) else {
            break;
        };
        out.push_str(&rest[..open]);
        match placeholder(&rest[open + 1..close], account, &windows, now_s, colour) {
            Some(text) => out.push_str(&text),
            None => out.push_str(&rest[open..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// One placeholder's text, or `None` for a name §13.5 does not define: the engine's list
/// (`is_statusline_placeholder`) decides, the same one `statusline.format` is validated with.
/// The window placeholders go by kind, so they mean the same for every provider.
fn placeholder(
    name: &str,
    account: &AccountView,
    windows: &[&Window],
    now_s: i64,
    colour: bool,
) -> Option<String> {
    if !is_statusline_placeholder(name) {
        return None;
    }
    let row = &account.row;
    let of_kind = |kind: WindowKind| windows.iter().copied().find(|w| w.kind == kind);
    let text = match name {
        "account" => row
            .alias
            .clone()
            .unwrap_or_else(|| local_part(&render::email(row)).to_owned()),
        "position" => row.position.to_string(),
        "email" => render::email(row),
        "5h" => pct(of_kind(WindowKind::Short), colour),
        "7d" => pct(of_kind(WindowKind::Long), colour),
        "5h_reset" => reset(of_kind(WindowKind::Short), now_s),
        "7d_reset" => reset(of_kind(WindowKind::Long), now_s),
        "spend" => spend(of_kind(WindowKind::Spend), colour),
        "stale" => stale(account.usage.fetched_at, now_s),
        _ => {
            let model = name.strip_prefix(STATUSLINE_MODEL_PREFIX)?;
            let scoped = windows
                .iter()
                .copied()
                .find(|w| w.kind == WindowKind::Scoped && w.label.eq_ignore_ascii_case(model));
            pct(scoped, colour)
        }
    };
    Some(text)
}

/// The part of an email before `@`; a label without one is itself.
fn local_part(email: &str) -> &str {
    email.split_once('@').map_or(email, |(local, _)| local)
}

/// A window's percentage as shown, whole, coloured by the severity of what is shown.
fn pct(w: Option<&Window>, colour: bool) -> String {
    w.map_or_else(
        || MISSING.to_owned(),
        |w| {
            let shown = w.pct.round();
            paint(&format!("{shown:.0}"), shown, colour)
        },
    )
}

fn paint(text: &str, shown_pct: f64, colour: bool) -> String {
    match render::severity(shown_pct as i64) {
        Some(code) if colour => format!("{code}{text}{RESET}"),
        _ => text.to_owned(),
    }
}

/// A window's time to reset, as `list` shows it.
fn reset(w: Option<&Window>, now_s: i64) -> String {
    w.and_then(|w| w.resets_at)
        .map_or_else(|| MISSING.to_owned(), |at| render::countdown(at, now_s))
}

/// The spend window as money (`€3.50 of €20`) when the provider's detail says how much, else
/// as a percentage.
fn spend(w: Option<&Window>, colour: bool) -> String {
    let Some(w) = w else {
        return MISSING.to_owned();
    };
    let shown = w.pct.round();
    let text = render::spend_text(w).unwrap_or_else(|| format!("{shown:.0}%"));
    paint(&text, shown, colour)
}

fn stale(fetched_at: Option<i64>, now_s: i64) -> String {
    match fetched_at {
        Some(at) if now_s - at > STALE_AFTER_S => {
            format!(" · {} old", render::duration(now_s - at))
        }
        _ => String::new(),
    }
}

/// `--print-config`'s snippet for Claude Code's `settings.json`. tagteam never edits that file
/// (§3); the user pastes this into it.
pub(crate) fn config_snippet() -> String {
    let v = json!({"statusLine": {"type": "command", "command": COMMAND}});
    format!(
        "{}\n",
        serde_json::to_string_pretty(&v).expect("a JSON value always serializes")
    )
}

/// Where the snippet goes. It is printed on stderr, so stdout stays the snippet alone.
pub(crate) fn config_hint(env: &Env) -> String {
    let path = CcPaths::resolve(env).config_home.join("settings.json");
    format!(
        "Add this to {}; tagteam never edits Claude Code's settings.",
        path.display()
    )
}

/// The engine the fast path runs on, built with walls rather than trust (§13.5): a Keychain that
/// refuses every call, no profile oracle, and a lazy HTTP port whose adapter could send nothing
/// even if it were built. The settings' warnings are dropped, since a status bar has nowhere to
/// show them. The walls are returned so a test can prove nothing reached them.
pub(crate) fn engine(
    ctx: Context,
    provider: &ProviderId,
) -> (Engine, Arc<LazyHttp>, Arc<NoKeychain>) {
    let keychain = Arc::new(NoKeychain::default());
    let http = Arc::new(LazyHttp::new(|| Arc::new(NoHttp) as Arc<dyn Http>));
    let (settings, _warnings) = Settings::load(&ctx.env, provider);
    let engine = Engine::new(EngineConfig {
        registry: ProviderRegistry::new()
            .with(Arc::new(ClaudeCode::new(keychain.clone(), ctx.platform))),
        vault: Vault::new(Box::new(KeychainVault::new(keychain.clone()))),
        oracle: Arc::new(NoOracle),
        clock: Arc::new(SystemClock),
        http: http.clone(),
        default_provider: ProviderId::new(CLAUDE_CODE),
        settings,
        env: ctx.env,
    });
    (engine, http, keychain)
}

/// The statusline engine's Keychain: every call is refused, and counted.
#[derive(Default)]
pub(crate) struct NoKeychain {
    calls: AtomicUsize,
}

impl NoKeychain {
    fn refuse(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Keychain for NoKeychain {
    fn find(&self, _service: &str, _account: &str) -> Read<Vec<u8>> {
        self.refuse();
        Read::Unreadable(ReadError::new("keychain", REFUSED))
    }

    fn exists(&self, _service: &str, _account: &str) -> Read<()> {
        self.refuse();
        Read::Unreadable(ReadError::new("keychain", REFUSED))
    }

    fn upsert(&self, _service: &str, _account: &str, _data: &[u8]) -> Result<(), KeychainError> {
        self.refuse();
        Err(KeychainError {
            rc: None,
            detail: REFUSED.into(),
        })
    }

    fn delete(&self, _service: &str, _account: &str) -> Result<(), KeychainError> {
        self.refuse();
        Err(KeychainError {
            rc: None,
            detail: REFUSED.into(),
        })
    }

    fn lock_state(&self) -> LockState {
        self.refuse();
        LockState::Unknown
    }

    fn unlock(&self) -> bool {
        self.refuse();
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use clap::Parser;
    use tagteam_cc::live::Platform;
    use tagteam_cc::{ItemKind, keychain_account, keychain_service};
    use tagteam_core::{PollBudget, PollPlan};
    use tagteam_engine::settings::DEFAULT_STATUSLINE_FORMAT;
    use tagteam_engine::store::{Reserve, Store};
    use tagteam_engine::views::{UsageStatus, UsageView};
    use tagteam_provider::FakeKeychain;

    use super::*;
    use crate::app::{Io, run};
    use crate::cli::Cli;
    use crate::prompt::Prompter;
    use crate::render::testutil::{NOW, OAUTH, fable, read, spend, unread, view};

    /// 5h at 9 % resetting in 2h40m, 7d at 77 % in 3d09h, €3.50 of €20 spent (17.5 %), and
    /// Fable at 0 %, read `age_s` ago.
    fn reading(age_s: i64) -> UsageView {
        read(age_s, 9.0, 77.0, false, vec![spend(3.5, 20.0), fable(0.0)])
    }

    /// `b@x.co` at position 2 is the live login, with `usage`.
    fn managed(alias: Option<&str>, usage: UsageView) -> StatuslineView {
        let mut account = view(2, "b@x.co", OAUTH, usage);
        account.row.alias = alias.map(str::to_owned);
        account.active = true;
        StatuslineView::Managed { account }
    }

    fn plain(view: &StatuslineView, format: &str) -> String {
        line(view, format, NOW, false)
    }

    #[test]
    fn the_default_format_names_the_account_and_its_two_main_windows() {
        let v = managed(None, reading(0));
        assert_eq!(plain(&v, DEFAULT_STATUSLINE_FORMAT), "b · 5h 9% · 7d 77%\n");
        let v = managed(Some("work"), reading(0));
        assert_eq!(
            plain(&v, DEFAULT_STATUSLINE_FORMAT),
            "work · 5h 9% · 7d 77%\n"
        );
    }

    #[test]
    fn every_placeholder_is_filled_from_the_row_and_the_reading() {
        let v = managed(None, reading(0));
        assert_eq!(
            plain(
                &v,
                "{position}|{email}|{5h_reset}|{7d_reset}|{spend}|{model:FABLE}"
            ),
            "2|b@x.co|2h40m|3d09h|€3.50 of €20|0\n"
        );
    }

    #[test]
    fn a_window_the_reading_lacks_shows_a_dash() {
        let v = managed(None, unread(UsageStatus::NoCredentials, None, None));
        assert_eq!(
            plain(
                &v,
                "{5h}|{7d}|{5h_reset}|{7d_reset}|{spend}|{model:Fable}|{stale}"
            ),
            "—|—|—|—|—|—|\n"
        );
    }

    #[test]
    fn an_unknown_or_unclosed_placeholder_stays_as_written() {
        let v = managed(None, reading(0));
        assert_eq!(
            plain(&v, "{nope} {model} {} {5h"),
            "{nope} {model} {} {5h\n"
        );
    }

    #[test]
    fn the_renderer_fills_exactly_the_engines_placeholders() {
        use tagteam_engine::settings::STATUSLINE_PLACEHOLDERS;
        let v = managed(None, reading(0));
        for name in STATUSLINE_PLACEHOLDERS {
            assert_ne!(
                plain(&v, &format!("{{{name}}}")),
                format!("{{{name}}}\n"),
                "{name} is on the engine's list but not filled"
            );
        }
        // A model name that settings would refuse is never matched: it stays as written.
        assert_eq!(
            plain(&v, "{model:} {model: Fable} {model:Fable } {model:{5h}"),
            "{model:} {model: Fable} {model:Fable } {model:{5h}\n"
        );
    }

    #[test]
    fn stale_reports_a_reading_older_than_fifteen_minutes() {
        let at = |age: i64| plain(&managed(None, reading(age)), "{stale}");
        assert_eq!(at(15 * 60), "\n");
        assert_eq!(at(15 * 60 + 1), " · 15m old\n");
        assert_eq!(at(3 * 3_600 + 5 * 60), " · 3h05m old\n");
    }

    #[test]
    fn percentages_are_coloured_by_severity_as_shown() {
        let mut usage = reading(0);
        {
            let ws = usage.windows.as_mut().unwrap();
            ws[0].0.pct = 95.0;
            ws[1].0.pct = 70.0;
            ws[3].0.pct = 89.6;
        }
        let v = managed(None, usage.clone());
        let format = "{5h} {7d} {model:Fable} {spend}";
        assert_eq!(
            line(&v, format, NOW, true),
            "\u{1b}[31m95\u{1b}[0m \u{1b}[33m70\u{1b}[0m \u{1b}[31m90\u{1b}[0m €3.50 of €20\n"
        );
        assert_eq!(line(&v, format, NOW, false), "95 70 90 €3.50 of €20\n");
        usage.windows.as_mut().unwrap()[1].0.pct = 69.4;
        let v = managed(None, usage);
        assert_eq!(line(&v, "{7d}", NOW, true), "69\n");
    }

    #[test]
    fn spend_is_a_percentage_when_the_provider_does_not_say_how_much() {
        let mut usage = reading(0);
        usage.windows.as_mut().unwrap()[2].0.detail = None;
        assert_eq!(plain(&managed(None, usage), "{spend}"), "18%\n");
    }

    #[test]
    fn a_reset_that_has_passed_says_so() {
        let v = managed(None, reading(0));
        assert_eq!(line(&v, "{5h_reset}", NOW + 10_000, false), "reset\n");
    }

    #[test]
    fn an_unmanaged_login_is_its_email_and_no_login_is_nothing() {
        let unmanaged = StatuslineView::Unmanaged {
            email: "c@x.co".into(),
        };
        assert_eq!(
            line(&unmanaged, DEFAULT_STATUSLINE_FORMAT, NOW, true),
            "c@x.co\n"
        );
        assert_eq!(
            line(
                &StatuslineView::NoLogin,
                DEFAULT_STATUSLINE_FORMAT,
                NOW,
                true
            ),
            ""
        );
    }

    /// A reader that fails the test if it is ever read.
    struct Untouchable;

    impl std::io::Read for Untouchable {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            panic!("a terminal is never read");
        }
    }

    #[test]
    fn stdin_is_drained_up_to_64_kib_and_a_terminal_is_never_read() {
        assert_eq!(
            drain(std::io::Cursor::new(vec![b'x'; 100 * 1024]), false),
            64 * 1024
        );
        assert_eq!(drain(std::io::Cursor::new(b"{}".to_vec()), false), 2);
        assert_eq!(drain(Untouchable, true), 0);
    }

    #[test]
    fn print_config_is_claude_codes_settings_snippet() {
        assert_eq!(
            config_snippet(),
            "{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"tagteam statusline\"\n  }\n}\n"
        );
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            config_hint(&Env::for_test(dir.path())),
            format!(
                "Add this to {}; tagteam never edits Claude Code's settings.",
                dir.path().join("home/.claude/settings.json").display()
            )
        );
    }

    #[test]
    fn a_provider_without_the_capability_is_refused() {
        assert_eq!(
            unsupported(Capabilities::default(), "Fake Agent").as_deref(),
            Some("Fake Agent has no statusline")
        );
        let capable = Capabilities {
            statusline: true,
            ..Capabilities::default()
        };
        assert_eq!(unsupported(capable, "Claude Code"), None);
    }

    #[test]
    fn the_no_keychain_refuses_and_counts_every_call() {
        let k = NoKeychain::default();
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
        assert!(matches!(k.exists("s", "a"), Read::Unreadable(_)));
        assert!(k.upsert("s", "a", b"x").is_err());
        assert!(k.delete("s", "a").is_err());
        assert_eq!(k.lock_state(), LockState::Unknown);
        assert!(!k.unlock());
        assert_eq!(k.calls(), 6);
    }

    /// Answers no prompt: the `add` below asks none.
    struct NoPrompts;

    impl Prompter for NoPrompts {
        fn interactive(&self) -> bool {
            false
        }
        fn confirm(&mut self, question: &str, _default_yes: bool) -> bool {
            panic!("unexpected prompt: {question}");
        }
        fn choose(&mut self, question: &str, _options: &[String]) -> Option<usize> {
            panic!("unexpected prompt: {question}");
        }
        fn secret(&mut self, question: &str) -> Option<String> {
            panic!("unexpected prompt: {question}");
        }
    }

    /// `a@x.co` logged in and added through `run`, with one reading recorded now: 5h at 9 %,
    /// 7d at 77 %.
    fn managed_home() -> (tempfile::TempDir, Env) {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        std::fs::create_dir_all(env.home.join(".claude")).unwrap();
        let config = json!({"oauthAccount": {"emailAddress": "a@x.co", "organizationUuid": "", "accountUuid": "uuid-a"}});
        std::fs::write(env.home.join(".claude.json"), config.to_string()).unwrap();
        let kc = Arc::new(FakeKeychain::new());
        let credential = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": "rt-a", "refreshTokenExpiresAt": 1_797_000_000_000i64}});
        kc.put(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
            credential.to_string().as_bytes(),
        );
        let ctx = Context {
            env: env.clone(),
            keychain: kc,
            platform: Platform::MacOs,
            api_base: Some("http://127.0.0.1:9".into()),
            stdout_terminal: false,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            Cli::try_parse_from(["tagteam", "add"]).unwrap(),
            ctx,
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut NoPrompts,
            },
        );
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
        let store = Store::open_existing(&env.data_dir().join("tagteam.db"))
            .unwrap()
            .unwrap();
        let row = store
            .accounts(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
            .remove(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let Reserve::Reserved(reservation) = store
            .reserve_usage(&row, now * 1000, true, &PollBudget::STANDARD)
            .unwrap()
        else {
            panic!("no reservation for the reading");
        };
        let reading = [
            Window {
                key: "5h".into(),
                label: "5h".into(),
                kind: WindowKind::Short,
                pct: 9.0,
                resets_at: None,
                period_s: None,
                detail: None,
            },
            Window {
                key: "7d".into(),
                label: "7d".into(),
                kind: WindowKind::Long,
                pct: 77.0,
                resets_at: None,
                period_s: None,
                detail: None,
            },
        ];
        let plan = PollPlan {
            interval_s: 300,
            next_poll_at: now + 300,
        };
        assert!(
            store
                .record_usage(&reservation, &reading, now, &plan, 180)
                .unwrap()
        );
        (dir, env)
    }

    #[test]
    fn the_settings_are_those_of_the_provider_the_command_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        std::fs::create_dir_all(env.config_dir()).unwrap();
        std::fs::write(
            env.config_dir().join("config.toml"),
            "[statusline]\nformat = \"{5h}\"\n\n[provider.other.statusline]\nformat = \"{7d}\"\n",
        )
        .unwrap();
        let format_for = |provider: &str| {
            let ctx = Context {
                env: env.clone(),
                keychain: Arc::new(FakeKeychain::new()),
                platform: Platform::MacOs,
                api_base: None,
                stdout_terminal: false,
            };
            engine(ctx, &ProviderId::new(provider))
                .0
                .settings()
                .statusline_format
                .clone()
        };
        assert_eq!(format_for(CLAUDE_CODE), "{5h}");
        assert_eq!(format_for("other"), "{7d}");
    }

    #[test]
    fn the_engine_reaches_neither_the_keychain_nor_the_network() {
        // §13.5: the walls count what reaches them, so none of this may.
        let (_dir, env) = managed_home();
        let provider = ProviderId::new(CLAUDE_CODE);
        let ctx = Context {
            env,
            keychain: Arc::new(FakeKeychain::new()),
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
        };
        let (built, http, keychain) = engine(ctx, &provider);
        let view = built.statusline(&provider).unwrap();
        let text = line(
            &view,
            &built.settings().statusline_format,
            built.now_ms() / 1000,
            false,
        );
        assert_eq!(text, "a · 5h 9% · 7d 77%\n");
        assert!(!http.is_built(), "the HTTP adapter was built");
        assert_eq!(keychain.calls(), 0, "the Keychain was asked");
    }
}
