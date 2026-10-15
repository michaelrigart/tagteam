//! The store API of the token index and the session evidence (token report spec §4–§6).

mod common;

use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{add, cc};
use tagteam_core::ProviderId;
use tagteam_core::tokens::TokenCounts;
use tagteam_engine::store::{
    BatchOutcome, EpochRow, LaunchRow, PruneOutcome, RefreshStats, SightingEnd, SightingRow,
    SightingSource, Store, StoreError, TokenFileRow, TokenStateRow, UsageRow, usage_key,
};

/// The fixtures' clock (`Fx`'s `FakeClock`).
const T: i64 = 1_790_000_000_000;
const DAY: i64 = 86_400_000;

fn store() -> (tempfile::TempDir, Store, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    (d, s, path)
}

fn fake() -> ProviderId {
    ProviderId::new("fake-agent")
}

/// One response of session `session` at `at`, with `output` output tokens and fixed input and
/// cache counts.
fn row(key: i64, session: &str, at: i64, output: u64) -> UsageRow {
    UsageRow {
        key,
        session_id: session.into(),
        project: Some("/work/app".into()),
        at_ms: at,
        model: "claude-opus-5-5".into(),
        sidechain: false,
        counts: TokenCounts {
            input: 100,
            output,
            cache_write_5m: 10,
            cache_write_1h: 20,
            cache_read: 30,
            web_search: 0,
        },
        speed: None,
        finalized: false,
    }
}

fn file(path: &str, offset: u64) -> TokenFileRow {
    TokenFileRow {
        path: path.into(),
        dev: 1,
        ino: u64::MAX,
        size: -1,
        mtime_ns: 5,
        offset,
        head_hash: [1; 8],
        tail_hash: [2; 8],
    }
}

/// The index's generation, as a refresh beginning now takes it.
fn generation(s: &Store) -> i64 {
    s.begin_token_refresh(&cc(), T).unwrap()
}

fn commit(s: &Store, rows: &[UsageRow], now: i64) -> BatchOutcome {
    s.commit_token_batch(&cc(), generation(s), rows, &[], &[], now)
        .unwrap()
}

fn all(s: &Store) -> Vec<UsageRow> {
    s.token_usage(&cc(), None, None, None).unwrap()
}

fn keys(rows: &[UsageRow]) -> Vec<i64> {
    rows.iter().map(|r| r.key).collect()
}

fn sighting(session: &str, home: &str, first: i64, last: i64) -> SightingRow {
    SightingRow {
        session_id: session.into(),
        home: home.into(),
        started_at: first,
        first_at: first,
        last_at: last,
        source: SightingSource::Scan,
        recorded_at: last,
    }
}

/// A tenure of `session` in the execution started at `started`, from its first read at `first`
/// to `last`.
fn tenure(session: &str, home: &str, started: i64, first: i64, last: i64) -> SightingRow {
    SightingRow {
        started_at: started,
        ..sighting(session, home, first, last)
    }
}

fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// A prune nothing stops.
fn prune(s: &Store, cutoff: i64, holder: &str, now: i64) -> PruneOutcome {
    s.prune_tokens(&cc(), generation(s), cutoff, holder, now, &|| false)
        .unwrap()
}

fn done(deleted: u64) -> PruneOutcome {
    PruneOutcome {
        deleted,
        complete: true,
    }
}

