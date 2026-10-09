//! §14.2's subscriber, installed once at the process boundary (Decision 5): the stderr layer
//! as M2 had it, and the log file with a filter and a line format of its own.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tagteam_cc::usage::format_iso8601;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{EnvFilter, LevelFilter, Targets};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;

use crate::logfile::LogFile;

/// Replaces the file's filter (§14.2).
pub(crate) const TAGTEAM_LOG: &str = "TAGTEAM_LOG";
/// A panic's log line (Decision 9), which stderr leaves out.
pub(crate) const PANIC_TARGET: &str = "tagteam::panic";
/// Decision 6: the tagteam crates at INFO, everything else at WARN; DEBUG with `--debug`.
const FILE_FILTER: &str =
    "warn,tagteam=info,tagteam_core=info,tagteam_provider=info,tagteam_cc=info,tagteam_engine=info";
const DEBUG_FILE_FILTER: &str = "warn,tagteam=debug,tagteam_core=debug,tagteam_provider=debug,tagteam_cc=debug,tagteam_engine=debug";

/// What logging needs from the process boundary.
pub(crate) struct LogConfig {
    /// `--debug`.
    pub debug: bool,
    /// Neither `--no-color` nor `NO_COLOR`: stderr's lines may be coloured, on a terminal.
    pub color: bool,
    /// `Env::log_file` (§5, §14.2).
    pub log_file: PathBuf,
    /// `Env::home`, whose paths the file writes as `~/…`.
    pub home: PathBuf,
    /// `TAGTEAM_LOG`, as the environment holds it.
    pub filter: Option<OsString>,
}

/// Installs the subscriber, and then the panic hook, once per process: with
/// `set_global_default`, never `try_init`, so the `log` crate's records (ureq's, rustls's) are
/// not bridged into it (Decision 5). An invalid `TAGTEAM_LOG` is one warning on `err`.
pub(crate) fn init(cfg: LogConfig, err: &mut dyn Write) {
    let file = file_directive(cfg.debug, cfg.filter.as_deref(), err).map(|directive| {
        tracing_subscriber::fmt::layer()
            .with_writer(log_file(&cfg))
            .with_ansi(false)
            .log_internal_errors(false)
            .event_format(Line::new(&cfg.home))
            .with_filter(EnvFilter::new(directive))
    });
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(cfg.color && std::io::stderr().is_terminal())
        .with_target(false)
        .with_filter(stderr_filter(cfg.debug));
    let subscriber = tracing_subscriber::registry().with(stderr).with(file);
    if tracing::subscriber::set_global_default(subscriber).is_ok() {
        install_panic_hook();
    }
}

/// M2's stderr: ERROR, or DEBUG with `--debug`. Never a panic's line: the default hook prints
/// the panic there already (Decision 9).
fn stderr_filter(debug: bool) -> Targets {
    let level = if debug {
        LevelFilter::DEBUG
    } else {
        LevelFilter::ERROR
    };
    Targets::new()
        .with_default(level)
        .with_target(PANIC_TARGET, LevelFilter::OFF)
}

/// The file's filter (Decision 6), or `None` for no file at all. `TAGTEAM_LOG` replaces the
/// default whole, `--debug` or not; `off` turns the file off. A value that does not parse
/// keeps the default and says so once on `err`. An empty value is as good as unset.
fn file_directive(debug: bool, var: Option<&OsStr>, err: &mut dyn Write) -> Option<String> {
    let default = if debug {
        DEBUG_FILE_FILTER
    } else {
        FILE_FILTER
    };
    let Some(var) = var.filter(|v| !v.is_empty()) else {
        return Some(default.to_owned());
    };
    let parsed = match var.to_str() {
        Some(v) if v.trim().eq_ignore_ascii_case("off") => return None,
        Some(v) => EnvFilter::try_new(v)
            .map(|_| v.to_owned())
            .map_err(|e| e.to_string()),
        None => Err("it is not UTF-8".to_owned()),
    };
    Some(parsed.unwrap_or_else(|why| {
        let _ = writeln!(
            err,
            "warning: {TAGTEAM_LOG} is not a valid filter ({why}); the log keeps its default"
        );
        default.to_owned()
    }))
}

/// The file layer's writer. Under `--debug`, a log that cannot be written says so once on
/// stderr; otherwise it falls silent (§14.2).
fn log_file(cfg: &LogConfig) -> LogFile {
    let file = LogFile::new(cfg.log_file.clone());
    if !cfg.debug {
        return file;
    }
    file.on_disable(Box::new(|path, e| {
        let _ = writeln!(
            std::io::stderr(),
            "warning: the log file {} cannot be written ({e}); nothing more is logged to it",
            path.display()
        );
    }))
}

/// Decision 8's line: `2026-10-01T18:42:33.581Z 4242 INFO tagteam_engine::switch: message
/// key=value …`, with every `<home>/` written `~/`, and on one line whatever a value holds.
struct Line {
    /// `<home>/`; `None` for a home of `/` (every path would be under it) or one that is not
    /// UTF-8.
    home: Option<String>,
    pid: u32,
    now_ms: fn() -> i64,
}

