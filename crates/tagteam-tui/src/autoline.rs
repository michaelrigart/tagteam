//! What the auto line says (redesign §5.3, T4-30): the store's last auto-line event, which the
//! accounts read carries (§12.2), in the words History's list gives it; auto's other decisions
//! from this link's events in live mode, or `status.lastOutcome` until one arrives; and, with
//! nothing stored in the last 30 days, that there were no switches, in both modes (M-7 lifted).

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_core::autoswitch::NoSwitchReason;
use tagteam_daemon::events::Mode;
use tagteam_daemon::protocol::{StatusOutcome, StatusProvider};
use tagteam_daemon::status::OutcomeKind;
use tagteam_engine::EventKind;
use tagteam_engine::settings::DEFAULT_THRESHOLD;
use tagteam_engine::timeline::EventView;
use tagteam_engine::views::ProviderAccounts;
use tagteam_engine::words::{self, MISSING, NoSwitchFacts};

use crate::chrome::WIDE;
use crate::daemon::words::{clock, iso_ms};
use crate::daemon::{self, Noted, NotedWhat};
use crate::screens::history::{list, marks};
use crate::state::State;
use crate::theme::Role;

/// A stored event this recent leads the line (rank 1).
const RECENT_MS: i64 = 30 * 60 * 1000;
/// A stored event this recent still shows (rank 3): History's longest range, and what the
/// accounts read asks the store for.
pub(crate) const MONTH_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// Rank 4's words.
const NO_SWITCHES: &str = "no switches in the last 30 days";