fn raw_count(path: &std::path::Path, sql: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_batch_upserts_by_the_merge_rule_in_any_order() {
    // Spec §2.2: each count is the maximum over a response's lines, its time the earliest, and
    // its speed and finality come from the final line, whatever order lines and batches come in.
    let key = usage_key(&cc(), "msg_1", "req_1");
    let stale_late = row(key, "s1", T + 20, 5);
    let stale_first = row(key, "s1", T, 1);
    let mut last = row(key, "s1", T + 40, 9);
    last.counts.web_search = 1;
    last.speed = Some("standard".into());
    last.finalized = true;
    let lines = [stale_late, stale_first, last];
    let mut want = row(key, "s1", T, 9);
    want.counts.web_search = 1;
    want.speed = Some("standard".into());
    want.finalized = true;

    const ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for order in ORDERS {
        let (_d, s, _) = store();
        let outcomes: Vec<BatchOutcome> = order
            .iter()
            .map(|&i| commit(&s, &[lines[i].clone()], T))
            .collect();
        assert_eq!(all(&s), [want.clone()], "one line per batch: {order:?}");
        assert_eq!(
            outcomes[0],
            BatchOutcome {
                added: 1,
                updated: 0
            },
            "{order:?}"
        );
        assert!(
            outcomes[1..].iter().all(|o| o.added == 0),
            "{order:?}: {outcomes:?}"
        );

        let (_d, s, _) = store();
        let batch: Vec<UsageRow> = order.iter().map(|&i| lines[i].clone()).collect();
        assert_eq!(
            commit(&s, &batch, T),
            BatchOutcome {
                added: 1,
                updated: 0
            },
            "one batch: {order:?}"
        );
        assert_eq!(all(&s), [want.clone()], "{order:?}");
        assert_eq!(
            commit(&s, &batch, T),
            BatchOutcome::default(),
            "read again, nothing changes: {order:?}"
        );
    }
}

#[test]
fn a_batch_creates_state_and_moves_changed_at_only_on_change() {
    let (_d, s, _) = store();
    assert_eq!(s.token_state(&cc()).unwrap(), None);
    let f = file("/t/a.jsonl", 10);
    assert_eq!(
        s.commit_token_batch(&cc(), generation(&s), &[], std::slice::from_ref(&f), &[], T)
            .unwrap(),
        BatchOutcome::default(),
        "offsets alone"
    );
    assert_eq!(
        s.token_state(&cc()).unwrap(),
        Some(TokenStateRow {
            refreshed_at: None,
            changed_at: None,
            stats: RefreshStats::default(),
            pruned_at: None,
        })
    );
    assert_eq!(s.token_files(&cc()).unwrap(), [f.clone()]);
    assert!(s.token_files(&fake()).unwrap().is_empty());

    let changed = |s: &Store| s.token_state(&cc()).unwrap().unwrap().changed_at;
    let r = row(1, "s1", T, 5);
    assert_eq!(
        commit(&s, std::slice::from_ref(&r), T + 1),
        BatchOutcome {
            added: 1,
            updated: 0
        }
    );
    assert_eq!(changed(&s), Some(T + 1));
    assert_eq!(
        commit(&s, std::slice::from_ref(&r), T + 2),
        BatchOutcome::default()
    );
    assert_eq!(changed(&s), Some(T + 1), "nothing changed");
    let mut more = r.clone();
    more.counts.output = 6;
    assert_eq!(
        commit(&s, &[more], T + 3),
        BatchOutcome {
            added: 0,
            updated: 1
        }
    );
    assert_eq!(changed(&s), Some(T + 3));

    let g = TokenFileRow {
        offset: 20,
        size: 300,
        ..f.clone()
    };
    s.commit_token_batch(
        &cc(),
        generation(&s),
        &[],
        std::slice::from_ref(&g),
        &[],
        T + 4,
    )
    .unwrap();
    assert_eq!(s.token_files(&cc()).unwrap(), [g], "rewritten at its path");
    s.commit_token_batch(
        &cc(),
        generation(&s),
        &[],
        &[],
        &["/t/a.jsonl".to_owned()],
        T + 5,
    )
    .unwrap();
    assert!(s.token_files(&cc()).unwrap().is_empty(), "removed");
    assert_eq!(
        all(&s).len(),
        1,
        "a removed file keeps its usage (spec §5.2)"
    );
    assert_eq!(changed(&s), Some(T + 3));
}

#[test]
fn finish_sets_refreshed_and_the_first_completion_moves_changed_at() {
    let (_d, s, _) = store();
    commit(&s, &[row(1, "s1", T, 5), row(2, "s2", T + 1, 5)], T);
    s.commit_token_batch(&fake(), generation(&s), &[row(3, "f1", T, 5)], &[], &[], T)
        .unwrap();
    let stats = RefreshStats {
        files: 3,
        responses: 0,
        skipped_lines: 2,
        unreadable_files: 1,
    };
    s.finish_token_refresh(&cc(), generation(&s), &stats, T + 10)
        .unwrap();
    assert_eq!(
        s.token_state(&cc()).unwrap(),
        Some(TokenStateRow {
            refreshed_at: Some(T + 10),
            changed_at: Some(T + 10),
            stats: RefreshStats {
                responses: 2,
                ..stats
            },
            pruned_at: None,
        }),
        "the first completion ends the partial index; responses are the index's own count"
    );
    let later = RefreshStats {
        files: 4,
        ..RefreshStats::default()
    };
    s.finish_token_refresh(&cc(), generation(&s), &later, T + 20)
        .unwrap();
    assert_eq!(
        s.token_state(&cc()).unwrap(),
        Some(TokenStateRow {
            refreshed_at: Some(T + 20),
            changed_at: Some(T + 10),
            stats: RefreshStats {
                files: 4,
                responses: 2,
                skipped_lines: 0,
                unreadable_files: 0,
            },
            pruned_at: None,
        })
    );
    let other = ProviderId::new("other");
    s.finish_token_refresh(&other, generation(&s), &RefreshStats::default(), T + 30)
        .unwrap();
    assert_eq!(
        s.token_state(&other).unwrap(),
        Some(TokenStateRow {
            refreshed_at: Some(T + 30),
            changed_at: Some(T + 30),
            stats: RefreshStats::default(),
            pruned_at: None,
        }),
        "a provider with nothing to index completes too"
    );
}

#[test]
fn begin_creates_a_new_generation_after_each_purge() {
    // Spec §5.1, §13: the epoch is the index's generation. A refresh begins on the one there
    // is; after a purge deleted it, the next refresh to begin records a new one, unseeded
    // (ruling R1), and a refresh of an older generation writes nothing.
    let (_d, s, _) = store();
    let migrated = s.token_epoch().unwrap().expect("the migration's epoch");
    assert!(!migrated.seeded, "a raw open seeds nothing (ruling R1)");
    assert_eq!(s.begin_token_refresh(&cc(), T).unwrap(), migrated.at);
    assert_eq!(
        s.token_epoch().unwrap(),
        Some(migrated),
        "an epoch is kept, never moved"
    );

    s.empty_all().unwrap();
    assert_eq!(s.token_epoch().unwrap(), None);
    let first = s.begin_token_refresh(&cc(), T + 1).unwrap();
    assert_eq!(first, T + 1);
    assert_eq!(
        s.token_epoch().unwrap(),
        Some(EpochRow {
            at: T + 1,
            seeded: false
        })
    );
    assert_eq!(
        s.begin_token_refresh(&fake(), T + 5).unwrap(),
        first,
        "one generation for every provider"
    );

    s.empty_all().unwrap();
    let second = s.begin_token_refresh(&cc(), T + 2).unwrap();
    assert_ne!(second, first);
    assert!(matches!(
        s.commit_token_batch(&cc(), first, &[row(1, "s1", T, 1)], &[], &[], T + 3),
        Err(StoreError::TokenIndexPurged)
    ));
    assert!(matches!(
        s.finish_token_refresh(&cc(), first, &RefreshStats::default(), T + 3),
        Err(StoreError::TokenIndexPurged)
    ));
    assert!(all(&s).is_empty(), "the older generation wrote nothing");
    assert_eq!(
        s.commit_token_batch(&cc(), second, &[row(1, "s1", T, 1)], &[], &[], T + 3)
            .unwrap(),
        BatchOutcome {
            added: 1,
            updated: 0
        }
    );
    s.finish_token_refresh(&cc(), second, &RefreshStats::default(), T + 4)
        .unwrap();
    assert_eq!(
        s.token_epoch().unwrap(),
        Some(EpochRow {
            at: T + 2,
            seeded: false
        }),
        "a completion records no epoch of its own"
    );
}

#[test]
fn a_batch_after_a_full_purge_is_refused() {
    // Spec §5.1, §13: a full purge deletes the epoch, the index's generation, so a refresh that
    // began before it writes nothing after it, not even the provider's state row.
    let (_d, s, _) = store();
    let started = generation(&s);
    assert_eq!(
        s.commit_token_batch(&cc(), started, &[row(1, "s1", T, 1)], &[], &[], T)
            .unwrap(),
        BatchOutcome {
            added: 1,
            updated: 0
        }
    );
    s.empty_all().unwrap();
    let err = s
        .commit_token_batch(
            &cc(),
            started,
            &[row(2, "s1", T + 1, 1)],
            &[file("/t/a.jsonl", 10)],
            &[],
            T + 1,
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::TokenIndexPurged), "{err:?}");
    assert_eq!(
        err.to_string(),
        "the token index was purged during the refresh"
    );
    assert!(all(&s).is_empty());
    assert!(s.token_files(&cc()).unwrap().is_empty());
    assert_eq!(s.token_state(&cc()).unwrap(), None);

    // A refresh that begins after the purge has a generation of its own, and writes.
    let fresh = s.begin_token_refresh(&cc(), T + 2).unwrap();
    assert_ne!(fresh, started);
    assert_eq!(
        s.commit_token_batch(&cc(), fresh, &[row(2, "s1", T + 1, 1)], &[], &[], T + 2)
            .unwrap(),
        BatchOutcome {
            added: 1,
            updated: 0
        }
    );
}

