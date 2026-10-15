//! The token tables (token report spec §4): the transcript files the index has read, its
//! sessions, models and responses, the session evidence (sightings, launches and the active
//! account's history), each provider's index state and the epoch. Every time is epoch
//! milliseconds. Counts are `u64` here and `INTEGER` in SQLite: a count above `i64::MAX` is
//! stored as `i64::MAX`.

use std::collections::HashMap;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use tagteam_core::ProviderId;
use tagteam_core::tokens::TokenCounts;

use super::usage::TAKE_LEASE_SQL;
use super::{Store, StoreError};

const DAY_MS: i64 = 86_400_000;

/// Responses one prune transaction deletes at most, so an overdue prune never holds the write
/// lock for long (spec §5.2).
const PRUNE_CHUNK: usize = 5_000;

/// What a prune deletes after the expired responses (spec §4), each phase in chunks of
/// `PRUNE_CHUNK` rows, one transaction per chunk: sessions left without a response, sightings
/// that ended before the cutoff, and launches that did. `?1` the provider, `?2` the cutoff, `?3`
/// the chunk.
const PRUNE_CLEANUP: [&str; 3] = [
    "DELETE FROM token_sessions WHERE id IN (SELECT id FROM token_sessions WHERE provider = ?1 \
     AND NOT EXISTS (SELECT 1 FROM token_usage WHERE session = token_sessions.id) LIMIT ?3)",
    "DELETE FROM session_sightings WHERE (provider, session_id, home, first_at) IN \
     (SELECT provider, session_id, home, first_at FROM session_sightings \
     WHERE provider = ?1 AND last_at < ?2 LIMIT ?3)",
    "DELETE FROM run_launches WHERE (provider, account_id, started_at) IN \
     (SELECT provider, account_id, started_at FROM run_launches \
     WHERE provider = ?1 AND ended_at < ?2 LIMIT ?3)",
];

/// A response's key: the first 8 bytes of SHA-256 over `provider NUL message_id NUL
/// request_id`, big-endian (spec §4). Response ids are kept only as this hash (spec §13).
pub fn usage_key(provider: &ProviderId, message_id: &str, request_id: &str) -> i64 {
    let mut hash = Sha256::new();
    hash.update(provider.as_str().as_bytes());
    hash.update([0]);
    hash.update(message_id.as_bytes());
    hash.update([0]);
    hash.update(request_id.as_bytes());
    let digest = hash.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(first)
}

/// One transcript file as the index last read it (spec §5.2). `size` is −1 until a read
/// consumed the file to its last newline, so no file matches it and none is skipped part-way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenFileRow {
    pub path: String,
    pub dev: u64,
    pub ino: u64,
    pub size: i64,
    pub mtime_ns: i64,
    pub offset: u64,
    pub head_hash: [u8; 8],
    pub tail_hash: [u8; 8],
}

/// One response (spec §2.2): `key` is its `usage_key`, and its counts, time and flags are
/// merged over its lines.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub key: i64,
    pub session_id: String,
    pub project: Option<String>,
    pub at_ms: i64,
    pub model: String,
    pub sidechain: bool,
    pub counts: TokenCounts,
    pub speed: Option<String>,
    pub finalized: bool,
}

/// What one batch did to the index: responses it added, and stored ones it changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    pub added: u64,
    pub updated: u64,
}

/// What one `prune_tokens` call did: the responses it deleted, and whether it finished. A
/// prune that stopped part-way released its lease, so the next call goes on with it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneOutcome {
    pub deleted: u64,
    pub complete: bool,
}

/// A completed refresh's statistics (spec §4 `token_state`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefreshStats {
    pub files: u64,
    pub responses: u64,
    pub skipped_lines: u64,
    pub unreadable_files: u64,
}

/// A provider's `token_state` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStateRow {
    pub refreshed_at: Option<i64>,
    pub changed_at: Option<i64>,
    pub stats: RefreshStats,
    pub pruned_at: Option<i64>,
}

/// Which source created a sighting (spec §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SightingSource {
    Run,
    Scan,
    Statusline,
}

impl SightingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SightingSource::Run => "run",
            SightingSource::Scan => "scan",
            SightingSource::Statusline => "statusline",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "run" => Some(SightingSource::Run),
            "scan" => Some(SightingSource::Scan),
            "statusline" => Some(SightingSource::Statusline),
            _ => None,
        }
    }
}

/// One execution of a session seen alive in a home (spec §6.4), keyed by the session, the
/// home and `first_at`. `home` is `""` for the default home, else the profile's account id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SightingRow {
    pub session_id: String,
    pub home: String,
    pub first_at: i64,
    pub last_at: i64,
    pub source: SightingSource,
    pub recorded_at: i64,
}

