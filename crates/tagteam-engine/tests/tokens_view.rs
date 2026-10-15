//! The token report (spec §10, §10.1): grouping, filters, order, blocks, cost, drill-down and
//! the index state, over a store seeded through its own API (usage batches, sightings,
//! launches and events), with Claude Code as the provider.

mod common;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use common::{FakeFx, Fx, LSTART, add, cc, record_reading, usage_window};
use jiff::civil::date;
use jiff::tz::TimeZone;
use serde_json::json;
use tagteam_core::tokens::{BlockSpan, CardOverride, PriceOverride, project};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId, WindowKind};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::{Settings, config_path};
use tagteam_engine::store::{EventRow, SightingRow, SightingSource, Store, UsageRow, usage_key};
use tagteam_engine::tokens::{
    AccountSel, ActiveBlock, Basis, Cost, IndexState, LaunchPoll, PeriodKey, RowAccount,
    TokenCounts, TokenGrouping, TokenPeriod, TokenQuery, TokenReport,
};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_fake::{FAKE_AGENT, FakeAgent};
use tagteam_provider::{FakeProcess, PendingLaunch, Provider, RunShell, parse_lstart};

const MIN: i64 = 60_000;
const H: i64 = 60 * MIN;
const D: i64 = 24 * H;
/// 2026-09-01T00:00:00Z. Every response lies within 40 days of it. The migration dates the
/// store's epoch with the engine clock, which `View::new` sets to `T0 + 40 days` first, so every
/// response predates the epoch and the default home's account comes from `events` (spec §7.3).
const T0: i64 = 1_788_220_800_000;

/// model-a costs $1 / $2 / $3 / $4 / $0.50 per MTok and model-b twice that. Neither is in the
/// shipped table, so both are overrides that add a model (spec §8.2). Any other model is
/// unpriced.
fn settings() -> Settings {
    let card = |k: f64| CardOverride {
        input: Some(k),
        output: Some(2.0 * k),
        cache_write_5m: Some(3.0 * k),
        cache_write_1h: Some(4.0 * k),
        cache_read: Some(0.5 * k),
    };
    let mut prices = BTreeMap::new();
    for (model, k) in [("model-a", 1.0), ("model-b", 2.0)] {
        prices.insert(
            model.to_owned(),
            PriceOverride {
                base: card(k),
                ..PriceOverride::default()
            },
        );
    }
    Settings {
        prices: BTreeMap::from([(cc(), prices)]),
        ..Settings::default()
    }
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input,
        output,
        ..TokenCounts::default()
    }
}

fn keys(r: &TokenReport) -> Vec<&str> {
    r.rows.iter().map(|row| row.key.as_str()).collect()
}

/// A store with two accounts, a@x.co at position 1 and b@x.co at 2, and no evidence.
struct View {
    fx: Fx,
    engine: Engine,
    store: Arc<Store>,
    a: AccountId,
    b: AccountId,
    responses: Cell<u32>,
}

impl View {
    fn new() -> Self {
        let fx = Fx::new();
        // Before the store opens: its migration dates the epoch with this clock.
        fx.clock.set(T0 + 40 * D);
        let engine = fx.engine_with_settings(settings());
        let store = engine.store().unwrap();
        let a = add(&store, &cc(), "acct-a", "a@x.co", 1);
        let b = add(&store, &cc(), "acct-b", "b@x.co", 2);
        let epoch = store
            .token_epoch()
            .unwrap()
            .expect("step 3 records the epoch");
        assert_eq!(
            epoch.at,
            T0 + 40 * D,
            "the responses these tests index must predate the store's epoch"
        );
        assert!(
            epoch.seeded,
            "an unsighted default-home response before the epoch is placed only on a seeded epoch"
        );
        View {
            fx,
            engine,
            store,
            a,
            b,
            responses: Cell::new(0),
        }
    }

    /// A finalized response of `session` at `at`, of `input` and `output` tokens of `model`.
    fn usage(&self, session: &str, at: i64, model: &str, input: u64, output: u64) -> UsageRow {
        let n = self.responses.get() + 1;
        self.responses.set(n);
        UsageRow {
            key: usage_key(&cc(), &format!("msg-{n}"), &format!("req-{n}")),
            session_id: session.to_owned(),
            project: Some(format!("/work/{session}")),
            at_ms: at,
            model: model.to_owned(),
            sidechain: false,
            counts: counts(input, output),
            speed: None,
            finalized: true,
        }
    }

    /// One batch of a refresh that has not completed (spec §5.2).
    fn index(&self, rows: &[UsageRow]) {
        self.store
            .commit_token_batch(
                &cc(),
                self.generation(),
                rows,
                &[],
                &[],
                0,
                self.engine.now_ms(),
            )
            .unwrap();
    }

    /// The purge generation a refresh begins with (Task 12's `begin_token_refresh`).
    fn generation(&self) -> i64 {
        self.store
            .begin_token_refresh(&cc(), self.engine.now_ms())
            .unwrap()
    }

    /// `session` seen alive in `home` ("" for the default home) from `first` to `last`.
    fn sight(&self, session: &str, home: &str, first: i64, last: i64) {
        let row = SightingRow {
            session_id: session.to_owned(),
            home: home.to_owned(),
            started_at: first,
            first_at: first,
            last_at: last,
            source: SightingSource::Scan,
            recorded_at: self.engine.now_ms(),
        };
        assert!(self.store.record_sighting(&cc(), &row, MIN).unwrap());
    }

    fn event(&self, at: i64, kind: &str, from: Option<&AccountId>, to: Option<&AccountId>) {
        let row = EventRow {
            at,
            provider: cc(),
            kind: kind.into(),
            from_id: from.cloned(),
            to_id: to.cloned(),
            trigger: None,
            source: "cli".into(),
            detail: None,
        };
        self.store.insert_event(&row, true).unwrap();
    }

    fn query(&self, period: TokenPeriod, grouping: TokenGrouping) -> TokenQuery {
        TokenQuery {
            provider: cc(),
            period,
            grouping,
            since_ms: None,
            until_ms: None,
            accounts: None,
            models: None,
            session: None,
            tz: TimeZone::UTC,
        }
    }

    fn report(&self, q: &TokenQuery) -> TokenReport {
        self.engine.token_report(q).unwrap()
    }

    fn account(&self, id: &AccountId) -> RowAccount {
        let row = self.store.account(id).unwrap().unwrap();
        RowAccount::Account {
            id: row.id,
            position: row.position,
            label: row.label,
            alias: row.alias,
        }
    }
}

