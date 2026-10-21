//! The daemon in words (redesign §5.2): auto's state for the header, from `status` alone, and
//! the daemon's times.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Strategy;
use tagteam_daemon::protocol::StatusProvider;
use tagteam_engine::words::{MISSING, threshold};

use crate::state::State;
use crate::theme::Role;

/// §5.2's state for the focus provider, long from 120 columns; each piece with its role.
/// Nothing while the TUI waits for the daemon's first `status` (T2-11). In live mode, the
/// provider's job in the last `status`; else why the daemon is not followed: a protocol this
/// TUI does not speak, the daemon's `stopping`, or no daemon.
pub(crate) fn auto_state(state: &State, wide: bool) -> Vec<(String, Role)> {
    let d = &state.daemon;
    if !d.settled() {
        return vec![];
    }
    let provider = super::focus(state);
    if d.live() {
        return match d.job(&provider) {
            Some(job) => job_state(state, job, &provider, wide),
            None => said("○", Role::Muted, "auto off", None),
        };
    }
    if d.mismatch().is_some() {
        let words = if wide {
            "daemon version unsupported"
        } else {
            "daemon unsupported"
        };
        return said("!", Role::Warn, words, None);
    }
    if let Some(reason) = d.stopping() {
        let words = if wide {
            format!("daemon stopping ({reason})")
        } else {
            "daemon stopping".to_owned()
        };
        return said("!", Role::Warn, &words, None);
    }
    said("○", Role::Muted, "auto off", Some("no daemon"))
}

/// `mark` in `role`, then `words` in text, then `· rest` muted.
fn said(mark: &str, role: Role, words: &str, rest: Option<&str>) -> Vec<(String, Role)> {
    let mut pieces = vec![(mark.to_owned(), role), (format!(" {words}"), Role::Text)];
    if let Some(rest) = rest {
        pieces.push((format!(" · {rest}"), Role::Muted));
    }
    pieces
}

/// §5.2's table: `job`'s state, with its threshold and strategy from `provider`'s accounts view,
/// and a keychain backoff's end in the TUI's zone. At 80 columns a backoff replaces the state.
fn job_state(
    state: &State,
    job: &StatusProvider,
    provider: &ProviderId,
    wide: bool,
) -> Vec<(String, Role)> {
    let until = job
        .backoff
        .as_ref()
        .map(|b| iso_clock(&b.until, state.tokens.tz()));
    if let (Some(until), false) = (&until, wide) {
        return said("!", Role::Warn, &format!("keychain locked {until}"), None);
    }
    let view = state.view_of(provider);
    let mut pieces = match (job.mode.as_str(), job.waiting_for.as_deref()) {
        ("auto", _) => {
            let rest = view.map(|v| match (v.strategy, wide) {
                (Strategy::ConsumeFirst, true) => "uses the soonest reset first".to_owned(),
                (Strategy::ConsumeFirst, false) => "soonest reset".to_owned(),
                (Strategy::Best, true) => format!("switches at {}", threshold(v.threshold)),
                (Strategy::Best, false) => format!("at {}", threshold(v.threshold)),
            });
            said("●", Role::Accent, "auto on", rest.as_deref())
        }
        ("collect", _) if !job.autoswitch => {
            let words = if wide { "auto paused" } else { "paused" };
            said("‖", Role::Warn, words, Some("P resumes"))
        }
        ("collect", _) if wide => said(
            "○",
            Role::Muted,
            "auto idle",
            Some("needs 2 usable accounts"),
        ),
        ("collect", _) => said("○", Role::Muted, "needs 2 usable accounts", None),
        ("waiting", Some("engine-lock")) => {
            let words = match (job.holder_pid, wide) {
                (Some(pid), true) => format!("auto runs in another terminal (pid {pid})"),
                (Some(pid), false) => format!("auto in pid {pid}"),
                (None, true) => "auto runs in another terminal".to_owned(),
                (None, false) => "auto runs elsewhere".to_owned(),
            };
            said("●", Role::Accent, &words, None)
        }
        ("waiting", Some("schema")) if wide => {
            said("!", Role::Warn, "daemon is outdated", Some("restarting"))
        }
        ("waiting", Some("schema")) => said("!", Role::Warn, "daemon outdated", None),
        ("idle", _) if wide => said("○", Role::Muted, "auto off", Some("no usable account")),
        ("idle", _) => said("○", Role::Muted, "no usable account", None),
        (mode, _) => said("○", Role::Muted, &format!("daemon {mode}"), None),
    };
    if let Some(until) = until {
        pieces.push((format!(" · keychain locked until {until}"), Role::Warn));
    }
    pieces
}

/// An ISO 8601 time the daemon wrote, as `HH:MM` in `tz`; `—` when it cannot be read.
fn iso_clock(iso: &str, tz: &TimeZone) -> String {
    iso_ms(iso).map_or_else(|| MISSING.to_owned(), |ms| clock(ms, tz))
}