/// A `run` launch (spec §6.1), or a profile seeded at the migration (spec §4). `ended_at` is
/// `None` while the launch may still run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRow {
    pub account_id: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub pid: Option<u32>,
    pub recorded_at: i64,
}

/// When this store first recorded sightings and launches, and whether it seeded the profiles
/// that existed then (spec §7.2, ruling R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochRow {
    pub at: i64,
    pub seeded: bool,
}

fn to_sql(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn from_sql(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// A file's hash as stored. One of another length reads as zeros, which no read matches, so
/// that file is read again from its start (spec §5.2).
fn hash_col(r: &Row<'_>, idx: usize) -> rusqlite::Result<[u8; 8]> {
    let bytes: Vec<u8> = r.get(idx)?;
    Ok(bytes.try_into().unwrap_or([0; 8]))
}

/// `counts` as the store holds them.
fn stored(counts: &TokenCounts) -> TokenCounts {
    let clamp = |n: u64| from_sql(to_sql(n));
    TokenCounts {
        input: clamp(counts.input),
        output: clamp(counts.output),
        cache_write_5m: clamp(counts.cache_write_5m),
        cache_write_1h: clamp(counts.cache_write_1h),
        cache_read: clamp(counts.cache_read),
        web_search: clamp(counts.web_search),
    }
}

/// The columns of a response that the merge rule writes (spec §2.2).
#[derive(Debug, Clone, PartialEq)]
struct Merged {
    at: i64,
    counts: TokenCounts,
    speed: Option<String>,
    finalized: bool,
}

impl Merged {
    /// The row as the upsert leaves it after `row`: the earliest time, each count's maximum,
    /// the newer speed when it has one, and final once any line was.
    fn with(&self, row: &UsageRow) -> Merged {
        let mut counts = self.counts;
        counts.merge_max(&stored(&row.counts));
        Merged {
            at: self.at.min(row.at_ms),
            counts,
            speed: row.speed.clone().or_else(|| self.speed.clone()),
            finalized: self.finalized || row.finalized,
        }
    }
}

fn merged_from_row(r: &Row<'_>) -> rusqlite::Result<Merged> {
    Ok(Merged {
        at: r.get(0)?,
        counts: TokenCounts {
            input: from_sql(r.get(1)?),
            output: from_sql(r.get(2)?),
            cache_write_5m: from_sql(r.get(3)?),
            cache_write_1h: from_sql(r.get(4)?),
            cache_read: from_sql(r.get(5)?),
            web_search: from_sql(r.get(6)?),
        },
        speed: r.get(7)?,
        finalized: r.get::<_, i64>(8)? != 0,
    })
}

/// Spec §2.2's rule for a later line of a response already held: the earliest time, each
/// count's maximum, the newer speed when it has one, and final once either line is. `into`
/// keeps its session, model and sidechain flag, as the upsert does, and its project unless it
/// has none and `row` is of the same session: a session's project is the first one known.
pub(crate) fn merge_line(into: &mut UsageRow, row: &UsageRow) {
    if into.project.is_none() && into.session_id == row.session_id {
        into.project.clone_from(&row.project);
    }
    into.at_ms = into.at_ms.min(row.at_ms);
    into.counts.merge_max(&row.counts);
    if row.speed.is_some() {
        into.speed = row.speed.clone();
    }
    into.finalized |= row.finalized;
}

/// `rows`, one per key: a key's later rows merge into its first by `merge_line`.
fn merge_batch(rows: &[UsageRow]) -> Vec<UsageRow> {
    let mut at: HashMap<i64, usize> = HashMap::with_capacity(rows.len());
    let mut out: Vec<UsageRow> = Vec::with_capacity(rows.len());
    for row in rows {
        match at.get(&row.key) {
            Some(&i) => merge_line(&mut out[i], row),
            None => {
                at.insert(row.key, out.len());
                out.push(row.clone());
            }
        }
    }
    out
}

const USAGE_MERGED: &str = "SELECT at, input, output, cw_5m, cw_1h, cache_read, web_search, speed, \
     finalized FROM token_usage WHERE key = ?1";

/// Spec §4's upsert: the earliest time, each count's maximum, the newer speed when it has one.
const UPSERT_USAGE: &str = "INSERT INTO token_usage (key, session, at, model, sidechain, input, \
     output, cw_5m, cw_1h, cache_read, web_search, speed, finalized) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
     ON CONFLICT(key) DO UPDATE SET at = min(at, excluded.at), \
     input = max(input, excluded.input), output = max(output, excluded.output), \
     cw_5m = max(cw_5m, excluded.cw_5m), cw_1h = max(cw_1h, excluded.cw_1h), \
     cache_read = max(cache_read, excluded.cache_read), \
     web_search = max(web_search, excluded.web_search), \
     speed = coalesce(excluded.speed, speed), finalized = max(finalized, excluded.finalized)";

/// A session's id, creating its row on its first response. The project is the first one
/// known (spec §4).
const UPSERT_SESSION: &str = "INSERT INTO token_sessions (provider, session_id, project) \
     VALUES (?1, ?2, ?3) ON CONFLICT(provider, session_id) DO UPDATE SET \
     project = coalesce(token_sessions.project, excluded.project) \
     RETURNING id, project IS NOT NULL";

/// A session's project, when it has none yet: a response that changes no count can still be
/// the first of its session's to know it.
const FILL_PROJECT: &str = "UPDATE token_sessions SET project = ?3 \
     WHERE provider = ?1 AND session_id = ?2 AND project IS NULL";

const MODEL_ID: &str = "SELECT id FROM token_models WHERE provider = ?1 AND name = ?2";

const INSERT_MODEL: &str = "INSERT INTO token_models (provider, name) VALUES (?1, ?2)";

const UPSERT_FILE: &str = "INSERT INTO token_files (path, provider, dev, ino, size, mtime_ns, \
     offset, head_hash, tail_hash, indexed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
     ON CONFLICT(path) DO UPDATE SET provider = excluded.provider, dev = excluded.dev, \
     ino = excluded.ino, size = excluded.size, mtime_ns = excluded.mtime_ns, \
     offset = excluded.offset, head_hash = excluded.head_hash, tail_hash = excluded.tail_hash, \
     indexed_at = excluded.indexed_at";

const ENSURE_STATE: &str = "INSERT OR IGNORE INTO token_state (provider) VALUES (?1)";

/// The provider's sessions, for a filter on `token_usage.session`.
const SESSIONS_OF: &str = "SELECT id FROM token_sessions WHERE provider = ?1";

const USAGE_COLUMNS: &str = "u.key, s.session_id, s.project, u.at, m.name, u.sidechain, \
     u.input, u.output, u.cw_5m, u.cw_1h, u.cache_read, u.web_search, u.speed, u.finalized";

fn usage_from_row(r: &Row<'_>) -> rusqlite::Result<UsageRow> {
    Ok(UsageRow {
        key: r.get(0)?,
        session_id: r.get(1)?,
        project: r.get(2)?,
        at_ms: r.get(3)?,
        model: r.get(4)?,
        sidechain: r.get::<_, i64>(5)? != 0,
        counts: TokenCounts {
            input: from_sql(r.get(6)?),
            output: from_sql(r.get(7)?),
            cache_write_5m: from_sql(r.get(8)?),
            cache_write_1h: from_sql(r.get(9)?),
            cache_read: from_sql(r.get(10)?),
            web_search: from_sql(r.get(11)?),
        },
        speed: r.get(12)?,
        finalized: r.get::<_, i64>(13)? != 0,
    })
}

const SIGHTING_COLUMNS: &str = "session_id, home, first_at, last_at, source, recorded_at";

fn sighting_from_row(r: &Row<'_>) -> rusqlite::Result<SightingRow> {
    let source: String = r.get(4)?;
    Ok(SightingRow {
        session_id: r.get(0)?,
        home: r.get(1)?,
        first_at: r.get(2)?,
        last_at: r.get(3)?,
        source: SightingSource::parse(&source).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                "unknown sighting source".into(),
            )
        })?,
        recorded_at: r.get(5)?,
    })
}