#[test]
fn days_group_by_account_and_model_newest_first() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + 3 * D);
    v.sight("s-b", "acct-b", T0, T0 + 3 * D);
    v.index(&[
        v.usage("s-a", T0 + H, "model-b", 10, 1),
        v.usage("s-a", T0 + 2 * H, "model-a", 20, 2),
        v.usage("s-b", T0 + 3 * H, "model-a", 40, 4),
        v.usage("s-a", T0 + D + H, "model-a", 30, 3),
        v.usage("s-b", T0 + D + 2 * H, "model-b", 50, 5),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::AccountModel));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-02|account:acct-a|model:model-a",
            "day:2026-09-02|account:acct-b|model:model-b",
            "day:2026-09-01|account:acct-a|model:model-a",
            "day:2026-09-01|account:acct-a|model:model-b",
            "day:2026-09-01|account:acct-b|model:model-a",
        ]
    );
    let first = &r.rows[0];
    assert_eq!(
        first.period,
        PeriodKey::Day {
            date: date(2026, 9, 2)
        }
    );
    assert_eq!(first.account, Some(v.account(&v.a)));
    assert_eq!(first.model.as_deref(), Some("model-a"));
    assert_eq!(first.accounts, [v.account(&v.a)]);
    assert_eq!(first.models, ["model-a"]);
    assert_eq!(
        (first.counts, first.responses, first.unfinalized),
        (counts(30, 3), 1, 0)
    );
    assert_eq!(r.rows[4].counts, counts(40, 4));

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
    assert_eq!(keys(&r), ["day:2026-09-02", "day:2026-09-01"]);
    let day = &r.rows[1];
    assert_eq!((day.account.as_ref(), day.model.as_deref()), (None, None));
    assert_eq!(day.accounts, [v.account(&v.a), v.account(&v.b)]);
    assert_eq!(day.models, ["model-a", "model-b"]);
    assert_eq!((day.counts, day.responses), (counts(70, 7), 3));
    assert_eq!((r.total.counts, r.total.responses), (counts(150, 15), 5));

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Model));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-02|model:model-a",
            "day:2026-09-02|model:model-b",
            "day:2026-09-01|model:model-a",
            "day:2026-09-01|model:model-b",
        ]
    );
}

#[test]
fn months_follow_the_query_zone() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0 - D, T0 + D);
    // 2026-08-31T23:30Z is still August in UTC, and 2026-09-01T01:30 in Brussels (CEST).
    v.index(&[
        v.usage("s-a", T0 - 30 * MIN, "model-a", 1, 0),
        v.usage("s-a", T0 + 2 * H, "model-a", 2, 0),
    ]);

    let utc = v.report(&v.query(TokenPeriod::Month, TokenGrouping::None));
    assert_eq!(utc.time_zone, "UTC");
    assert_eq!(keys(&utc), ["month:2026-09", "month:2026-08"]);
    assert_eq!(
        utc.rows[1].period,
        PeriodKey::Month {
            year: 2026,
            month: 8
        }
    );
    assert_eq!(
        keys(&v.report(&v.query(TokenPeriod::Day, TokenGrouping::None))),
        ["day:2026-09-01", "day:2026-08-31"]
    );

    let brussels = TimeZone::get("Europe/Brussels").unwrap();
    let local = v.report(&TokenQuery {
        tz: brussels.clone(),
        ..v.query(TokenPeriod::Month, TokenGrouping::None)
    });
    assert_eq!(local.time_zone, "Europe/Brussels");
    assert_eq!(keys(&local), ["month:2026-09"]);
    assert_eq!(local.rows[0].counts, counts(3, 0));
    let local = v.report(&TokenQuery {
        tz: brussels,
        ..v.query(TokenPeriod::Day, TokenGrouping::None)
    });
    assert_eq!(keys(&local), ["day:2026-09-01"]);
}

#[test]
fn a_session_spanning_two_accounts_splits_by_account() {
    let v = View::new();
    // s-1 ran in a's profile, then was resumed in b's (spec §7.4 A1).
    v.sight("s-1", "acct-a", T0, T0 + 2 * H);
    v.sight("s-1", "acct-b", T0 + 3 * H, T0 + 5 * H);
    v.sight("s-2", "acct-a", T0 + 6 * H, T0 + 7 * H);
    v.index(&[
        v.usage("s-1", T0 + H, "model-a", 1, 0),
        v.usage("s-1", T0 + 4 * H, "model-b", 2, 0),
        v.usage("s-2", T0 + 6 * H + 30 * MIN, "model-a", 4, 0),
    ]);
    let s1 = |first_ms, last_ms| PeriodKey::Session {
        session_id: "s-1".into(),
        project: Some("/work/s-1".into()),
        first_ms,
        last_ms,
    };

    let r = v.report(&v.query(TokenPeriod::Session, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "session:s-2|account:acct-a",
            "session:s-1|account:acct-a",
            "session:s-1|account:acct-b",
        ]
    );
    assert_eq!(r.rows[1].period, s1(T0 + H, T0 + H));
    assert_eq!(r.rows[2].period, s1(T0 + 4 * H, T0 + 4 * H));
    assert_eq!(r.rows[2].account, Some(v.account(&v.b)));

    let r = v.report(&v.query(TokenPeriod::Session, TokenGrouping::None));
    assert_eq!(keys(&r), ["session:s-2", "session:s-1"]);
    let row = &r.rows[1];
    assert_eq!(row.period, s1(T0 + H, T0 + 4 * H));
    assert_eq!(row.accounts, [v.account(&v.a), v.account(&v.b)]);
    assert_eq!(row.models, ["model-a", "model-b"]);
    assert_eq!((row.counts, row.responses), (counts(3, 0), 2));
}

