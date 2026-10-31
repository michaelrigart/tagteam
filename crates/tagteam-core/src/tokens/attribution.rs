//! Attribution (token report §7): which account a response went to, decided at query time from
//! the evidence of where its session ran. A response is placed only where the evidence allows
//! one home; anything else is unattributed, never guessed.

use std::collections::BTreeMap;

use crate::ids::AccountId;

/// A Claude Code home a session ran in: the default home, or an account's `run` profile.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Home {
    Default,
    Profile(AccountId),
}

/// One execution of a session, seen alive in `home` from `first_ms` to `last_ms` (token report
/// §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub home: Home,
    pub first_ms: i64,
    pub last_ms: i64,
}

/// A `run` launch: a profile session may have run for `account` from `start_ms` until `end_ms`,
/// or still runs (`None`) (token report §6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub account: AccountId,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
}

/// Whether every profile session at a time has its launch recorded (token report §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage {
    /// When the store began recording launches and sightings (`token_epoch.at`).
    pub epoch_ms: i64,
    /// Whether the migration seeded the profiles that existed then (ruling R1).
    pub seeded: bool,
    /// The provider's last `remove` event before the epoch.
    pub last_pre_epoch_remove_ms: Option<i64>,
    /// Whether launch evidence could not be read at all: the profiles could not be listed, so
    /// any of them may hold a launch (token report §7.2).
    pub unreadable: bool,
}

impl Coverage {
    /// Complete from the epoch on; before it, only from seeded launches, and only after the
    /// last `remove` before the epoch: a removed account may have had a profile that left no
    /// trace. With evidence that could not be read, complete at no time.
    pub fn complete_at(&self, t: i64) -> bool {
        !self.unreadable
            && (t >= self.epoch_ms
                || (self.seeded && self.last_pre_epoch_remove_ms.is_none_or(|r| t >= r)))
    }
}

/// The times `run` launches cover, merged for a binary search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launches {
    /// `[start, end)`, sorted by start and disjoint; an open launch ends at `i64::MAX`.
    spans: Vec<(i64, i64)>,
}

impl Launches {
    pub fn new(launches: Vec<Launch>) -> Self {
        let mut all: Vec<(i64, i64)> = launches
            .into_iter()
            .map(|l| (l.start_ms, l.end_ms.unwrap_or(i64::MAX)))
            .filter(|(start, end)| start < end)
            .collect();
        all.sort_unstable();
        let mut spans: Vec<(i64, i64)> = Vec::with_capacity(all.len());
        for (start, end) in all {
            match spans.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => spans.push((start, end)),
            }
        }
        Self { spans }
    }

    /// Whether a launch covers `t`: one started at or before it and had not ended by then.
    pub fn any_covers(&self, t: i64) -> bool {
        let i = self.spans.partition_point(|&(start, _)| start <= t);
        i > 0 && t < self.spans[i - 1].1
    }
}

/// Where a response ran, as far as the evidence tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Home(Home),
    Unattributed,
}

/// Token report §7.1–7.2, for a response at `t` whose session has `sightings`. Sighted in one
/// home at `t`: that home; in two: unattributed. Sighted nowhere: the default home when it is
/// the only candidate (no launch covers `t`, and launch coverage is complete at `t`), else
/// unattributed.
pub fn place(
    sightings: &[Sighting],
    launches: &Launches,
    coverage: &Coverage,
    t: i64,
) -> Placement {
    let mut sighted: Option<&Home> = None;
    for s in sightings
        .iter()
        .filter(|s| s.first_ms <= t && t <= s.last_ms)
    {
        match sighted {
            Some(home) if *home != s.home => return Placement::Unattributed,
            _ => sighted = Some(&s.home),
        }
    }
    match sighted {
        Some(home) => Placement::Home(home.clone()),
        None if !launches.any_covers(t) && coverage.complete_at(t) => {
            Placement::Home(Home::Default)
        }
        None => Placement::Unattributed,
    }
}

/// One of the provider's `events` rows that the timeline reads before the epoch (core §6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineEvent {
    Switch {
        at_ms: i64,
        from: Option<AccountId>,
        to: AccountId,
    },
    Add {
        at_ms: i64,
        account: AccountId,
    },
    Remove {
        at_ms: i64,
        account: AccountId,
    },
}

/// An `active_history` row: the provider's active account from `at_ms` on (token report §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveChange {
    pub at_ms: i64,
    pub account: Option<AccountId>,
}