#[test]
fn a_finish_after_a_full_purge_is_refused() {
    let (_d, s, _) = store();
    let started = generation(&s);
    commit(&s, &[row(1, "s1", T, 1)], T);
    s.empty_all().unwrap();
    let stats = RefreshStats {
        files: 1,
        ..RefreshStats::default()
    };
    let err = s
        .finish_token_refresh(&cc(), started, &stats, T + 1)
        .unwrap_err();
    assert!(matches!(err, StoreError::TokenIndexPurged), "{err:?}");
    assert_eq!(s.token_state(&cc()).unwrap(), None, "nothing written");
    assert_eq!(s.token_epoch().unwrap(), None, "and no epoch recorded");

    // A refresh that began after the purge completes.
    let fresh = s.begin_token_refresh(&cc(), T + 2).unwrap();
    s.finish_token_refresh(&cc(), fresh, &stats, T + 3).unwrap();
    assert_eq!(
        s.token_state(&cc()).unwrap().unwrap().refreshed_at,
        Some(T + 3)
    );
    assert!(matches!(
        s.commit_token_batch(&cc(), started, &[], &[], &[], T + 4),
        Err(StoreError::TokenIndexPurged)
    ));
}

#[test]
fn prune_runs_once_a_day_and_moves_changed_at_when_it_deletes() {
    let (_d, s, _) = store();
    let cutoff = T - 365 * DAY;
    let mut old: Vec<UsageRow> = (1000..1000 + 5_001)
        .map(|k| row(k, "bulk", cutoff - 1, 1))
        .collect();
    old.extend([row(1, "old", cutoff - 2, 1), row(2, "live", cutoff - 1, 1)]);
    commit(&s, &old, T);
    commit(&s, &[row(3, "live", cutoff, 1)], T);
    s.commit_token_batch(
        &fake(),
        generation(&s),
        &[row(9, "f1", cutoff - 5, 1)],
        &[],
        &[],
        T,
    )
    .unwrap();
    s.record_sighting(&cc(), &sighting("old", "", cutoff - 10, cutoff - 1), 0)
        .unwrap();
    s.record_sighting(&cc(), &sighting("live", "", cutoff - 10, T), 0)
        .unwrap();
    for (account, ended) in [("a", Some(cutoff - 1)), ("b", Some(cutoff)), ("c", None)] {
        s.open_launch(&cc(), account, cutoff - 10, T).unwrap();
        if let Some(end) = ended {
            s.close_launch(&cc(), account, cutoff - 10, end, T).unwrap();
        }
    }

    assert_eq!(prune(&s, cutoff, "h1", T), done(5_003));
    assert_eq!(keys(&all(&s)), [3], "only the response inside the window");
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [sighting("live", "", cutoff - 10, T)],
        "a sighting that ended before the window goes; a live one stays"
    );
    assert_eq!(
        s.launches(&cc())
            .unwrap()
            .iter()
            .map(|l| l.account_id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"],
        "only a launch that ended before the window goes"
    );
    assert_eq!(
        s.token_usage(&fake(), None, None, None).unwrap().len(),
        1,
        "another provider's rows are not this prune's"
    );
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!((state.pruned_at, state.changed_at), (Some(T), Some(T)));
    assert_eq!(state.stats.responses, 1, "the count follows the prune");
    assert_eq!(
        s.token_models(&cc()).unwrap(),
        ["claude-opus-5-5"],
        "ruling R8: a model's name stays"
    );

    commit(&s, &[row(4, "live", cutoff - 3, 1)], T);
    assert_eq!(
        prune(&s, cutoff, "h2", T + DAY - 1),
        done(0),
        "within the day, the lease holds: nothing is due"
    );
    assert_eq!(keys(&all(&s)), [4, 3]);
    assert_eq!(prune(&s, cutoff, "h2", T + DAY), done(1));
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.pruned_at, state.changed_at, state.stats.responses),
        (Some(T + DAY), Some(T + DAY), 1)
    );
    assert_eq!(prune(&s, cutoff, "h3", T + 2 * DAY), done(0));
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.pruned_at, state.changed_at),
        (Some(T + 2 * DAY), Some(T + DAY)),
        "a prune that deletes nothing changes no report"
    );
}

#[test]
fn prune_deletes_a_sighting_by_its_full_key() {
    // Spec §4: of two executions' tenures whose first reads fell on the same millisecond, only
    // the one that ended before the cutoff goes.
    let (_d, s, _) = store();
    let cutoff = T - 365 * DAY;
    let ended = tenure("s1", "", cutoff - 20, cutoff - 10, cutoff - 1);
    let live = tenure("s1", "", cutoff - 15, cutoff - 10, T);
    for row in [&ended, &live] {
        s.record_sighting(&cc(), row, 0).unwrap();
    }
    assert_eq!(prune(&s, cutoff, "h1", T), done(0));
    assert_eq!(s.sightings(&cc()).unwrap(), [live]);
}

#[test]
fn a_prune_that_fails_part_way_still_moves_changed_at() {
    // Each chunk that deletes a response moves `changed_at` with it: a prune that stops after
    // its chunks, under a lease that keeps a retry away for a day, leaves no change unseen.
    let (_d, s, path) = store();
    let cutoff = T - 365 * DAY;
    commit(&s, &[row(1, "old", cutoff - 1, 1), row(2, "live", T, 1)], T);
    s.open_launch(&cc(), "a", cutoff - 10, T).unwrap();
    s.close_launch(&cc(), "a", cutoff - 10, cutoff - 1, T)
        .unwrap();
    // The last transaction deletes that launch; a trigger makes it fail.
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER fail_the_last_step BEFORE DELETE ON run_launches \
         BEGIN SELECT RAISE(ABORT, 'injected'); END;",
    )
    .unwrap();

    assert!(
        s.prune_tokens(&cc(), generation(&s), cutoff, "h1", T + 5, &|| false)
            .is_err()
    );
    assert_eq!(keys(&all(&s)), [2], "the chunk committed");
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.changed_at, state.stats.responses, state.pruned_at),
        (Some(T + 5), 1, None)
    );
    assert_eq!(
        prune(&s, cutoff, "h2", T + 6),
        done(0),
        "the lease holds the retry off"
    );

    raw.execute_batch("DROP TRIGGER fail_the_last_step;")
        .unwrap();
    assert_eq!(prune(&s, cutoff, "h2", T + DAY + 5), done(0));
    assert!(
        s.launches(&cc()).unwrap().is_empty(),
        "the next day's prune finishes it"
    );
    assert_eq!(
        s.token_state(&cc()).unwrap().unwrap().changed_at,
        Some(T + 5),
        "deleting nothing changes no report"
    );
}