#[test]
fn filters_apply_before_grouping_and_some_empty_matches_nothing() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + D);
    v.sight("s-b", "acct-b", T0, T0 + D);
    // Open in both profiles at once: unattributed (spec §7.1).
    v.sight("s-u", "acct-a", T0, T0 + D);
    v.sight("s-u", "acct-b", T0, T0 + D);
    v.index(&[
        v.usage("s-a", T0 + H, "model-a", 1, 0),
        v.usage("s-a", T0 + 2 * H, "model-b", 2, 0),
        v.usage("s-b", T0 + 3 * H, "model-a", 4, 0),
        v.usage("s-u", T0 + 4 * H, "model-a", 8, 0),
    ]);
    let day = v.query(TokenPeriod::Day, TokenGrouping::None);
    let only_row = |q: TokenQuery| {
        let r = v.report(&q);
        assert_eq!(r.rows.len(), 1, "{q:?}");
        r.rows.into_iter().next().unwrap()
    };

    let row = only_row(TokenQuery {
        accounts: Some(vec![AccountSel::Account(v.a.clone())]),
        ..day.clone()
    });
    assert_eq!(row.accounts, [v.account(&v.a)]);
    assert_eq!(row.models, ["model-a", "model-b"]);
    assert_eq!(row.counts, counts(3, 0));

    let row = only_row(TokenQuery {
        models: Some(vec!["model-b".into()]),
        ..day.clone()
    });
    assert_eq!(
        (row.accounts, row.models, row.counts),
        (
            vec![v.account(&v.a)],
            vec!["model-b".to_owned()],
            counts(2, 0)
        )
    );

    let row = only_row(TokenQuery {
        accounts: Some(vec![
            AccountSel::Unattributed,
            AccountSel::Account(v.b.clone()),
        ]),
        ..day.clone()
    });
    assert_eq!(row.accounts, [v.account(&v.b), RowAccount::Unattributed]);
    assert_eq!(row.counts, counts(12, 0));

    let row = only_row(TokenQuery {
        session: Some("s-b".into()),
        ..day.clone()
    });
    assert_eq!(row.counts, counts(4, 0));

    for q in [
        TokenQuery {
            accounts: Some(vec![]),
            ..day.clone()
        },
        TokenQuery {
            models: Some(vec![]),
            ..day.clone()
        },
        // Exact names only: the CLI resolves a `--model` substring first (spec §11.1).
        TokenQuery {
            models: Some(vec!["model".into()]),
            ..day.clone()
        },
    ] {
        let r = v.report(&q);
        assert!(r.rows.is_empty(), "{q:?}");
        assert_eq!((r.total.responses, r.total.cost), (0, None), "{q:?}");
    }
}

#[test]
fn removed_accounts_keep_their_rows() {
    let v = View::new();
    let c = add(&v.store, &cc(), "acct-c", "c@x.co", 3);
    // c's profile session, and a default-home session while c was the login (spec §7.3).
    v.sight("s-c", "acct-c", T0, T0 + D);
    v.event(T0 - D, "switch", None, Some(&c));
    v.sight("s-d", "", T0, T0 + D);
    v.sight("s-a", "acct-a", T0, T0 + D);
    v.index(&[
        v.usage("s-a", T0 + H, "model-a", 1, 0),
        v.usage("s-c", T0 + 2 * H, "model-a", 2, 0),
        v.usage("s-d", T0 + 3 * H, "model-a", 4, 0),
    ]);
    v.store.delete_account(&c).unwrap();
    v.event(T0 + 2 * D, "remove", Some(&c), None);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-01|account:acct-a",
            "day:2026-09-01|account:acct-c"
        ]
    );
    assert_eq!(
        r.rows[1].account,
        Some(RowAccount::Removed { id: c.clone() })
    );
    assert_eq!(r.rows[1].counts, counts(6, 0));
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 3),
            (Basis::Timeline, 4),
            (Basis::Unattributed, 0)
        ]
    );

    // A filter naming the removed account's id matches its rows (spec §10.1).
    let r = v.report(&TokenQuery {
        accounts: Some(vec![AccountSel::Account(c.clone())]),
        ..v.query(TokenPeriod::Day, TokenGrouping::None)
    });
    assert_eq!(keys(&r), ["day:2026-09-01"]);
    assert_eq!(r.rows[0].accounts, [RowAccount::Removed { id: c }]);
}

#[test]
fn unattributed_is_its_own_bucket_and_last() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + D);
    v.sight("s-b", "acct-b", T0, T0 + D);
    // b's profile may have been running, and s-u was never sighted: the default home is not
    // its only candidate, so it is unattributed (spec §7.2).
    v.store
        .open_launch(&cc(), v.b.as_str(), T0 - D, v.engine.now_ms())
        .unwrap();
    v.index(&[
        v.usage("s-u", T0 + H, "model-a", 1, 0),
        v.usage("s-b", T0 + 2 * H, "model-a", 2, 0),
        v.usage("s-a", T0 + 3 * H, "model-a", 4, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-01|account:acct-a",
            "day:2026-09-01|account:acct-b",
            "day:2026-09-01|account:unattributed",
        ]
    );
    assert_eq!(r.rows[2].account, Some(RowAccount::Unattributed));
    assert_eq!(r.rows[2].counts, counts(1, 0));

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
    assert_eq!(
        r.rows[0].accounts,
        [v.account(&v.a), v.account(&v.b), RowAccount::Unattributed]
    );
}

#[test]
fn a_session_resumed_in_a_running_profile_keeps_its_earlier_responses_off_that_account() {
    // Spec §1.1 criterion 2, the example of the sighting fix: b's `run` child starts at +100 s;
    // s-1 answers in the default home at +200 s, unsighted there, and exits; the child does
    // `/resume s-1` at +300 s. `run`'s checks sight s-1 in b's profile from the read that first
    // named it there, so the answer at +200 s is not b's: b's launch covers it, so it is
    // unattributed (spec §7.2). The answer at +350 s, inside the sighting, is b's.
    let v = View::new();
    let epoch = v.engine.now_ms();
    let profile = v.fx.make_profile(&v.b);
    let at = |s: i64| epoch + s * 1_000;
    v.fx.clock.set(at(100));
    let id = v.engine.open_launch(&cc(), &v.b).unwrap();
    v.fx.process.set(
        777,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART),
            ..FakeProcess::default()
        },
    );
    let mut state = LaunchPoll::default();
    let names = |session: &str, mtime_s: u64| {
        let record = json!({
            "pid": 777,
            "procStart": LSTART,
            "startedAt": at(100) + 500,
            "kind": "interactive",
            "sessionId": session
        });
        let path =
            v.fx.plant_record(&profile, "777", record.to_string().as_bytes());
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(mtime_s))
            .unwrap();
        path
    };
    let mut tick = |when: i64| {
        v.fx.clock.set(when);
        v.engine
            .observe_launch(&id, &profile, 777, &mut state)
            .unwrap();
        v.engine.poll_launch(&id, &mut state).unwrap();
    };
    names("s-0", 1_790_000_000);
    tick(at(101));
    names("s-1", 1_790_000_001);
    tick(at(300));
    let path = names("s-1", 1_790_000_002);
    tick(at(360));
    fs::remove_file(path).unwrap();
    v.engine
        .close_launch(&id, &profile, &state, at(400), true)
        .stored
        .unwrap();
    v.index(&[
        v.usage("s-1", at(200), "model-a", 1, 0),
        v.usage("s-1", at(350), "model-a", 2, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        r.rows
            .iter()
            .map(|row| (row.account.clone(), row.counts))
            .collect::<Vec<_>>(),
        [
            (Some(v.account(&v.b)), counts(2, 0)),
            (Some(RowAccount::Unattributed), counts(1, 0))
        ]
    );
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 2),
            (Basis::Timeline, 0),
            (Basis::Unattributed, 1)
        ]
    );
}