/// The default home's account over time (token report §7.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timeline {
    epoch_ms: i64,
    /// Before the epoch: where the account changed, by `events`, in time order.
    before: Vec<(i64, Option<AccountId>)>,
    /// From the epoch on: `active_history`, in time order.
    after: Vec<(i64, Option<AccountId>)>,
}

impl Timeline {
    /// Token report §7.3. From `epoch_ms` on, the latest `history` row at or before a time.
    /// Before it, from `events` before the epoch: the latest switch's `to`; before the first
    /// switch, its `from` from that account's earliest add; with no such switch, the account
    /// `history` was seeded with (its first row), from its earliest add. `added_at` stands in for an account with
    /// no add event. A remove of an account ends its segment.
    pub fn build(
        events: &[TimelineEvent],
        history: &[ActiveChange],
        epoch_ms: i64,
        added_at: &BTreeMap<AccountId, i64>,
    ) -> Timeline {
        let mut after: Vec<(i64, Option<AccountId>)> = history
            .iter()
            .map(|c| (c.at_ms, c.account.clone()))
            .collect();
        after.sort_by_key(|(at, _)| *at);

        let mut switches: Vec<(i64, Option<&AccountId>, &AccountId)> = Vec::new();
        let mut adds: BTreeMap<&AccountId, i64> = BTreeMap::new();
        let mut removes: BTreeMap<&AccountId, Vec<i64>> = BTreeMap::new();
        for e in events {
            match e {
                TimelineEvent::Switch { at_ms, from, to } => {
                    // The `before` timeline reads only switches before the epoch.
                    if *at_ms < epoch_ms {
                        switches.push((*at_ms, from.as_ref(), to));
                    }
                }
                TimelineEvent::Add { at_ms, account } => {
                    let first = adds.entry(account).or_insert(*at_ms);
                    *first = (*first).min(*at_ms);
                }
                TimelineEvent::Remove { at_ms, account } => {
                    removes.entry(account).or_default().push(*at_ms);
                }
            }
        }
        switches.sort_by_key(|(at, _, _)| *at);
        for times in removes.values_mut() {
            times.sort_unstable();
        }
        let added = |account: &AccountId| {
            adds.get(account)
                .copied()
                .or_else(|| added_at.get(account).copied())
        };

        let mut before: Vec<(i64, Option<AccountId>)> = Vec::new();
        // `account` from `start`, until `end` or its first remove before then.
        let mut segment = |account: &AccountId, start: i64, end: i64| {
            before.push((start, Some(account.clone())));
            let removed = removes
                .get(account)
                .and_then(|times| times.iter().copied().find(|r| (start..end).contains(r)));
            if let Some(r) = removed {
                before.push((r, None));
            }
        };
        match switches.first() {
            Some(&(first_at, from, _)) => {
                if let Some(from) = from
                    && let Some(start) = added(from).filter(|start| *start < first_at)
                {
                    segment(from, start, first_at);
                }
                for (i, &(at, _, to)) in switches.iter().enumerate() {
                    let end = switches.get(i + 1).map_or(i64::MAX, |next| next.0);
                    segment(to, at, end);
                }
            }
            None => {
                // Only the row the migration seeded at the epoch: a later activation says
                // nothing about who was active before it.
                let seeded = history
                    .iter()
                    .filter(|c| c.at_ms <= epoch_ms)
                    .min_by_key(|c| c.at_ms)
                    .and_then(|c| c.account.as_ref());
                if let Some(account) = seeded
                    && let Some(start) = added(account)
                {
                    segment(account, start, i64::MAX);
                }
            }
        }
        before.sort_by_key(|(at, _)| *at);
        Timeline {
            epoch_ms,
            before,
            after,
        }
    }

    /// The default home's account at `t`, when the evidence names one.
    pub fn account_at(&self, t: i64) -> Option<AccountId> {
        let changes = if t >= self.epoch_ms {
            &self.after
        } else {
            &self.before
        };
        let i = changes.partition_point(|(at, _)| *at <= t);
        i.checked_sub(1).and_then(|i| changes[i].1.clone())
    }
}

/// What a response's account rests on (token report §7, §10's `by_basis`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Basis {
    /// A profile's own account.
    Profile,
    /// The default home's account at the time, by the timeline.
    Timeline,
    Unattributed,
}