impl Line {
    fn new(home: &Path) -> Self {
        Self::with_clock(home, std::process::id(), now_ms)
    }

    fn with_clock(home: &Path, pid: u32, now_ms: fn() -> i64) -> Self {
        let home = home
            .to_str()
            .map(|h| h.trim_end_matches('/'))
            .filter(|h| !h.is_empty())
            .map(|h| format!("{h}/"));
        Self { home, pid, now_ms }
    }

    /// The finished line, newline included. `fields` is the message, then `key=value` pairs.
    fn finish(&self, now_ms: i64, level: &Level, target: &str, fields: &str) -> String {
        let mut line = format!(
            "{} {} {level} {target}: {fields}",
            timestamp(now_ms),
            self.pid
        );
        if let Some(home) = &self.home {
            line = line.replace(home.as_str(), "~/");
        }
        let mut line = line.replace('\r', "\\r").replace('\n', "\\n");
        line.push('\n');
        line
    }
}

impl<S, N> FormatEvent<S, N> for Line
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut fields = String::new();
        ctx.field_format()
            .format_fields(Writer::new(&mut fields), event)?;
        let meta = event.metadata();
        writer.write_str(&self.finish((self.now_ms)(), meta.level(), meta.target(), &fields))
    }
}

/// UTC to the millisecond: `format_iso8601`'s seconds, then `.mmm` before the `Z`.
fn timestamp(now_ms: i64) -> String {
    let seconds = format_iso8601(now_ms.div_euclid(1000));
    format!(
        "{}.{:03}Z",
        seconds.trim_end_matches('Z'),
        now_ms.rem_euclid(1000)
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Decision 9: a panic is logged at ERROR, with its location, before the process unwinds
/// (§14.2); the default hook then prints it on stderr as before. Its message is logged only
/// when it is a literal: a formatted one can carry runtime values (serde's `invalid type:
/// string "…"` quotes the string it met), and stderr shows it anyway.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(String::new, |l| format!("{}:{}", l.file(), l.line()));
        let message = info
            .payload()
            .downcast_ref::<&'static str>()
            .copied()
            .unwrap_or("(a formatted message, not logged)");
        tracing::error!(target: PANIC_TARGET, location = %location, "panic: {message}");
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    /// One capture at a time: `tracing` keeps one maximum level, and one cache of which call
    /// sites are enabled, across every subscriber alive in the process, so captures running
    /// side by side can lose each other's lines.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn one_at_a_time() -> MutexGuard<'static, ()> {
        ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 2026-10-01T18:42:33.581Z, Decision 8's example.
    fn decision_8_clock() -> i64 {
        1_790_880_153_581
    }

    /// Whatever a layer wrote, shared with the test.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Captured {
            self.clone()
        }
    }

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    /// The file layer as `init` builds it, writing to memory under `directive`, with Decision
    /// 8's clock and pid 4242: what the file would hold after `emit`.
    fn file_lines(home: &str, directive: &str, emit: impl FnOnce()) -> String {
        let _serial = one_at_a_time();
        let out = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(out.clone())
            .with_ansi(false)
            .event_format(Line::with_clock(Path::new(home), 4242, decision_8_clock))
            .with_filter(EnvFilter::new(directive));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), emit);
        out.text()
    }

    /// The stderr layer's filter over a memory writer: what stderr would show after `emit`.
    fn stderr_lines(debug: bool, emit: impl FnOnce()) -> String {
        let _serial = one_at_a_time();
        let out = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(out.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_filter(stderr_filter(debug));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), emit);
        out.text()
    }

    #[test]
    fn a_line_is_utc_time_pid_level_target_then_the_message_and_its_fields() {
        let out = file_lines("/home/u", FILE_FILTER, || {
            tracing::info!(
                target: "tagteam_engine::switch",
                account = %"0192aa",
                position = 3,
                kind = "switch",
                "switched"
            );
        });
        assert_eq!(
            out,
            "2026-10-01T18:42:33.581Z 4242 INFO tagteam_engine::switch: switched account=0192aa position=3 kind=\"switch\"\n"
        );
    }

    #[test]
    fn the_time_is_utc_to_the_millisecond() {
        assert_eq!(timestamp(1_790_880_153_581), "2026-10-01T18:42:33.581Z");
        assert_eq!(timestamp(1_790_880_153_007), "2026-10-01T18:42:33.007Z");
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_path_under_home_is_written_with_a_tilde() {
        // §14.2: `<home>/` anywhere in the line, in the message and the values alike.
        for home in ["/Users/me", "/Users/me/"] {
            let line = Line::with_clock(Path::new(home), 1, decision_8_clock);
            assert_eq!(
                line.finish(
                    0,
                    &Level::WARN,
                    "tagteam_cc::live",
                    "moved /Users/me/.claude.json path=/Users/me/.local/share/tagteam/x"
                ),
                "1970-01-01T00:00:00.000Z 1 WARN tagteam_cc::live: moved ~/.claude.json path=~/.local/share/tagteam/x\n",
                "{home}"
            );
        }
        // The home itself, and a sibling that only starts like it, are not under it.
        let line = Line::with_clock(Path::new("/Users/me"), 1, decision_8_clock);
        assert!(
            line.finish(0, &Level::INFO, "t", "a=/Users/me b=/Users/meg/x")
                .ends_with(" t: a=/Users/me b=/Users/meg/x\n")
        );
        // A home of `/` would write every absolute path `~/…`: nothing is rewritten.
        let root = Line::with_clock(Path::new("/"), 1, decision_8_clock);
        assert!(
            root.finish(0, &Level::INFO, "t", "p=/etc/x")
                .ends_with(" t: p=/etc/x\n")
        );
    }

    #[test]
    fn a_value_holding_a_newline_stays_on_its_line() {
        let out = file_lines("/home/u", FILE_FILTER, || {
            tracing::warn!(target: "tagteam_engine::x", "first\nsecond\r");
        });
        assert_eq!(
            out,
            "2026-10-01T18:42:33.581Z 4242 WARN tagteam_engine::x: first\\nsecond\\r\n"
        );
    }

    #[test]
    fn the_file_takes_info_from_tagteam_and_warn_from_the_rest() {
        // Decision 6, and with `--debug` tagteam's DEBUG too.
        let emit = || {
            tracing::info!(target: "tagteam_engine::store", "kept");
            tracing::debug!(target: "tagteam_engine::store", "debug");
            tracing::info!(target: "tagteam_cc::live", "kept");
            tracing::error!(target: "tagteam::panic", "kept");
            tracing::info!(target: "ureq::unversioned", "dropped");
            tracing::warn!(target: "ureq::unversioned", "kept");
        };
        let out = file_lines("/h", FILE_FILTER, emit);
        assert_eq!(out.matches(": kept").count(), 4, "{out}");
        assert!(
            !out.contains(": debug") && !out.contains(": dropped"),
            "{out}"
        );
        let out = file_lines("/h", DEBUG_FILE_FILTER, emit);
        assert_eq!(out.matches(": kept").count(), 4, "{out}");
        assert_eq!(out.matches(": debug").count(), 1, "{out}");
        assert!(!out.contains(": dropped"), "{out}");
    }

    #[test]
    fn tagteam_log_replaces_the_file_filter_and_off_removes_the_file() {
        let pick = |debug: bool, var: Option<&str>| {
            let mut err = Vec::new();
            let got = file_directive(debug, var.map(OsStr::new), &mut err);
            assert!(err.is_empty(), "{}", String::from_utf8_lossy(&err));
            got
        };
        assert_eq!(pick(false, None).as_deref(), Some(FILE_FILTER));
        assert_eq!(pick(true, None).as_deref(), Some(DEBUG_FILE_FILTER));
        assert_eq!(pick(false, Some("")).as_deref(), Some(FILE_FILTER));
        assert_eq!(
            pick(true, Some("tagteam_engine=trace")).as_deref(),
            Some("tagteam_engine=trace"),
            "the whole filter, --debug or not"
        );
        assert_eq!(pick(false, Some("off")), None);
        assert_eq!(pick(true, Some(" OFF ")), None);
    }

    #[test]
    fn an_invalid_tagteam_log_keeps_the_default_with_one_warning() {
        let cases = [
            (OsString::from("tagteam=loud"), false, FILE_FILTER),
            (
                OsString::from_vec(vec![b't', 0xff]),
                true,
                DEBUG_FILE_FILTER,
            ),
        ];
        for (var, debug, default) in cases {
            let mut err = Vec::new();
            let got = file_directive(debug, Some(var.as_os_str()), &mut err);
            assert_eq!(got.as_deref(), Some(default), "{var:?}");
            let err = String::from_utf8(err).unwrap();
            assert!(
                err.starts_with("warning: TAGTEAM_LOG is not a valid filter (")
                    && err.ends_with("; the log keeps its default\n")
                    && err.lines().count() == 1,
                "{err:?}"
            );
        }
    }

    #[test]
    fn stderr_shows_errors_or_with_debug_diagnostics_and_never_a_panic() {
        let emit = || {
            tracing::error!(target: "tagteam_engine::refresh", "an error");
            tracing::warn!(target: "tagteam_engine::switch", "a warning");
            tracing::debug!(target: "tagteam_engine::oracle", "a diagnostic");
            tracing::trace!(target: "tagteam_engine::oracle", "a trace");
            tracing::error!(target: PANIC_TARGET, "panic: boom");
        };
        let quiet = stderr_lines(false, emit);
        assert!(quiet.contains("an error"), "{quiet}");
        assert!(
            !quiet.contains("a warning") && !quiet.contains("boom"),
            "{quiet}"
        );
        let debug = stderr_lines(true, emit);
        assert!(
            debug.contains("an error")
                && debug.contains("a warning")
                && debug.contains("a diagnostic"),
            "{debug}"
        );
        assert!(
            !debug.contains("a trace") && !debug.contains("boom"),
            "{debug}"
        );
    }
}