#[test]
fn a_switch_places_an_unsighted_response_unless_a_launch_may_cover_it() {
    let v = View::new();
    // b was the default home's login from T0-1d (spec §7.3), and b's profile may have been
    // running from T0+4h on, so a response from then on may be one of its sessions (spec §7.2).
    v.event(T0 - D, "switch", None, Some(&v.b));
    v.store
        .open_launch(&cc(), v.b.as_str(), T0 + 4 * H, v.engine.now_ms())
        .unwrap();
    v.index(&[
        v.usage("s-1", T0 + H, "model-a", 1, 0),
        v.usage("s-2", T0 + 5 * H, "model-a", 2, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-01|account:acct-b",
            "day:2026-09-01|account:unattributed"
        ]
    );
    assert_eq!(r.rows[0].account, Some(v.account(&v.b)));
    assert_eq!(r.rows[0].counts, counts(1, 0));
    assert_eq!(r.rows[1].counts, counts(2, 0));
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 0),
            (Basis::Timeline, 1),
            (Basis::Unattributed, 2)
        ]
    );
}

#[test]
fn a_pending_launch_leaves_an_unsighted_response_unattributed_without_the_scan() {
    // Spec §7.2: a launch `run` kept in a pending file counts as a stored one, so a report made
    // before the next scan (`--no-refresh`) leaves a response inside it unattributed, where the
    // default home's login would have been credited without it. The report only reads: the file
    // stays, and the store gains no launch.
    let v = View::new();
    v.event(T0 - D, "switch", None, Some(&v.b));
    v.index(&[v.usage("s-2", T0 + 5 * H, "model-a", 2, 0)]);
    let q = v.query(TokenPeriod::Day, TokenGrouping::Account);

    let r = v.report(&q);
    assert_eq!(keys(&r), ["day:2026-09-01|account:acct-b"]);
    assert_eq!(r.total.by_basis[1], (Basis::Timeline, 2));

    let cc_provider = v.engine.provider(&cc()).unwrap();
    let profile = v.fx.make_profile_for(cc_provider.as_ref(), &v.a);
    PendingLaunch {
        provider: cc(),
        account_id: v.a.clone(),
        started_at_ms: T0 + 4 * H,
        ended_at_ms: None,
    }
    .write(&profile, 4242)
    .unwrap();
    let r = v.report(&q);
    assert_eq!(keys(&r), ["day:2026-09-01|account:unattributed"]);
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 0),
            (Basis::Timeline, 0),
            (Basis::Unattributed, 2)
        ]
    );
    assert_eq!(
        PendingLaunch::read_all(&profile).present().unwrap().len(),
        1,
        "the file stays"
    );
    assert!(v.store.launches(&cc()).unwrap().is_empty());

    // A pending launch that ended before the response does not cover it.
    PendingLaunch {
        provider: cc(),
        account_id: v.a.clone(),
        started_at_ms: T0 + 4 * H,
        ended_at_ms: Some(T0 + 5 * H - 1),
    }
    .write(&profile, 4242)
    .unwrap();
    assert_eq!(
        keys(&v.report(&q)),
        ["day:2026-09-01|account:acct-b"],
        "its end bounds it"
    );
}

#[test]
fn a_pending_file_that_cannot_be_read_stands_for_a_launch_open_from_its_name() {
    // Spec §6.1, §7.2: a pending file whose content cannot be used still names its launch's
    // start, `<started_at ms>-<pid>.json`. The report takes it as a launch of the profile's
    // account open from there, so the unsighted response inside it is unattributed rather than
    // credited to b. The end in the file is not read, so the launch over-covers.
    use std::os::unix::fs::PermissionsExt;
    let v = View::new();
    v.event(T0 - D, "switch", None, Some(&v.b));
    v.index(&[v.usage("s-2", T0 + 5 * H, "model-a", 2, 0)]);
    let cc_provider = v.engine.provider(&cc()).unwrap();
    let profile = v.fx.make_profile_for(cc_provider.as_ref(), &v.a);
    PendingLaunch {
        provider: cc(),
        account_id: v.a.clone(),
        started_at_ms: T0 + 4 * H,
        ended_at_ms: Some(T0 + 4 * H + MIN),
    }
    .write(&profile, 4242)
    .unwrap();
    let file = PendingLaunch::path(&profile, T0 + 4 * H, 4242);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));

    assert_eq!(keys(&r), ["day:2026-09-01|account:unattributed"]);
    assert!(file.exists(), "the report only reads");
    assert!(v.store.launches(&cc()).unwrap().is_empty());
}

/// For the races below: b is the default home's login, and an unsighted response of `s-2` at
/// T0+5h lies inside a launch of a's profile from T0+4h, which only a pending file holds yet.
/// Returns the profile and that launch, not yet written.
#[cfg(feature = "test-hooks")]
fn unsighted_inside_a_pending_launch(v: &View) -> (std::path::PathBuf, PendingLaunch) {
    v.event(T0 - D, "switch", None, Some(&v.b));
    v.index(&[v.usage("s-2", T0 + 5 * H, "model-a", 2, 0)]);
    let cc_provider = v.engine.provider(&cc()).unwrap();
    let profile = v.fx.make_profile_for(cc_provider.as_ref(), &v.a);
    let launch = PendingLaunch {
        provider: cc(),
        account_id: v.a.clone(),
        started_at_ms: T0 + 4 * H,
        ended_at_ms: Some(T0 + 6 * H),
    };
    (profile, launch)
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_pending_file_written_after_the_report_read_the_usage_still_covers_its_responses() {
    // Spec §7.2: the report reads the usage, then the pending files. `run` keeps the launch in
    // its file before it spawns the child, so the launch of every response the usage holds is
    // in the store or a file by the time the files are read. The file appears right after the
    // usage read here; read the other way round, the response goes to b.
    let v = View::new();
    let (profile, launch) = unsighted_inside_a_pending_launch(&v);
    v.engine.on_point(
        "report-usage-read",
        Box::new(move || launch.write(&profile, 4242).unwrap()),
    );

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));

    assert_eq!(keys(&r), ["day:2026-09-01|account:unattributed"]);
    assert_eq!(r.total.by_basis[2], (Basis::Unattributed, 2));
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_pending_launch_a_scan_stores_while_the_report_reads_still_covers_its_responses() {
    // Spec §7.2: the report reads the stored launches after the pending files, and a scan
    // removes a file only once its launch is stored (spec §6.2). A scan that moves the launch
    // between the two reads leaves it in one of them; read the other way round, the report
    // would miss it in both and credit the response to b.
    let v = View::new();
    let (profile, launch) = unsighted_inside_a_pending_launch(&v);
    launch.write(&profile, 4242).unwrap();
    let scan = v.fx.engine_with_settings(settings());
    v.engine.on_point(
        "report-pending-read",
        Box::new(move || {
            scan.sight_sessions(&cc()).unwrap();
        }),
    );

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));

    let stored = v.store.launches(&cc()).unwrap();
    assert_eq!(stored.len(), 1, "the premise: the scan stored it");
    assert!(
        PendingLaunch::read_all(&profile)
            .present()
            .unwrap()
            .is_empty(),
        "and removed its file"
    );
    assert_eq!(keys(&r), ["day:2026-09-01|account:unattributed"]);
    assert_eq!(r.total.by_basis[2], (Basis::Unattributed, 2));
}