/// An ISO 8601 time, in epoch ms.
pub(crate) fn iso_ms(iso: &str) -> Option<i64> {
    iso.parse::<Timestamp>().ok().map(|t| t.as_millisecond())
}

/// `ms` as `HH:MM` in `tz`.
pub(crate) fn clock(ms: i64, tz: &TimeZone) -> String {
    Timestamp::from_millisecond(ms).map_or_else(
        |_| MISSING.to_owned(),
        |t| t.to_zoned(tz.clone()).strftime("%H:%M").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use tagteam_daemon::protocol::StatusBackoff;

    use super::*;
    use crate::state::Screen;
    use crate::testutil::*;

    fn utc() -> TimeZone {
        TimeZone::UTC
    }

    /// `pieces` as the header writes them.
    fn joined(pieces: &[(String, Role)]) -> String {
        pieces.iter().map(|(text, _)| text.as_str()).collect()
    }

    #[test]
    fn each_daemon_state_of_spec_5_2_has_its_words_at_both_widths_with_and_without_a_backoff() {
        // §5.2's table, and T2-12's rows: (mode, autoswitch, waiting for, pid, 120, 80).
        for (mode, autoswitch, waiting_for, holder, wide, narrow) in [
            (
                "auto",
                true,
                None,
                None,
                "● auto on · switches at 90%",
                "● auto on · at 90%",
            ),
            (
                "collect",
                false,
                None,
                None,
                "‖ auto paused · P resumes",
                "‖ paused · P resumes",
            ),
            (
                "collect",
                true,
                None,
                None,
                "○ auto idle · needs 2 usable accounts",
                "○ needs 2 usable accounts",
            ),
            (
                "waiting",
                true,
                Some("engine-lock"),
                Some(4711),
                "● auto runs in another terminal (pid 4711)",
                "● auto in pid 4711",
            ),
            (
                "waiting",
                true,
                Some("engine-lock"),
                None,
                "● auto runs in another terminal",
                "● auto runs elsewhere",
            ),
            (
                "waiting",
                true,
                Some("schema"),
                None,
                "! daemon is outdated · restarting",
                "! daemon outdated",
            ),
            (
                "idle",
                true,
                None,
                None,
                "○ auto off · no usable account",
                "○ no usable account",
            ),
            (
                "added-later",
                true,
                None,
                None,
                "○ daemon added-later",
                "○ daemon added-later",
            ),
        ] {
            let mut job = daemon_job("claude-code", mode, autoswitch);
            job.waiting_for = waiting_for.map(str::to_owned);
            job.holder_pid = holder;
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(snapshot(many(1)));
            go_live(&mut state, daemon_status(vec![job.clone()], vec![]));
            assert_eq!(
                joined(&auto_state(&state, true)),
                wide,
                "{mode} {waiting_for:?}"
            );
            assert_eq!(joined(&auto_state(&state, false)), narrow, "{mode}");
            job.backoff = Some(StatusBackoff {
                reason: "keychain".into(),
                until: "2026-09-21T15:05:00Z".into(),
            });
            go_live(&mut state, daemon_status(vec![job], vec![]));
            let locked = auto_state(&state, true);
            assert_eq!(
                joined(&locked),
                format!("{wide} · keychain locked until 15:05"),
                "{mode} {waiting_for:?}"
            );
            assert_eq!(
                locked.last(),
                Some(&(" · keychain locked until 15:05".to_owned(), Role::Warn))
            );
            assert_eq!(
                auto_state(&state, false),
                [
                    ("!".to_owned(), Role::Warn),
                    (" keychain locked 15:05".to_owned(), Role::Text)
                ],
                "{mode}: the backoff replaces the state at 80 columns"
            );
        }
    }

    #[test]
    fn each_piece_of_the_state_has_its_role() {
        // §5.2: `●` accent, `‖` and `!` warn, `○` muted, the words text, what follows ` · ` muted.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(1)));
        go_live(
            &mut state,
            daemon_status(vec![daemon_job("claude-code", "collect", false)], vec![]),
        );
        let piece = |text: &str, role| (text.to_owned(), role);
        assert_eq!(
            auto_state(&state, true),
            [
                piece("‖", Role::Warn),
                piece(" auto paused", Role::Text),
                piece(" · P resumes", Role::Muted),
            ]
        );
        update_live(&mut state, daemon_job("claude-code", "auto", true));
        assert_eq!(
            auto_state(&state, true),
            [
                piece("●", Role::Accent),
                piece(" auto on", Role::Text),
                piece(" · switches at 90%", Role::Muted),
            ]
        );
        update_live(&mut state, daemon_job("claude-code", "idle", true));
        assert_eq!(auto_state(&state, false)[0], piece("○", Role::Muted));
    }

    /// A new link whose `status` has `job` alone.
    fn update_live(state: &mut State, job: StatusProvider) {
        go_live(state, daemon_status(vec![job], vec![]));
    }

    #[test]
    fn an_unreadable_time_is_a_dash() {
        assert_eq!(iso_clock("soon", &utc()), MISSING);
        assert_eq!(clock(NOW_MS, &utc()), "14:13");
    }
}
