//! Live mode (spec §6.3, §8): the daemon's link as the state machine sees it. What the live
//! thread learns, and the daemon lane's `status` replies, arrive as messages; what they ask for
//! leaves as commands. The store stays the source of truth: an event is a reason to read the
//! views again, and only `status` says what the daemon does (§7.4, §8). Auto's decisions show
//! on the auto line (redesign §5.3).

pub mod words;

use std::collections::BTreeMap;

use tagteam_core::autoswitch::NoSwitchReason;
use tagteam_core::rank::binding_window;
use tagteam_core::{AccountId, ProviderId, Window};
use tagteam_daemon::events::StopReason;
use tagteam_daemon::protocol::{
    Event, PollPayload, Status, StatusProvider, Wire, parse_token_index,
};
use tagteam_daemon::tokens::TOKEN_INDEX;
use tagteam_engine::views::ProviderAccounts;

use crate::live::{Live, StatusError};
use crate::msg::Cmd;
use crate::screens;
use crate::state::{Reread, Screen, State, ToastKind};

/// Spec §6.3: what an event asks for is read this long after it, and the events that come
/// meanwhile share that read.
pub const REREAD_MS: i64 = 250;
/// Spec §6.3: while linked, `status` is fetched this often, and in live mode `accounts` read.
pub const POLL_MS: i64 = 60_000;
/// Spec §8: after `stopping`, the header names its reason this long, unless a link comes up.
pub const STOPPING_MS: i64 = 30_000;
/// C-R19: a `status` asked for and not answered this long after is given up on, and the link
/// with it. The daemon lane bounds one fetch at 2.25 s (D5.7), and another link's fetch may sit
/// in the lane ahead of this one, so the wait covers two of them (2 x 2.25 s).
pub const STATUS_WAIT_MS: i64 = 5_000;
/// C-R20: the first collection waits at most this long for the daemon's first `status`, no
/// longer than the daemon lane's own bound.
pub const SETTLE_MS: i64 = 2_000;
/// Spec §7.4: `status` is fetched again this long after `P`'s setting was written. The daemon
/// applies a setting within about a second (D4.2).
pub const RECHECK_MS: i64 = 2_000;

/// The daemon's latest decision for a provider on this link (spec §5.3's rank 2).
#[derive(Debug, Clone)]
pub(crate) struct Noted {
    pub(crate) at_ms: i64,
    pub(crate) what: NotedWhat,
}

#[derive(Debug, Clone)]
pub(crate) enum NotedWhat {
    /// A `no-switch`, with the usage of the account in use at the link's last `poll`, and for
    /// `all-exhausted` the earliest reset its `all-exhausted` event names.
    NoSwitch {
        reason: NoSwitchReason,
        detail: String,
        used_pct: Option<f64>,
        /// That poll's windows of the account in use, the one that counts marked.
        windows: Option<String>,
        frees_at_ms: Option<i64>,
    },
    /// An `error`'s message.
    Error(String),
    /// A `config-warning`'s message.
    Warning(String),
}

/// The daemon as the TUI knows it (spec §8).
#[derive(Default)]
pub struct DaemonState {
    /// The link followed now, by the live thread's name for it (C-R21): a `status` reply asked
    /// for under another link is dropped, and a link given up on is ended by its name.
    link: Option<u64>,
    /// This link's last `status`. Live mode is a link with one (spec §8).
    status: Option<Status>,
    /// `stopping`'s reason, and until when the header names it.
    stopping: Option<(Wire<StopReason>, i64)>,
    /// The protocol of a daemon this TUI does not speak.
    mismatch: Option<u32>,
    /// When the next 60 s fetch of `status` is due.
    next_poll_ms: i64,
    /// When `P`'s fetch of `status` is due, whether or not a link was up when the setting was
    /// written. It is made on whichever link is up then: a new link's own fetch can come before
    /// the daemon applies the setting. A link that comes up after it fetches at once, which
    /// serves (C-R21).
    recheck_ms: Option<i64>,
    /// When the reads the last events asked for are made, all at once.
    reread_ms: Option<i64>,
    /// They read the views: an account's reading or state changed.
    reread_views: bool,
    /// They read every view shown, the token report among them: the link came up.
    reread_all: bool,
    /// They read History's timeline at once, not paced (T4-19): a `switch` or a quarantine
    /// event, or the link came up.
    reread_timeline: bool,
    /// They fetch `status`: a provider job ran, and publishes its state as the run ends.
    reread_status: bool,
    /// When the `status` out now, the link's only one, is given up on (C-R19, C-R22).
    status_due_ms: Option<i64>,
    /// `status` was asked for again while one was out: it is asked once that one is answered
    /// (C-R22).
    status_again: bool,
    /// C-R20: until when the first collection waits to know who collects: no daemon, or the
    /// daemon's first `status`. `None` once known, and for a TUI that follows no daemon.
    unsettled_until: Option<i64>,
    /// Each provider's latest decision on this link (spec §5.3's rank 2).
    noted: BTreeMap<String, Noted>,
    /// Each provider's last `poll` on this link, whose usage a `no-switch` of the same tick
    /// reads. A link can miss events before it, so a new one starts with none (C-R21).
    polls: BTreeMap<String, PollPayload>,
    /// A live link was lost in this session, and none has answered since (T2-10): the first
    /// `status` after it says the daemon is back.
    lost: bool,
}

impl DaemonState {
    /// Live mode: subscribed, and the daemon's `status` has come since (spec §8).
    pub fn live(&self) -> bool {
        self.link.is_some() && self.status.is_some()
    }

    /// The daemon's last `status`, in live mode.
    pub fn status(&self) -> Option<&Status> {
        self.status.as_ref().filter(|_| self.link.is_some())
    }

    /// `provider`'s job in the last `status`, in live mode.
    pub fn job(&self, provider: &ProviderId) -> Option<&StatusProvider> {
        self.status()?
            .providers
            .iter()
            .find(|p| p.provider == provider.as_str())
    }

    /// The reason the daemon gave for stopping, while the header names it.
    pub fn stopping(&self) -> Option<&str> {
        self.stopping.as_ref().map(|(reason, _)| reason.as_str())
    }

    /// The protocol of a daemon that speaks another.
    pub fn mismatch(&self) -> Option<u32> {
        self.mismatch
    }

    /// C-R20: whether the TUI knows who collects: no daemon answered, the daemon's first
    /// `status` came, or `SETTLE_MS` passed.
    pub fn settled(&self) -> bool {
        self.unsettled_until.is_none()
    }

    /// C-R20: a TUI that follows a daemon, started at `now_ms`, waits for it before its first
    /// collection.
    pub(crate) fn await_daemon(&mut self, now_ms: i64) {
        self.unsettled_until = Some(now_ms + SETTLE_MS);
    }

    /// The daemon's latest decision for `provider` on this link (spec §5.3's rank 2).
    pub(crate) fn noted(&self, provider: &str) -> Option<&Noted> {
        self.noted.get(provider)
    }
}

/// The provider the header follows and `P` acts on (spec §7.4): the selected account's, else
/// `--provider`'s, else the default, which Tokens reports on.
pub(crate) fn focus(state: &State) -> ProviderId {
    state
        .accounts
        .selected()
        .map(|a| a.provider)
        .or_else(|| state.provider.clone())
        .unwrap_or_else(|| state.tokens.provider().clone())
}

/// What the live thread learned (spec §8).
pub(crate) fn on_live(state: &mut State, live: Live, now_ms: i64) -> Vec<Cmd> {
    match live {
        Live::Up { link } => up(state, link, now_ms),
        Live::Down => {
            down(state, now_ms);
            vec![]
        }
        Live::Mismatch { server } => {
            down(state, now_ms);
            state.daemon.stopping = None;
            state.daemon.mismatch = Some(server);
            state.dirty = true;
            vec![]
        }
        Live::Event(event) => on_event(state, event, now_ms),
    }
}