#[test]
fn a_remove_before_the_epoch_leaves_earlier_unsighted_responses_unattributed() {
    let v = View::new();
    let c = AccountId::from_string("acct-c");
    v.event(T0 - D, "switch", None, Some(&v.b));
    // An account removed at T0+2h may have had a profile that left no trace: launch coverage is
    // complete only after it (spec §7.2).
    v.event(T0 + 2 * H, "remove", Some(&c), None);
    v.index(&[
        v.usage("s-1", T0 + H, "model-a", 1, 0),
        v.usage("s-2", T0 + 3 * H, "model-a", 2, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-01|account:acct-b",
            "day:2026-09-01|account:unattributed"
        ]
    );
    assert_eq!(r.rows[0].counts, counts(2, 0));
    assert_eq!(r.rows[1].counts, counts(1, 0));
}

#[test]
fn recovered_switches_and_imports_move_the_default_homes_login() {
    let v = View::new();
    // c was imported and is gone from the store: it has no other record of being added.
    let c = AccountId::from_string("acct-c");
    v.event(T0 - 5 * D, "import", None, Some(&c));
    v.event(T0 - 3 * D, "switch", Some(&c), Some(&v.b));
    v.event(T0 - D, "switch-recovered", Some(&v.b), Some(&v.a));
    v.index(&[
        v.usage("s-1", T0 - 4 * D, "model-a", 1, 0),
        v.usage("s-2", T0 - 2 * D, "model-a", 2, 0),
        v.usage("s-3", T0 - 12 * H, "model-a", 4, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-08-31|account:acct-a",
            "day:2026-08-30|account:acct-b",
            "day:2026-08-28|account:acct-c",
        ]
    );
    assert_eq!(r.rows[0].account, Some(v.account(&v.a)));
    assert_eq!(r.rows[1].account, Some(v.account(&v.b)));
    assert_eq!(r.rows[2].account, Some(RowAccount::Removed { id: c }));
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 0),
            (Basis::Timeline, 7),
            (Basis::Unattributed, 0)
        ]
    );
}

#[test]
fn after_the_epoch_the_default_homes_login_comes_from_the_history() {
    let v = View::new();
    // The history trigger stamps the database's own clock, so read back the time it recorded
    // (as `index_state_moves_on_new_evidence_alone` does).
    v.store.set_active(&cc(), Some(&v.a), Some(0)).unwrap();
    let switched_at = v.store.active_history(&cc()).unwrap().last().unwrap().0;
    let epoch = v.store.token_epoch().unwrap().unwrap().at;
    assert!(
        switched_at - MIN >= epoch,
        "the database's clock must be past the fixture epoch for this to be a post-epoch case"
    );
    v.index(&[
        v.usage("s-1", switched_at - MIN, "model-a", 1, 0),
        v.usage("s-2", switched_at + MIN, "model-a", 2, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    let counts_of = |account: RowAccount| -> Vec<TokenCounts> {
        r.rows
            .iter()
            .filter(|row| row.account.as_ref() == Some(&account))
            .map(|row| row.counts)
            .collect()
    };
    // Before the login change nothing names the default home's account.
    assert_eq!(counts_of(v.account(&v.a)), [counts(2, 0)]);
    assert_eq!(counts_of(RowAccount::Unattributed), [counts(1, 0)]);
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 0),
            (Basis::Timeline, 2),
            (Basis::Unattributed, 1)
        ]
    );
}

#[test]
fn blocks_are_per_account_with_the_active_projection() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + D);
    v.sight("s-b", "acct-b", T0, T0 + D);
    // a's 5h window, read at T0+1h30, resets at T0+6h: the observed block [T0+1h, T0+6h). Its
    // 7d window is not the short window and makes no block (spec §9), though its reset at T0+10h
    // would make one holding two responses.
    record_reading(
        &v.engine,
        &v.a,
        &[
            usage_window("5h", WindowKind::Short, 10.0, (T0 + 6 * H) / 1000),
            usage_window("7d", WindowKind::Long, 5.0, (T0 + 10 * H) / 1000),
        ],
        (T0 + 90 * MIN) / 1000,
        (T0 + 2 * H) / 1000,
    );
    v.index(&[
        v.usage("s-a", T0 + 2 * H, "model-a", 100, 0),
        v.usage("s-b", T0 + 150 * MIN, "model-a", 200, 0),
        v.usage("s-a", T0 + 5 * H, "model-b", 300, 0),
        // Past the observed block: an inferred one, [T0+7h, T0+12h).
        v.usage("s-a", T0 + 450 * MIN, "model-a", 600, 0),
    ]);
    let now = T0 + 8 * H;
    v.fx.clock.set(now);
    let at = |hours: i64| T0 + hours * H;
    let block = |hours: i64, account: &str| format!("block:{}|account:{account}", at(hours));

    let q = v.query(TokenPeriod::Block, TokenGrouping::None);
    let r = v.report(&q);
    assert_eq!(
        keys(&r),
        [block(7, "acct-a"), block(2, "acct-b"), block(1, "acct-a")]
    );
    // 600 tokens of model-a, 30 minutes after the block's first response, 4 h before its end.
    let span = BlockSpan {
        start_ms: at(7),
        end_ms: at(12),
        observed: false,
    };
    let want = project(&span, T0 + 450 * MIN, now, 600, Some(600_000)).unwrap();
    assert_eq!(want.remaining_s, 14_400);
    let priced = |nanos: Option<u64>| {
        nanos.map(|usd_nanos| Cost {
            usd_nanos,
            partial: false,
        })
    };
    assert_eq!(
        r.rows[0].period,
        PeriodKey::Block {
            start_ms: at(7),
            end_ms: at(12),
            observed: false,
            active: Some(ActiveBlock {
                elapsed_s: want.elapsed_s,
                remaining_s: want.remaining_s,
                tokens_per_min: want.tokens_per_min,
                cost_per_hour: priced(want.cost_nanos_per_hour),
                projected_tokens: want.projected_tokens,
                projected_cost: priced(want.projected_cost_nanos),
            }),
        }
    );
    assert_eq!(
        r.rows[1].period,
        PeriodKey::Block {
            start_ms: at(2),
            end_ms: at(7),
            observed: false,
            active: None,
        }
    );
    assert_eq!(
        r.rows[2].period,
        PeriodKey::Block {
            start_ms: at(1),
            end_ms: at(6),
            observed: true,
            active: None,
        }
    );
    assert_eq!(r.rows[2].counts, counts(400, 0));

    let r = v.report(&v.query(TokenPeriod::Block, TokenGrouping::Model));
    assert_eq!(
        keys(&r)[2..],
        [
            format!("{}|model:model-a", block(1, "acct-a")),
            format!("{}|model:model-b", block(1, "acct-a")),
        ]
    );

    // A block that straddles `since` is reported whole; one that ends by it is not.
    let q = TokenQuery {
        since_ms: Some(at(3)),
        ..q
    };
    let r = v.report(&q);
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.rows[2].counts, counts(400, 0));
    assert_eq!(
        q.drill(&r.rows[2]),
        Some(TokenQuery {
            period: TokenPeriod::Session,
            since_ms: Some(at(1)),
            until_ms: Some(at(6)),
            accounts: Some(vec![AccountSel::Account(v.a.clone())]),
            ..q.clone()
        })
    );
    let r = v.report(&TokenQuery {
        since_ms: Some(at(6) + 30 * MIN),
        ..q
    });
    assert_eq!(keys(&r), [block(7, "acct-a"), block(2, "acct-b")]);
    assert_eq!(r.rows[1].counts, counts(200, 0));
}