/// The auto line's parts, which `chrome::auto_line` lays out.
#[derive(Debug)]
pub(crate) struct Said {
    /// The glyph, in its role (T2-13): a stored event's `marks::mark`.
    pub(crate) mark: Option<(&'static str, Role)>,
    /// When it happened (`when`), for ranks 1 and 2, and rank 3's events other than a switch.
    pub(crate) when: Option<String>,
    /// Rank 3's `last switch {when} · `, muted and run straight into `text`.
    pub(crate) lead: Option<String>,
    pub(crate) text: String,
    /// Rank 4's `text` is muted.
    pub(crate) muted: bool,
    /// `next check HH:MM`: from 120 columns, in live mode with the provider's job in `auto`.
    pub(crate) next: Option<String>,
    /// Ranks 1 and 3: the event `w` opens (T4-15).
    pub(crate) pin: Option<(i64, EventKind)>,
}

/// What the line says for the focus provider (`daemon::focus`), first match (§5.3): its stored
/// event of the last 30 minutes; in live mode, the daemon's latest decision; its stored event of
/// the last 30 days, a switch as the last one; else that there was none. Nothing before the
/// first accounts read.
pub(crate) fn said(state: &State, width: u16, now_ms: i64) -> Option<Said> {
    let snapshot = state.accounts.snapshot()?;
    let provider = daemon::focus(state);
    let job = state.daemon.job(&provider);
    let tz = state.tokens.tz();
    let next = job
        .filter(|j| width >= WIDE && j.mode.known() == Some(Mode::Auto))
        .and_then(|j| iso_ms(j.next_tick_at.as_deref()?))
        .map(|ms| format!("next check {}", clock(ms, tz)));
    let said = |mark, when, text| Said {
        mark,
        when,
        lead: None,
        text,
        muted: false,
        next: next.clone(),
        pin: None,
    };
    let stored = |within: i64| {
        snapshot
            .last_auto
            .iter()
            .find(|(p, e)| *p == provider && now_ms - e.at_ms <= within)
            .map(|(_, e)| e)
    };
    let of = |e: &EventView| {
        let mark = marks::mark(e);
        Said {
            pin: Some((e.at_ms, e.kind)),
            ..said(
                Some((mark.glyph, mark.role)),
                Some(when(e.at_ms, now_ms, tz)),
                list::sentence(e, tz),
            )
        }
    };
    if let Some(e) = stored(RECENT_MS) {
        return Some(of(e));
    }
    if let Some(noted) = decision(state, &provider, job) {
        let (mark, text) = decided(state, &provider, &noted);
        return Some(said(mark, Some(when(noted.at_ms, now_ms, tz)), text));
    }
    if let Some(e) = stored(MONTH_MS) {
        return Some(match e.kind {
            EventKind::Switch | EventKind::SwitchRecovered => Said {
                when: None,
                lead: Some(format!("last switch {} · ", when(e.at_ms, now_ms, tz))),
                ..of(e)
            },
            _ => of(e),
        });
    }
    Some(Said {
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
    let what = match o.kind.known() {
        Some(OutcomeKind::NoSwitch) => NotedWhat::NoSwitch {
            reason: NoSwitchReason::parse(reason)?,
            detail: String::new(),
            used_pct: None,
            frees_at_ms: None,
        },
        Some(OutcomeKind::Error) => NotedWhat::Error(reason.to_owned()),
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
    use tagteam_daemon::protocol::{Event, Wire};
    use tagteam_engine::EventSource;
    use tagteam_engine::timeline::EventView;

    use super::*;
    use crate::live::Live;
    use crate::msg::Msg;
    use crate::screens::history::{account_ref, event_at};
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

    /// Timer mode, on Accounts, with `many(3)` read.
    fn timer() -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(3)));
        state
    }

    fn at(iso: &str) -> i64 {
        iso.parse::<Timestamp>().unwrap().as_millisecond()
    }

    /// Auto's switch 1 → 2 stored at `iso`, as a1's 5h reached 93%.
    fn stored_switch(iso: &str) -> EventView {
        let mut e = event_at(at(iso), EventKind::Switch, EventSource::Auto);
        e.trigger = Some("proactive".into());
        e.from = Some(account_ref(1));
        e.to = Some(account_ref(2));
        e.detail = Some(json!({"window": "5h", "label": "5h", "pct": 93.0, "threshold": 90.0}));
        e
    }

    /// Its words.
    const SWITCHED: &str = "auto switched 1 a1 → 2 a2 · a1's 5h reached 93%";

    /// `many(3)` read again, with `e` Claude Code's last auto-line event.
    fn store(state: &mut State, e: EventView) {
        let mut snap = snapshot(many(3));
        snap.last_auto = vec![(cc(), e)];
        state.accounts.apply(snap);
    }

    /// The auto line at `w` columns.
    fn line(state: &State, w: u16) -> String {
        draw(state, w, if w >= WIDE { 40 } else { 24 })[1].clone()
    }

    /// `left`, then `w why` ending one column in, on a row `w` wide (§5.3).
    fn hinted(left: &str, w: u16) -> String {
        let gap = usize::from(w) - 1 - HINT.len() - crate::view::width_of(left);
        format!("{left}{}{HINT}", " ".repeat(gap))
    }

    /// `hinted` in role letters: `w` accent, `why` muted.
    fn hinted_letters(left: &str, w: u16) -> String {
        let gap = usize::from(w) - 1 - HINT.len() - crate::view::width_of(left);
        format!("{left}{}a mmm", " ".repeat(gap))
    }

    const HINT: &str = "w why";

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

    /// Row 1's letters in tier 1, at `w` columns.
    fn letters_at(state: &mut State, w: u16) -> String {
        state.theme = crate::theme::Theme::new(true);
        let buf = draw_buffer(state, w, if w >= WIDE { 40 } else { 24 }, NOW_MS);
        roles(&buf, state.theme, 1).trim_end().to_owned()
    }

    fn letters(state: &mut State) -> String {
        letters_at(state, 80)
    }

    #[test]
    fn a_stored_event_of_the_last_30_minutes_leads_the_line() {
        // §5.3 rank 1, T4-30: in both modes, from the store; in live mode a later decision, a
        // poll or a usage never replaces it (UX-3).
        let row = hinted(&format!(" ⇄ 14:11  {SWITCHED}"), 80);
        let mut timer = timer();
        store(&mut timer, stored_switch("2026-09-21T14:11:20Z"));
        assert_eq!(line(&timer, 80), row);
        assert_eq!(
            letters(&mut timer),
            hinted_letters(&format!(" a mmmmm  {}", all(SWITCHED, 't')), 80)
        );
        let mut live = live_with(auto_on());
        store(&mut live, stored_switch("2026-09-21T14:11:20Z"));
        assert_eq!(line(&live, 80), row);
        let poll = json!({"active": {"number": 2}, "headroomPct": {"2": 60.0}});
        update(&mut live, no_switch("cooldown", "4m"), NOW_MS);
        update(&mut live, auto("poll", poll), NOW_MS);
        update(&mut live, usage(), NOW_MS);
        assert_eq!(line(&live, 80), row);
        let said = said(&live, 80, NOW_MS).unwrap();
        assert_eq!(
            said.pin,
            Some((at("2026-09-21T14:11:20Z"), EventKind::Switch)),
            "w opens this event"
        );
    }

    #[test]
    fn a_decision_shows_without_a_recent_stored_event() {
        // §5.3 rank 2, live mode only; timer mode reads the same store as rank 3.
        let mut live = live_with(auto_on());
        store(&mut live, stored_switch("2026-09-21T12:13:20Z"));
        update(&mut live, no_switch("cooldown", "4m"), NOW_MS);
        assert_eq!(
            line(&live, 80),
            hinted("   14:13  waiting 4m after the last switch", 80)
        );
        assert_eq!(said(&live, 80, NOW_MS).unwrap().pin, None, "w opens at now");
        let mut timer = timer();
        store(&mut timer, stored_switch("2026-09-21T12:13:20Z"));
        assert_eq!(
            line(&timer, 80),
            hinted(&format!(" ⇄ last switch 12:13 · {SWITCHED}"), 80)
        );
        assert_eq!(
            said(&timer, 80, NOW_MS).unwrap().pin,
            Some((at("2026-09-21T12:13:20Z"), EventKind::Switch))
        );
    }

    #[test]
    fn a_no_switch_shows_in_words_without_a_recent_switch() {
        let mut state = live_with(auto_on());
        update(&mut state, no_switch("cooldown", "4m"), NOW_MS);
        let left = "   14:13  waiting 4m after the last switch";
        assert_eq!(line(&state, 80), hinted(left, 80));
        let text = "waiting 4m after the last switch";
        assert_eq!(
            letters(&mut state),
            hinted_letters(&format!("   mmmmm  {}", all(text, 't')), 80)
        );
        let poll = json!({"active": {"number": 1}, "headroomPct": {"1": 23}, "windowsPct": {"1": {"5h": 77.0}}});
        update(&mut state, auto("poll", poll), NOW_MS);
        update(&mut state, no_switch("below-threshold", ""), NOW_MS);
        assert_eq!(
            line(&state, 80),
            hinted("   14:13  no switch needed (77%, switches at 90%)", 80)
        );
    }

    #[test]
    fn all_exhausted_names_when_the_first_frees_up() {
        let mut state = live_with(auto_on());
        update(&mut state, no_switch("all-exhausted", ""), NOW_MS);
        assert_eq!(
            line(&state, 80),
            hinted(" ✕ 14:13  every account is near its limit", 80)
        );
        let reset = json!({"earliestResetAt": "2026-09-21T16:40:00Z"});
        update(&mut state, auto("all-exhausted", reset), NOW_MS);
        assert_eq!(
            line(&state, 80),
            hinted(
                " ✕ 14:13  every account is near its limit; the first frees up at 16:40",
                80
            )
        );
        assert_eq!(&letters(&mut state)[..3], " c ");
    }

    #[test]
    fn an_error_or_a_config_warning_shows_its_message() {
        // T2-13: `!` warn, the message as the daemon sent it.
        let mut state = live_with(auto_on());
        let error = json!({"message": "the store is locked", "transient": true});
        update(&mut state, auto("error", error), NOW_MS);
        assert_eq!(
            line(&state, 80),
            hinted(" ! 14:13  the store is locked", 80)
        );
        assert_eq!(&letters(&mut state)[..3], " w ");
        let warning = json!({"message": "unknown key autoswitch.treshold (ignored)"});
        update(&mut state, auto("config-warning", warning), NOW_MS);
        assert_eq!(
            line(&state, 80),
            hinted(" ! 14:13  unknown key autoswitch.treshold (ignored)", 80)
        );
    }

    #[test]
    fn the_last_outcome_fills_the_line_on_connect() {
        // T2-7, UX-3: `status.lastOutcome` until this link's own decision, the newer winning.
        let outcome = |kind: &str, reason: &str, at: &str| StatusOutcome {
            kind: Wire::from_wire(kind),
            reason: Some(reason.into()),
            at: at.into(),
        };
        let mut job = auto_on();
        job.last_outcome = Some(outcome("no-switch", "cooldown", "2026-09-21T14:10:00Z"));
        let mut state = live_with(job.clone());
        let waiting = hinted("   14:10  waiting after the last switch", 80);
        assert_eq!(line(&state, 80), waiting);
        let older = auto(
            "no-switch",
            json!({"reason": "below-threshold", "detail": "", "ts": "2026-09-21T14:05:00Z"}),
        );
        update(&mut state, older, NOW_MS);
        assert_eq!(line(&state, 80), waiting, "an event older than the outcome");
        update(&mut state, no_switch("below-threshold", ""), NOW_MS);
        assert_eq!(
            line(&state, 80),
            hinted("   14:13  no switch needed (switches at 90%)", 80)
        );
        job.last_outcome = Some(outcome(
            "error",
            "the store is locked",
            "2026-09-21T14:10:00Z",
        ));
        assert_eq!(
            line(&live_with(job.clone()), 80),
            hinted(" ! 14:10  the store is locked", 80)
        );
        for kind in ["switch", "collect"] {
            job.last_outcome = Some(outcome(kind, "x", "2026-09-21T14:10:00Z"));
            assert_eq!(
                line(&live_with(job.clone()), 80),
                hinted("   no switches in the last 30 days", 80),
                "{kind} is no decision of rank 2"
            );
        }
    }

    #[test]
    fn an_older_event_reads_as_the_last_switch_with_its_day() {
        // §5.3 rank 3: a switch as `last switch {when} · …`, the lead muted and the words text;
        // any other kind as rank 1 lays it out.
        let mut state = timer();
        store(&mut state, stored_switch("2026-09-19T15:38:00Z"));
        let lead = "last switch Sat 15:38 · ";
        assert_eq!(
            line(&state, 120),
            hinted(&format!(" ⇄ {lead}{SWITCHED}"), 120)
        );
        assert_eq!(
            letters_at(&mut state, 120),
            hinted_letters(&format!(" a {}{}", all(lead, 'm'), all(SWITCHED, 't')), 120)
        );
        store(&mut state, stored_switch("2026-09-13T15:38:00Z"));
        assert!(
            line(&state, 120).starts_with(" ⇄ last switch Sep 13 15:38 · auto switched"),
            "{}",
            line(&state, 120)
        );
        let mut refused = event_at(
            at("2026-09-19T08:58:00Z"),
            EventKind::Quarantine,
            EventSource::Auto,
        );
        refused.to = Some(account_ref(3));
        refused.detail = Some(json!({"reason": "invalid_grant"}));
        let words = words::event(&refused, None);
        assert!(words.starts_with("3 a3 needs a new login · "), "{words}");
        store(&mut state, refused);
        assert_eq!(
            line(&state, 120),
            hinted(&format!(" ✕ Sat 08:58  {words}"), 120)
        );
        assert_eq!(&letters_at(&mut state, 120)[..3], " c ");
        let mut live = live_with(auto_on());
        store(&mut live, stored_switch("2026-09-19T15:38:00Z"));
        update(&mut live, no_switch("cooldown", "4m"), NOW_MS);
        assert_eq!(
            line(&live, 120),
            hinted("   14:13  waiting 4m after the last switch", 120),
            "a decision comes before an older switch"
        );
    }

    #[test]
    fn timer_mode_says_there_were_no_switches_when_the_store_has_none() {
        // M-7 lifted (T4-30): the store says there were none, in timer and in live mode; the
        // row stays empty only before the first snapshot.
        let mut timer = timer();
        for w in [80, 120] {
            assert_eq!(
                line(&timer, w),
                hinted("   no switches in the last 30 days", w),
                "timer mode at {w}"
            );
        }
        assert_eq!(
            letters(&mut timer),
            hinted_letters(&format!("   {}", all(NO_SWITCHES, 'm')), 80)
        );
        assert_eq!(said(&timer, 80, NOW_MS).unwrap().pin, None);
        let mut live = live_with(auto_on());
        store(&mut live, stored_switch("2026-08-21T14:00:00Z"));
        assert_eq!(
            line(&live, 80),
            hinted("   no switches in the last 30 days", 80),
            "a switch older than 30 days"
        );
        assert_eq!(
            letters(&mut live),
            hinted_letters(&format!("   {}", all(NO_SWITCHES, 'm')), 80)
        );
        let empty = fresh(Screen::Accounts);
        assert_eq!(line(&empty, 80), "", "before the first snapshot");
        let mut linked = fresh(Screen::Accounts);
        go_live(&mut linked, daemon_status(vec![auto_on()], vec![]));
        update(&mut linked, no_switch("cooldown", "4m"), NOW_MS);
        assert_eq!(line(&linked, 80), "", "live, before the first snapshot");
    }

    #[test]
    fn next_check_shows_from_120_columns_with_auto_on() {
        let mut job = auto_on();
        job.next_tick_at = Some("2026-09-21T14:16:00Z".into());
        let state = live_with(job.clone());
        assert_eq!(
            line(&state, 120),
            hinted("   no switches in the last 30 days · next check 14:16", 120)
        );
        assert_eq!(
            line(&state, 80),
            hinted("   no switches in the last 30 days", 80)
        );
        job.mode = Wire::from_wire("collect");
        assert_eq!(
            line(&live_with(job), 120),
            hinted("   no switches in the last 30 days", 120)
        );
    }

    #[test]
    fn the_hint_s_w_is_accent_and_its_why_muted_in_every_tier() {
        // §5.3's roles: `w` accent.
        for theme in TIERS {
            let mut state = timer();
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let row = roles(&buf, theme, 1);
            // `w why` ends one column in: columns 74 to 78 of 80.
            assert_eq!(&row[74..], canon(theme, "a mmm "), "{theme:?}");
            assert_eq!(&text_of(&buf)[1][74..], HINT);
        }
    }

    /// The auto line drawn alone, `width` wide, as text.
    fn alone(state: &State, width: u16) -> String {
        crate::chrome::auto_line(state, width, NOW_MS)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect()
    }

    #[test]
    fn a_narrow_line_cuts_the_sentence_and_keeps_the_hint_whole() {
        // The sentence gives way first; the hint is never cut.
        let mut state = timer();
        store(&mut state, stored_switch("2026-09-21T14:11:20Z"));
        // 40 columns leave the left part 40 − 5 − 3 − 1 = 31.
        let left = crate::view::fit(&format!(" ⇄ 14:11  {SWITCHED}…"), 31);
        assert!(left.ends_with('…'), "{left}");
        assert_eq!(alone(&state, 40), format!("{left}   {HINT}"));
    }

    #[test]
    fn a_very_narrow_line_drops_the_hint() {
        // The hint goes only when keeping it would leave the left part under 16 columns: at 25
        // columns it keeps 16, at 24 it would keep 15.
        let mut state = timer();
        store(&mut state, stored_switch("2026-09-21T14:11:20Z"));
        let kept = alone(&state, 25);
        assert!(kept.ends_with(&format!("   {HINT}")), "{kept:?}");
        assert_eq!(crate::view::width_of(&kept), 24, "ending one column in");
        let dropped = alone(&state, 24);
        assert!(!dropped.contains(HINT), "{dropped:?}");
        assert_eq!(
            dropped,
            crate::view::fit(&format!(" ⇄ 14:11  {SWITCHED}…"), 24)
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
            provider: fake.clone(),
            active_position: None,
            forecast: tagteam_engine::views::forecast_of(
                std::slice::from_ref(&other),
                &tagteam_engine::switch::LiveLogin::None,
                &[],
            ),
            accounts: vec![other],
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
            models: vec![],
            live: tagteam_engine::switch::LiveLogin::None,
        });
        let mut refused = event_at(
            at("2026-09-21T14:12:00Z"),
            EventKind::Quarantine,
            EventSource::Daemon,
        );
        refused.to = Some(tagteam_engine::timeline::AccountRef {
            id: tagteam_core::AccountId::from_string("fake-1"),
            position: Some(1),
            alias: None,
            email: Some("f@x.co".into()),
            removed: false,
        });
        snap.last_auto = vec![
            (cc(), stored_switch("2026-09-21T14:11:20Z")),
            (fake, refused),
        ];
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        assert_eq!(
            line(&state, 80),
            hinted(&format!(" ⇄ 14:11  {SWITCHED}"), 80)
        );
        for _ in 0..3 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(
            line(&state, 80),
            hinted(&format!(" ✕ 14:12  1 f {}", words::NEEDS_LOGIN), 80),
            "the fake account's provider's stored event"
        );
    }