#[test]
fn a_stopped_prune_resumes_on_the_next_call_and_completes() {
    // A retention cut can make a prune large. `stop` (the refresh's budget and cancel) is asked
    // only after a chunk that deleted something, and a stopped prune hands its lease back: a
    // `stop` that trips at its first ask still lets call after call finish.
    let (_d, s, _) = store();
    let cutoff = T - 30 * DAY;
    let mut rows: Vec<UsageRow> = (1000..1000 + 5_001)
        .map(|k| row(k, "old", cutoff - 1, 1))
        .collect();
    rows.push(row(1, "live", T, 1));
    commit(&s, &rows, T);

    let asked = Cell::new(0);
    let at_first_ask = || {
        asked.set(asked.get() + 1);
        true
    };
    assert_eq!(
        s.prune_tokens(&cc(), generation(&s), cutoff, "h1", T + 1, &at_first_ask)
            .unwrap(),
        PruneOutcome {
            deleted: 5_000,
            complete: false
        },
        "a full chunk, then the first ask"
    );
    assert_eq!(asked.get(), 1);
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.changed_at, state.stats.responses, state.pruned_at),
        (Some(T + 1), 2, None),
        "the chunk that ran is seen; the prune is not done"
    );

    let mut outcomes = Vec::new();
    for (n, holder) in ["h2", "h3", "h4", "h5"].into_iter().enumerate() {
        let outcome = s
            .prune_tokens(
                &cc(),
                generation(&s),
                cutoff,
                holder,
                T + 2 + n as i64,
                &at_first_ask,
            )
            .unwrap();
        outcomes.push(outcome);
        if outcome.complete {
            break;
        }
    }
    assert_eq!(
        outcomes,
        [
            PruneOutcome {
                deleted: 1,
                complete: false
            },
            PruneOutcome {
                deleted: 0,
                complete: false
            },
            done(0),
        ],
        "the last expired response; the session it left empty; then the rest, deleting nothing"
    );
    assert_eq!(keys(&all(&s)), [1]);
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        (state.changed_at, state.stats.responses, state.pruned_at),
        (Some(T + 2), 1, Some(T + 4))
    );
    assert_eq!(
        prune(&s, cutoff, "h6", T + 5),
        done(0),
        "complete, it holds the day's lease"
    );
}

#[test]
fn the_cleanup_after_the_responses_stops_and_resumes_too() {
    // The phases after the expired responses (sessions, sightings, launches) also delete in
    // chunks, each that deletes something followed by an ask: a large backlog of expired
    // sightings never runs as one transaction, and a `stop` that trips at its first ask still
    // lets the calls finish.
    let (_d, s, path) = store();
    let cutoff = T - 30 * DAY;
    {
        let mut raw = rusqlite::Connection::open(&path).unwrap();
        let tx = raw.transaction().unwrap();
        for i in 0..6_000 {
            tx.execute(
                "INSERT INTO session_sightings \
                 (provider, session_id, home, started_at, first_at, last_at, source, \
                 recorded_at) VALUES ('claude-code', ?1, '', ?2, ?2, ?2, 'scan', ?2)",
                rusqlite::params![format!("s{i}"), cutoff - 1],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let sightings = || -> i64 {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("SELECT count(*) FROM session_sightings", [], |r| r.get(0))
            .unwrap()
    };
    let stopped = PruneOutcome {
        deleted: 0,
        complete: false,
    };

    // The responses and sessions delete nothing and move on unasked; 5,000 sightings go, then
    // the first ask stops it.
    assert_eq!(
        s.prune_tokens(&cc(), generation(&s), cutoff, "h1", T, &|| true)
            .unwrap(),
        stopped
    );
    assert_eq!(sightings(), 1_000);
    assert_eq!(
        s.token_state(&cc()).unwrap().and_then(|st| st.pruned_at),
        None
    );
    assert_eq!(
        s.prune_tokens(&cc(), generation(&s), cutoff, "h2", T + 1, &|| true)
            .unwrap(),
        stopped,
        "the last 1,000, then an ask"
    );
    assert_eq!(sightings(), 0);
    assert_eq!(
        s.prune_tokens(&cc(), generation(&s), cutoff, "h3", T + 2, &|| true)
            .unwrap(),
        done(0),
        "nothing left to delete: it completes without asking"
    );
    assert_eq!(
        s.token_state(&cc()).unwrap().unwrap().pruned_at,
        Some(T + 2)
    );
}

#[test]
fn sightings_insert_then_extend_only_after_the_gap() {
    let (_d, s, _) = store();
    let seen = |last: i64, source| SightingRow {
        source,
        ..sighting("s1", "", T, last)
    };
    assert!(
        s.record_sighting(&cc(), &seen(T, SightingSource::Scan), 60_000)
            .unwrap()
    );
    assert!(
        !s.record_sighting(&cc(), &seen(T + 59_999, SightingSource::Statusline), 60_000)
            .unwrap(),
        "within the gap"
    );
    assert!(
        s.record_sighting(&cc(), &seen(T + 60_000, SightingSource::Statusline), 60_000)
            .unwrap()
    );
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "").unwrap(),
        Some(seen(T + 60_000, SightingSource::Scan)),
        "an extension keeps the source that created it"
    );
    assert!(
        !s.record_sighting(&cc(), &seen(T + 10, SightingSource::Run), 0)
            .unwrap(),
        "last_at never moves back"
    );

    let next = sighting("s1", "", T + 100_000, T + 100_000);
    assert!(s.record_sighting(&cc(), &next, 60_000).unwrap());
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "").unwrap(),
        Some(next.clone()),
        "another execution is its own sighting, and the latest"
    );
    let profile = sighting("s1", "acct-a", T, T + 5);
    assert!(s.record_sighting(&cc(), &profile, 60_000).unwrap());
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "acct-a").unwrap(),
        Some(profile.clone())
    );
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [
            seen(T + 60_000, SightingSource::Scan),
            profile,
            next.clone()
        ]
    );
    assert_eq!(s.latest_sighting(&cc(), "s2", "").unwrap(), None);
    assert!(s.sightings(&fake()).unwrap().is_empty());
}

#[test]
fn tenures_are_keyed_by_their_execution_and_first_read() {
    // Spec §6.4: a session that leaves an execution and comes back has two tenures there, each
    // extended by its own key only; two executions whose first reads fell on the same
    // millisecond are two sightings too, the later execution the latest.
    let (_d, s, _) = store();
    let first = tenure("s1", "acct-a", T, T + 1_000, T + 5_000);
    let back = tenure("s1", "acct-a", T, T + 9_000, T + 9_000);
    let elsewhere = tenure("s1", "acct-a", T - 7, T + 9_000, T + 9_500);
    for row in [&first, &back, &elsewhere] {
        assert!(s.record_sighting(&cc(), row, 60_000).unwrap(), "{row:?}");
    }
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [first.clone(), elsewhere.clone(), back.clone()]
    );
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "acct-a").unwrap(),
        Some(back.clone())
    );
    let later = SightingRow {
        last_at: T + 70_000,
        recorded_at: T + 70_000,
        ..back
    };
    assert!(s.record_sighting(&cc(), &later, 60_000).unwrap());
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [first, elsewhere, later],
        "only the tenure the key names is extended"
    );
}