#[test]
fn a_filter_never_changes_a_blocks_burn_window() {
    let v = View::new();
    v.sight("s-1", "acct-a", T0, T0 + D);
    v.sight("s-2", "acct-a", T0, T0 + D);
    // One block, [T0, T0+5h), active at T0+4h30: its burn rate runs from its first response, at
    // T0, whichever responses a filter keeps (spec §9).
    v.index(&[
        v.usage("s-1", T0, "model-a", 100, 0),
        v.usage("s-2", T0 + 4 * H, "model-b", 600, 0),
    ]);
    let now = T0 + 270 * MIN;
    v.fx.clock.set(now);
    let span = BlockSpan {
        start_ms: T0,
        end_ms: T0 + 5 * H,
        observed: false,
    };
    // model-b: 600 × 2,000 nano-dollars.
    let want = project(&span, T0, now, 600, Some(1_200_000)).unwrap();
    let by_model = v.query(TokenPeriod::Block, TokenGrouping::Model);
    let key = format!("block:{T0}|account:acct-a|model:model-b");
    for q in [
        by_model.clone(),
        TokenQuery {
            models: Some(vec!["model-b".into()]),
            ..by_model.clone()
        },
        TokenQuery {
            session: Some("s-2".into()),
            ..by_model.clone()
        },
    ] {
        let r = v.report(&q);
        let row = r
            .rows
            .iter()
            .find(|row| row.key.as_str() == key)
            .unwrap_or_else(|| panic!("{q:?}: no row {key}"));
        let PeriodKey::Block {
            active: Some(active),
            ..
        } = &row.period
        else {
            panic!("{q:?}: not the active block: {:?}", row.period);
        };
        assert_eq!(row.counts, counts(600, 0), "{q:?}");
        assert_eq!(
            (
                active.elapsed_s,
                active.tokens_per_min,
                active.projected_tokens
            ),
            (want.elapsed_s, want.tokens_per_min, want.projected_tokens),
            "{q:?}"
        );
    }
}

#[test]
fn cost_is_partial_with_an_unpriced_model_and_none_when_all_are() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0, T0 + 3 * D);
    v.index(&[
        v.usage("s-a", T0 + H, "model-a", 10, 20),
        v.usage("s-a", T0 + 2 * H, "model-x", 5, 0),
        v.usage("s-a", T0 + D + H, "model-x", 7, 0),
    ]);
    // model-a: 10 × 1,000 + 20 × 2,000 nano-dollars (spec §8.3).
    let priced = |partial| Cost {
        usd_nanos: 50_000,
        partial,
    };

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
    assert_eq!(keys(&r), ["day:2026-09-02", "day:2026-09-01"]);
    assert_eq!((r.rows[0].cost, r.rows[1].cost), (None, Some(priced(true))));
    assert_eq!(r.total.cost, Some(priced(true)));
    assert_eq!(r.unpriced_models, ["model-x"]);
    assert_eq!(r.price_overrides, ["model-a", "model-b"]);
    assert_eq!(r.prices_as_of, v.fx.cc.price_table().as_of);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Model));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-02|model:model-x",
            "day:2026-09-01|model:model-a",
            "day:2026-09-01|model:model-x",
        ]
    );
    let costs: Vec<Option<Cost>> = r.rows.iter().map(|row| row.cost).collect();
    assert_eq!(costs, [None, Some(priced(false)), None]);
}

#[test]
fn drill_from_a_day_gives_its_sessions_and_from_a_session_its_models() {
    let v = View::new();
    for (session, home) in [
        ("s-1", "acct-a"),
        ("s-2", "acct-a"),
        ("s-3", "acct-a"),
        ("s-4", "acct-b"),
    ] {
        v.sight(session, home, T0, T0 + 3 * D);
    }
    v.index(&[
        v.usage("s-1", T0 + 15 * MIN, "model-a", 1, 0),
        v.usage("s-1", T0 + H, "model-a", 2, 0),
        v.usage("s-1", T0 + 2 * H, "model-b", 4, 0),
        v.usage("s-2", T0 + 3 * H, "model-a", 8, 0),
        v.usage("s-4", T0 + 4 * H, "model-a", 16, 0),
        v.usage("s-3", T0 + D + H, "model-a", 32, 0),
    ]);
    let since = T0 + 30 * MIN;
    let q = TokenQuery {
        since_ms: Some(since),
        ..v.query(TokenPeriod::Day, TokenGrouping::Account)
    };
    let r = v.report(&q);
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-02|account:acct-a",
            "day:2026-09-01|account:acct-a",
            "day:2026-09-01|account:acct-b",
        ]
    );
    let day = &r.rows[1];

    // The day within the query's range, its account as the filter, the same grouping.
    let sessions = q.drill(day).unwrap();
    assert_eq!(
        sessions,
        TokenQuery {
            period: TokenPeriod::Session,
            since_ms: Some(since),
            until_ms: Some(T0 + D),
            accounts: Some(vec![AccountSel::Account(v.a.clone())]),
            ..q.clone()
        }
    );
    let r = v.report(&sessions);
    assert_eq!(
        keys(&r),
        ["session:s-2|account:acct-a", "session:s-1|account:acct-a"]
    );
    assert_eq!(r.total.counts, day.counts);

    // A session: the whole session, by account and model.
    let models = sessions.drill(&r.rows[1]).unwrap();
    assert_eq!(
        models,
        TokenQuery {
            grouping: TokenGrouping::AccountModel,
            since_ms: None,
            until_ms: None,
            session: Some("s-1".into()),
            ..sessions.clone()
        }
    );
    let r = v.report(&models);
    assert_eq!(
        keys(&r),
        [
            "session:s-1|account:acct-a|model:model-a",
            "session:s-1|account:acct-a|model:model-b",
        ]
    );
    assert_eq!(r.rows[0].counts, counts(3, 0));
    // A session's per-model rows are the last level.
    assert_eq!(models.drill(&r.rows[0]), None);
}