/// Spec §8, "Connecting": link `link` is up. `status` is fetched at once. Every view shown is
/// read again `REREAD_MS` later, so that its snapshot follows the subscription and no change
/// falls between the two, while the first frame's read (§6.1) is left to finish.
fn up(state: &mut State, link: u64, now_ms: i64) -> Vec<Cmd> {
    let d = &mut state.daemon;
    d.link = Some(link);
    d.status = None;
    d.status_due_ms = None;
    d.status_again = false;
    d.polls.clear();
    d.noted.clear();
    d.stopping = None;
    d.mismatch = None;
    d.next_poll_ms = now_ms + POLL_MS;
    // The link's own fetch comes after `P`'s, if that is due, and serves for it (C-R21).
    if d.recheck_ms.is_some_and(|at| now_ms >= at) {
        d.recheck_ms = None;
    }
    d.reread_ms = Some(now_ms + REREAD_MS);
    d.reread_views = true;
    d.reread_all = true;
    d.reread_timeline = true;
    d.reread_status = false;
    state.dirty = true;
    fetch(state, now_ms).into_iter().collect()
}

/// The link is down, or none came up: timer mode (spec §8), and the TUI knows who collects
/// (C-R20). A mismatch seen before is gone too, since no daemon answered this time. A live link
/// lost toasts it, once (T2-10).
fn down(state: &mut State, now_ms: i64) {
    screens::tokens::index::on_link_down(state);
    let d = &mut state.daemon;
    if d.mismatch.take().is_some() {
        state.dirty = true;
    }
    if d.unsettled_until.take().is_some() {
        state.dirty = true;
    }
    if d.link.take().is_none() {
        return;
    }
    let was_live = d.status.is_some();
    d.status = None;
    d.status_due_ms = None;
    d.status_again = false;
    d.noted.clear();
    d.reread_ms = None;
    d.reread_views = false;
    d.reread_all = false;
    d.reread_timeline = false;
    d.reread_status = false;
    state.dirty = true;
    if was_live && state.quitting.is_none() {
        state.daemon.lost = true;
        let words = "lost the daemon; tagteam updates on its own every 30 s";
        state.toast(ToastKind::Warn, words, now_ms);
    }
}

/// Spec §7.4: `P`'s setting was written; `status` is fetched `RECHECK_MS` from now, whether or
/// not a link is up now (C-R21).
pub(crate) fn recheck(state: &mut State, now_ms: i64) {
    state.daemon.recheck_ms = Some(now_ms + RECHECK_MS);
}

/// `status`, on the daemon lane, for this link. One is out at a time (C-R22): one asked for
/// while another is out is asked once that one is answered, so that it reads what came after
/// it. The one out is answered within `STATUS_WAIT_MS`, or the link is given up on (C-R19).
/// Nothing is asked once the shutdown has begun.
fn fetch(state: &mut State, now_ms: i64) -> Option<Cmd> {
    let d = &mut state.daemon;
    let link = d.link.filter(|_| state.quitting.is_none())?;
    if d.status_due_ms.is_some() {
        d.status_again = true;
        return None;
    }
    d.status_due_ms = Some(now_ms + STATUS_WAIT_MS);
    Some(Cmd::FetchStatus { link })
}

/// C-R19: link `link` does not work. Timer mode, and the live thread is told to end that link,
/// if it still holds it (C-R21), which it then connects again with backoff.
fn broken(state: &mut State, link: u64, now_ms: i64) -> Vec<Cmd> {
    down(state, now_ms);
    vec![Cmd::DropLink { link }]
}

/// Spec §8, "What each event does". An account's change re-reads the views; `mode` fetches
/// `status`, whose words the header shows; `stopping` names its reason. Every event of a
/// provider job's run (auto's kinds, `usage`) also fetches `status`, since the daemon publishes
/// a job's state when its run ends, after its events (D5.4): `mode`'s own fetch can read the
/// state from before the run (C-R5). All but `mode`'s are debounced. An account's change other
/// than a `poll` reads History's timeline at once; `poll`, `usage` and `changed` pace it (T4-19).
/// A kind this TUI does not know reads nothing.
fn on_event(state: &mut State, event: Event, now_ms: i64) -> Vec<Cmd> {
    if state.daemon.link.is_none() || state.quitting.is_some() {
        return vec![];
    }
    note(state, &event, now_ms);
    match event {
        Event::Auto { .. } => {
            let views = event.changes_an_account();
            reread(state, now_ms, views, true);
            state.daemon.reread_timeline |= views && event.poll().is_none();
        }
        Event::Usage { .. } => reread(state, now_ms, true, true),
        Event::Changed => reread(state, now_ms, true, false),
        Event::Mode { .. } => return fetch(state, now_ms).into_iter().collect(),
        // C-R18: the link ends here, whether or not the daemon goes on to close it; the live
        // thread drops it and connects again with backoff.
        Event::Stopping { reason } => {
            down(state, now_ms);
            state.daemon.stopping = Some((reason, now_ms + STOPPING_MS));
            state.dirty = true;
        }
        Event::TokenIndex { provider, raw } => {
            let index = parse_token_index(&raw);
            return screens::tokens::index::on_token_index(state, &provider, &index, now_ms);
        }
        Event::Unknown { .. } => {}
    }
    vec![]
}

/// Auto's decisions, for the auto line (spec §5.3): a `no-switch`, `error` or `config-warning`
/// at its `ts`, or now when it has none. A `poll` is kept for the `no-switch` of its tick, and an
/// `all-exhausted` gives the `no-switch all-exhausted` before it its earliest reset.
fn note(state: &mut State, event: &Event, now_ms: i64) {
    let Event::Auto { provider, .. } = event else {
        return;
    };
    let windows = event.no_switch().and_then(|_| {
        let poll = state.daemon.polls.get(provider)?;
        windows(poll, state.view_of(&ProviderId::new(provider)))
    });
    let d = &mut state.daemon;
    if let Some(poll) = event.poll() {
        d.polls.insert(provider.clone(), poll);
        return;
    }
    if let Some(exhausted) = event.all_exhausted() {
        if let Some(Noted {
            what:
                NotedWhat::NoSwitch {
                    reason: NoSwitchReason::AllExhausted,
                    frees_at_ms,
                    ..
                },
            ..
        }) = d.noted.get_mut(provider)
        {
            *frees_at_ms = exhausted
                .earliest_reset_at
                .as_deref()
                .and_then(words::iso_ms);
            state.dirty = true;
        }
        return;
    }
    let what = if let Some(no_switch) = event.no_switch() {
        let Some(reason) = no_switch.reason.known() else {
            return;
        };
        NotedWhat::NoSwitch {
            reason,
            detail: no_switch.detail,
            used_pct: d.polls.get(provider).and_then(used_pct),
            windows,
            frees_at_ms: None,
        }
    } else if let Some(message) = event.error() {
        NotedWhat::Error(message)
    } else if let Some(message) = event.config_warning() {
        NotedWhat::Warning(message)
    } else {
        return;
    };
    let at_ms = event.at().and_then(words::iso_ms).unwrap_or(now_ms);
    d.noted.insert(provider.clone(), Noted { at_ms, what });
    state.dirty = true;
}

/// The usage of the account in use at `poll`: a binding window's (T3-29), which any window at
/// that usage gives. None when the poll names no window near the account's own usage.
fn used_pct(poll: &PollPayload) -> Option<f64> {
    let active = poll.active.as_ref()?.number;
    poll.binding_usage(active).map(|usage| usage as f64)
}

/// How far the counted window's percentage may be from `100 - headroomPct`: the poll's two figures
/// come from one reading with no rounding between, so they differ only by float error.
const SAME_READING: f64 = 1e-9;