#[test]
fn a_continued_sighting_extends_the_latest_tenure_or_starts_one() {
    // Spec §6.4, for the scan and the statusline, which keep no state: a read of a session goes
    // on in the latest tenure of its execution there, by the gap rule, or starts one at the read
    // when the execution has none, however long ago the execution started.
    let (_d, s, _) = store();
    let started = T - 3_600_000;
    let read = |at: i64| tenure("s1", "", started, at, at);
    assert!(s.continue_sighting(&cc(), &read(T), 60_000).unwrap());
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [read(T)],
        "a first read starts a tenure at itself"
    );
    assert!(
        !s.continue_sighting(&cc(), &read(T + 59_999), 60_000)
            .unwrap(),
        "within the gap"
    );

    // `run`'s tenure of the session in the execution began later: it is the latest, and the
    // read goes on in it, keeping its source.
    let polled = SightingRow {
        source: SightingSource::Run,
        ..tenure("s1", "", started, T + 30_000, T + 40_000)
    };
    s.record_sighting(&cc(), &polled, 0).unwrap();
    assert!(
        s.continue_sighting(&cc(), &read(T + 100_000), 60_000)
            .unwrap()
    );
    let extended = SightingRow {
        last_at: T + 100_000,
        recorded_at: T + 100_000,
        ..polled
    };
    assert_eq!(s.sightings(&cc()).unwrap(), [read(T), extended.clone()]);

    // Another execution of the session, or another home, starts its own.
    let next = SightingRow {
        started_at: T + 150_000,
        ..read(T + 200_000)
    };
    let profile = tenure("s1", "acct-a", started, T + 200_000, T + 200_000);
    assert!(s.continue_sighting(&cc(), &next, 60_000).unwrap());
    assert!(s.continue_sighting(&cc(), &profile, 60_000).unwrap());
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [read(T), extended, next, profile]
    );
}

#[test]
fn a_continued_sighting_never_bridges_a_read_of_another_session() {
    // Spec §6.4: once the execution has been seen naming another session at or after the end of
    // the session's latest tenure, the session may have left it in between: a read of it starts
    // a new tenure. Another execution's or another home's sessions say nothing of this one.
    let (_d, s, _) = store();
    let started = T - 3_600_000;
    let run = |session: &str, first: i64, last: i64| SightingRow {
        source: SightingSource::Run,
        ..tenure(session, "acct-a", started, first, last)
    };
    let (left, other) = (
        run("s-a", T, T + 10_000),
        run("s-b", T + 10_050, T + 70_000),
    );
    s.record_sighting(&cc(), &left, 0).unwrap();
    s.record_sighting(&cc(), &other, 0).unwrap();
    let back = tenure("s-a", "acct-a", started, T + 90_000, T + 90_000);
    assert!(s.continue_sighting(&cc(), &back, 60_000).unwrap());
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [left.clone(), back.clone(), other.clone()],
        "the return to s-a is a new tenure; the gap is not bridged"
    );

    // The latest tenure is the new one, and nothing has been seen since: a later read goes on
    // in it, whatever other executions and homes saw meanwhile.
    let elsewhere = tenure("s-c", "acct-a", started + 1, T + 95_000, T + 150_000);
    let default_home = tenure("s-c", "", started, T + 95_000, T + 150_000);
    s.record_sighting(&cc(), &elsewhere, 0).unwrap();
    s.record_sighting(&cc(), &default_home, 0).unwrap();
    let again = tenure("s-a", "acct-a", started, T + 160_000, T + 160_000);
    assert!(s.continue_sighting(&cc(), &again, 60_000).unwrap());
    let back = SightingRow {
        last_at: T + 160_000,
        recorded_at: T + 160_000,
        ..back
    };
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [left, back, other, default_home, elsewhere]
    );
}

#[test]
fn ending_a_tenure_sets_its_end_back_or_forward_by_its_full_key() {
    // Spec §6.1: run ends a tenure a switch ended exactly at its last confirmation, below a
    // later stamp too, and never before its first read. Only the tenure its full key names
    // moves.
    let (_d, s, _) = store();
    let ours = tenure("s1", "acct-a", T - 500, T, T + 10_000);
    let other_execution = tenure("s1", "acct-a", T - 400, T, T + 10_000);
    let other_tenure = tenure("s1", "acct-a", T - 500, T + 50_000, T + 60_000);
    for row in [&ours, &other_execution, &other_tenure] {
        s.record_sighting(&cc(), row, 0).unwrap();
    }
    let end = |last_at: i64, recorded_at: i64| SightingRow {
        last_at,
        recorded_at,
        ..ours.clone()
    };
    let stored = |s: &Store| s.sightings(&cc()).unwrap();
    let with = |row: SightingRow| [row, other_execution.clone(), other_tenure.clone()];

    s.end_sighting(&cc(), &end(T + 20_000, T + 20_001), SightingEnd::Exactly)
        .unwrap();
    assert_eq!(stored(&s), with(end(T + 20_000, T + 20_001)), "forward");
    s.end_sighting(&cc(), &end(T + 9_000, T + 20_002), SightingEnd::Exactly)
        .unwrap();
    assert_eq!(
        stored(&s),
        with(end(T + 9_000, T + 20_002)),
        "back: a later stamp was not one of run's reads"
    );
    s.end_sighting(&cc(), &end(T + 9_000, T + 20_003), SightingEnd::Exactly)
        .unwrap();
    assert_eq!(
        stored(&s),
        with(end(T + 9_000, T + 20_002)),
        "the same end writes nothing"
    );
    s.end_sighting(&cc(), &end(T - 5_000, T + 20_004), SightingEnd::Exactly)
        .unwrap();
    assert_eq!(
        stored(&s),
        with(end(T, T + 20_004)),
        "an end before the first read clamps to it"
    );
}

