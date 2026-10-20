//! What the auto line says (redesign §5.3, T2-7). Before T4 the store's events are not read:
//! a switch comes from the daemon's `status.lastSwitch`, worded from its trigger, and auto's
//! other decisions from this link's events, or `status.lastOutcome` until one arrives. Timer
//! mode says nothing (M-7): without the store it cannot know there were no switches.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_core::autoswitch::{NoSwitchReason, Trigger};
use tagteam_daemon::protocol::{StatusOutcome, StatusProvider, StatusSwitch};
use tagteam_engine::settings::DEFAULT_THRESHOLD;
use tagteam_engine::views::ProviderAccounts;
use tagteam_engine::words::{self, MISSING, NoSwitchFacts};

use crate::chrome::WIDE;
use crate::daemon::words::{clock, iso_ms};
use crate::daemon::{self, Noted, NotedWhat};
use crate::state::State;
use crate::theme::Role;

/// A switch this recent leads the line (rank 1).
const RECENT_MS: i64 = 30 * 60 * 1000;
/// A switch this recent still shows, as the last one (rank 3): History's longest range.
const MONTH_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// Rank 4's words.
const NO_SWITCHES: &str = "no switches in the last 30 days";
/// A switch by auto or the daemon (§7.4).
const SWITCH: (&str, Role) = ("⇄", Role::Accent);