#[test]
fn drill_from_a_month_clips_to_the_range_and_a_model_row_filters_by_model() {
    let v = View::new();
    v.sight("s-a", "acct-a", T0 - 20 * D, T0 + 60 * D);
    v.index(&[
        v.usage("s-a", T0 - 7 * D, "model-a", 1, 0),
        v.usage("s-a", T0 + H, "model-a", 2, 0),
        v.usage("s-a", T0 + 2 * H, "model-b", 4, 0),
        v.usage("s-a", T0 + 34 * D, "model-a", 8, 0),
    ]);
    // A range wider than September (30 days from T0).
    let q = TokenQuery {
        since_ms: Some(T0 - 10 * D),
        until_ms: Some(T0 + 50 * D),
        ..v.query(TokenPeriod::Month, TokenGrouping::Model)
    };
    let r = v.report(&q);
    assert_eq!(
        keys(&r),
        [
            "month:2026-10|model:model-a",
            "month:2026-09|model:model-a",
            "month:2026-09|model:model-b",
            "month:2026-08|model:model-a",
        ]
    );
    let september = &r.rows[1];

    // The month, clipped to the range; the model it is grouped by becomes the filter.
    let sessions = q.drill(september).unwrap();
    assert_eq!(
        sessions,
        TokenQuery {
            period: TokenPeriod::Session,
            since_ms: Some(T0),
            until_ms: Some(T0 + 30 * D),
            models: Some(vec!["model-a".into()]),
            ..q.clone()
        }
    );
    let r = v.report(&sessions);
    assert_eq!(keys(&r), ["session:s-a|model:model-a"]);
    assert_eq!(r.total.counts, september.counts);

    // A range inside the month is kept as it is.
    let inside = TokenQuery {
        since_ms: Some(T0 + 10 * D),
        until_ms: Some(T0 + 20 * D),
        ..q.clone()
    };
    let drilled = inside.drill(september).unwrap();
    assert_eq!(
        (drilled.since_ms, drilled.until_ms),
        (Some(T0 + 10 * D), Some(T0 + 20 * D))
    );
}

#[test]
fn row_keys_are_stable_across_refreshes() {
    let v = View::new();
    v.sight("s-1", "acct-a", T0, T0 + D);
    let stale = v.usage("s-1", T0 + H, "model-a", 1, 2);
    v.index(&[stale.clone()]);
    let queries = [
        v.query(TokenPeriod::Day, TokenGrouping::AccountModel),
        v.query(TokenPeriod::Month, TokenGrouping::AccountModel),
        v.query(TokenPeriod::Session, TokenGrouping::AccountModel),
        v.query(TokenPeriod::Block, TokenGrouping::AccountModel),
    ];
    let before: Vec<Vec<String>> = queries
        .iter()
        .map(|q| keys(&v.report(q)).into_iter().map(str::to_owned).collect())
        .collect();
    assert_eq!(before[2], ["session:s-1|account:acct-a|model:model-a"]);
    assert_eq!(
        before[3],
        [format!("block:{}|account:acct-a|model:model-a", T0 + H)]
    );

    // A later refresh finds that response's final line and another response of the same day,
    // session and block, and new evidence lands: every row keeps its key.
    v.fx.clock.advance_ms(MIN);
    v.index(&[
        UsageRow {
            counts: counts(1, 9),
            ..stale
        },
        v.usage("s-1", T0 + 3 * H, "model-a", 4, 0),
    ]);
    v.sight("s-1", "acct-a", T0 + 2 * D, T0 + 3 * D);
    v.store
        .finish_token_refresh(&cc(), v.generation(), 0, 0, v.engine.now_ms())
        .unwrap();
    for (q, before) in queries.iter().zip(&before) {
        let r = v.report(q);
        assert_eq!(keys(&r), *before, "{q:?}");
        assert_eq!(r.rows[0].counts, counts(5, 9), "{q:?}");
    }
}

#[test]
fn a_partial_index_reports_rows_with_no_refreshed_at() {
    let v = View::new();
    v.sight("s-1", "acct-a", T0, T0 + D);
    v.index(&[
        v.usage("s-1", T0 + H, "model-a", 1, 0),
        v.usage("s-1", T0 + 2 * H, "model-a", 2, 0),
    ]);

    // Before any refresh completes (spec §14).
    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
    assert_eq!(r.total.responses, 2);
    assert_eq!(r.index.refreshed_at_ms, None);
    assert_eq!(r.index.changed_at_ms, Some(v.engine.now_ms()));
    assert_eq!(
        (r.index.oldest_ms, r.index.newest_ms),
        (Some(T0 + H), Some(T0 + 2 * H))
    );
    assert_eq!(
        (r.index.files, r.index.responses),
        (0, 2),
        "files are the last completed refresh's; responses are the index's own count"
    );

    let done = v.engine.now_ms() + 1_000;
    v.fx.clock.set(done);
    v.store
        .commit_token_batch(&cc(), v.generation(), &[], &[], &[], 1, done)
        .unwrap();
    v.store
        .finish_token_refresh(&cc(), v.generation(), 2, 0, done)
        .unwrap();
    let index = IndexState {
        refreshed_at_ms: Some(done),
        changed_at_ms: Some(done),
        files: 2,
        responses: 2,
        skipped_lines: 1,
        unreadable_files: 0,
        oldest_ms: Some(T0 + H),
        newest_ms: Some(T0 + 2 * H),
    };
    assert_eq!(v.engine.token_index_state(&cc()).unwrap(), index);
    assert_eq!(
        v.report(&v.query(TokenPeriod::Day, TokenGrouping::None))
            .index,
        index
    );
}