/// The windows of the account in use at `poll`, as `tagteam list` names them, the one auto counts
/// marked (`words::windows_said`). The wire does not carry which window that is, so it is worked
/// out as the engine does it: `binding_window` over the account's windows, under `view`'s
/// `autoswitch.models`, `view` being the last accounts view. The view gives the windows' keys,
/// kinds and labels, and their order, which is the stored reading's and decides ties; the provider
/// lists its windows in a stable order. The poll gives their percentages, which the view, always
/// older than the tick, may have moved on from. None, so no window is marked and the sentence reads
/// as it does without windows, when the poll names no windows of the account, when `view` lacks the
/// account or its windows or has other keys than the poll, when the poll gives the account no
/// `headroomPct`, and when the window counted is not at the usage the poll gives the account
/// (`100 - headroomPct`): a `models` the view and the daemon do not share.
fn windows(poll: &PollPayload, view: Option<&ProviderAccounts>) -> Option<String> {
    let active = poll.active.as_ref()?.number;
    let polled = poll.windows_pct.get(&active)?;
    let view = view?;
    let read = view
        .accounts
        .iter()
        .find(|a| a.row.position == active)?
        .usage
        .windows
        .as_deref()?;
    if polled.len() != read.len() {
        return None;
    }
    let current: Vec<Window> = read
        .iter()
        .map(|(w, _)| {
            let (_, pct) = polled.iter().find(|(key, _)| key == &w.key)?;
            Some(Window {
                pct: *pct,
                ..w.clone()
            })
        })
        .collect::<Option<_>>()?;
    let counting = binding_window(&current, &view.models)?;
    let usage = 100.0 - (*poll.headroom_pct.get(&active)?)?;
    if (counting.pct - usage).abs() >= SAME_READING {
        return None;
    }
    tagteam_engine::words::windows_said(
        polled.iter().map(|(k, pct)| (k.as_str(), *pct)),
        Some(&counting.key),
    )
}

/// What an event asks for is read `REREAD_MS` from the first event that asks, once for every
/// event until then: the views, `status`, or both.
fn reread(state: &mut State, now_ms: i64, views: bool, status: bool) {
    let d = &mut state.daemon;
    d.reread_ms.get_or_insert(now_ms + REREAD_MS);
    d.reread_views |= views;
    d.reread_status |= status;
}

/// The daemon lane's `status` for link `link` (spec §8): the link's one out (C-R22). A reply
/// asked for under another link is dropped (C-R21). A link's first reply starts live mode, and
/// says who collects (C-R20); the `status` asked for meanwhile is asked now. One that failed
/// ends the link (C-R19): the daemon holds it but does not answer.
pub(crate) fn on_status(
    state: &mut State,
    link: u64,
    result: Result<Status, StatusError>,
    now_ms: i64,
) -> Vec<Cmd> {
    let d = &mut state.daemon;
    if d.link != Some(link) {
        return vec![];
    }
    d.status_due_ms = None;
    match result {
        Ok(status) => {
            d.status = Some(status);
            d.unsettled_until = None;
            state.dirty = true;
            if std::mem::take(&mut d.lost) {
                state.toast(ToastKind::Done, "connected to the daemon", now_ms);
            }
            if keeps_index(state) {
                screens::tokens::index::on_daemon_index(state);
            }
            if !std::mem::take(&mut state.daemon.status_again) {
                return vec![];
            }
            fetch(state, now_ms).into_iter().collect()
        }
        Err(_) => broken(state, link, now_ms),
    }
}

/// Spec §6.4, C-R12: whether the daemon keeps the Tokens provider's index current. In live
/// mode, the daemon runs a job for that provider (its index job serves the providers it runs,
/// D13, which `status` names in `providers`), and its `status.jobs` has the index job, whose
/// last run did not fail for this provider or for the store (its `lastError` names one or the
/// other, as `<provider>: …` or `store…`). A daemon that runs other providers only, or whose job
/// fails, leaves the TUI to refresh on its timer, which shows its own refresh's failure.
pub(crate) fn keeps_index(state: &State) -> bool {
    let provider = state.tokens.provider();
    let Some(status) = state.daemon.status() else {
        return false;
    };
    // C-R8: a job waiting for a newer binary (`schema`) runs no index job.
    let Some(job) = state.daemon.job(provider) else {
        return false;
    };
    if job.waits_for_upgrade() {
        return false;
    }
    status.jobs.iter().any(|job| {
        job.name == TOKEN_INDEX
            && job
                .last_error
                .as_deref()
                .is_none_or(|e| names_another_provider(e, provider))
    })
}