#[test]
fn ending_a_tenure_stores_it_and_clamps_the_session_s_other_tenures_there() {
    // Spec §6.1, §6.4: a tenure no write stored yet is stored by its end. Every other tenure of
    // the session in that execution and home that began by the end and runs past it is clamped
    // to it: a scan or a statusline may have continued one over the time the session was away.
    // Tenures that began after the end, and other sessions, executions and homes, keep theirs.
    let (_d, s, _) = store();
    let started = T - 500;
    let bridged = tenure("s1", "acct-a", started, T + 10, T + 90_000);
    let inside = tenure("s1", "acct-a", started, T + 20, T + 5_000);
    let later = tenure("s1", "acct-a", started, T + 30_000, T + 90_000);
    let other_execution = tenure("s1", "acct-a", started + 1, T + 10, T + 90_000);
    let other_home = tenure("s1", "", started, T + 10, T + 90_000);
    let other_session = tenure("s2", "acct-a", started, T + 10, T + 90_000);
    for row in [
        &bridged,
        &inside,
        &later,
        &other_execution,
        &other_home,
        &other_session,
    ] {
        s.record_sighting(&cc(), row, 0).unwrap();
    }
    let ours = SightingRow {
        source: SightingSource::Run,
        recorded_at: T + 95_000,
        ..tenure("s1", "acct-a", started, T, T + 20_000)
    };

    s.end_sighting(&cc(), &ours, SightingEnd::Exactly).unwrap();
    let clamped = SightingRow {
        last_at: T + 20_000,
        recorded_at: T + 95_000,
        ..bridged
    };
    let mut want = vec![
        ours,
        other_home,
        clamped,
        other_execution,
        inside,
        later,
        other_session,
    ];
    want.sort_by(|a, b| {
        (&a.session_id, a.first_at, &a.home, a.started_at).cmp(&(
            &b.session_id,
            b.first_at,
            &b.home,
            b.started_at,
        ))
    });
    assert_eq!(s.sightings(&cc()).unwrap(), want);
}

#[test]
fn ending_a_tenure_at_least_moves_its_end_forward_only_and_never_past_until() {
    // Spec §6.1: at the child's exit, run's last confirmation is no reason to drop a later read
    // already stored: the end moves forward only, but never past the exit. The session's other tenures there are clamped
    // to where it ends.
    let (_d, s, _) = store();
    let started = T - 500;
    let read_later = tenure("s1", "acct-a", started, T, T + 30_000);
    let duplicate = tenure("s1", "acct-a", started, T + 10, T + 40_000);
    for row in [&read_later, &duplicate] {
        s.record_sighting(&cc(), row, 0).unwrap();
    }
    let ours = |last_at: i64| SightingRow {
        source: SightingSource::Run,
        recorded_at: T + 50_000,
        ..tenure("s1", "acct-a", started, T, last_at)
    };

    s.end_sighting(
        &cc(),
        &ours(T + 20_000),
        SightingEnd::AtLeast { until: i64::MAX },
    )
    .unwrap();
    let clamped = SightingRow {
        last_at: T + 30_000,
        recorded_at: T + 50_000,
        ..duplicate.clone()
    };
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [read_later.clone(), clamped.clone()],
        "the later read stands, and bounds the other tenure"
    );
    s.end_sighting(
        &cc(),
        &ours(T + 35_000),
        SightingEnd::AtLeast { until: i64::MAX },
    )
    .unwrap();
    let moved = SightingRow {
        last_at: T + 35_000,
        recorded_at: T + 50_000,
        ..read_later
    };
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [moved.clone(), clamped.clone()],
        "a later end moves it forward, and the row keeps the source that created it"
    );

    // Never past `until`, the exit: not even an end already stored, nor the other tenure.
    let capped = SightingRow {
        recorded_at: T + 60_000,
        ..ours(T + 40_000)
    };
    s.end_sighting(&cc(), &capped, SightingEnd::AtLeast { until: T + 25_000 })
        .unwrap();
    assert_eq!(
        s.sightings(&cc()).unwrap(),
        [
            SightingRow {
                last_at: T + 25_000,
                recorded_at: T + 60_000,
                ..moved
            },
            SightingRow {
                last_at: T + 25_000,
                recorded_at: T + 60_000,
                ..clamped
            }
        ]
    );
}

#[test]
fn continue_sighting_within_gives_up_on_a_busy_store() {
    // Spec §6.3: while another connection holds the write lock, the statusline's write gives up
    // after its own short wait and writes nothing. Free, it writes as `continue_sighting` does.
    let (_d, s, path) = store();
    let mut other = rusqlite::Connection::open(&path).unwrap();
    let held = other
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let seen = sighting("s1", "", T, T);
    let err = s
        .continue_sighting_within(&cc(), &seen, 60_000, Duration::from_millis(1))
        .unwrap_err();
    assert_eq!(
        err.sqlite_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy),
        "{err:?}"
    );
    drop(held);
    assert_eq!(
        s.latest_sighting(&cc(), "s1", "").unwrap(),
        None,
        "nothing written"
    );

    assert!(
        s.continue_sighting_within(&cc(), &seen, 60_000, Duration::from_millis(1))
            .unwrap()
    );
    assert!(
        !s.continue_sighting_within(&cc(), &seen, 60_000, Duration::from_millis(1))
            .unwrap(),
        "within the gap"
    );
    assert_eq!(s.latest_sighting(&cc(), "s1", "").unwrap(), Some(seen));
}

#[cfg(feature = "test-hooks")]
#[test]
fn continue_sighting_within_restores_the_connection_s_wait() {
    // Every other write of the process waits its full `busy_timeout` again, whether the short
    // write succeeded or gave up.
    let (_d, s, path) = store();
    let normal = s.busy_timeout().unwrap();
    assert_eq!(normal, Duration::from_millis(5000));
    let seen = sighting("s1", "", T, T);
    s.continue_sighting_within(&cc(), &seen, 60_000, Duration::from_millis(1))
        .unwrap();
    assert_eq!(s.busy_timeout().unwrap(), normal, "after a write");
    let mut other = rusqlite::Connection::open(&path).unwrap();
    let held = other
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    s.continue_sighting_within(&cc(), &seen, 0, Duration::from_millis(1))
        .unwrap_err();
    drop(held);
    assert_eq!(s.busy_timeout().unwrap(), normal, "after giving up");
}

#[test]
fn launches_open_attach_close() {
    let (_d, s, _) = store();
    s.open_launch(&cc(), "acct-a", T, T).unwrap();
    let open = LaunchRow {
        account_id: "acct-a".into(),
        started_at: T,
        ended_at: None,
        pid: None,
        recorded_at: T,
    };
    assert_eq!(s.launches(&cc()).unwrap(), [open.clone()]);
    s.attach_launch_pid(&cc(), "acct-a", T, 4242, T + 1)
        .unwrap();
    let spawned = LaunchRow {
        pid: Some(4242),
        recorded_at: T + 1,
        ..open
    };
    assert_eq!(s.launches(&cc()).unwrap(), [spawned.clone()]);
    s.close_launch(&cc(), "acct-a", T, T + 50, T + 51).unwrap();
    let closed = LaunchRow {
        ended_at: Some(T + 50),
        recorded_at: T + 51,
        ..spawned
    };
    assert_eq!(s.launches(&cc()).unwrap(), [closed.clone()]);
    s.close_launch(&cc(), "acct-a", T, T + 90, T + 91).unwrap();
    s.open_launch(&cc(), "acct-a", T, T + 99).unwrap();
    assert_eq!(
        s.launches(&cc()).unwrap(),
        [closed.clone()],
        "an ended launch keeps its end, and a recorded one its row"
    );
    s.open_launch(&cc(), "acct-b", T - 5, T).unwrap();
    s.open_launch(&fake(), "acct-a", T, T).unwrap();
    assert_eq!(
        s.launches(&cc())
            .unwrap()
            .iter()
            .map(|l| (l.account_id.as_str(), l.started_at))
            .collect::<Vec<_>>(),
        [("acct-b", T - 5), ("acct-a", T)],
        "by start"
    );
}