#[test]
fn index_state_moves_on_new_evidence_alone() {
    let v = View::new();
    v.index(&[v.usage("s-1", T0 + H, "model-a", 1, 0)]);
    v.store
        .finish_token_refresh(&cc(), v.generation(), 0, 0, v.engine.now_ms())
        .unwrap();
    let state = || v.engine.token_index_state(&cc()).unwrap();
    let refreshed = state();
    assert_eq!(refreshed.changed_at_ms, Some(v.engine.now_ms()));

    // A sighting recorded later moves it, with no response added (spec §6.4).
    v.fx.clock.advance_ms(5_000);
    v.sight("s-1", "acct-a", T0, T0 + D);
    let sighted = state();
    assert_eq!(sighted.changed_at_ms, Some(v.engine.now_ms()));
    assert_eq!(
        (sighted.refreshed_at_ms, sighted.responses),
        (refreshed.refreshed_at_ms, refreshed.responses)
    );

    // So does a launch.
    v.fx.clock.advance_ms(5_000);
    v.store
        .open_launch(&cc(), v.b.as_str(), T0, v.engine.now_ms())
        .unwrap();
    let launched = state();
    assert_eq!(launched.changed_at_ms, Some(v.engine.now_ms()));

    // And a change of the default home's login. Its trigger stamps the database's own clock,
    // not the fixture's (spec §4), so the time to expect is the one it recorded.
    v.store.set_active(&cc(), Some(&v.a), Some(0)).unwrap();
    let switched_at = v.store.active_history(&cc()).unwrap().last().unwrap().0;
    assert_eq!(
        state().changed_at_ms,
        launched.changed_at_ms.max(Some(switched_at))
    );
}

#[test]
fn totals_split_by_basis() {
    let v = View::new();
    // a's profile: profile basis.
    v.sight("s-p", "acct-a", T0, T0 + D);
    // The default home while b is its login: timeline basis.
    v.event(T0 - D, "switch", None, Some(&v.b));
    v.sight("s-d", "", T0, T0 + D);
    // Open in the default home and a's profile at once: unattributed.
    v.sight("s-u", "", T0, T0 + D);
    v.sight("s-u", "acct-a", T0, T0 + D);
    let mut stale = v.usage("s-p", T0 + H, "model-a", 100, 0);
    stale.finalized = false;
    v.index(&[
        stale,
        v.usage("s-d", T0 + 2 * H, "model-a", 200, 0),
        v.usage("s-u", T0 + 3 * H, "model-b", 300, 0),
    ]);

    let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::Account));
    assert_eq!(
        keys(&r),
        [
            "day:2026-09-01|account:acct-a",
            "day:2026-09-01|account:acct-b",
            "day:2026-09-01|account:unattributed",
        ]
    );
    assert_eq!(r.rows[0].unfinalized, 1);
    assert_eq!(
        r.total.by_basis,
        [
            (Basis::Profile, 100),
            (Basis::Timeline, 200),
            (Basis::Unattributed, 300)
        ]
    );
    assert_eq!(
        (r.total.counts, r.total.responses, r.total.unfinalized),
        (counts(600, 0), 3, 1)
    );
    // model-a 300 × 1,000 and model-b 300 × 2,000 nano-dollars.
    assert_eq!(
        r.total.cost,
        Some(Cost {
            usd_nanos: 900_000,
            partial: false
        })
    );
}

#[test]
fn without_a_store_the_report_is_empty_and_creates_nothing() {
    let fx = Fx::new();
    let q = TokenQuery {
        provider: cc(),
        period: TokenPeriod::Day,
        grouping: TokenGrouping::Account,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: TimeZone::UTC,
    };
    let r = fx.engine.token_report(&q).unwrap();
    assert!(r.rows.is_empty());
    assert_eq!((r.total.responses, r.index), (0, IndexState::default()));
    assert_eq!(
        fx.engine.token_index_state(&cc()).unwrap(),
        IndexState::default()
    );
    assert!(!fx.env.data_dir().join("tagteam.db").exists());
}

/// An engine over `fx` with Claude Code and `fake` registered and `settings`, as the CLI builds
/// one for a Claude Code command.
fn engine_with(fx: &Fx, fake: Arc<FakeAgent>, settings: Settings) -> Engine {
    Engine::new(EngineConfig {
        env: fx.env.clone(),
        registry: ProviderRegistry::new().with(fx.cc.clone()).with(fake),
        vault: Vault::new(Box::new(KeychainVault::new(fx.kc.clone()))),
        oracle: fx.oracle.clone(),
        clock: fx.clock.clone(),
        http: fx.http.clone(),
        default_provider: ProviderId::new(CLAUDE_CODE),
        settings,
        process: fx.process.clone(),
        spawner: fx.spawner.clone(),
        run_shell: RunShell::Outside,
    })
}

fn day_query(provider: ProviderId) -> TokenQuery {
    TokenQuery {
        provider,
        period: TokenPeriod::Day,
        grouping: TokenGrouping::Account,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: TimeZone::UTC,
    }
}

#[test]
fn a_provider_without_transcripts_is_refused() {
    let fx = Fx::new();
    let fake = Arc::new(FakeAgent::new().without_transcripts());
    let engine = engine_with(&fx, fake, Settings::default());
    let q = day_query(ProviderId::new(FAKE_AGENT));
    for err in [
        engine.token_report(&q).unwrap_err(),
        engine.token_index_state(&q.provider).unwrap_err(),
    ] {
        assert_eq!(err.kind(), "invalid-input", "{err:?}");
    }
}

#[test]
fn price_overrides_follow_the_report_provider() {
    let ff = FakeFx::new();
    let path = config_path(&ff.fx.env);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Each provider's own table (spec §8.2): fake-1's input at $7 per MTok, where FakeAgent's
    // shipped table has $1, and a Claude Code model the shipped table lacks.
    fs::write(
        &path,
        "[provider.fake-agent.prices.\"fake-1\"]\ninput = 7.0\n\n\
         [provider.claude-code.prices.\"model-a\"]\n\
         input = 1.0\noutput = 2.0\ncache_write_5m = 3.0\ncache_write_1h = 4.0\ncache_read = 0.5\n",
    )
    .unwrap();
    // The settings of a Claude Code command, as the CLI loads them.
    let (settings, warnings) = Settings::load(&ff.fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    let engine = engine_with(&ff.fx, ff.fake.clone(), settings);
    let fake = ProviderId::new(FAKE_AGENT);
    let row = UsageRow {
        key: usage_key(&fake, "msg-1", "req-1"),
        session_id: "s-f".into(),
        project: None,
        at_ms: T0 + H,
        model: "fake-1".into(),
        sidechain: false,
        counts: counts(10, 0),
        speed: None,
        finalized: true,
    };
    let store = engine.store().unwrap();
    let generation = store.begin_token_refresh(&fake, engine.now_ms()).unwrap();
    store
        .commit_token_batch(&fake, generation, &[row], &[], &[], 0, engine.now_ms())
        .unwrap();

    let r = engine.token_report(&day_query(fake)).unwrap();
    assert_eq!(r.price_overrides, ["fake-1"]);
    assert_eq!(r.prices_as_of, ff.fake.price_table().as_of);
    // 10 input tokens at 7,000 nano-dollars each.
    assert_eq!(
        r.total.cost,
        Some(Cost {
            usd_nanos: 70_000,
            partial: false
        })
    );
    let r = engine.token_report(&day_query(cc())).unwrap();
    assert_eq!(r.price_overrides, ["model-a"]);
}