/// Spec §6.4's sighting write on one connection, which `Store::record_sighting` and
/// `Store::record_sighting_within` share: inserts the sighting, or extends the stored one when
/// `s.last_at` is at least `min_gap_ms` past its own. Returns whether it wrote.
fn write_sighting(
    c: &Connection,
    provider: &ProviderId,
    s: &SightingRow,
    min_gap_ms: i64,
) -> rusqlite::Result<bool> {
    let n = c.execute(
        "INSERT INTO session_sightings \
         (provider, session_id, home, first_at, last_at, source, recorded_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(provider, session_id, home, first_at) DO UPDATE SET \
         last_at = excluded.last_at, recorded_at = excluded.recorded_at \
         WHERE excluded.last_at - session_sightings.last_at >= ?8",
        params![
            provider.as_str(),
            s.session_id,
            s.home,
            s.first_at,
            s.last_at,
            s.source.as_str(),
            s.recorded_at,
            min_gap_ms,
        ],
    )?;
    Ok(n > 0)
}

/// The epoch's `at`: the index's generation (spec §5.1, §13). `None` after a full purge.
fn current_generation(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT at FROM token_epoch ORDER BY rowid LIMIT 1",
        [],
        |r| r.get(0),
    )
    .optional()
}

/// The index's generation check: `epoch` is what `Store::begin_token_refresh` gave the
/// refresh. Every full purge deletes that row, and the next refresh to begin records a new
/// one, so a missing or different row means a purge ran since, and the refresh's write would
/// land in the emptied index.
fn same_generation(c: &Connection, epoch: i64) -> Result<(), StoreError> {
    if current_generation(c)? == Some(epoch) {
        Ok(())
    } else {
        Err(StoreError::TokenIndexPurged)
    }
}