/// The auto line's parts, which `chrome::auto_line` lays out.
#[derive(Debug)]
pub(crate) struct Said {
    /// The glyph, in its role (T2-13).
    pub(crate) mark: Option<(&'static str, Role)>,
    /// When it happened (`when`), for ranks 1 and 2.
    pub(crate) when: Option<String>,
    /// Rank 3's `last switch {when} · `, muted and run straight into `text`.
    pub(crate) lead: Option<String>,
    pub(crate) text: String,
    /// Rank 4's `text` is muted.
    pub(crate) muted: bool,
    /// `next check HH:MM`: from 120 columns, in live mode with the provider's job in `auto`.
    pub(crate) next: Option<String>,
}

/// What the line says for the focus provider (`daemon::focus`), first match: a switch of the
/// last 30 minutes; the daemon's latest decision; a switch of the last 30 days; else, in live
/// mode, that there was none. Timer mode reaches none of them, and until T4 reads the store its
/// line stays empty (M-7).
pub(crate) fn said(state: &State, width: u16, now_ms: i64) -> Option<Said> {
    let provider = daemon::focus(state);
    let job = state.daemon.job(&provider);
    let tz = state.tokens.tz();
    let view = state.view_of(&provider);
    let next = job
        .filter(|j| width >= WIDE && j.mode == "auto")
        .and_then(|j| iso_ms(j.next_tick_at.as_deref()?))
        .map(|ms| format!("next check {}", clock(ms, tz)));
    let said = |mark, when, text| Said {
        mark,
        when,
        lead: None,
        text,
        muted: false,
        next: next.clone(),
    };
    let switch = job
        .and_then(|j| j.last_switch.as_ref())
        .and_then(|s| Some((iso_ms(&s.at)?, s)));
    if let Some((at, s)) = switch.filter(|(at, _)| now_ms - at <= RECENT_MS) {
        return Some(said(
            Some(SWITCH),
            Some(when(at, now_ms, tz)),
            switched(view, s),
        ));
    }
    if let Some(noted) = decision(state, &provider, job) {
        let (mark, text) = decided(state, &provider, &noted);
        return Some(said(mark, Some(when(noted.at_ms, now_ms, tz)), text));
    }
    if let Some((at, s)) = switch.filter(|(at, _)| now_ms - at <= MONTH_MS) {
        return Some(Said {
            lead: Some(format!("last switch {} · ", when(at, now_ms, tz))),
            ..said(Some(SWITCH), None, switched(view, s))
        });
    }
    state.daemon.live().then(|| Said {
        muted: true,
        ..said(None, None, NO_SWITCHES.to_owned())
    })
}

/// §5.3's time form in `tz` (T2-24): `HH:MM` on today's date, `Fri 15:38` within the six days
/// before, else `Oct 12 15:38`.
pub(crate) fn when(at_ms: i64, now_ms: i64, tz: &TimeZone) -> String {
    let zoned = |ms| Timestamp::from_millisecond(ms).map(|t| t.to_zoned(tz.clone()));
    let (Ok(at), Ok(now)) = (zoned(at_ms), zoned(now_ms)) else {
        return MISSING.to_owned();
    };
    let days = now
        .date()
        .since(at.date())
        .map_or(-1, |span| span.get_days());
    let format = match days {
        0 => "%H:%M",
        1..=6 => "%a %H:%M",
        _ => "%b %-d %H:%M",
    };
    at.strftime(format).to_string()
}

/// `auto switched {from} → {to} ({trigger})`, each account by its short name.
fn switched(view: Option<&ProviderAccounts>, s: &StatusSwitch) -> String {
    let moved = format!(
        "auto switched {} → {}",
        named(view, s.from),
        named(view, s.to)
    );
    match Trigger::parse(&s.trigger) {
        Some(trigger) => format!("{moved} ({})", words::trigger(trigger)),
        None => moved,
    }
}

/// The account at `position` by `words::short_name`, else the position alone.
pub(crate) fn named(view: Option<&ProviderAccounts>, position: u32) -> String {
    view.and_then(|v| v.accounts.iter().find(|a| a.row.position == position))
        .map_or_else(
            || position.to_string(),
            |a| {
                let email = a.row.email.as_deref().unwrap_or(&a.row.label);
                words::short_name(position, a.row.alias.as_deref(), email)
            },
        )
}

/// The daemon's latest decision for `provider` in live mode: this link's, or `status`'s
/// `lastOutcome`, the newer winning (T2-7).
fn decision(state: &State, provider: &ProviderId, job: Option<&StatusProvider>) -> Option<Noted> {
    let outcome = job?.last_outcome.as_ref().and_then(outcome);
    match (state.daemon.noted(provider.as_str()).cloned(), outcome) {
        (Some(noted), Some(outcome)) if outcome.at_ms > noted.at_ms => Some(outcome),
        (Some(noted), _) => Some(noted),
        (None, outcome) => outcome,
    }
}

/// `status.lastOutcome` as a decision: a `no-switch` (with no detail) or an `error`.
fn outcome(o: &StatusOutcome) -> Option<Noted> {
    let (at_ms, reason) = (iso_ms(&o.at)?, o.reason.as_deref()?);
    let what = match o.kind.as_str() {
        "no-switch" => NotedWhat::NoSwitch {
            reason: daemon::no_switch_reason(reason)?,
            detail: String::new(),
            used_pct: None,
            frees_at_ms: None,
        },
        "error" => NotedWhat::Error(reason.to_owned()),
        _ => return None,
    };
    Some(Noted { at_ms, what })
}

/// A decision's mark and words (§11.3, T2-13): a `no-switch` in plain words, marked only when
/// every account is exhausted; an `error` or `config-warning` as the daemon sent it.
fn decided(
    state: &State,
    provider: &ProviderId,
    noted: &Noted,
) -> (Option<(&'static str, Role)>, String) {
    match &noted.what {
        NotedWhat::NoSwitch {
            reason,
            detail,
            used_pct,
            frees_at_ms,
        } => {
            let frees_at = frees_at_ms.map(|ms| clock(ms, state.tokens.tz()));
            let facts = NoSwitchFacts {
                detail,
                used_pct: *used_pct,
                threshold: state
                    .view_of(provider)
                    .map_or(DEFAULT_THRESHOLD, |v| v.threshold),
                frees_at: frees_at.as_deref(),
                display_name: state.names_of(provider).display_name,
            };
            let mark = (*reason == NoSwitchReason::AllExhausted).then_some(("✕", Role::Crit));
            (mark, words::no_switch(*reason, &facts))
        }
        NotedWhat::Error(message) | NotedWhat::Warning(message) => {
            (Some(("!", Role::Warn)), message.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tagteam_daemon::protocol::Event;

    use super::*;
    use crate::live::Live;
    use crate::msg::Msg;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;

    /// Claude Code's job switching accounts.
    fn auto_on() -> StatusProvider {
        daemon_job("claude-code", "auto", true)
    }

    /// Live, on Accounts, with `many(3)` read and `job` the daemon's only one.
    fn live_with(job: StatusProvider) -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(3)));
        go_live(&mut state, daemon_status(vec![job], vec![]));
        state
    }

    fn switch(at: &str, from: u32, to: u32, trigger: &str) -> StatusSwitch {
        StatusSwitch {
            at: at.into(),
            from,
            to,
            trigger: trigger.into(),
        }
    }

    fn with_switch(at: &str, from: u32, to: u32) -> StatusProvider {
        let mut job = auto_on();
        job.last_switch = Some(switch(at, from, to, "proactive"));
        job
    }

    /// The auto line at `w` columns.
    fn line(state: &State, w: u16) -> String {
        draw(state, w, if w >= WIDE { 40 } else { 24 })[1].clone()
    }

    /// One of auto's kinds for `provider`, at 14:13:20 unless `fields` says otherwise.
    fn auto_of(provider: &str, kind: &str, fields: Value) -> Msg {
        let mut raw = json!({
            "schemaVersion": 1,
            "event": kind,
            "ts": "2026-09-21T14:13:20Z",
            "provider": provider,
        });
        for (k, v) in fields.as_object().unwrap() {
            raw[k] = v.clone();
        }
        Msg::Live(Live::Event(Event::Auto {
            kind: kind.into(),
            provider: provider.into(),
            raw,
        }))
    }

    fn auto(kind: &str, fields: Value) -> Msg {
        auto_of("claude-code", kind, fields)
    }

    fn no_switch(reason: &str, detail: &str) -> Msg {
        auto("no-switch", json!({"reason": reason, "detail": detail}))
    }

    fn usage() -> Msg {
        Msg::Live(Live::Event(Event::Usage {
            provider: "claude-code".into(),
            updated: vec![1],
            fetch_errors: Default::default(),
        }))
    }

    /// `text` in role letters, every character of it `letter`.
    fn all(text: &str, letter: char) -> String {
        text.chars()
            .map(|c| if c == ' ' { ' ' } else { letter })
            .collect()
    }

    /// Row 1's letters in tier 1.
    fn letters(state: &mut State) -> String {
        state.theme = crate::theme::Theme::new(true);
        let buf = draw_buffer(state, 80, 24, NOW_MS);
        roles(&buf, state.theme, 1).trim_end().to_owned()
    }

    #[test]
    fn the_daemon_s_last_switch_is_worded_from_its_trigger() {
        // T2-7, C-14: from its trigger; the window a poll names is no longer looked up.
        let mut state = live_with(with_switch("2026-09-21T14:11:20Z", 1, 2));
        let poll = json!({"active": {"number": 1}, "headroomPct": {"1": 5.0}, "windowsPct": {"1": {"5h": 95.0}}});
        update(&mut state, auto("poll", poll), NOW_MS);
        assert_eq!(
            line(&state, 80),
            " ⇄ 14:11  auto switched 1 a1 → 2 a2 (before it reached the limit)"
        );
        let text = "auto switched 1 a1 → 2 a2 (before it reached the limit)";
        assert_eq!(letters(&mut state), format!(" a mmmmm  {}", all(text, 't')));
        for (trigger, words) in [
            ("at-limit", " (at the limit)"),
            ("failover", " (its usage couldn't be read)"),
            (
                "consume-first",
                " (to use up the account that resets first)",
            ),
            ("added-later", ""),
        ] {
            let mut job = auto_on();
            job.last_switch = Some(switch("2026-09-21T14:11:20Z", 3, 9, trigger));
            let state = live_with(job);
            assert_eq!(
                line(&state, 120),
                format!(" ⇄ 14:11  auto switched 3 a3 → 9{words}"),
                "{trigger}: an account the view does not list is its position"
            );
        }
    }

    #[test]
    fn rank_3_mutes_the_last_switch_time_and_leaves_the_event_in_the_text_role() {
        // §5.3: the time is muted; the event's words are text.
        let mut job = auto_on();
        job.last_switch = Some(switch("2026-09-19T15:38:00Z", 3, 1, "added-later"));
        let mut state = live_with(job);
        assert_eq!(
            line(&state, 80),
            " ⇄ last switch Sat 15:38 · auto switched 3 a3 → 1 a1"
        );
        assert_eq!(
            letters(&mut state),
            format!(
                " a {}{}",
                all("last switch Sat 15:38 · ", 'm'),
                all("auto switched 3 a3 → 1 a1", 't')
            )
        );
    }

    #[test]
    fn a_poll_or_usage_never_replaces_a_recent_switch() {
        // §5.3, UX-3: a switch of the last 30 minutes leads the line.
        let mut state = live_with(with_switch("2026-09-21T14:11:20Z", 1, 2));
        let before = line(&state, 80);
        let poll = json!({"active": {"number": 2}, "headroomPct": {"2": 60.0}});
        update(&mut state, auto("poll", poll), NOW_MS);
        update(&mut state, usage(), NOW_MS);
        update(&mut state, no_switch("cooldown", "4m"), NOW_MS);
        assert_eq!(line(&state, 80), before);
    }

    #[test]
    fn a_no_switch_shows_in_words_without_a_recent_switch() {
        let mut state = live_with(auto_on());
        update(&mut state, no_switch("cooldown", "4m"), NOW_MS);
        assert_eq!(
            line(&state, 80),
            "   14:13  waiting 4m after the last switch"
        );
        let text = "waiting 4m after the last switch";
        assert_eq!(letters(&mut state), format!("   mmmmm  {}", all(text, 't')));
        let poll = json!({"active": {"number": 1}, "headroomPct": {"1": 23}});
        update(&mut state, auto("poll", poll), NOW_MS);
        update(&mut state, no_switch("below-threshold", ""), NOW_MS);
        assert_eq!(
            line(&state, 80),
            "   14:13  no switch needed (77%, switches at 90%)"
        );
    }

    #[test]
    fn all_exhausted_names_when_the_first_frees_up() {
        let mut state = live_with(auto_on());
        update(&mut state, no_switch("all-exhausted", ""), NOW_MS);
        assert_eq!(
            line(&state, 80),
            " ✕ 14:13  every account is near its limit"
        );
        let reset = json!({"earliestResetAt": "2026-09-21T16:40:00Z"});
        update(&mut state, auto("all-exhausted", reset), NOW_MS);
        assert_eq!(
            line(&state, 80),
            " ✕ 14:13  every account is near its limit; the first frees up at 16:40"
        );
        assert_eq!(&letters(&mut state)[..3], " c ");
    }

    #[test]
    fn an_error_or_a_config_warning_shows_its_message() {
        // T2-13: `!` warn, the message as the daemon sent it.
        let mut state = live_with(auto_on());
        let error = json!({"message": "the store is locked", "transient": true});
        update(&mut state, auto("error", error), NOW_MS);
        assert_eq!(line(&state, 80), " ! 14:13  the store is locked");
        assert_eq!(&letters(&mut state)[..3], " w ");
        let warning = json!({"message": "unknown key autoswitch.treshold (ignored)"});
        update(&mut state, auto("config-warning", warning), NOW_MS);
        assert_eq!(
            line(&state, 80),
            " ! 14:13  unknown key autoswitch.treshold (ignored)"
        );
    }

    #[test]
    fn the_last_outcome_fills_the_line_on_connect() {
        // T2-7, UX-3: `status.lastOutcome` until this link's own decision, the newer winning.
        let outcome = |kind: &str, reason: &str, at: &str| StatusOutcome {
            kind: kind.into(),
            reason: Some(reason.into()),
            at: at.into(),
        };
        let mut job = auto_on();
        job.last_outcome = Some(outcome("no-switch", "cooldown", "2026-09-21T14:10:00Z"));
        let mut state = live_with(job.clone());
        assert_eq!(line(&state, 80), "   14:10  waiting after the last switch");
        let older = auto(
            "no-switch",
            json!({"reason": "below-threshold", "detail": "", "ts": "2026-09-21T14:05:00Z"}),
        );
        update(&mut state, older, NOW_MS);
        assert_eq!(
            line(&state, 80),
            "   14:10  waiting after the last switch",
            "an event older than the outcome"
        );
        update(&mut state, no_switch("below-threshold", ""), NOW_MS);
        assert_eq!(
            line(&state, 80),
            "   14:13  no switch needed (switches at 90%)"
        );
        job.last_outcome = Some(outcome(
            "error",
            "the store is locked",
            "2026-09-21T14:10:00Z",
        ));
        assert_eq!(
            line(&live_with(job.clone()), 80),
            " ! 14:10  the store is locked"
        );
        for kind in ["switch", "collect"] {
            job.last_outcome = Some(outcome(kind, "x", "2026-09-21T14:10:00Z"));
            assert_eq!(
                line(&live_with(job.clone()), 80),
                "   no switches in the last 30 days",
                "{kind} is no decision of rank 2"
            );
        }
    }

    #[test]
    fn an_older_switch_reads_as_the_last_switch_with_its_day() {
        let mut job = with_switch("2026-09-19T15:38:00Z", 3, 1);
        let mut state = live_with(job.clone());
        assert_eq!(
            line(&state, 120),
            " ⇄ last switch Sat 15:38 · auto switched 3 a3 → 1 a1 (before it reached the limit)"
        );
        job.last_switch = Some(switch("2026-09-13T15:38:00Z", 3, 1, "proactive"));
        assert!(
            line(&live_with(job), 120).starts_with(" ⇄ last switch Sep 13 15:38 · auto switched"),
        );
        update(&mut state, no_switch("cooldown", "4m"), NOW_MS);
        assert_eq!(
            line(&state, 120),
            "   14:13  waiting 4m after the last switch",
            "a decision comes before an older switch"
        );
    }

    #[test]
    fn with_nothing_live_mode_says_no_switches_in_the_last_30_days_and_timer_mode_nothing() {
        // M-7: without the store, timer mode cannot know there were none, so rank 4's place
        // stays empty there until T4; the row itself stays (UX-5).
        let mut timer = fresh(Screen::Accounts);
        timer.accounts.apply(snapshot(many(3)));
        for w in [80, 120] {
            assert_eq!(line(&timer, w), "", "timer mode at {w}");
        }
        assert_eq!(letters(&mut timer), "");
        let mut live = live_with(with_switch("2026-08-21T14:00:00Z", 1, 2));
        assert_eq!(line(&live, 80), "   no switches in the last 30 days");
        assert_eq!(letters(&mut live), format!("   {}", all(NO_SWITCHES, 'm')));
    }

    #[test]
    fn next_check_shows_from_120_columns_with_auto_on() {
        let mut job = auto_on();
        job.next_tick_at = Some("2026-09-21T14:16:00Z".into());
        let state = live_with(job.clone());
        assert_eq!(
            line(&state, 120),
            "   no switches in the last 30 days · next check 14:16"
        );
        assert_eq!(line(&state, 80), "   no switches in the last 30 days");
        job.mode = "collect".into();
        assert_eq!(
            line(&live_with(job), 120),
            "   no switches in the last 30 days"
        );
    }

    #[test]
    fn the_line_follows_the_selected_account_s_provider() {
        // §5.3: the focus provider's, by `daemon::focus`.
        let fake = ProviderId::new("fake-agent");
        let mut other = account(1, "f@x.co", read(60, 10.0, 20.0, false));
        other.row.provider = fake.clone();
        other.row.id = tagteam_core::AccountId::from_string("fake-1");
        let mut snap = snapshot(many(3));
        snap.providers.push(ProviderAccounts {
            provider: fake,
            active_position: None,
            accounts: vec![other],
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
        });
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let jobs = vec![
            with_switch("2026-09-21T14:11:20Z", 1, 2),
            daemon_job("fake-agent", "auto", true),
        ];
        go_live(&mut state, daemon_status(jobs, vec![]));
        let fake_said = auto_of(
            "fake-agent",
            "no-switch",
            json!({"reason": "no-active-account", "detail": ""}),
        );
        update(&mut state, fake_said, NOW_MS);
        assert!(line(&state, 80).starts_with(" ⇄ 14:11  auto switched 1 a1 → 2 a2"));
        for _ in 0..3 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(
            line(&state, 80),
            "   14:13  FakeAgent isn't logged in",
            "the fake account's provider, in its own name"
        );
    }

    #[test]
    fn times_read_hh_mm_today_the_weekday_this_week_and_the_date_before() {
        // T2-24, from Monday 2026-09-21 14:13:20 UTC.
        let at = |iso: &str| iso.parse::<Timestamp>().unwrap().as_millisecond();
        let utc = TimeZone::UTC;
        for (iso, want) in [
            ("2026-09-21T14:11:00Z", "14:11"),
            ("2026-09-21T00:00:00Z", "00:00"),
            ("2026-09-20T23:59:00Z", "Sun 23:59"),
            ("2026-09-19T15:38:00Z", "Sat 15:38"),
            ("2026-09-15T15:38:00Z", "Tue 15:38"),
            ("2026-09-14T15:38:00Z", "Sep 14 15:38"),
            ("2026-09-13T15:38:00Z", "Sep 13 15:38"),
            ("2026-09-01T09:05:00Z", "Sep 1 09:05"),
        ] {
            assert_eq!(when(at(iso), NOW_MS, &utc), want, "{iso}");
        }
        // Today is the TUI's zone's: 23:30 UTC the day before is 01:30 today at UTC+2.
        let plus_two = TimeZone::fixed(jiff::tz::offset(2));
        assert_eq!(when(at("2026-09-20T23:30:00Z"), NOW_MS, &plus_two), "01:30");
    }

    #[test]
    fn a_decision_shows_at_its_own_time_and_the_line_stays_when_the_link_goes() {
        // The old activity line's test: a decision at its own `ts`, or now without one; the
        // kinds that are no decision leave the line; and timer mode keeps the row (UX-5), at
        // rank 4.
        let mut state = live_with(auto_on());
        let at_14_04 = json!({"reason": "cooldown", "detail": "4m", "ts": "2026-09-21T14:04:00Z"});
        update(&mut state, auto("no-switch", at_14_04), NOW_MS);
        assert_eq!(
            line(&state, 80),
            "   14:04  waiting 4m after the last switch"
        );
        let unknown = Msg::Live(Live::Event(Event::Unknown {
            kind: "added-later".into(),
            raw: json!({"event": "added-later"}),
        }));
        let mode = Msg::Live(Live::Event(Event::Mode {
            provider: "claude-code".into(),
            mode: "auto".into(),
            waiting_for: None,
            holder_pid: None,
            backoff: None,
        }));
        let index = Msg::Live(Live::Event(Event::TokenIndex {
            provider: "claude-code".into(),
            raw: json!({}),
        }));
        for other in [
            auto("poll", json!({"active": null, "headroomPct": {}})),
            auto("sleep", json!({"seconds": 300.0})),
            auto(
                "switch",
                json!({"from": 1, "to": 2, "trigger": "proactive"}),
            ),
            auto(
                "account-quarantined",
                json!({"number": 2, "reason": "invalid_grant"}),
            ),
            auto("all-exhausted", json!({"earliestResetAt": null})),
            usage(),
            Msg::Live(Live::Event(Event::Changed)),
            mode,
            index,
            unknown,
            no_switch("a-reason-added-later", ""),
        ] {
            update(&mut state, other, NOW_MS);
            assert_eq!(
                line(&state, 80),
                "   14:04  waiting 4m after the last switch"
            );
        }
        let mut raw = json!({"event": "error", "provider": "claude-code", "message": "x"});
        raw.as_object_mut().unwrap().remove("ts");
        let untimed = Msg::Live(Live::Event(Event::Auto {
            kind: "error".into(),
            provider: "claude-code".into(),
            raw,
        }));
        update(&mut state, untimed, NOW_MS);
        assert_eq!(line(&state, 80), " ! 14:13  x", "now, without a ts");
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(line(&state, 80), "", "timer mode: empty until T4 (M-7)");
        go_live(&mut state, daemon_status(vec![auto_on()], vec![]));
        update(&mut state, no_switch("cooldown", "4m"), NOW_MS);
        go_live(&mut state, daemon_status(vec![auto_on()], vec![]));
        assert_eq!(
            line(&state, 80),
            "   no switches in the last 30 days",
            "a new link starts with none of the last one's, even with no Down between"
        );
    }

    #[test]
    fn each_decision_is_said_in_auto_s_plain_words() {
        // The old activity words' test, for the kinds the auto line says (§11.3).
        for (reason, detail, words) in [
            ("cooldown", "", "waiting after the last switch"),
            (
                "active-usage-unknown",
                "2/3",
                "can't read the account in use (2 of 3 tries)",
            ),
            ("no-active-account", "", "Claude Code isn't logged in"),
            (
                "no-qualifying-candidate",
                "",
                "no other account has enough headroom",
            ),
            ("engine-running", "", "auto already runs in another process"),
        ] {
            let mut state = live_with(auto_on());
            update(&mut state, no_switch(reason, detail), NOW_MS);
            assert_eq!(line(&state, 80), format!("   14:13  {words}"), "{reason}");
        }
    }

    #[test]
    fn a_no_switch_reads_its_usage_only_from_a_poll_of_its_own_link() {
        // C-R21: a link can come up after events were missed, so a `no-switch` reads the usage
        // only from a `poll` the same link carried.
        let mut state = live_with(auto_on());
        let poll = json!({"active": {"number": 1}, "headroomPct": {"1": 23.0}});
        update(&mut state, auto("poll", poll), NOW_MS);
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        go_live(&mut state, daemon_status(vec![auto_on()], vec![]));
        update(&mut state, no_switch("below-threshold", ""), NOW_MS);
        assert_eq!(
            line(&state, 80),
            "   14:13  no switch needed (switches at 90%)",
            "the poll was the last link's"
        );
    }
}