    #[test]
    fn times_read_hh_mm_today_the_weekday_this_week_and_the_date_before() {
        // T2-24, from Monday 2026-09-21 14:13:20 UTC.
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
        let waiting = hinted("   14:04  waiting 4m after the last switch", 80);
        assert_eq!(line(&state, 80), waiting);
        let unknown = Msg::Live(Live::Event(Event::Unknown {
            kind: "added-later".into(),
            raw: json!({"event": "added-later"}),
        }));
        let mode = Msg::Live(Live::Event(Event::Mode {
            provider: "claude-code".into(),
            mode: Wire::from_wire("auto"),
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
            assert_eq!(line(&state, 80), waiting);
        }
        let mut raw = json!({"event": "error", "provider": "claude-code", "message": "x"});
        raw.as_object_mut().unwrap().remove("ts");
        let untimed = Msg::Live(Live::Event(Event::Auto {
            kind: "error".into(),
            provider: "claude-code".into(),
            raw,
        }));
        update(&mut state, untimed, NOW_MS);
        assert_eq!(
            line(&state, 80),
            hinted(" ! 14:13  x", 80),
            "now, without a ts"
        );
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        let none = hinted("   no switches in the last 30 days", 80);
        assert_eq!(line(&state, 80), none, "timer mode: rank 4 (M-7 lifted)");
        go_live(&mut state, daemon_status(vec![auto_on()], vec![]));
        update(&mut state, no_switch("cooldown", "4m"), NOW_MS);
        go_live(&mut state, daemon_status(vec![auto_on()], vec![]));
        assert_eq!(
            line(&state, 80),
            none,
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
            assert_eq!(
                line(&state, 80),
                hinted(&format!("   14:13  {words}"), 80),
                "{reason}"
            );
        }
    }

    #[test]
    fn a_no_switch_reads_its_usage_only_from_a_poll_of_its_own_link() {
        // C-R21: a link can come up after events were missed, so a `no-switch` reads the usage
        // only from a `poll` the same link carried.
        let mut state = live_with(auto_on());
        let poll = json!({"active": {"number": 1}, "headroomPct": {"1": 23.0}, "windowsPct": {"1": {"5h": 77.0}}});
        update(&mut state, auto("poll", poll), NOW_MS);
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        go_live(&mut state, daemon_status(vec![auto_on()], vec![]));
        update(&mut state, no_switch("below-threshold", ""), NOW_MS);
        assert_eq!(
            line(&state, 80),
            hinted("   14:13  no switch needed (switches at 90%)", 80),
            "the poll was the last link's"
        );
    }
}