#[test]
fn evidence_changed_at_is_the_latest_recorded_at() {
    let (_d, s, _) = store();
    assert_eq!(s.evidence_changed_at(&cc()).unwrap(), None);
    s.open_launch(&cc(), "acct-a", 1_000, 1_000).unwrap();
    assert_eq!(s.evidence_changed_at(&cc()).unwrap(), Some(1_000));
    s.record_sighting(
        &cc(),
        &SightingRow {
            recorded_at: 2_000,
            ..sighting("s1", "", 1_500, 1_500)
        },
        0,
    )
    .unwrap();
    assert_eq!(s.evidence_changed_at(&cc()).unwrap(), Some(2_000));
    // active_history is stamped by SQLite's own clock (spec §4).
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let before = wall_ms();
    s.set_active(&cc(), Some(&a), Some(0)).unwrap();
    let at = s.evidence_changed_at(&cc()).unwrap().unwrap();
    assert!(at >= before - 1, "{before} {at}");
    assert_eq!(s.active_history(&cc()).unwrap(), [(at, Some("a".into()))]);
    s.close_launch(&cc(), "acct-a", 1_000, 5_000, at + 10)
        .unwrap();
    assert_eq!(s.evidence_changed_at(&cc()).unwrap(), Some(at + 10));
    s.open_launch(&fake(), "acct-f", 0, at + 1_000_000).unwrap();
    assert_eq!(
        s.evidence_changed_at(&cc()).unwrap(),
        Some(at + 10),
        "another provider's evidence is not this one's"
    );
}

#[test]
fn purge_provider_deletes_token_rows_and_keeps_the_epoch() {
    // Spec §13: `purge --provider` deletes the provider's rows of the provider-keyed token
    // tables (seven with ruling R8's `token_models`), its responses with its sessions;
    // `token_epoch` stays.
    let (_d, s, path) = store();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let f = add(&s, &fake(), "f", "f@x.co", 1);
    for (p, id) in [(cc(), &a), (fake(), &f)] {
        s.set_active(&p, Some(id), Some(0)).unwrap();
        let path = format!("/t/{p}.jsonl");
        s.commit_token_batch(
            &p,
            generation(&s),
            &[row(usage_key(&p, "m", "r"), "s1", T, 1)],
            &[file(&path, 1)],
            &[],
            T,
        )
        .unwrap();
        s.finish_token_refresh(&p, generation(&s), &RefreshStats::default(), T)
            .unwrap();
        s.prune_tokens(&p, generation(&s), 0, "h", T, &|| false)
            .unwrap();
        s.record_sighting(&p, &sighting("s1", "", T, T), 0).unwrap();
        s.open_launch(&p, "acct", T, T).unwrap();
    }
    let epoch = s.token_epoch().unwrap();
    assert!(epoch.is_some());

    s.delete_provider_rows(&cc()).unwrap();

    for (p, kept) in [(cc(), false), (fake(), true)] {
        assert_eq!(!s.token_files(&p).unwrap().is_empty(), kept, "{p}");
        assert_eq!(
            !s.token_usage(&p, None, None, None).unwrap().is_empty(),
            kept,
            "{p}"
        );
        assert_eq!(s.token_state(&p).unwrap().is_some(), kept, "{p}");
        assert_eq!(!s.sightings(&p).unwrap().is_empty(), kept, "{p}");
        assert_eq!(!s.launches(&p).unwrap().is_empty(), kept, "{p}");
        assert_eq!(!s.active_history(&p).unwrap().is_empty(), kept, "{p}");
        assert_eq!(!s.token_models(&p).unwrap().is_empty(), kept, "{p}");
        let sessions = raw_count(
            &path,
            &format!("SELECT count(*) FROM token_sessions WHERE provider = '{p}'"),
        );
        assert_eq!(sessions > 0, kept, "{p}");
        let leases = raw_count(
            &path,
            &format!("SELECT count(*) FROM leases WHERE name = 'prune:tokens:{p}'"),
        );
        assert_eq!(leases > 0, kept, "{p}");
    }
    assert_eq!(
        raw_count(&path, "SELECT count(*) FROM token_usage"),
        1,
        "the provider's responses went with its sessions"
    );
    assert_eq!(s.token_epoch().unwrap(), epoch);
}

#[test]
fn usage_key_is_stable() {
    // A key is stored: changing how it is derived would count every response twice.
    assert_eq!(
        usage_key(&cc(), "msg_01ABC", "req_01XYZ"),
        -3_640_870_161_912_078_139
    );
    let k = usage_key(&cc(), "m", "r");
    assert_ne!(
        k,
        usage_key(&fake(), "m", "r"),
        "the provider is part of it"
    );
    assert_ne!(
        k,
        usage_key(&cc(), "m", ""),
        "ruling R5: no request id is its own key"
    );
    assert_ne!(
        usage_key(&cc(), "ab", ""),
        usage_key(&cc(), "a", "b"),
        "the separator keeps the fields apart"
    );
}

#[test]
fn reads_are_per_provider_by_range_and_session() {
    let (_d, s, _) = store();
    let mut first = row(1, "s1", T, 1);
    first.model = "m-b".into();
    first.sidechain = true;
    let mut second = row(2, "s2", T + 10, 1);
    second.model = "m-a".into();
    second.project = None;
    let mut third = row(3, "s1", T + 20, 1);
    third.model = "m-b".into();
    commit(&s, &[third.clone(), first.clone(), second.clone()], T);
    let mut theirs = row(9, "s1", T + 5, 1);
    theirs.model = "m-z".into();
    s.commit_token_batch(&fake(), generation(&s), &[theirs], &[], &[], T)
        .unwrap();

    let usage = |since, until, session| keys(&s.token_usage(&cc(), since, until, session).unwrap());
    assert_eq!(all(&s), [first.clone(), second, third.clone()]);
    assert_eq!(
        usage(Some(T + 10), None, None),
        [2, 3],
        "since is inclusive"
    );
    assert_eq!(usage(None, Some(T + 10), None), [1], "until is exclusive");
    assert_eq!(usage(None, None, Some("s1")), [1, 3]);
    assert_eq!(usage(Some(T + 10), None, Some("s1")), [3]);
    assert!(usage(None, None, Some("nope")).is_empty());
    assert_eq!(
        s.token_usage(&cc(), None, None, Some("s1")).unwrap(),
        [first, third],
        "a session's responses keep their project and flags"
    );
    assert_eq!(s.token_models(&cc()).unwrap(), ["m-a", "m-b"]);
    assert_eq!(s.token_span(&cc()).unwrap(), (Some(T), Some(T + 20)));
    assert_eq!(s.token_models(&fake()).unwrap(), ["m-z"]);
    assert_eq!(
        s.token_span(&ProviderId::new("none")).unwrap(),
        (None, None)
    );
}