/// Whether the index job's `lastError` begins `<p>: ` for a provider `p` other than `provider`.
/// Any other wording, a store error among them, is not known to leave this provider's index
/// current.
fn names_another_provider(error: &str, provider: &ProviderId) -> bool {
    error.split_once(": ").is_some_and(|(p, _)| {
        !p.is_empty()
            && p != provider.as_str()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

/// Spec §6.3, §7.4 and §8 on the UI's tick: the end of `stopping`'s words, and of the first
/// collection's wait (C-R20); a link whose `status` did not come (C-R19); the events' debounced
/// reads; `P`'s fetch of `status`; and every 60 s, `status`, and in live mode `accounts` (and
/// `history` when History is open, paced as T4-19 says).
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let d = &mut state.daemon;
    if d.stopping
        .as_ref()
        .is_some_and(|(_, until)| now_ms >= *until)
    {
        d.stopping = None;
        state.dirty = true;
    }
    if d.unsettled_until.is_some_and(|until| now_ms >= until) {
        d.unsettled_until = None;
        state.dirty = true;
    }
    let Some(link) = d.link.filter(|_| state.quitting.is_none()) else {
        return vec![];
    };
    if d.status_due_ms.is_some_and(|due| now_ms >= due) {
        return broken(state, link, now_ms);
    }
    let (mut views, mut all, mut fetch_now) = (false, false, false);
    let mut timeline = Reread::Paced;
    if d.reread_ms.is_some_and(|at| now_ms >= at) {
        d.reread_ms = None;
        views = std::mem::take(&mut d.reread_views);
        all = std::mem::take(&mut d.reread_all);
        fetch_now = std::mem::take(&mut d.reread_status);
        if std::mem::take(&mut d.reread_timeline) {
            timeline = Reread::Now;
        }
    }
    if d.recheck_ms.is_some_and(|at| now_ms >= at) {
        d.recheck_ms = None;
        fetch_now = true;
    }
    if now_ms >= d.next_poll_ms {
        d.next_poll_ms = now_ms + POLL_MS;
        fetch_now = true;
        views |= d.live();
    }
    let mut cmds = Vec::new();
    if fetch_now {
        cmds.extend(fetch(state, now_ms));
    }
    if views {
        cmds.extend(state.reload_views_as(now_ms, timeline));
        if all && state.screen == Screen::Tokens {
            cmds.extend(screens::tokens::requery(state, now_ms));
        }
    }
    cmds
}

/// Spec §6.3: whether something else collects for `provider`. In live mode, that is a daemon
/// job in `auto` or `collect`, or one waiting for another engine, which collects for itself. A
/// job that is `idle` or waits for a newer binary (`schema`), and a provider the daemon runs no
/// job for, leave it to the TUI.
pub(crate) fn collected_elsewhere(state: &State, provider: &ProviderId) -> bool {
    state
        .daemon
        .job(provider)
        .is_some_and(StatusProvider::collects_for_itself)
}

/// The accounts the timer offers the collector (spec §6.2, §6.3): those shown whose provider
/// nothing else collects for. In timer mode, every one shown.
pub(crate) fn to_collect(state: &State) -> Vec<AccountId> {
    let Some(snap) = state.accounts.snapshot() else {
        return vec![];
    };
    snap.providers
        .iter()
        .filter(|p| !collected_elsewhere(state, &p.provider))
        .flat_map(|p| p.accounts.iter().map(|v| v.row.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use tagteam_core::{Pace, WindowKind};
    use tagteam_daemon::client::ClientError;
    use tagteam_engine::views::{UsageStatus, UsageView};

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::{drawn, update};

    fn event(e: Event) -> Msg {
        Msg::Live(Live::Event(e))
    }

    /// One of auto's kinds (§11.4), for Claude Code.
    fn auto(kind: &str) -> Event {
        Event::Auto {
            kind: kind.into(),
            provider: "claude-code".into(),
            raw: json!({"schemaVersion": 1, "event": kind, "ts": "2026-09-21T14:13:20Z"}),
        }
    }

    fn usage() -> Event {
        Event::Usage {
            provider: "claude-code".into(),
            updated: vec![1],
            fetch_errors: BTreeMap::new(),
        }
    }

    fn mode(mode: &str) -> Event {
        Event::Mode {
            provider: "claude-code".into(),
            mode: Wire::from_wire(mode),
            waiting_for: None,
            holder_pid: None,
            backoff: None,
        }
    }

    /// Claude Code's job switching accounts.
    fn auto_on() -> Status {
        daemon_status(vec![daemon_job("claude-code", "auto", true)], vec![])
    }

    /// The reads and fetches among `cmds`; the token index's timers are left out.
    fn reads(cmds: Vec<Cmd>) -> Vec<Cmd> {
        cmds.into_iter()
            .filter(|c| {
                matches!(
                    c,
                    Cmd::LoadAccounts { .. }
                        | Cmd::LoadTimeline { .. }
                        | Cmd::LoadReport { .. }
                        | Cmd::CollectDue { .. }
                        | Cmd::FetchStatus { .. }
                )
            })
            .collect()
    }

    fn tick(state: &mut State, at_ms: i64) -> Vec<Cmd> {
        reads(update(state, Msg::Tick, at_ms))
    }

    /// `tick`, with each `status` it asks for answered as a working daemon answers it.
    fn tick_answered(state: &mut State, at_ms: i64) -> Vec<Cmd> {
        let cmds = tick(state, at_ms);
        for c in &cmds {
            if let Cmd::FetchStatus { link } = c {
                let result = Ok(auto_on());
                update(
                    state,
                    Msg::DaemonStatus {
                        link: *link,
                        result,
                    },
                    at_ms,
                );
            }
        }
        cmds
    }

    /// The reads `cmds` asks for, by name, in order.
    fn names(cmds: &[Cmd]) -> Vec<&'static str> {
        cmds.iter()
            .map(|c| match c {
                Cmd::LoadAccounts { .. } => "accounts",
                Cmd::LoadTimeline { .. } => "history",
                Cmd::LoadReport { .. } => "report",
                Cmd::CollectDue { .. } => "collect",
                Cmd::FetchStatus { .. } => "status",
                _ => "other",
            })
            .collect()
    }

    /// On `screen`, with `n` accounts read, History on all of them.
    fn showing(screen: Screen, n: u32) -> State {
        let mut state = fresh(screen);
        state.accounts.apply(snapshot(many(n)));
        state
    }

    #[test]
    fn a_link_fetches_the_status_at_once_and_reads_every_view_shown_250_ms_later() {
        // Spec §8, "Connecting": the snapshot follows the subscription, and the first frame's
        // own read is left to finish (§6.1).
        for (screen, want) in [
            (Screen::Accounts, vec!["accounts"]),
            (Screen::History, vec!["accounts", "history"]),
            (Screen::Tokens, vec!["accounts", "report"]),
        ] {
            let mut state = showing(screen, 1);
            let cmds = update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS);
            assert_eq!(cmds, [Cmd::FetchStatus { link: 1 }], "{screen:?}");
            assert!(!state.daemon.live(), "timer mode until the status comes");
            assert!(tick(&mut state, NOW_MS + 249).is_empty(), "{screen:?}");
            assert_eq!(names(&tick(&mut state, NOW_MS + 250)), want, "{screen:?}");
            assert!(tick(&mut state, NOW_MS + 500).is_empty(), "once");
        }
    }

    #[test]
    fn the_first_status_starts_live_mode_and_a_reply_for_another_link_is_dropped() {
        let mut state = fresh(Screen::Accounts);
        let up = |state: &mut State, link| {
            let cmds = update(state, Msg::Live(Live::Up { link }), NOW_MS);
            assert_eq!(cmds, [Cmd::FetchStatus { link }]);
            link
        };
        let reply = |link, result| Msg::DaemonStatus { link, result };
        let first = up(&mut state, 1);
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        let second = up(&mut state, 2);
        update(&mut state, reply(first, Ok(auto_on())), NOW_MS);
        assert!(!state.daemon.live(), "the reply of a link that went down");
        state.dirty = false;
        update(&mut state, reply(second, Ok(auto_on())), NOW_MS);
        assert!(state.daemon.live() && state.dirty);
        assert_eq!(
            state.daemon.job(&cc()).map(|j| j.mode.as_str()),
            Some("auto")
        );
    }

    #[test]
    fn losing_the_daemon_toasts_once_and_connecting_again_toasts_once() {
        // T2-10: the first link at start-up is no change the person noticed; a live link lost
        // is, and so is the first `status` of the link after it.
        let toast = |state: &State| state.toast.as_ref().map(|t| (t.kind, t.text.clone()));
        let lost = (
            ToastKind::Warn,
            "lost the daemon; tagteam updates on its own every 30 s".to_owned(),
        );
        let mut state = fresh(Screen::Accounts);
        let first = go_live(&mut state, auto_on());
        assert_eq!(toast(&state), None, "the first link's status says nothing");
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(toast(&state), Some(lost.clone()));
        state.toast = None;
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(toast(&state), None, "once: no link was left to lose");
        // A link that comes up and fails before its `status` was never live.
        update(&mut state, Msg::Live(Live::Up { link: 2 }), NOW_MS);
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(toast(&state), None);
        update(&mut state, Msg::Live(Live::Up { link: 3 }), NOW_MS);
        assert_eq!(toast(&state), None, "a link up is not yet a daemon heard");
        let reply = |link, result| Msg::DaemonStatus { link, result };
        update(&mut state, reply(first, Ok(auto_on())), NOW_MS);
        assert_eq!(toast(&state), None, "a reply for a link long gone");
        update(&mut state, reply(3, Ok(auto_on())), NOW_MS);
        assert_eq!(
            toast(&state),
            Some((ToastKind::Done, "connected to the daemon".to_owned()))
        );
        state.toast = None;
        update(&mut state, reply(3, Ok(auto_on())), NOW_MS);
        assert_eq!(toast(&state), None, "once: only the link's first status");
        // The next loss and return say so again.
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(toast(&state), Some(lost));
        // No toast once the shutdown has begun.
        let mut quitting = fresh(Screen::Accounts);
        go_live(&mut quitting, auto_on());
        quitting.quitting = Some(0);
        update(&mut quitting, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(toast(&quitting), None);
    }

    #[test]
    fn a_link_that_goes_down_is_timer_mode_and_a_late_reply_does_not_undo_that() {
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        state.dirty = false;
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert!(!state.daemon.live() && state.dirty);
        assert!(state.daemon.status().is_none());
        let late = Msg::DaemonStatus {
            link,
            result: Ok(auto_on()),
        };
        update(&mut state, late, NOW_MS);
        assert!(!state.daemon.live());
        // The link's own timers stop: its re-read and its 60 s fetch.
        assert!(tick(&mut state, NOW_MS + 250).is_empty());
        let at_60 = tick(&mut state, NOW_MS + 60_000);
        assert!(!names(&at_60).contains(&"status"), "{at_60:?}");
        state.dirty = false;
        update(&mut state, Msg::Live(Live::Down), NOW_MS + 61_000);
        assert!(!state.dirty, "a second Down changes nothing");
    }

    #[test]
    fn an_event_reads_what_it_affects_250_ms_later_once_for_every_event_until_then() {
        // Spec §8: `poll`, `switch`, `usage`, `changed` and the quarantine events re-read
        // `accounts`, and `history` while it is open, debounced (§6.3); within 30 s of its last
        // read, `poll`, `usage` and `changed` leave `history` to a paced read (T4-19). Every
        // event of a provider job's run also fetches `status` (C-R5); `changed` is another
        // process's. A kind this TUI does not know reads nothing.
        let mut state = showing(Screen::History, 1);
        go_live(&mut state, auto_on());
        tick(&mut state, NOW_MS + 250);
        let unknown = Event::Unknown {
            kind: "added-later".into(),
            raw: json!({"event": "added-later"}),
        };
        let all = vec!["status", "accounts", "history"];
        let paced = vec!["status", "accounts"];
        let mut at = NOW_MS + 1_000;
        for (e, want) in [
            (auto("poll"), paced.clone()),
            (auto("switch"), all.clone()),
            (auto("account-quarantined"), all.clone()),
            (auto("account-unquarantined"), all.clone()),
            (usage(), paced.clone()),
            (Event::Changed, vec!["accounts"]),
            (auto("no-switch"), vec!["status"]),
            (auto("all-exhausted"), vec!["status"]),
            (auto("error"), vec!["status"]),
            (auto("sleep"), vec!["status"]),
            (auto("config-warning"), vec!["status"]),
            (unknown, vec![]),
        ] {
            assert!(update(&mut state, event(e.clone()), at).is_empty(), "{e:?}");
            assert!(tick(&mut state, at + 249).is_empty(), "{e:?}");
            let read = tick_answered(&mut state, at + 250);
            assert_eq!(names(&read), want, "{e:?}");
            at += 1_000;
        }
        // A burst within the 250 ms: one batch for them all.
        for (i, e) in [auto("poll"), Event::Changed, auto("no-switch")]
            .into_iter()
            .enumerate()
        {
            update(&mut state, event(e), at + i as i64 * 20);
        }
        assert_eq!(names(&tick_answered(&mut state, at + 250)), paced);
        assert!(tick(&mut state, at + 500).is_empty());
    }

    #[test]
    fn in_live_mode_usage_re_reads_the_timeline_at_most_every_30_s_and_a_switch_at_once() {
        // T4-19: `usage`, `changed`, `poll` and the 60 s live poll pace History's re-read, at
        // most every 30 s, one inside 30 s of the last running 30 s after it; a `switch` reads
        // it at once.
        let mut state = showing(Screen::History, 1);
        go_live(&mut state, auto_on());
        // The link's own re-read asks for the timeline at `t`.
        let t = NOW_MS + REREAD_MS;
        assert_eq!(
            names(&tick_answered(&mut state, t)),
            ["accounts", "history"]
        );
        update(&mut state, event(usage()), t + 10_000);
        assert_eq!(
            names(&tick_answered(&mut state, t + 10_250)),
            ["status", "accounts"]
        );
        assert!(tick(&mut state, t + 29_999).is_empty());
        assert_eq!(names(&tick(&mut state, t + 30_000)), ["history"]);
        assert!(tick(&mut state, t + 30_001).is_empty(), "once");
        update(&mut state, event(auto("switch")), t + 31_000);
        assert_eq!(
            names(&tick_answered(&mut state, t + 31_250)),
            ["status", "accounts", "history"]
        );
        // `changed` and `poll` pace as `usage` does, from the switch's read.
        update(&mut state, event(Event::Changed), t + 32_000);
        assert_eq!(names(&tick_answered(&mut state, t + 32_250)), ["accounts"]);
        update(&mut state, event(auto("poll")), t + 33_000);
        assert_eq!(
            names(&tick_answered(&mut state, t + 33_250)),
            ["status", "accounts"]
        );
        // So does the 60 s poll, due at `NOW_MS + 60_000`.
        assert_eq!(
            names(&tick_answered(&mut state, t + 61_249)),
            ["status", "accounts"]
        );
        assert_eq!(names(&tick(&mut state, t + 61_250)), ["history"]);
        // In timer mode the 30 s timer reads the timeline at once.
        let mut state = showing(Screen::History, 1);
        let read = update(&mut state, ch(']'), NOW_MS + 10_000);
        assert_eq!(names(&read), ["history"]);
        assert_eq!(
            names(&tick(&mut state, NOW_MS + 30_000)),
            ["accounts", "history", "collect"]
        );
    }

    #[test]
    fn a_poll_without_a_binding_window_leaves_the_sentence_without_its_percentage() {
        // T3-29: the no-switch sentence's usage is the binding window's, from the parser.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(3)));
        go_live(&mut state, auto_on());
        let auto_with = |kind: &str, fields: serde_json::Value| {
            let mut raw = json!({"schemaVersion": 1, "event": kind, "ts": "2026-09-21T14:13:20Z"});
            for (k, v) in fields.as_object().unwrap() {
                raw[k] = v.clone();
            }
            event(Event::Auto {
                kind: kind.into(),
                provider: "claude-code".into(),
                raw,
            })
        };
        let below = || auto_with("no-switch", json!({"reason": "below-threshold"}));
        let poll = json!({"active": {"number": 1}, "headroomPct": {"1": 23.0}});
        // The auto line, `w why` ending one column in (§5.3).
        let hinted = |left: &str| {
            format!(
                "{left}{}w why",
                " ".repeat(74 - crate::view::width_of(left))
            )
        };
        update(&mut state, auto_with("poll", poll), NOW_MS);
        update(&mut state, below(), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[1],
            hinted("   14:13  no switch needed (switches at 90%)")
        );
        let unmatched = json!({"active": {"number": 1}, "headroomPct": {"1": 23.0},
            "windowsPct": {"1": {"5h": 40.0}}});
        let mut accounts = many(3);
        accounts[0].usage = windowed(&[("5h", "5h", WindowKind::Short, 40.0)]);
        state.accounts.apply(snapshot(accounts));
        update(&mut state, auto_with("poll", unmatched), NOW_MS);
        update(&mut state, below(), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[1],
            hinted("   14:13  no switch needed (switches at 90%)"),
            "no window near the account's usage"
        );
        // The view's account holds the poll's windows: its binding window is 7d.
        let mut accounts = many(3);
        accounts[0].usage = windowed(&[
            ("5h", "5h", WindowKind::Short, 40.0),
            ("7d", "7d", WindowKind::Long, 77.0),
        ]);
        state.accounts.apply(snapshot(accounts));
        let poll = json!({"active": {"number": 1}, "headroomPct": {"1": 23.0},
            "windowsPct": {"1": {"5h": 40.0, "7d": 77.0}}});
        update(&mut state, auto_with("poll", poll), NOW_MS);
        update(&mut state, below(), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[1],
            hinted("   14:13  no switch needed (5h 40% · 7d 77%◂, switches at 90%)"),
            "the binding window's usage"
        );
    }

    /// A reading of `windows` (key, label, kind, pct), decision-grade.
    fn windowed(windows: &[(&str, &str, WindowKind, f64)]) -> UsageView {
        UsageView {
            windows: Some(
                windows
                    .iter()
                    .map(|(key, label, kind, pct)| {
                        (
                            window(key, label, *kind, *pct, Some(3_600)),
                            Pace::default(),
                        )
                    })
                    .collect(),
            ),
            ..read(60, 0.0, 0.0, false)
        }
    }

    /// A state following the daemon whose view holds account 1 with `windows`, under `models`.
    fn viewing(models: &[&str], windows: &[(&str, &str, WindowKind, f64)]) -> State {
        let mut accounts = many(2);
        accounts[0].usage = windowed(windows);
        let mut snap = snapshot(accounts);
        snap.providers[0].models = models.iter().map(|m| (*m).to_owned()).collect();
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        go_live(&mut state, auto_on());
        state
    }

    /// Account `active`'s poll at 47% headroom with `polled` windows, then a `below-threshold`,
    /// and the auto line they make.
    fn auto_line(state: &mut State, active: u32, polled: serde_json::Value) -> String {
        auto_line_at(state, active, 47.0, polled)
    }

    /// `auto_line` with the poll's headroom for the account in use.
    fn auto_line_at(
        state: &mut State,
        active: u32,
        headroom: f64,
        polled: serde_json::Value,
    ) -> String {
        let auto_with = |kind: &str, fields: serde_json::Value| {
            let mut raw = json!({"schemaVersion": 1, "event": kind, "ts": "2026-09-21T14:13:20Z"});
            for (k, v) in fields.as_object().unwrap() {
                raw[k] = v.clone();
            }
            event(Event::Auto {
                kind: kind.into(),
                provider: "claude-code".into(),
                raw,
            })
        };
        let n = active.to_string();
        let poll = json!({"active": {"number": active}, "headroomPct": {n.clone(): headroom},
            "windowsPct": {n: polled}});
        update(state, auto_with("poll", poll), NOW_MS);
        update(
            state,
            auto_with("no-switch", json!({"reason": "below-threshold"})),
            NOW_MS,
        );
        draw(state, 120, 24)[1].trim_end().to_owned()
    }

    const OPUS_TIE: [(&str, &str, WindowKind, f64); 4] = [
        ("5h", "5h", WindowKind::Short, 9.0),
        ("7d", "7d", WindowKind::Long, 20.0),
        ("scoped:Fable", "Fable", WindowKind::Scoped, 53.0),
        ("scoped:Opus", "Opus", WindowKind::Scoped, 53.0),
    ];

    #[test]
    fn the_auto_line_marks_the_window_auto_counts_under_the_models_setting() {
        // `models = ["Opus"]`: Fable ties Opus at 53 and comes first, but only Opus counts.
        let mut state = viewing(&["Opus"], &OPUS_TIE);
        let polled = json!({"5h": 9.0, "7d": 20.0, "scoped:Fable": 53.0, "scoped:Opus": 53.0});
        let line = auto_line(&mut state, 1, polled);
        assert!(
            line.contains(
                "no switch needed (5h 9% · 7d 20% · Fable 53% · Opus 53%◂, switches at 90%)"
            ),
            "{line}"
        );
        // Under `["Fable"]` it is Fable.
        let mut state = viewing(&["Fable"], &OPUS_TIE);
        let polled = json!({"5h": 9.0, "7d": 20.0, "scoped:Fable": 53.0, "scoped:Opus": 53.0});
        let line = auto_line(&mut state, 1, polled);
        assert!(line.contains("Fable 53%◂ · Opus 53%,"), "{line}");
    }

    #[test]
    fn a_view_that_is_not_the_polls_leaves_the_auto_line_without_windows() {
        let tie = json!({"5h": 9.0, "7d": 20.0, "scoped:Fable": 53.0, "scoped:Opus": 53.0});
        let bare = "no switch needed (53%, switches at 90%)";
        // The view has not read the account's windows yet.
        let mut state = viewing(&["Opus"], &OPUS_TIE);
        let mut snap = snapshot(many(2));
        snap.providers[0].models = vec!["Opus".into()];
        snap.providers[0].accounts[0].usage = unread(UsageStatus::Unavailable, None, None);
        state.accounts.apply(snap);
        let line = auto_line(&mut state, 1, tie.clone());
        assert!(line.contains(bare) && !line.contains('◂'), "{line}");
        // The view lacks a window the poll names.
        let mut state = viewing(&["Opus"], &OPUS_TIE[..3]);
        let line = auto_line(&mut state, 1, tie.clone());
        assert!(line.contains(bare) && !line.contains('◂'), "{line}");
        // The view holds a window the poll lacks.
        let mut state = viewing(&["Opus"], &OPUS_TIE);
        let fewer = json!({"5h": 9.0, "7d": 20.0, "scoped:Opus": 53.0});
        let line = auto_line(&mut state, 1, fewer);
        assert!(line.contains(bare) && !line.contains('◂'), "{line}");
        // The same number of windows under other keys.
        let mut state = viewing(&["Opus"], &OPUS_TIE);
        let renamed = json!({"5h": 9.0, "7d": 20.0, "scoped:Fable": 53.0, "scoped:Sonnet": 53.0});
        let line = auto_line(&mut state, 1, renamed);
        assert!(line.contains(bare) && !line.contains('◂'), "{line}");
        // The poll's account is not in the view.
        let mut state = viewing(&["Opus"], &OPUS_TIE);
        let line = auto_line(&mut state, 9, tie);
        assert!(line.contains(bare) && !line.contains('◂'), "{line}");
    }

    #[test]
    fn a_view_older_than_the_poll_still_marks_the_window_with_the_polls_percentages() {
        // The view is re-read 250 ms after a `poll`, so a `no-switch` meets the view before it:
        // the same keys, at percentages that have moved.
        let mut state = viewing(&["Opus"], &OPUS_TIE);
        let moved = json!({"5h": 9.6, "7d": 30.0, "scoped:Fable": 53.0, "scoped:Opus": 53.0});
        let line = auto_line(&mut state, 1, moved);
        assert!(
            line.contains(
                "no switch needed (5h 10% · 7d 30% · Fable 53% · Opus 53%◂, switches at 90%)"
            ),
            "{line}"
        );
        // The view's 5h was 9.4 and rounded to 9; the poll's 9.6 shows as 10.
        let mut windows = OPUS_TIE;
        windows[0].3 = 9.4;
        let mut state = viewing(&["Opus"], &windows);
        let polled = json!({"5h": 9.6, "7d": 20.0, "scoped:Fable": 53.0, "scoped:Opus": 53.0});
        let line = auto_line(&mut state, 1, polled);
        assert!(line.contains("5h 10% · 7d 20%"), "{line}");
        assert!(line.contains("Opus 53%◂"), "{line}");
        // The window that counts is chosen from the poll's percentages: 7d overtook the scoped
        // ones in the poll.
        let mut state = viewing(&["Opus"], &OPUS_TIE);
        let overtaken = json!({"5h": 9.0, "7d": 53.0, "scoped:Fable": 40.0, "scoped:Opus": 40.0});
        let line = auto_line(&mut state, 1, overtaken);
        assert!(line.contains("7d 53%◂ · Fable 40% · Opus 40%,"), "{line}");
    }

    #[test]
    fn a_models_setting_the_view_does_not_share_with_the_daemon_marks_nothing() {
        // The daemon has reloaded `["Opus"]`, so auto counted Opus at 53 (headroom 47); the view
        // still has `["all"]` and would count Fable at 60.
        let mut windows = OPUS_TIE;
        windows[2].3 = 60.0;
        let mut state = viewing(&["all"], &windows);
        let polled = json!({"5h": 9.0, "7d": 20.0, "scoped:Fable": 60.0, "scoped:Opus": 53.0});
        let line = auto_line(&mut state, 1, polled);
        assert!(
            line.contains("no switch needed (53%, switches at 90%)") && !line.contains('◂'),
            "{line}"
        );
    }

    #[test]
    fn a_near_tie_is_decided_by_the_polls_percentages_not_the_views() {
        // The view had Fable ahead (53.2 over 52.9); the poll has Opus ahead.
        let windows = [
            ("scoped:Fable", "Fable", WindowKind::Scoped, 53.2),
            ("scoped:Opus", "Opus", WindowKind::Scoped, 52.9),
        ];
        let mut state = viewing(&["all"], &windows);
        let polled = json!({"scoped:Fable": 52.9, "scoped:Opus": 53.2});
        let line = auto_line_at(&mut state, 1, 46.8, polled);
        assert!(
            line.contains("no switch needed (Fable 53% · Opus 53%◂, switches at 90%)"),
            "{line}"
        );
    }

    #[test]
    fn a_models_setting_the_view_does_not_share_marks_nothing_on_a_near_tie_too() {
        // The daemon has `["Opus"]` (Opus 52.48, headroom 47.52); the view has `["all"]` and
        // counts Fable at 52.52, 0.04 off the usage: a tolerance that wide would mark it.
        let windows = [
            ("scoped:Fable", "Fable", WindowKind::Scoped, 52.52),
            ("scoped:Opus", "Opus", WindowKind::Scoped, 52.48),
        ];
        let mut state = viewing(&["all"], &windows);
        let polled = json!({"scoped:Fable": 52.52, "scoped:Opus": 52.48});
        let line = auto_line_at(&mut state, 1, 47.52, polled);
        assert!(
            line.contains("switches at 90%") && !line.contains('◂'),
            "{line}"
        );
    }

    #[test]
    fn a_view_with_fewer_windows_than_the_poll_marks_nothing_even_when_the_tie_agrees() {
        // The view lacks Opus; the poll has Fable and Opus tied. Without the length check Fable
        // would be counted from the view's windows and pass the usage check.
        let windows = [("scoped:Fable", "Fable", WindowKind::Scoped, 53.04)];
        let mut state = viewing(&["all"], &windows);
        let polled = json!({"scoped:Fable": 53.04, "scoped:Opus": 53.04});
        let line = auto_line_at(&mut state, 1, 46.96, polled);
        assert!(
            line.contains("switches at 90%") && !line.contains('◂'),
            "{line}"
        );
    }

    #[test]
    fn a_mode_event_fetches_the_status_and_its_fields_are_not_applied() {
        // Spec §8: `status` is the authority; an event is a reason to fetch it.
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        let cmds = update(&mut state, event(mode("idle")), NOW_MS + 1_000);
        assert_eq!(cmds, [Cmd::FetchStatus { link }]);
        assert_eq!(
            state.daemon.job(&cc()).map(|j| j.mode.as_str()),
            Some("auto"),
            "until the status says otherwise"
        );
    }

    #[test]
    fn every_60_s_a_link_fetches_the_status_and_in_live_mode_reads_the_views() {
        // Spec §6.3: a missed event, or a setting changed elsewhere, costs at most a minute.
        let mut state = showing(Screen::History, 1);
        go_live(&mut state, auto_on());
        tick(&mut state, NOW_MS + 250);
        assert!(tick(&mut state, NOW_MS + 59_999).is_empty());
        assert_eq!(
            names(&tick_answered(&mut state, NOW_MS + 60_000)),
            ["status", "accounts", "history"]
        );
        assert!(tick(&mut state, NOW_MS + 119_999).is_empty());
        assert_eq!(names(&tick(&mut state, NOW_MS + 120_000))[0], "status");
        // A link whose `status` does not come is timer mode, and is dropped after 5 s (C-R19):
        // the 30 s timer reads the views and collects as it does without a daemon.
        let mut state = showing(Screen::Accounts, 1);
        update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS);
        tick(&mut state, NOW_MS + 250);
        let dropped = update(&mut state, Msg::Tick, NOW_MS + 5_000);
        assert_eq!(dropped, [Cmd::DropLink { link: 1 }]);
        assert_eq!(
            names(&tick(&mut state, NOW_MS + 30_000)),
            ["accounts", "collect"]
        );
        assert_eq!(
            names(&tick(&mut state, NOW_MS + 60_000)),
            ["accounts", "collect"]
        );
    }

    /// Claude Code's accounts 1 and 2, and FakeAgent's account 3.
    fn two_providers() -> crate::source::AccountsSnapshot {
        let fake = tagteam_core::ProviderId::new("fake-agent");
        let mut third = account(3, "c@x.co", read(60, 10.0, 20.0, false));
        third.row.provider = fake.clone();
        let mut snap = snapshot(many(2));
        snap.providers.push(ProviderAccounts {
            provider: fake,
            active_position: None,
            forecast: tagteam_engine::views::forecast_of(
                std::slice::from_ref(&third),
                &tagteam_engine::switch::LiveLogin::None,
                &[],
            ),
            accounts: vec![third],
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
            models: vec![],
            live: tagteam_engine::switch::LiveLogin::None,
        });
        snap
    }

    #[test]
    fn the_timer_collects_only_for_the_providers_nothing_else_collects_for() {
        // Spec §6.3: `auto`, `collect`, and `waiting` for `engine-lock` collect elsewhere;
        // `idle`, `waiting` for `schema`, and a provider with no job do not.
        for (fake_job, collected) in [
            (Some(("auto", None)), false),
            (Some(("collect", None)), false),
            (Some(("waiting", Some("engine-lock"))), false),
            (Some(("waiting", Some("schema"))), true),
            (Some(("idle", None)), true),
            (None, true),
        ] {
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(two_providers());
            let mut jobs = vec![daemon_job("claude-code", "auto", true)];
            if let Some((mode, waiting_for)) = fake_job {
                let mut job = daemon_job("fake-agent", mode, false);
                job.waiting_for = waiting_for.map(Wire::from_wire);
                jobs.push(job);
            }
            go_live(&mut state, daemon_status(jobs, vec![]));
            tick(&mut state, NOW_MS + 250);
            let at_30 = tick(&mut state, NOW_MS + 30_000);
            let want: Vec<Cmd> = if collected {
                vec![Cmd::CollectDue {
                    r#gen: 1,
                    accounts: vec![id(3)],
                }]
            } else {
                vec![]
            };
            assert_eq!(at_30, want, "{fake_job:?}");
            // `r` still collects every account shown, as `list` would.
            let r = update(&mut state, ch('r'), NOW_MS + 31_000);
            let all = r.iter().find_map(|c| match c {
                Cmd::CollectDue { accounts, .. } => Some(accounts.clone()),
                _ => None,
            });
            assert_eq!(all, Some(vec![id(1), id(2), id(3)]), "{fake_job:?}");
        }
        // Without a link, the timer collects for every provider.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(two_providers());
        let at_30 = tick(&mut state, NOW_MS + 30_000);
        assert!(
            matches!(at_30.as_slice(), [Cmd::LoadAccounts { .. }, Cmd::CollectDue { accounts, .. }] if accounts.len() == 3),
            "{at_30:?}"
        );
    }

    #[test]
    fn the_first_collection_leaves_out_what_the_daemon_collects_for() {
        // Spec §1.1 criterion 4, §6.1, §6.3.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(two_providers());
        go_live(&mut state, auto_on());
        let first = reads(drawn(&mut state));
        assert_eq!(
            first,
            [Cmd::CollectDue {
                r#gen: 1,
                accounts: vec![id(3)]
            }]
        );
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(2)));
        go_live(&mut state, auto_on());
        assert!(
            reads(drawn(&mut state)).is_empty(),
            "the daemon collects for all"
        );
        assert_eq!(state.gens.collect, 0);
        update(&mut state, Msg::Live(Live::Down), NOW_MS + 1_000);
        assert_eq!(
            reads(drawn(&mut state)),
            [Cmd::CollectDue {
                r#gen: 1,
                accounts: vec![id(1), id(2)]
            }],
            "without the daemon, the next frame collects"
        );
    }

    /// A TUI that follows a daemon, on Accounts with `accounts` read: its first collection
    /// waits until it knows who collects (C-R20).
    fn awaiting(accounts: crate::source::AccountsSnapshot) -> State {
        let mut state = fresh(Screen::Accounts);
        state.daemon.await_daemon(NOW_MS);
        state.accounts.apply(accounts);
        state
    }

    #[test]
    fn the_first_collection_waits_for_the_daemon_s_first_status() {
        // C-R20: the accounts come, and a frame is drawn, before `status` says who collects.
        let mut state = awaiting(two_providers());
        assert!(
            reads(drawn(&mut state)).is_empty(),
            "drawn, and nothing collected"
        );
        let cmds = update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS + 10);
        assert_eq!(cmds, [Cmd::FetchStatus { link: 1 }]);
        assert!(
            reads(drawn(&mut state)).is_empty(),
            "linked, `status` to come"
        );
        let reply = Msg::DaemonStatus {
            link: 1,
            result: Ok(auto_on()),
        };
        update(&mut state, reply, NOW_MS + 20);
        assert_eq!(
            reads(drawn(&mut state)),
            [Cmd::CollectDue {
                r#gen: 1,
                accounts: vec![id(3)]
            }],
            "only what the daemon leaves"
        );
    }

    #[test]
    fn the_first_collection_goes_ahead_when_no_daemon_answers_or_after_2_s() {
        // C-R20: `NoDaemon` or a mismatch settles it at once; a link whose `status` has not come
        // settles it 2 s after start.
        for answer in [Some(Live::Down), Some(Live::Mismatch { server: 2 }), None] {
            let mut state = awaiting(snapshot(many(2)));
            assert!(reads(drawn(&mut state)).is_empty(), "{answer:?}");
            match answer.clone() {
                Some(live) => {
                    update(&mut state, Msg::Live(live), NOW_MS + 10);
                }
                None => {
                    update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS + 10);
                    update(&mut state, Msg::Tick, NOW_MS + 1_999);
                    assert!(reads(drawn(&mut state)).is_empty(), "within the 2 s");
                    update(&mut state, Msg::Tick, NOW_MS + 2_000);
                }
            }
            assert_eq!(
                reads(drawn(&mut state)),
                [Cmd::CollectDue {
                    r#gen: 1,
                    accounts: vec![id(1), id(2)]
                }],
                "{answer:?}"
            );
        }
    }

    #[test]
    fn a_status_that_fails_or_never_comes_ends_the_link() {
        // C-R19: a daemon that holds the link but does not answer `status` is not followed: the
        // TUI is in timer mode, and tells the live thread to end the link, which then connects
        // again with backoff.
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        let asked = update(&mut state, event(mode("collect")), NOW_MS + 500);
        assert_eq!(asked, [Cmd::FetchStatus { link }]);
        let failed = Msg::DaemonStatus {
            link,
            result: Err(StatusError::Client(ClientError::Timeout)),
        };
        assert_eq!(
            update(&mut state, failed, NOW_MS + 1_000),
            [Cmd::DropLink { link }]
        );
        assert!(!state.daemon.live() && state.daemon.status().is_none());
        // Asked for, and no answer within 5 s: a lane stuck in `connect`, say.
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        let asked = update(&mut state, event(mode("collect")), NOW_MS + 1_000);
        assert_eq!(asked, [Cmd::FetchStatus { link }]);
        let before = update(&mut state, Msg::Tick, NOW_MS + 5_999);
        assert!(!before.contains(&Cmd::DropLink { link }), "{before:?}");
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + 6_000),
            [Cmd::DropLink { link }]
        );
        assert!(!state.daemon.live());
        let after = update(&mut state, Msg::Tick, NOW_MS + 7_000);
        assert!(!after.contains(&Cmd::DropLink { link }), "{after:?}");
    }

    #[test]
    fn a_failure_for_a_link_the_live_thread_has_replaced_ends_that_link_alone() {
        // C-R21, the order a suspension (§7.3) can leave the messages in: link 1's `status`
        // failed, and meanwhile the live thread lost link 1 and brought link 2 up. The failure
        // ends link 1 by its name, which the live thread no longer holds; link 2 then comes up
        // as any link does, and link 1's replies change nothing.
        let mut state = showing(Screen::Accounts, 1);
        let up = update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS);
        assert_eq!(up, [Cmd::FetchStatus { link: 1 }]);
        let answered = Msg::DaemonStatus {
            link: 1,
            result: Ok(auto_on()),
        };
        update(&mut state, answered, NOW_MS);
        let asked = update(&mut state, event(mode("collect")), NOW_MS + 1_000);
        assert_eq!(asked, [Cmd::FetchStatus { link: 1 }]);
        let failed = Msg::DaemonStatus {
            link: 1,
            result: Err(StatusError::Client(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the daemon closed the connection",
            )))),
        };
        let at = NOW_MS + 60_000;
        assert_eq!(update(&mut state, failed, at), [Cmd::DropLink { link: 1 }]);
        assert!(update(&mut state, Msg::Live(Live::Down), at).is_empty());
        let up = update(&mut state, Msg::Live(Live::Up { link: 2 }), at);
        assert_eq!(up, [Cmd::FetchStatus { link: 2 }]);
        let answered = Msg::DaemonStatus {
            link: 2,
            result: Ok(auto_on()),
        };
        update(&mut state, answered, at);
        assert!(state.daemon.live(), "link 2 is followed");
        for result in [Err(StatusError::NoDaemon), Ok(auto_on())] {
            let late = Msg::DaemonStatus { link: 1, result };
            assert!(update(&mut state, late, at + 1).is_empty());
            assert!(state.daemon.live());
        }
    }

    #[test]
    fn one_status_is_out_at_a_time_and_the_one_out_is_given_up_on_5_s_after_it_was_asked() {
        // C-R22: A is out when an event asks again. A cannot answer for what came after it was
        // asked, so B is asked when A is answered, with 5 s of its own: B stuck in the lane's
        // `connect` ends the link, though A was answered.
        let mut state = showing(Screen::Accounts, 1);
        let link = go_live(&mut state, auto_on());
        let a = update(&mut state, event(mode("collect")), NOW_MS + 1_000);
        assert_eq!(a, [Cmd::FetchStatus { link }]);
        let b = update(&mut state, event(mode("auto")), NOW_MS + 1_100);
        assert!(b.is_empty(), "B waits for A: {b:?}");
        let tick_b = update(&mut state, Msg::Tick, NOW_MS + 1_150);
        assert!(!tick_b.contains(&Cmd::FetchStatus { link }), "{tick_b:?}");
        let a_answered = Msg::DaemonStatus {
            link,
            result: Ok(auto_on()),
        };
        let b = update(&mut state, a_answered, NOW_MS + 1_200);
        assert_eq!(b, [Cmd::FetchStatus { link }], "B, once A is answered");
        assert!(state.daemon.live());
        let before = update(&mut state, Msg::Tick, NOW_MS + 6_199);
        assert!(!before.contains(&Cmd::DropLink { link }), "{before:?}");
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + 6_200),
            [Cmd::DropLink { link }]
        );
        assert!(!state.daemon.live());
    }

    #[test]
    fn stopping_ends_the_link_at_once_and_names_its_reason_until_the_next_link_or_30_s() {
        // C-R18: timer mode at once, whether or not the daemon goes on to close its socket.
        let mut state = fresh(Screen::Accounts);
        go_live(&mut state, auto_on());
        let stopping = Event::Stopping {
            reason: Wire::from_wire("upgrade"),
        };
        assert!(update(&mut state, event(stopping.clone()), NOW_MS).is_empty());
        assert!(!state.daemon.live() && state.daemon.status().is_none());
        assert_eq!(state.daemon.stopping(), Some("upgrade"));
        let after = update(&mut state, event(Event::Changed), NOW_MS);
        assert!(after.is_empty(), "the link is over");
        state.dirty = false;
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert!(!state.dirty, "the live thread's own Down is no news");
        update(&mut state, Msg::Tick, NOW_MS + 29_999);
        assert_eq!(state.daemon.stopping(), Some("upgrade"));
        state.dirty = false;
        update(&mut state, Msg::Tick, NOW_MS + 30_000);
        assert_eq!(state.daemon.stopping(), None);
        assert!(state.dirty);
        go_live(&mut state, auto_on());
        update(&mut state, event(stopping), NOW_MS);
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        update(&mut state, Msg::Live(Live::Up { link: 9 }), NOW_MS + 1_000);
        assert_eq!(state.daemon.stopping(), None, "the next daemon answered");
    }

    #[test]
    fn a_mismatch_is_kept_until_a_try_finds_another_daemon_or_none() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, Msg::Live(Live::Mismatch { server: 2 }), NOW_MS);
        assert_eq!(state.daemon.mismatch(), Some(2));
        assert!(!state.daemon.live());
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(state.daemon.mismatch(), None, "no daemon answered");
        update(&mut state, Msg::Live(Live::Mismatch { server: 2 }), NOW_MS);
        update(&mut state, Msg::Live(Live::Up { link: 1 }), NOW_MS);
        assert_eq!(state.daemon.mismatch(), None, "a daemon of this protocol");
    }

    #[test]
    fn nothing_is_asked_of_the_daemon_once_the_shutdown_has_begun() {
        let mut state = showing(Screen::Accounts, 1);
        go_live(&mut state, auto_on());
        state.quitting = Some(0);
        let up = update(&mut state, Msg::Live(Live::Up { link: 9 }), NOW_MS);
        assert!(up.is_empty());
        assert!(update(&mut state, event(mode("collect")), NOW_MS).is_empty());
        assert!(update(&mut state, event(Event::Changed), NOW_MS).is_empty());
        assert!(tick(&mut state, NOW_MS + 60_000).is_empty());
    }
}