fn prune_lease(provider: &ProviderId) -> String {
    format!("prune:tokens:{provider}")
}

/// The provider's responses older than `cutoff_ms`, at most `PRUNE_CHUNK` of them. A chunk
/// that deletes any takes their count off `token_state.responses` and sets `changed_at` to
/// `now_ms` in the same transaction, so a prune that stops part-way leaves no change unseen.
/// Returns how many went.
fn prune_chunk(
    c: &Connection,
    provider: &ProviderId,
    cutoff_ms: i64,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    let n = c.execute(
        &format!(
            "DELETE FROM token_usage WHERE id IN (SELECT id FROM token_usage \
             WHERE at < ?2 AND session IN ({SESSIONS_OF}) LIMIT ?3)"
        ),
        params![provider.as_str(), cutoff_ms, PRUNE_CHUNK as i64],
    )?;
    if n > 0 {
        c.execute(
            "UPDATE token_state SET responses = max(responses - ?2, 0), changed_at = ?3 \
             WHERE provider = ?1",
            params![provider.as_str(), n as i64, now_ms],
        )?;
    }
    Ok(n)
}

// Every transaction here that reads before it writes is `IMMEDIATE`, as in `usage.rs`, and each
// stays short (spec §5.2): a batch is at most 2,000 responses, and a prune deletes in chunks.
impl Store {
    /// The provider's transcript files as the index last read them, by path.
    pub fn token_files(&self, provider: &ProviderId) -> Result<Vec<TokenFileRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT path, dev, ino, size, mtime_ns, offset, head_hash, tail_hash FROM token_files \
             WHERE provider = ?1 ORDER BY path",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| {
                Ok(TokenFileRow {
                    path: r.get(0)?,
                    dev: r.get::<_, i64>(1)? as u64,
                    ino: r.get::<_, i64>(2)? as u64,
                    size: r.get(3)?,
                    mtime_ns: r.get(4)?,
                    offset: from_sql(r.get(5)?),
                    head_hash: hash_col(r, 6)?,
                    tail_hash: hash_col(r, 7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The start of a refresh, under its lock (spec §5.1): the index's generation, which every
    /// batch and the completion of this refresh pass back. In one `IMMEDIATE` transaction it
    /// creates the provider's `token_state` row if absent, and, when a full purge emptied
    /// `token_epoch`, records the epoch anew at `now_ms` (spec §5.2 step 5) with
    /// `seeded = 0`: the purge deleted the seeded launches, so coverage before it is not
    /// complete (ruling R1). Every purge deletes the row, so each one starts a new generation.
    pub fn begin_token_refresh(
        &self,
        provider: &ProviderId,
        now_ms: i64,
    ) -> Result<i64, StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(ENSURE_STATE, [provider.as_str()])?;
        let epoch = match current_generation(&tx)? {
            Some(at) => at,
            None => {
                tx.execute(
                    "INSERT INTO token_epoch (at, seeded) VALUES (?1, 0)",
                    [now_ms],
                )?;
                now_ms
            }
        };
        tx.commit()?;
        Ok(epoch)
    }

    /// One batch of a refresh (spec §5.2 step 4), in one `IMMEDIATE` transaction: upserts
    /// `rows` by the merge rule (spec §4), writes each of `files`, deletes the file rows at
    /// `removed`, and creates the provider's `token_state` row if absent. When a response was
    /// added or changed, or a session's project became known, it sets `changed_at` to `now_ms`,
    /// and it adds the new responses to `responses`. A response whose merge changes nothing is
    /// not written. `epoch` is what
    /// `begin_token_refresh` gave the refresh; when a full purge has run since, nothing is
    /// written and the answer is `TokenIndexPurged`.
    pub fn commit_token_batch(
        &self,
        provider: &ProviderId,
        epoch: i64,
        rows: &[UsageRow],
        files: &[TokenFileRow],
        removed: &[String],
        now_ms: i64,
    ) -> Result<BatchOutcome, StoreError> {
        let rows = merge_batch(rows);
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        same_generation(&tx, epoch)?;
        tx.execute(ENSURE_STATE, [provider.as_str()])?;
        let mut outcome = BatchOutcome::default();
        let mut filled = 0;
        {
            let mut read = tx.prepare(USAGE_MERGED)?;
            let mut upsert = tx.prepare(UPSERT_USAGE)?;
            let mut session = tx.prepare(UPSERT_SESSION)?;
            let mut fill = tx.prepare(FILL_PROJECT)?;
            let mut model_id = tx.prepare(MODEL_ID)?;
            let mut insert_model = tx.prepare(INSERT_MODEL)?;
            // Each session's id, and whether its project is known yet: a later row that knows
            // it upserts the session again, so the first project known is kept (spec §2.2).
            let mut sessions: HashMap<&str, (i64, bool)> = HashMap::new();
            let mut models: HashMap<&str, i64> = HashMap::new();
            for row in &rows {
                match read.query_row([row.key], merged_from_row).optional()? {
                    None => outcome.added += 1,
                    Some(old) if old.with(row) != old => outcome.updated += 1,
                    Some(_) => {
                        let known = sessions
                            .get(row.session_id.as_str())
                            .is_some_and(|&(_, known)| known);
                        if !known && row.project.is_some() {
                            filled += fill.execute(params![
                                provider.as_str(),
                                row.session_id,
                                row.project
                            ])?;
                        }
                        continue;
                    }
                }
                let id = match sessions.get(row.session_id.as_str()) {
                    Some(&(id, known)) if known || row.project.is_none() => id,
                    _ => {
                        let (id, known) = session.query_row(
                            params![provider.as_str(), row.session_id, row.project],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )?;
                        sessions.insert(&row.session_id, (id, known));
                        id
                    }
                };
                let model = match models.get(row.model.as_str()) {
                    Some(id) => *id,
                    None => {
                        let found = model_id
                            .query_row(params![provider.as_str(), row.model], |r| r.get(0))
                            .optional()?;
                        let id = match found {
                            Some(id) => id,
                            None => {
                                insert_model.execute(params![provider.as_str(), row.model])?;
                                tx.last_insert_rowid()
                            }
                        };
                        models.insert(&row.model, id);
                        id
                    }
                };
                let counts = &row.counts;
                upsert.execute(params![
                    row.key,
                    id,
                    row.at_ms,
                    model,
                    row.sidechain,
                    to_sql(counts.input),
                    to_sql(counts.output),
                    to_sql(counts.cache_write_5m),
                    to_sql(counts.cache_write_1h),
                    to_sql(counts.cache_read),
                    to_sql(counts.web_search),
                    row.speed,
                    row.finalized,
                ])?;
            }
            let mut file = tx.prepare(UPSERT_FILE)?;
            for f in files {
                file.execute(params![
                    f.path,
                    provider.as_str(),
                    f.dev as i64,
                    f.ino as i64,
                    f.size,
                    f.mtime_ns,
                    to_sql(f.offset),
                    &f.head_hash[..],
                    &f.tail_hash[..],
                    now_ms,
                ])?;
            }
            let mut remove =
                tx.prepare("DELETE FROM token_files WHERE provider = ?1 AND path = ?2")?;
            for path in removed {
                remove.execute(params![provider.as_str(), path])?;
            }
        }
        if outcome.added + outcome.updated > 0 || filled > 0 {
            tx.execute(
                "UPDATE token_state SET changed_at = ?2, responses = responses + ?3 \
                 WHERE provider = ?1",
                params![provider.as_str(), now_ms, to_sql(outcome.added)],
            )?;
        }
        tx.commit()?;
        Ok(outcome)
    }

    /// Spec §5.2 step 5, in one `IMMEDIATE` transaction: sets `refreshed_at` and the refresh's
    /// `files`, `skipped_lines` and `unreadable_files`, and on the provider's first completion
    /// `changed_at` too, which ends the index's partial state (spec §14). `stats.responses` is
    /// not written: `responses` is the index's own count, which each batch and prune keep.
    /// Like a batch, it writes nothing and answers `TokenIndexPurged` when the index's
    /// generation is no longer `epoch`, what `begin_token_refresh` gave the refresh.
    pub fn finish_token_refresh(
        &self,
        provider: &ProviderId,
        epoch: i64,
        stats: &RefreshStats,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        same_generation(&tx, epoch)?;
        tx.execute(ENSURE_STATE, [provider.as_str()])?;
        tx.execute(
            "UPDATE token_state SET \
             changed_at = CASE WHEN refreshed_at IS NULL THEN ?2 ELSE changed_at END, \
             refreshed_at = ?2, files = ?3, skipped_lines = ?4, unreadable_files = ?5 \
             WHERE provider = ?1",
            params![
                provider.as_str(),
                now_ms,
                to_sql(stats.files),
                to_sql(stats.skipped_lines),
                to_sql(stats.unreadable_files),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Retention (spec §4), at most once a day per provider: the lease
    /// `prune:tokens:<provider>` spaces prunes, as `prune_samples` does (core §6.1). Deletes
    /// the provider's responses older than `cutoff_ms`, in chunks of their own transactions,
    /// each of which that deletes any also sets `changed_at`; then its sessions left without
    /// one, the sightings that ended before the cutoff (those of every pruned session, but one
    /// still alive), and the launches that did, and sets `pruned_at`. Model names stay.
    ///
    /// `stop` is asked after each chunk that deleted something, before the next one, so a
    /// refresh's budget and cancel reach a prune that a retention cut made large. A chunk that
    /// deletes nothing moves straight on without asking: every call deletes something or
    /// completes, so a prune stopped call after call still finishes. Stopped, the prune
    /// releases its lease and returns `complete: false`: the next call goes on. Another
    /// prune's lease, held or spent today, is nothing to do: `complete: true` with nothing
    /// deleted. Like a batch, it writes nothing and answers `TokenIndexPurged` when the
    /// index's generation is no longer `epoch`, what `begin_token_refresh` gave the refresh;
    /// checked in each chunk's transaction, and a lease it holds is released first.
    pub fn prune_tokens(
        &self,
        provider: &ProviderId,
        epoch: i64,
        cutoff_ms: i64,
        holder: &str,
        now_ms: i64,
        stop: &dyn Fn() -> bool,
    ) -> Result<PruneOutcome, StoreError> {
        // A prune of a purged index writes nothing, not even its lease or the state row.
        same_generation(&self.lock(), epoch)?;
        let lease = prune_lease(provider);
        let taken = self.lock().execute(
            TAKE_LEASE_SQL,
            params![lease, holder, now_ms + DAY_MS, now_ms],
        )?;
        if taken != 1 {
            return Ok(PruneOutcome {
                deleted: 0,
                complete: true,
            });
        }
        let mut deleted = 0u64;
        // Phase 0 is the expired responses, then each of `PRUNE_CLEANUP`; every chunk is its own
        // transaction, and `stop` is asked only after one that deleted something.
        let mut phase = 0;
        let mut progressed = false;
        loop {
            if progressed && stop() {
                self.exec(
                    "UPDATE leases SET expires_at = ?3 WHERE name = ?1 AND holder = ?2",
                    &[&lease, &holder, &now_ms],
                )?;
                return Ok(PruneOutcome {
                    deleted,
                    complete: false,
                });
            }
            let mut c = self.lock();
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Err(e) = same_generation(&tx, epoch) {
                drop(tx);
                drop(c);
                self.exec(
                    "UPDATE leases SET expires_at = ?3 WHERE name = ?1 AND holder = ?2",
                    &[&lease, &holder, &now_ms],
                )?;
                return Err(e);
            }
            let n = if phase == 0 {
                let n = prune_chunk(&tx, provider, cutoff_ms, now_ms)?;
                deleted += n as u64;
                n
            } else {
                tx.execute(
                    PRUNE_CLEANUP[phase - 1],
                    params![provider.as_str(), cutoff_ms, PRUNE_CHUNK as i64],
                )?
            };
            let last = phase == PRUNE_CLEANUP.len() && n < PRUNE_CHUNK;
            if last {
                tx.execute(ENSURE_STATE, [provider.as_str()])?;
                tx.execute(
                    "UPDATE token_state SET pruned_at = ?2 WHERE provider = ?1",
                    params![provider.as_str(), now_ms],
                )?;
            }
            tx.commit()?;
            if last {
                break;
            }
            progressed = n > 0;
            if n < PRUNE_CHUNK {
                phase += 1;
            }
        }
        Ok(PruneOutcome {
            deleted,
            complete: true,
        })
    }

    /// The connection's busy timeout: for the tests that pin `record_sighting_within`
    /// restoring it.
    #[cfg(feature = "test-hooks")]
    pub fn busy_timeout(&self) -> Result<Duration, StoreError> {
        let ms: i64 = self
            .lock()
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
        Ok(Duration::from_millis(from_sql(ms)))
    }

    /// The provider's `token_state` row; `None` before its first refresh wrote one.
    pub fn token_state(&self, provider: &ProviderId) -> Result<Option<TokenStateRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT refreshed_at, changed_at, files, responses, skipped_lines, \
                 unreadable_files, pruned_at FROM token_state WHERE provider = ?1",
                [provider.as_str()],
                |r| {
                    Ok(TokenStateRow {
                        refreshed_at: r.get(0)?,
                        changed_at: r.get(1)?,
                        stats: RefreshStats {
                            files: from_sql(r.get(2)?),
                            responses: from_sql(r.get(3)?),
                            skipped_lines: from_sql(r.get(4)?),
                            unreadable_files: from_sql(r.get(5)?),
                        },
                        pruned_at: r.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    /// The epoch (spec §7.2); `None` after a full purge until a refresh begins.
    pub fn token_epoch(&self) -> Result<Option<EpochRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT at, seeded FROM token_epoch ORDER BY rowid LIMIT 1",
                [],
                |r| {
                    Ok(EpochRow {
                        at: r.get(0)?,
                        seeded: r.get::<_, i64>(1)? != 0,
                    })
                },
            )
            .optional()?)
    }

    /// The latest `recorded_at` of the provider's sightings, launches and active-account
    /// history (spec §6.4): with `token_state.changed_at`, what `IndexState.changed_at_ms`
    /// follows (spec §10). `None` with no evidence at all.
    pub fn evidence_changed_at(&self, provider: &ProviderId) -> Result<Option<i64>, StoreError> {
        Ok(self.lock().query_row(
            "SELECT max(m) FROM ( \
             SELECT max(recorded_at) AS m FROM session_sightings WHERE provider = ?1 \
             UNION ALL SELECT max(recorded_at) FROM run_launches WHERE provider = ?1 \
             UNION ALL SELECT max(recorded_at) FROM active_history WHERE provider = ?1)",
            [provider.as_str()],
            |r| r.get(0),
        )?)
    }

    /// The provider's responses at `since_ms` (inclusive) to `until_ms` (exclusive), of one
    /// session when `session` names it, by time, then the order they were first indexed in.
    pub fn token_usage(
        &self,
        provider: &ProviderId,
        since_ms: Option<i64>,
        until_ms: Option<i64>,
        session: Option<&str>,
    ) -> Result<Vec<UsageRow>, StoreError> {
        let c = self.lock();
        let since = since_ms.unwrap_or(i64::MIN);
        let until = until_ms.unwrap_or(i64::MAX);
        let rows = match session {
            Some(id) => c
                .prepare(&format!(
                    "SELECT {USAGE_COLUMNS} FROM token_sessions s \
                     JOIN token_usage u ON u.session = s.id JOIN token_models m ON m.id = u.model \
                     WHERE s.provider = ?1 AND s.session_id = ?4 AND u.at >= ?2 AND u.at < ?3 \
                     ORDER BY u.at, u.id"
                ))?
                .query_map(params![provider.as_str(), since, until, id], usage_from_row)?
                .collect::<Result<Vec<_>, _>>()?,
            None => c
                .prepare(&format!(
                    "SELECT {USAGE_COLUMNS} FROM token_usage u \
                     JOIN token_sessions s ON s.id = u.session JOIN token_models m ON m.id = u.model \
                     WHERE s.provider = ?1 AND u.at >= ?2 AND u.at < ?3 ORDER BY u.at, u.id"
                ))?
                .query_map(params![provider.as_str(), since, until], usage_from_row)?
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    /// Every provider with a row in a token or evidence table, sorted. A full purge takes the
    /// tokens lock of each (spec §13), one with no account, or one only another build
    /// registers, included.
    pub fn token_providers(&self) -> Result<Vec<ProviderId>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT provider FROM token_files UNION SELECT provider FROM token_sessions \
             UNION SELECT provider FROM token_models UNION SELECT provider FROM token_state \
             UNION SELECT provider FROM session_sightings UNION SELECT provider FROM run_launches \
             UNION SELECT provider FROM active_history ORDER BY provider",
        )?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|p| p.map(ProviderId::new))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every model the provider's index has held since its last purge, sorted: a model whose
    /// responses retention pruned is still named (ruling R8).
    pub fn token_models(&self, provider: &ProviderId) -> Result<Vec<String>, StoreError> {
        let c = self.lock();
        let mut stmt =
            c.prepare("SELECT name FROM token_models WHERE provider = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map([provider.as_str()], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The times of the provider's oldest and newest responses, each read from one end of the
    /// time index.
    pub fn token_span(
        &self,
        provider: &ProviderId,
    ) -> Result<(Option<i64>, Option<i64>), StoreError> {
        let c = self.lock();
        let end = |order: &str| -> rusqlite::Result<Option<i64>> {
            c.query_row(
                &format!(
                    "SELECT at FROM token_usage INDEXED BY token_usage_by_at \
                     WHERE session IN ({SESSIONS_OF}) ORDER BY at {order} LIMIT 1"
                ),
                [provider.as_str()],
                |r| r.get(0),
            )
            .optional()
        };
        Ok((end("ASC")?, end("DESC")?))
    }

    /// Spec §6.4: inserts the sighting, or extends the stored one to `s.last_at` when that is
    /// at least `min_gap_ms` past its own `last_at`. An extension sets `last_at` and
    /// `recorded_at` alone: `source` stays the creator's. Returns whether it wrote.
    pub fn record_sighting(
        &self,
        provider: &ProviderId,
        s: &SightingRow,
        min_gap_ms: i64,
    ) -> Result<bool, StoreError> {
        Ok(write_sighting(&self.lock(), provider, s, min_gap_ms)?)
    }

    /// `record_sighting`, giving up after `wait` on another connection's write lock
    /// (`SQLITE_BUSY`) instead of the connection's own, far longer, wait: the statusline's
    /// sighting, which never holds its line up (spec §6.3). The connection stays locked across
    /// the write, so no other thread of this process writes with the short wait.
    pub fn record_sighting_within(
        &self,
        provider: &ProviderId,
        s: &SightingRow,
        min_gap_ms: i64,
        wait: Duration,
    ) -> Result<bool, StoreError> {
        let c = self.lock();
        c.busy_timeout(wait)?;
        let written = write_sighting(&c, provider, s, min_gap_ms);
        c.busy_timeout(super::BUSY_TIMEOUT)?;
        Ok(written?)
    }

    /// `run`'s exit write (spec §6.1): sets the sighting's end to the child's exit, back as well
    /// as forward (the child was not alive after it, whatever a poll stamped just before it
    /// stopped); never before `first_at`. Sets `recorded_at` when the end changed.
    pub fn set_sighting_last_at(
        &self,
        provider: &ProviderId,
        session_id: &str,
        home: &str,
        first_at: i64,
        last_at: i64,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.exec(
            "UPDATE session_sightings SET last_at = max(first_at, ?5), recorded_at = ?6 \
             WHERE provider = ?1 AND session_id = ?2 AND home = ?3 AND first_at = ?4 \
             AND last_at != max(first_at, ?5)",
            &[
                &provider.as_str(),
                &session_id,
                &home,
                &first_at,
                &last_at,
                &now_ms,
            ],
        )?;
        Ok(())
    }

    /// The latest execution's sighting of `session_id` in `home` (spec §6.3).
    pub fn latest_sighting(
        &self,
        provider: &ProviderId,
        session_id: &str,
        home: &str,
    ) -> Result<Option<SightingRow>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                &format!(
                    "SELECT {SIGHTING_COLUMNS} FROM session_sightings \
                     WHERE provider = ?1 AND session_id = ?2 AND home = ?3 \
                     ORDER BY first_at DESC LIMIT 1"
                ),
                params![provider.as_str(), session_id, home],
                sighting_from_row,
            )
            .optional()?)
    }

    /// Every sighting of the provider, by session, then `first_at`, then home.
    pub fn sightings(&self, provider: &ProviderId) -> Result<Vec<SightingRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(&format!(
            "SELECT {SIGHTING_COLUMNS} FROM session_sightings WHERE provider = ?1 \
             ORDER BY session_id, first_at, home"
        ))?;
        let rows = stmt
            .query_map([provider.as_str()], sighting_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Spec §6.1: an open launch, recorded before `run` spawns its child. A launch already
    /// recorded at that key is kept as it is.
    pub fn open_launch(
        &self,
        provider: &ProviderId,
        account_id: &str,
        started_at: i64,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.exec(
            "INSERT OR IGNORE INTO run_launches \
             (provider, account_id, started_at, ended_at, pid, recorded_at) \
             VALUES (?1, ?2, ?3, NULL, NULL, ?4)",
            &[&provider.as_str(), &account_id, &started_at, &now_ms],
        )?;
        Ok(())
    }

    /// The child's pid, once `run` has spawned it (spec §6.1).
    pub fn attach_launch_pid(
        &self,
        provider: &ProviderId,
        account_id: &str,
        started_at: i64,
        pid: u32,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.exec(
            "UPDATE run_launches SET pid = ?4, recorded_at = ?5 \
             WHERE provider = ?1 AND account_id = ?2 AND started_at = ?3",
            &[&provider.as_str(), &account_id, &started_at, &pid, &now_ms],
        )?;
        Ok(())
    }

    /// Ends an open launch at `ended_at` (spec §6.1). A launch already ended keeps its end.
    pub fn close_launch(
        &self,
        provider: &ProviderId,
        account_id: &str,
        started_at: i64,
        ended_at: i64,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.exec(
            "UPDATE run_launches SET ended_at = ?4, recorded_at = ?5 \
             WHERE provider = ?1 AND account_id = ?2 AND started_at = ?3 AND ended_at IS NULL",
            &[
                &provider.as_str(),
                &account_id,
                &started_at,
                &ended_at,
                &now_ms,
            ],
        )?;
        Ok(())
    }

    /// Every launch of the provider, by start, then account.
    pub fn launches(&self, provider: &ProviderId) -> Result<Vec<LaunchRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT account_id, started_at, ended_at, pid, recorded_at FROM run_launches \
             WHERE provider = ?1 ORDER BY started_at, account_id",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| {
                Ok(LaunchRow {
                    account_id: r.get(0)?,
                    started_at: r.get(1)?,
                    ended_at: r.get(2)?,
                    pid: r
                        .get::<_, Option<i64>>(3)?
                        .and_then(|p| u32::try_from(p).ok()),
                    recorded_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every change of the provider's active account (spec §4): `(at, account_id)`, by time,
    /// a row with no account meaning none was active.
    pub fn active_history(
        &self,
        provider: &ProviderId,
    ) -> Result<Vec<(i64, Option<String>)>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT at, account_id FROM active_history WHERE provider = ?1 ORDER BY at, rowid",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