/// A placement's account (token report §7.3): a profile's is its own; the default home's is
/// the timeline's at `t`, and without one the response is unattributed.
pub fn attribute(placement: &Placement, timeline: &Timeline, t: i64) -> (Option<AccountId>, Basis) {
    match placement {
        Placement::Home(Home::Profile(account)) => (Some(account.clone()), Basis::Profile),
        Placement::Home(Home::Default) => match timeline.account_at(t) {
            Some(account) => (Some(account), Basis::Timeline),
            None => (None, Basis::Unattributed),
        },
        Placement::Unattributed => (None, Basis::Unattributed),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::tokens::test_support::pinned;

    const H: i64 = 3_600_000;
    const EPOCH: i64 = 1_000 * H;

    fn id(s: &str) -> AccountId {
        AccountId::from_string(s)
    }

    fn profile(s: &str) -> Home {
        Home::Profile(id(s))
    }

    fn seen(home: Home, first_ms: i64, last_ms: i64) -> Sighting {
        Sighting {
            home,
            first_ms,
            last_ms,
        }
    }

    fn launch(account: &str, start_ms: i64, end_ms: Option<i64>) -> Launch {
        Launch {
            account: id(account),
            start_ms,
            end_ms,
        }
    }

    fn none() -> Launches {
        Launches::new(Vec::new())
    }

    fn complete() -> Coverage {
        Coverage {
            epoch_ms: 0,
            seeded: true,
            last_pre_epoch_remove_ms: None,
            unreadable: false,
        }
    }

    fn switch(at_ms: i64, from: Option<&str>, to: &str) -> TimelineEvent {
        TimelineEvent::Switch {
            at_ms,
            from: from.map(id),
            to: id(to),
        }
    }

    fn add(at_ms: i64, account: &str) -> TimelineEvent {
        TimelineEvent::Add {
            at_ms,
            account: id(account),
        }
    }

    fn remove(at_ms: i64, account: &str) -> TimelineEvent {
        TimelineEvent::Remove {
            at_ms,
            account: id(account),
        }
    }

    fn active(at_ms: i64, account: Option<&str>) -> ActiveChange {
        ActiveChange {
            at_ms,
            account: account.map(id),
        }
    }

    #[test]
    fn one_home_places_two_homes_conflict() {
        let a = seen(profile("a"), 10, 20);
        assert_eq!(
            place(std::slice::from_ref(&a), &none(), &complete(), 15),
            Placement::Home(profile("a"))
        );
        // Two executions in the same home still name one home.
        let twice = [a.clone(), seen(profile("a"), 12, 30)];
        assert_eq!(
            place(&twice, &none(), &complete(), 15),
            Placement::Home(profile("a"))
        );
        // Open in a profile and the default home at once: unattributed, never guessed.
        let both = [a.clone(), seen(Home::Default, 14, 16)];
        assert_eq!(
            place(&both, &none(), &complete(), 15),
            Placement::Unattributed
        );
        assert_eq!(
            place(&both, &none(), &complete(), 17),
            Placement::Home(profile("a")),
            "outside the overlap, one home again"
        );
        // A sighting places a response whatever launches cover it.
        let other = Launches::new(vec![launch("b", 0, None)]);
        assert_eq!(
            place(&[a], &other, &complete(), 15),
            Placement::Home(profile("a"))
        );
    }

    #[test]
    fn a_sighting_never_covers_before_its_first_or_after_its_last() {
        let a = [seen(profile("a"), 10, 20)];
        let incomplete = Coverage {
            epoch_ms: 1_000,
            seeded: false,
            last_pre_epoch_remove_ms: None,
            unreadable: false,
        };
        for t in [9, 21] {
            assert_eq!(
                place(&a, &none(), &incomplete, t),
                Placement::Unattributed,
                "{t}"
            );
        }
        for t in [10, 20] {
            assert_eq!(
                place(&a, &none(), &incomplete, t),
                Placement::Home(profile("a")),
                "{t}: both ends are inclusive"
            );
        }
        assert_eq!(
            place(&a, &none(), &complete(), 21),
            Placement::Home(Home::Default),
            "after it, with nothing else running, the default home"
        );
    }

    #[test]
    fn unsighted_is_default_only_without_a_covering_launch() {
        let launches = Launches::new(vec![launch("a", 100, Some(200))]);
        assert_eq!(
            place(&[], &launches, &complete(), 99),
            Placement::Home(Home::Default)
        );
        assert_eq!(
            place(&[], &launches, &complete(), 100),
            Placement::Unattributed,
            "a launch covers its start"
        );
        assert_eq!(
            place(&[], &launches, &complete(), 199),
            Placement::Unattributed
        );
        assert_eq!(
            place(&[], &launches, &complete(), 200),
            Placement::Home(Home::Default),
            "and not its end"
        );
    }

    #[test]
    fn an_open_launch_covers_until_closed() {
        let open = Launches::new(vec![launch("a", 100, None)]);
        assert!(!open.any_covers(99));
        assert!(open.any_covers(100) && open.any_covers(i64::MAX - 1));
        let closed = Launches::new(vec![launch("a", 100, Some(150))]);
        assert!(closed.any_covers(149) && !closed.any_covers(150));
        // Overlapping, nested, touching and empty launches cover what they cover together.
        let many = Launches::new(vec![
            launch("b", 300, Some(400)),
            launch("a", 100, Some(200)),
            launch("c", 150, Some(180)),
            launch("d", 200, Some(250)),
            launch("e", 500, Some(500)),
        ]);
        for (t, covered) in [
            (99, false),
            (100, true),
            (199, true),
            (200, true),
            (249, true),
            (250, false),
            (300, true),
            (399, true),
            (400, false),
            (500, false),
        ] {
            assert_eq!(many.any_covers(t), covered, "{t}");
        }
        assert!(!none().any_covers(0));
    }

    #[test]
    fn pre_epoch_coverage_needs_seeding_and_follows_the_last_remove() {
        let unseeded = Coverage {
            epoch_ms: EPOCH,
            seeded: false,
            last_pre_epoch_remove_ms: None,
            unreadable: false,
        };
        assert!(!unseeded.complete_at(EPOCH - 1));
        assert!(
            unseeded.complete_at(EPOCH),
            "from the epoch on, run records every launch"
        );
        let seeded = Coverage {
            seeded: true,
            ..unseeded
        };
        assert!(seeded.complete_at(0));
        let removed = Coverage {
            last_pre_epoch_remove_ms: Some(EPOCH - 10 * H),
            ..seeded
        };
        assert!(
            !removed.complete_at(EPOCH - 10 * H - 1),
            "a removed account may have had a profile that left no trace"
        );
        assert!(removed.complete_at(EPOCH - 10 * H));
        assert_eq!(
            place(&[], &none(), &removed, EPOCH - 11 * H),
            Placement::Unattributed
        );
        assert_eq!(
            place(&[], &none(), &removed, EPOCH - 9 * H),
            Placement::Home(Home::Default)
        );
        let unreadable = Coverage {
            unreadable: true,
            ..seeded
        };
        for t in [0, EPOCH - 1, EPOCH, i64::MAX] {
            assert!(
                !unreadable.complete_at(t),
                "evidence unread is never absence: {t}"
            );
        }
    }

    #[test]
    fn timeline_follows_switches_then_history() {
        let events = [
            add(H, "a"),
            add(H, "b"),
            switch(10 * H, Some("a"), "b"),
            switch(20 * H, Some("b"), "a"),
        ];
        // The epoch at 30 h, seeded with a; b from 40 h; no active account from 50 h.
        let history = [
            active(30 * H, Some("a")),
            active(40 * H, Some("b")),
            active(50 * H, None),
        ];
        let t = Timeline::build(&events, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(10 * H), Some(id("b")), "a switch's own time");
        assert_eq!(t.account_at(15 * H), Some(id("b")));
        assert_eq!(t.account_at(25 * H), Some(id("a")));
        assert_eq!(t.account_at(30 * H), Some(id("a")), "the seeded row");
        assert_eq!(t.account_at(45 * H), Some(id("b")));
        assert_eq!(t.account_at(55 * H), None, "no active account");
        // From the epoch on, only the history counts: a switch event there is not read.
        let late = [add(H, "a"), add(H, "c"), switch(35 * H, Some("a"), "c")];
        let t = Timeline::build(&late, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(36 * H), Some(id("a")));
        assert_eq!(t.account_at(5 * H), Some(id("a")), "before it, its from");
        // From the epoch on, a time before the first history row has no account.
        let t = Timeline::build(
            &events,
            &[active(31 * H, Some("a"))],
            30 * H,
            &BTreeMap::new(),
        );
        assert_eq!(t.account_at(30 * H), None);
    }

    #[test]
    fn timeline_before_the_first_switch_uses_from_since_its_add() {
        let history = [active(30 * H, Some("b"))];
        let events = [
            add(5 * H, "a"),
            add(6 * H, "b"),
            switch(10 * H, Some("a"), "b"),
        ];
        let t = Timeline::build(&events, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(5 * H - 1), None, "before a was added");
        assert_eq!(t.account_at(5 * H), Some(id("a")));
        assert_eq!(t.account_at(10 * H - 1), Some(id("a")));
        assert_eq!(t.account_at(10 * H), Some(id("b")));
        // With no add event, the account row's added_at stands in.
        let bare = [switch(10 * H, Some("a"), "b")];
        let t = Timeline::build(&bare, &history, 30 * H, &BTreeMap::from([(id("a"), 7 * H)]));
        assert_eq!(t.account_at(7 * H - 1), None);
        assert_eq!(t.account_at(8 * H), Some(id("a")));
        // With neither, nothing before the first switch is placed.
        let t = Timeline::build(&bare, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(8 * H), None);
        assert_eq!(t.account_at(11 * H), Some(id("b")));
        // A first switch from no account leaves the time before it unplaced.
        let from_none = [add(H, "b"), switch(10 * H, None, "b")];
        let t = Timeline::build(&from_none, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(5 * H), None);
    }

    #[test]
    fn timeline_without_switches_uses_the_seeded_account() {
        let history = [active(30 * H, Some("a")), active(40 * H, Some("b"))];
        let t = Timeline::build(&[add(5 * H, "a")], &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(4 * H), None, "before its add");
        assert_eq!(t.account_at(5 * H), Some(id("a")));
        assert_eq!(t.account_at(29 * H), Some(id("a")));
        assert_eq!(t.account_at(41 * H), Some(id("b")));
        // Seeded with no active account: nothing before the epoch.
        let t = Timeline::build(
            &[add(5 * H, "a")],
            &[active(30 * H, None)],
            30 * H,
            &BTreeMap::new(),
        );
        assert_eq!(t.account_at(10 * H), None);
        // With no add event left, the account row's added_at stands in.
        let t = Timeline::build(&[], &history, 30 * H, &BTreeMap::from([(id("a"), 2 * H)]));
        assert_eq!(t.account_at(3 * H), Some(id("a")));
    }

    #[test]
    fn a_missing_seed_never_backdates_a_later_activation() {
        // No account was active at the epoch, so the migration seeded no row; `b` became
        // active later without a switch.
        let t = Timeline::build(
            &[add(5 * H, "b")],
            &[active(40 * H, Some("b"))],
            30 * H,
            &BTreeMap::new(),
        );
        assert_eq!(t.account_at(10 * H), None);
        assert_eq!(t.account_at(35 * H), None);
        assert_eq!(t.account_at(41 * H), Some(id("b")));
    }

    #[test]
    fn a_switch_after_the_epoch_leaves_the_pre_epoch_timeline_alone() {
        // `a` is seeded at the epoch, then removed; the first switch ever goes to `b`.
        let events = [
            add(5 * H, "a"),
            remove(32 * H, "a"),
            add(33 * H, "b"),
            switch(35 * H, None, "b"),
        ];
        let history = [
            active(30 * H, Some("a")),
            active(32 * H, None),
            active(35 * H, Some("b")),
        ];
        let t = Timeline::build(&events, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(4 * H), None, "before its add");
        assert_eq!(t.account_at(10 * H), Some(id("a")));
        assert_eq!(t.account_at(29 * H), Some(id("a")));
        assert_eq!(t.account_at(34 * H), None);
        assert_eq!(t.account_at(36 * H), Some(id("b")));

        // One switch before the epoch and one after: the later one changes nothing before.
        let events = [
            add(H, "a"),
            add(H, "b"),
            switch(10 * H, Some("a"), "b"),
            remove(32 * H, "b"),
            switch(35 * H, Some("b"), "a"),
        ];
        let history = [active(30 * H, Some("b")), active(35 * H, Some("a"))];
        let t = Timeline::build(&events, &history, 30 * H, &BTreeMap::new());
        assert_eq!(t.account_at(5 * H), Some(id("a")));
        assert_eq!(t.account_at(10 * H), Some(id("b")));
        assert_eq!(t.account_at(29 * H), Some(id("b")));
        assert_eq!(t.account_at(36 * H), Some(id("a")));
    }

    #[test]
    fn remove_ends_a_segment() {
        let events = [
            add(H, "a"),
            add(H, "b"),
            add(H, "c"),
            switch(10 * H, Some("a"), "b"),
            remove(12 * H, "b"),
            switch(15 * H, None, "c"),
            remove(18 * H, "a"),
        ];
        let t = Timeline::build(
            &events,
            &[active(30 * H, Some("c"))],
            30 * H,
            &BTreeMap::new(),
        );
        assert_eq!(
            t.account_at(5 * H),
            Some(id("a")),
            "a removed account keeps its past"
        );
        assert_eq!(t.account_at(11 * H), Some(id("b")));
        assert_eq!(t.account_at(12 * H), None, "b's remove ends its segment");
        assert_eq!(t.account_at(14 * H), None, "until the next switch");
        assert_eq!(t.account_at(15 * H), Some(id("c")));
        assert_eq!(
            t.account_at(19 * H),
            Some(id("c")),
            "another account's remove ends nothing"
        );
    }

    #[test]
    fn attribute_maps_homes_to_accounts_and_bases() {
        let t = Timeline::build(&[], &[active(0, Some("a"))], 0, &BTreeMap::new());
        assert_eq!(
            attribute(&Placement::Home(profile("p")), &t, 5),
            (Some(id("p")), Basis::Profile)
        );
        assert_eq!(
            attribute(&Placement::Home(Home::Default), &t, 5),
            (Some(id("a")), Basis::Timeline)
        );
        assert_eq!(
            attribute(&Placement::Home(Home::Default), &t, -1),
            (None, Basis::Unattributed),
            "the default home with no account then"
        );
        assert_eq!(
            attribute(&Placement::Unattributed, &t, 5),
            (None, Basis::Unattributed)
        );
    }

    fn home() -> impl Strategy<Value = Home> {
        prop_oneof![Just(Home::Default), Just(profile("a")), Just(profile("b"))]
    }

    fn sighting() -> impl Strategy<Value = Sighting> {
        (home(), 0..100i64, 0..30i64).prop_map(|(home, first, len)| seen(home, first, first + len))
    }

    fn launches() -> impl Strategy<Value = Vec<Launch>> {
        prop::collection::vec(
            (
                prop_oneof![Just("a"), Just("b")],
                0..100i64,
                prop::option::of(0..40i64),
            )
                .prop_map(|(account, start, len)| launch(account, start, len.map(|l| start + l))),
            0..4,
        )
    }

    proptest! {
        #![proptest_config(pinned(512))]

        /// Token report §7.1–7.2: a profile is only ever a sighting's home, and the default
        /// home without one only as the sole candidate.
        #[test]
        fn place_is_never_a_profile_without_a_sighting(
            sightings in prop::collection::vec(sighting(), 0..4),
            launches in launches(),
            epoch_ms in 0..120i64,
            seeded in any::<bool>(),
            last_remove in prop::option::of(0..120i64),
            t in 0..130i64,
        ) {
            let coverage = Coverage {
                epoch_ms,
                seeded,
                last_pre_epoch_remove_ms: last_remove,
                unreadable: false,
            };
            let launches = Launches::new(launches);
            let homes: Vec<&Home> = sightings
                .iter()
                .filter(|s| s.first_ms <= t && t <= s.last_ms)
                .map(|s| &s.home)
                .collect();
            let alone = !launches.any_covers(t) && coverage.complete_at(t);
            match place(&sightings, &launches, &coverage, t) {
                Placement::Home(home) if homes.is_empty() => {
                    prop_assert_eq!(home, Home::Default);
                    prop_assert!(alone);
                }
                Placement::Home(home) => prop_assert!(homes.iter().all(|h| **h == home)),
                Placement::Unattributed => prop_assert!(
                    homes.iter().any(|h| *h != homes[0]) || (homes.is_empty() && !alone)
                ),
            }
        }

        /// Merging launches into spans changes nothing they cover.
        #[test]
        fn merged_launches_cover_exactly_what_their_launches_cover(
            list in launches(),
            t in 0..150i64,
        ) {
            let one_by_one = list
                .iter()
                .any(|l| l.start_ms <= t && l.end_ms.is_none_or(|e| t < e));
            prop_assert_eq!(Launches::new(list).any_covers(t), one_by_one);
        }
    }
}