#[test]
fn a_session_takes_the_first_project_known_within_a_batch_and_across_batches() {
    // Spec §2.2: a session's project is the first one known. A response with no working
    // directory leaves it unknown, and a later response of that session supplies it, whether
    // it comes in the same batch or a later one.
    let (_d, s, _) = store();
    let mut unknown = row(1, "s1", T, 1);
    unknown.project = None;
    let known = row(2, "s1", T + 10, 1);
    commit(&s, &[unknown, known.clone()], T);
    let projects = |s: &Store, session| {
        s.token_usage(&cc(), None, None, Some(session))
            .unwrap()
            .into_iter()
            .map(|r| r.project)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        projects(&s, "s1"),
        [known.project.clone(), known.project.clone()],
        "a later row of the same batch supplies it"
    );

    let mut first = row(3, "s2", T, 1);
    first.project = None;
    commit(&s, &[first], T);
    assert_eq!(projects(&s, "s2"), [None]);
    let mut other = row(4, "s2", T + 10, 1);
    other.project = Some("/work/other".into());
    commit(&s, &[other], T);
    let mut late = row(5, "s2", T + 20, 1);
    late.project = Some("/work/late".into());
    commit(&s, &[late], T);
    assert_eq!(
        projects(&s, "s2"),
        vec![Some("/work/other".to_owned()); 3],
        "a later batch supplies it, and the first one known stays"
    );

    // A later line of the same response: in the same batch, and in a later batch where it
    // changes no count. Either one supplies the project, and the second moves `changed_at`.
    let mut line = row(6, "s3", T, 1);
    line.project = None;
    let mut again = line.clone();
    again.project = Some("/work/s3".into());
    commit(&s, &[line.clone(), again.clone()], T);
    assert_eq!(projects(&s, "s3"), [again.project.clone()], "same batch");

    let mut line = row(7, "s4", T, 1);
    line.project = None;
    commit(&s, &[line.clone()], T);
    let mut again = line.clone();
    again.project = Some("/work/s4".into());
    assert_eq!(
        commit(&s, &[again.clone()], T + 50),
        BatchOutcome::default(),
        "no response changed"
    );
    assert_eq!(projects(&s, "s4"), [again.project.clone()], "later batch");
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(
        state.changed_at,
        Some(T + 50),
        "a session's project changed"
    );

    // A copy of the response under another session supplies none to the session it keeps.
    let mut line = row(8, "s5", T, 1);
    line.project = None;
    let mut copy = line.clone();
    copy.session_id = "s6".into();
    commit(&s, &[line, copy], T);
    assert_eq!(projects(&s, "s5"), [None]);
}

#[test]
fn a_prune_after_a_full_purge_is_refused() {
    // Spec §5.1, §13: a prune is a write of the refresh's generation. A purge between the last
    // batch and the prune leaves it nothing to write: no lease, no state row.
    let (_d, s, _) = store();
    let started = generation(&s);
    commit(&s, &[row(1, "s1", T - 10 * DAY, 1)], T);
    s.empty_all().unwrap();
    let err = s
        .prune_tokens(&cc(), started, T - DAY, "h1", T + 1, &|| false)
        .unwrap_err();
    assert!(matches!(err, StoreError::TokenIndexPurged), "{err:?}");
    assert_eq!(s.token_state(&cc()).unwrap(), None, "not recreated");
    assert_eq!(s.token_epoch().unwrap(), None);

    // A generation begun after the purge prunes, and the refused one left no lease behind.
    let fresh = s.begin_token_refresh(&cc(), T + 2).unwrap();
    assert_eq!(
        s.prune_tokens(&cc(), fresh, T - DAY, "h2", T + 2, &|| false)
            .unwrap(),
        done(0)
    );
    assert_eq!(
        s.token_state(&cc()).unwrap().unwrap().pruned_at,
        Some(T + 2)
    );
}

#[test]
fn a_purge_between_a_prunes_chunks_stops_it_and_frees_its_lease() {
    // Spec §5.1, §13: each chunk is a write of the refresh's generation, so a purge that lands
    // between two chunks (while `stop` runs, the connection free) is met by the next chunk's
    // check, not only by the one at the start. The prune writes nothing more, answers
    // `TokenIndexPurged` and releases its lease at once. The purge here removes the generation
    // alone (`token_epoch`, which every full purge deletes): the lease row stays, so only the
    // release lets the next holder take it.
    let (_d, s, path) = store();
    let cutoff = T - 30 * DAY;
    let started = generation(&s);
    commit(&s, &[row(1, "old", cutoff - 1, 1), row(2, "live", T, 1)], T);
    let asks = Cell::new(0);
    let purge_between_chunks = || {
        asks.set(asks.get() + 1);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("DELETE FROM token_epoch", [])
            .unwrap();
        false
    };
    let err = s
        .prune_tokens(&cc(), started, cutoff, "h1", T + 1, &purge_between_chunks)
        .unwrap_err();
    assert!(matches!(err, StoreError::TokenIndexPurged), "{err:?}");
    assert_eq!(asks.get(), 1, "asked once, after the chunk that deleted");
    let state = s.token_state(&cc()).unwrap().unwrap();
    assert_eq!(state.pruned_at, None, "the refused prune finished nothing");
    assert_eq!(
        raw_count(
            &path,
            "SELECT expires_at FROM leases WHERE name = 'prune:tokens:claude-code'"
        ),
        T + 1,
        "the refused prune handed its lease back, expiring at its `now`"
    );

    // A new holder in a fresh generation takes the lease at the same instant.
    let fresh = s.begin_token_refresh(&cc(), T + 2).unwrap();
    assert_eq!(
        s.prune_tokens(&cc(), fresh, cutoff, "h2", T + 1, &|| false)
            .unwrap(),
        done(0)
    );
    assert_eq!(
        s.token_state(&cc()).unwrap().unwrap().pruned_at,
        Some(T + 1),
        "h2 took the lease and pruned"
    );
}
