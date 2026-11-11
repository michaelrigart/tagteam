//! The daemon in words (redesign §5.2): auto's state for the header, from `status` alone, and
//! the daemon's times.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_core::autoswitch::AutoStrategy;
use tagteam_daemon::events::{Mode, WaitingFor};
use tagteam_daemon::protocol::{StatusProvider, Wire};
use tagteam_engine::auto::HolderKind;
use tagteam_engine::words::{MISSING, threshold};

use crate::actions::UPGRADED;
use crate::source::AutoHolder;
use crate::state::State;
use crate::theme::Role;

/// §5.2's state for the focus provider, long from 120 columns; each piece with its role.
/// Nothing while the TUI waits for the daemon's first `status` (T2-11) or the lane's first
/// report, and nothing after that wait for a lane that still has not reported where nothing
/// else says who runs auto (S§3.3). In order (Ruling 6): the provider's job in a live daemon's last `status`; the lane
/// stopped for a newer store; a protocol this TUI does not speak; the daemon's `stopping`; the
/// lane's own job; a live daemon record with no link; the holder of the provider's engine lock
/// (C-16); no daemon.
pub(crate) fn auto_state(state: &State, wide: bool) -> Vec<(String, Role)> {
    let d = &state.daemon;
    if !d.settled() {
        return vec![];
    }
    let provider = super::focus(state);
    if d.live() {
        return match d.job(&provider) {
            Some(job) => job_state(state, job, &provider, wide, false),
            None => said("○", Role::Muted, "auto off", None),
        };
    }
    if d.upgraded() {
        return if wide {
            said("!", Role::Warn, "auto stopped", Some(UPGRADED))
        } else {
            said("!", Role::Warn, "restart the TUI", None)
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
    if let Some(job) = d.hosted_job(&provider) {
        return job_state(state, job, &provider, wide, true);
    }
    match (state.holder_of(&provider), state.daemon_pid()) {
        (Some(holder), _) => held_by(holder, wide),
        (None, Some(pid)) if wide => said(
            "○",
            Role::Muted,
            &format!("daemon runs (pid {pid})"),
            Some("not connected, state unknown"),
        ),
        (None, Some(_)) => said("○", Role::Muted, "daemon state unknown", None),
        // A lane still in its first run has said nothing, and may be running auto.
        (None, None) if d.lane_starting() => vec![],
        (None, None) => said("○", Role::Muted, "auto off", Some("no daemon")),
    }
}

/// Where auto runs when `holder` has the provider's engine lock and nothing else says: the
/// daemon (whose state the TUI cannot read), another tagteam window, or a `tagteam auto`.
fn held_by(holder: &AutoHolder, wide: bool) -> Vec<(String, Role)> {
    let pid = holder.pid;
    match (holder.kind, wide) {
        (HolderKind::Daemon, true) => said(
            "●",
            Role::Accent,
            &format!("auto runs in the daemon (pid {pid})"),
            Some("not connected"),
        ),
        (HolderKind::Daemon, false) => said("●", Role::Accent, "auto in daemon", None),
        (HolderKind::Tui, true) => said(
            "●",
            Role::Accent,
            &format!("auto runs in another tagteam window (pid {pid})"),
            None,
        ),
        (HolderKind::Auto, true) => said(
            "●",
            Role::Accent,
            &format!("auto runs in another terminal (pid {pid})"),
            None,
        ),
        (HolderKind::Tui | HolderKind::Auto, false) => {
            said("●", Role::Accent, &format!("auto in pid {pid}"), None)
        }
    }
}

/// This process's pid: a job that waits for the engine lock on it waits for this window's lane.
fn own_pid() -> u32 {
    std::process::id()
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
/// `here`: the job is the auto lane's, so `auto` runs in this window (S§3.3), except under a
/// keychain backoff, which reads as the daemon's. A job waiting for
/// the engine lock names its holder as the accounts view knows it.
fn job_state(
    state: &State,
    job: &StatusProvider,
    provider: &ProviderId,
    wide: bool,
    here: bool,
) -> Vec<(String, Role)> {
    let until = job
        .backoff
        .as_ref()
        .map(|b| iso_clock(&b.until, state.tokens.tz()));
    if let (Some(until), false) = (&until, wide) {
        return said("!", Role::Warn, &format!("keychain locked {until}"), None);
    }
    let view = state.view_of(provider);
    let waiting_for = job.waiting_for.as_ref().and_then(Wire::known);
    let mut pieces = match (job.mode.known(), waiting_for) {
        (Some(Mode::Auto), _) => {
            let rest = view.map(|v| match (v.strategy, wide) {
                (AutoStrategy::ConsumeFirst, true) => "uses the soonest reset first".to_owned(),
                (AutoStrategy::ConsumeFirst, false) => "soonest reset".to_owned(),
                (AutoStrategy::Best, true) => format!("switches at {}", threshold(v.threshold)),
                (AutoStrategy::Best, false) => format!("at {}", threshold(v.threshold)),
            });
            let words = if here && wide && until.is_none() {
                "auto on in this window"
            } else {
                "auto on"
            };
            said("●", Role::Accent, words, rest.as_deref())
        }
        (Some(Mode::Collect), _) if !job.autoswitch => {
            let words = if wide { "auto paused" } else { "paused" };
            said("‖", Role::Warn, words, Some("P resumes"))
        }
        (Some(Mode::Collect), _) if wide => said(
            "○",
            Role::Muted,
            "auto idle",
            Some("needs 2 usable accounts"),
        ),
        (Some(Mode::Collect), _) => said("○", Role::Muted, "needs 2 usable accounts", None),
        (Some(Mode::Waiting), Some(WaitingFor::EngineLock)) => {
            let window = job.holder_pid.is_some_and(|pid| {
                state
                    .holder_of(provider)
                    .is_some_and(|h| h.pid == pid && h.kind == HolderKind::Tui)
            });
            let words = match (job.holder_pid, wide, window) {
                (Some(pid), wide, _) if pid == own_pid() => {
                    // A daemon that started beside this window waits on the lock this window's
                    // lane still holds, and says so until it next looks (S§6).
                    if wide {
                        "auto moving to the daemon"
                    } else {
                        "auto → daemon"
                    }
                    .to_owned()
                }
                (Some(pid), true, true) => {
                    format!("auto runs in another tagteam window (pid {pid})")
                }
                (Some(pid), true, false) => format!("auto runs in another terminal (pid {pid})"),
                (Some(pid), false, _) => format!("auto in pid {pid}"),
                (None, true, _) => "auto runs in another terminal".to_owned(),
                (None, false, _) => "auto runs elsewhere".to_owned(),
            };
            said("●", Role::Accent, &words, None)
        }
        (Some(Mode::Waiting), Some(WaitingFor::Schema)) if wide => {
            said("!", Role::Warn, "daemon is outdated", Some("restarting"))
        }
        (Some(Mode::Waiting), Some(WaitingFor::Schema)) => {
            said("!", Role::Warn, "daemon outdated", None)
        }
        (Some(Mode::Idle), _) if wide => {
            said("○", Role::Muted, "auto off", Some("no usable account"))
        }
        (Some(Mode::Idle), _) => said("○", Role::Muted, "no usable account", None),
        (Some(Mode::Waiting) | None, _) => {
            let words = format!("daemon {}", job.mode.as_str());
            said("○", Role::Muted, &words, None)
        }
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
    use crate::daemon::SETTLE_MS;
    use crate::msg::{LaneReport, Msg};
    use crate::source::AccountsSnapshot;
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
            job.waiting_for = waiting_for.map(Wire::from_wire);
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

    /// A state that reads `snap` and has had `report` from the auto lane, if any.
    fn lane_with(report: Option<LaneReport>, snap: AccountsSnapshot) -> State {
        let mut state = fresh(Screen::Accounts);
        if let Some(report) = report {
            crate::update::update(&mut state, Msg::Lane(report), NOW_MS);
        }
        state.accounts.apply(snap);
        state
    }

    fn lane_job(mode: &str, autoswitch: bool, lock: Option<u32>) -> LaneReport {
        let mut job = daemon_job("claude-code", mode, autoswitch);
        if let Some(pid) = lock {
            job.waiting_for = Some(Wire::from_wire("engine-lock"));
            job.holder_pid = Some(pid);
        }
        LaneReport::Status(daemon_status(vec![job], vec![]))
    }

    #[test]
    fn each_state_of_the_autoswitch_header_table_has_its_words_at_both_widths() {
        let tui = |pid| held(&[(HolderKind::Tui, pid)], None);
        let auto = |pid| held(&[(HolderKind::Auto, pid)], None);
        let window = "● auto runs in another tagteam window (pid 4711)";
        let terminal = "● auto runs in another terminal (pid 4711)";
        let mut consume_first = held(&[], None);
        consume_first.providers[0].strategy = AutoStrategy::ConsumeFirst;
        let mut backoff = daemon_job("claude-code", "auto", true);
        backoff.backoff = Some(StatusBackoff {
            reason: "keychain".into(),
            until: "2026-09-21T15:05:00Z".into(),
        });
        let mut live_waiting = daemon_job("claude-code", "waiting", true);
        live_waiting.waiting_for = Some(Wire::from_wire("engine-lock"));
        live_waiting.holder_pid = Some(4711);
        let live = |job| {
            let mut state = fresh(Screen::Accounts);
            go_live(&mut state, daemon_status(vec![job], vec![]));
            state
        };
        let mut live_state = live(live_waiting);
        live_state.accounts.apply(tui(4711));
        let mut own_waiting = daemon_job("claude-code", "waiting", true);
        own_waiting.waiting_for = Some(Wire::from_wire("engine-lock"));
        own_waiting.holder_pid = Some(std::process::id());
        let live_own = live(own_waiting);
        let mut live_auto = live(daemon_job("claude-code", "auto", true));
        live_auto.accounts.apply(held(&[], Some(4242)));
        let daemon = |holders: &[(HolderKind, u32)]| held(holders, Some(4242));
        let cases: Vec<(&str, State, &str, &str)> = vec![
            (
                "lane auto, best",
                lane_with(Some(lane_job("auto", true, None)), held(&[], None)),
                "● auto on in this window · switches at 90%",
                "● auto on · at 90%",
            ),
            (
                "lane auto, consume-first",
                lane_with(Some(lane_job("auto", true, None)), consume_first),
                "● auto on in this window · uses the soonest reset first",
                "● auto on · soonest reset",
            ),
            (
                "lane waiting, holder tui",
                lane_with(Some(lane_job("waiting", true, Some(4711))), tui(4711)),
                window,
                "● auto in pid 4711",
            ),
            (
                "lane waiting, holder auto",
                lane_with(Some(lane_job("waiting", true, Some(4711))), auto(4711)),
                terminal,
                "● auto in pid 4711",
            ),
            (
                "lane collect, paused",
                lane_with(Some(lane_job("collect", false, None)), held(&[], None)),
                "‖ auto paused · P resumes",
                "‖ paused · P resumes",
            ),
            (
                "lane idle",
                lane_with(Some(lane_job("idle", true, None)), held(&[], None)),
                "○ auto off · no usable account",
                "○ no usable account",
            ),
            (
                "lane auto, backoff",
                lane_with(
                    Some(LaneReport::Status(daemon_status(vec![backoff], vec![]))),
                    held(&[], None),
                ),
                "● auto on · switches at 90% · keychain locked until 15:05",
                "! keychain locked 15:05",
            ),
            (
                "live waiting, holder tui",
                live_state,
                window,
                "● auto in pid 4711",
            ),
            (
                "live waiting, holder this window's own lane",
                live_own,
                "● auto moving to the daemon",
                "● auto → daemon",
            ),
            (
                "no lane jobs, holder tui",
                lane_with(Some(LaneReport::Off), tui(4711)),
                window,
                "● auto in pid 4711",
            ),
            (
                "upgraded",
                lane_with(Some(LaneReport::Upgraded), held(&[], None)),
                "! auto stopped · tagteam was upgraded; restart the TUI",
                "! restart the TUI",
            ),
            (
                "daemon, no link, held by the daemon",
                lane_with(None, daemon(&[(HolderKind::Daemon, 4242)])),
                "● auto runs in the daemon (pid 4242) · not connected",
                "● auto in daemon",
            ),
            (
                "daemon, no link, held by auto",
                lane_with(None, daemon(&[(HolderKind::Auto, 4711)])),
                terminal,
                "● auto in pid 4711",
            ),
            (
                "daemon, no link, held by tui",
                lane_with(None, daemon(&[(HolderKind::Tui, 4711)])),
                window,
                "● auto in pid 4711",
            ),
            (
                "daemon, no link, not held",
                lane_with(None, daemon(&[])),
                "○ daemon runs (pid 4242) · not connected, state unknown",
                "○ daemon state unknown",
            ),
            (
                "live daemon, auto",
                live_auto,
                "● auto on · switches at 90%",
                "● auto on · at 90%",
            ),
            (
                "lane off, nothing else",
                lane_with(Some(LaneReport::Off), held(&[], None)),
                "○ auto off · no daemon",
                "○ auto off · no daemon",
            ),
        ];
        for (case, state, wide, narrow) in cases {
            assert_eq!(joined(&auto_state(&state, true)), wide, "{case}");
            assert_eq!(joined(&auto_state(&state, false)), narrow, "{case}");
        }
        assert!(UPGRADED.ends_with("restart the TUI"));
    }

    #[test]
    fn a_live_daemon_s_status_outranks_the_lane_and_the_holders() {
        // Ruling 6: with a live `status`, only it counts, whatever else is held or hosted.
        let mut state = fresh(Screen::Accounts);
        crate::update::update(
            &mut state,
            Msg::Lane(lane_job("collect", false, None)),
            NOW_MS,
        );
        go_live(
            &mut state,
            daemon_status(vec![daemon_job("claude-code", "auto", true)], vec![]),
        );
        state
            .accounts
            .apply(held(&[(HolderKind::Daemon, 4242)], Some(4242)));
        assert_eq!(
            joined(&auto_state(&state, true)),
            "● auto on · switches at 90%"
        );
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

    #[test]
    fn a_lane_that_has_not_reported_says_nothing_after_the_settle_and_not_no_daemon() {
        // G5: past `SETTLE_MS` the first collection no longer waits, but a lane still in its
        // first run has said nothing, so "auto off · no daemon" would be false.
        let mut state = fresh(Screen::Accounts);
        state.daemon.await_lane(NOW_MS);
        state.accounts.apply(held(&[], None));
        crate::update::update(&mut state, Msg::Tick, NOW_MS + SETTLE_MS);
        assert!(state.daemon.settled());
        assert!(auto_state(&state, true).is_empty());
        assert!(auto_state(&state, false).is_empty());
        crate::update::update(
            &mut state,
            Msg::Lane(LaneReport::Off),
            NOW_MS + SETTLE_MS + 1,
        );
        assert_eq!(joined(&auto_state(&state, true)), "○ auto off · no daemon");
        assert_eq!(joined(&auto_state(&state, false)), "○ auto off · no daemon");
        // A board shows its job's words.
        let mut state = fresh(Screen::Accounts);
        state.daemon.await_lane(NOW_MS);
        state.accounts.apply(held(&[], None));
        crate::update::update(
            &mut state,
            Msg::Lane(lane_job("auto", true, None)),
            NOW_MS + SETTLE_MS + 1,
        );
        assert_eq!(
            joined(&auto_state(&state, true)),
            "● auto on in this window · switches at 90%"
        );
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
