# tagteam — token report

**Status:** Approved
**Date:** 2026-10-14
**Scope:** A per-account token and cost report over Claude Code's transcripts: an index in the
store, an engine view, and the `tagteam tokens` command. It extends sub-project 1, whose
[core spec](2026-09-26-tagteam-core-cli-design.md) is cited as "core §n"; a bare §n is this
spec. It is the view the TUI (sub-project 3) renders, and the daemon (sub-project 2) keeps its
index current. Both build on §10 and §12 without reshaping them.

Claude Code facts were verified against Claude Code **2.1.299** on 2026-10-14, over this
machine's full transcript tree (2,517 files, 3.42 GB, 246,718 assistant lines).

---

## 1. Purpose

ccusage reports Claude Code's token use and its API-equivalent cost from the transcripts. It
cannot say which account a request was billed to, because no transcript records the account.
tagteam can: it knows each `run` profile's account, and when it switched the default home's
login. This report gives ccusage's views — by day, month, session and 5-hour block, by model —
with the account each request went to.

Claude Code deletes transcripts after 30 days by default; this machine's span 2026-09-15 to
2026-10-14. The index keeps the numbers past that, so it is the report's history, not a cache.

### 1.1 Success criteria

1. `tagteam tokens` totals per model equal an independent count over the same transcripts with
   the dedup rule of §2.2, to the token.
2. Every response is attributed to an account, or shown as unattributed; no response is
   counted twice. Under §7.4's two assumptions, none is attributed to an account it cannot
   have used (§7).
3. A cold index of this machine's 3.42 GB takes ≤ 10 s; a refresh with nothing new takes
   ≤ 100 ms; a 30-day report query takes ≤ 200 ms (release build, Apple silicon).
4. `statusline` stays within core §1.1's 10 ms p95 with its session sighting (§6.3).
5. No Claude Code file is ever written (core §3, pinned by core §15.3's byte-identity test).
6. The TUI renders the engine view of §10 with no shape of its own; the CLI's `--json` is the
   same view.

### 1.2 Non-goals

- Grouping or filtering by project or git branch. The project is stored (§4) for a later
  version; the branch is not.
- Reconciling with Claude Code's `cost-state` lines (§2.5).
- Any network fetch, of prices or of usage. Prices ship with the binary (§8).
- Transcripts of providers other than Claude Code. The capability is provider-neutral (§3.2);
  only Claude Code implements it.
- Notifications on spend. The daemon's spec may build them on this view.

---

## 2. Claude Code facts

### 2.1 Where usage lives

- Transcripts are `<home>/projects/<project>/<sessionId>.jsonl`. `projects` is must-share
  (core §12.2), so the default home and every `run` profile write into the default home's tree.
- Subagent transcripts are `<home>/projects/<project>/<sessionId>/subagents/agent-<16 hex>.jsonl`,
  each beside an `agent-<id>.meta.json`. The layout is flat: no deeper nesting, no other
  extension. Every subagent line has `isSidechain: true` and the **parent's** `sessionId`.
- Only `type: "assistant"` lines carry usage, in `message.usage`. Each also carries
  `requestId`, `sessionId`, `timestamp` (ISO 8601 UTC with milliseconds and `Z`), `cwd`,
  `isSidechain`, and `message.id`, `message.model` and `message.stop_reason`.
- `message.usage` has `input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
  `cache_creation.ephemeral_5m_input_tokens`, `cache_creation.ephemeral_1h_input_tokens`,
  `cache_read_input_tokens`, `server_tool_use.web_search_requests`, `speed` and
  `iterations[]`. `output_tokens_details.thinking_tokens` is part of `output_tokens`.
- No line records the account, the home or the credential.

### 2.2 One response, many lines

One API response is written as one line per content block, with the same `message.id` and
`requestId`, up to 9 lines. The lines are streaming snapshots:

- Input and cache counts are identical on every line of a response.
- `output_tokens` grows. A line with `stop_reason: null` holds a stale snapshot; only a line
  with a non-null `stop_reason` holds the final count, and `iterations`, `speed` and
  `server_tool_use` appear only there. In every group whose lines differ (11%), the last line
  has the highest output and the first never does.
- `iterations[]` has at most one entry, equal to the top-level counts. In one observed pair the
  top level was all zero and the iteration held the counts.
- The same response also appears in other files: a resumed or forked session copies history
  into its new file, and a subagent with `isFork: true` copies its parent's (1,714 groups across
  43 files). Deduplicating within a file overcounts output by 3.1%.
- A line with a `message.id` and no `requestId` exists only for `model: "<synthetic>"`, which
  is not an API call and has zero usage.

**The rule.** A response is identified by `(message.id, requestId)` across all files. Each of
its counts is the **maximum** over its lines; its time is the **earliest** line's. When a
line's top-level counts are all zero and `iterations[0]` is not, the iteration's counts are
used. `<synthetic>` lines are skipped. The maximum makes the result independent of the order
in which lines and files are read, so an incremental or concurrent index converges. A copy of
a response in another file keeps the session and sidechain flag of the batch that committed it
first, and a session's project is the first one known (§7.4).

### 2.3 Subagent output is a lower bound

63% of responses never get a line with a `stop_reason`; all but 3 of them are in subagent
transcripts. Their input and cache counts are correct, but their `output_tokens` is a stale
snapshot (median 8, against 333 for subagent responses that do get a final line). Over this
machine's tree that understates output by roughly 25M tokens, against 45.8M counted. No other
record of the true value was found. The index flags each response without a final line, and the
report says how many it counted (§10).

### 2.4 Session records

`<home>/sessions/<pid>.json` holds `sessionId`, `cwd`, `startedAt` (epoch ms), `procStart` and
more (core Appendix A.7). It is written at session start and removed on a graceful exit, and a
session CC spawns itself writes none. `sessions` is private to each `run` profile, so a record's
directory names its home, and a profile's home names its account (core §12.2).

### 2.5 What the transcripts leave out

Some sessions carry `cost-state` lines with a running `totalCostUSD` and per-model counts. They
include calls that no transcript records (a model with no transcript line at all), so they do
not reconcile with the transcripts. This report counts transcripts only and says so.

### 2.6 Observed values

- Models: `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-sonnet-5`, `claude-opus-5`,
  `claude-fable-5-1`, `claude-haiku-4-5-20251001`, and `<synthetic>`. `requestedModel` can carry
  a suffix (`claude-opus-5-5[1m]`); `message.model` never does and is the one used.
- `speed`: `standard` only so far. `service_tier`: `standard`. `server_tool_use`: web search
  and web fetch requests, all zero so far.
- Cache writes split into 5-minute (550M) and 1-hour (185M) tokens; the split sums to
  `cache_creation_input_tokens`.

---

## 3. Architecture

### 3.1 Crates (core §4.1)

| Crate | Adds |
|---|---|
| `tagteam-core` | `tokens`: `TokenCounts`, the merge rule of §2.2, rate cards and cost in integer nano-dollars (§8), the default home's timeline (§7.3), blocks (§9), day and month bucketing. Pure, no I/O. |
| `tagteam-provider` | The `transcripts` capability, the neutral `UsageLine`, the trait methods of §3.2, and `SessionRecord.session_id`. |
| `tagteam-cc` | The transcript parser, file discovery, and the shipped price table. |
| `tagteam-engine` | `tokens`: the index (§5), sightings (§6), attribution (§7) and the view (§10). Migration step 3 (§4). |
| `tagteam` | The `tokens` command (§11); `run`'s launch record and session poll, and `statusline`'s sighting (§6). |

`jiff` joins the workspace for time zones: day and month boundaries need the local zone's
offsets, DST included, and tagteam has no zone support today. It reads the system zone
database and adds no runtime dependency.

### 3.2 Provider trait (core §4.5)

`Capabilities` gains `transcripts: bool`. A provider with it implements:

```rust
/// Every transcript file under the default home, in no particular order.
fn transcript_files(&self, env: &Env) -> io::Result<Vec<PathBuf>>;
/// One line of a transcript, or None for a line that carries no usage.
fn parse_usage_line(&self, line: &[u8]) -> Result<Option<UsageLine>, LineError>;
/// The shipped price table and the date its prices were taken.
fn price_table(&self) -> &'static PriceTable;
/// The default home's `sessions/`, whose records the scan and the statusline read.
fn default_session_records_dir(&self, env: &Env) -> Option<PathBuf>;

pub struct UsageLine {
    pub message_id: String,
    pub request_id: String,
    pub session_id: String,
    pub at_ms: i64,
    pub model: String,
    pub project: Option<String>, // the line's working directory
    pub sidechain: bool,
    pub counts: TokenCounts,
    pub speed: Option<String>,
    pub final_line: bool,        // the line carries a stop_reason
}
```

`parse_usage_line` returns `Ok(None)` for a line of another type, a `<synthetic>` line, or one
without usage, and `Err` for an assistant line it cannot read (§5.4). The Claude Code parser
tests a byte prefilter for `"type":"assistant"` before parsing, and deserializes only the
fields above. `SessionRecord` (the liveness reader, core §12.6) gains `session_id: Option<String>`.
Session records are found through the existing `session_records_dir`.

---

## 4. Store: migration step 3

Step 3 of `MIGRATIONS` (core §6.1) adds nine tables and two triggers. The branch that merges second renumbers
its step when it rebases.

```sql
CREATE TABLE token_files (
  path       TEXT PRIMARY KEY,   -- absolute path as discovered
  provider   TEXT NOT NULL,
  dev        INTEGER NOT NULL,
  ino        INTEGER NOT NULL,
  size       INTEGER NOT NULL,   -- the file's size and mtime when last read
  mtime_ns   INTEGER NOT NULL,
  offset     INTEGER NOT NULL,   -- bytes indexed, always just past a newline
  head_hash  BLOB NOT NULL,      -- SHA-256, first 8 bytes, of the first min(4096, offset) bytes
  tail_hash  BLOB NOT NULL,      -- the same, of the up to 4096 bytes ending at offset
  indexed_at INTEGER NOT NULL    -- epoch ms
);

CREATE TABLE token_sessions (
  id         INTEGER PRIMARY KEY,
  provider   TEXT NOT NULL,
  session_id TEXT NOT NULL,
  project    TEXT,               -- the first indexed line's working directory
  UNIQUE (provider, session_id)
);

CREATE TABLE token_models (       -- each model name once (ruling R8)
  id         INTEGER PRIMARY KEY,
  provider   TEXT NOT NULL,
  name       TEXT NOT NULL,
  UNIQUE (provider, name)
);

CREATE TABLE token_usage (
  id         INTEGER PRIMARY KEY, -- insertion order: a time range's rows share pages (ruling R7)
  key        INTEGER NOT NULL UNIQUE, -- 8 bytes of SHA-256(provider NUL message.id NUL requestId)
  session    INTEGER NOT NULL REFERENCES token_sessions(id) ON DELETE CASCADE,
  at         INTEGER NOT NULL,    -- epoch ms of the earliest line
  model      INTEGER NOT NULL REFERENCES token_models(id),
  sidechain  INTEGER NOT NULL,
  input      INTEGER NOT NULL,
  output     INTEGER NOT NULL,
  cw_5m      INTEGER NOT NULL,
  cw_1h      INTEGER NOT NULL,
  cache_read INTEGER NOT NULL,
  web_search INTEGER NOT NULL,
  speed      TEXT,
  finalized  INTEGER NOT NULL     -- 1 once any line of the response had a stop_reason
);
CREATE INDEX token_usage_by_at ON token_usage (at);
CREATE INDEX token_usage_by_session ON token_usage (session, at);

CREATE TABLE session_sightings (
  provider    TEXT NOT NULL,
  session_id  TEXT NOT NULL,
  home        TEXT NOT NULL,     -- '' for the default home, else the profile's account id
  first_at    INTEGER NOT NULL,  -- epoch ms: the record's startedAt, or the first sighting (§6.3)
  last_at     INTEGER NOT NULL,  -- epoch ms: the last time a source saw it there, alive
  source      TEXT NOT NULL,     -- 'run' | 'scan' | 'statusline': the source that created it
  recorded_at INTEGER NOT NULL,  -- epoch ms of the last write
  PRIMARY KEY (provider, session_id, home, first_at)
) WITHOUT ROWID;

CREATE TABLE run_launches (
  provider    TEXT NOT NULL,
  account_id  TEXT NOT NULL,     -- the profile's account; not a foreign key
  started_at  INTEGER NOT NULL,  -- epoch ms; 0 when a seeded profile's birth time is unknown
  ended_at    INTEGER,           -- NULL while the launch may still run
  pid         INTEGER,           -- the child's, once spawned; NULL when seeded (§4 Seeding)
  recorded_at INTEGER NOT NULL,
  PRIMARY KEY (provider, account_id, started_at)
) WITHOUT ROWID;

CREATE TABLE active_history (    -- every change of a provider's active account, from the epoch on
  provider    TEXT NOT NULL,
  at          INTEGER NOT NULL,  -- epoch ms
  account_id  TEXT,              -- NULL: no active account; not a foreign key
  recorded_at INTEGER NOT NULL
);
CREATE INDEX active_history_by_at ON active_history (provider, at);
CREATE TRIGGER active_history_insert AFTER INSERT ON active_accounts BEGIN
  INSERT INTO active_history VALUES (NEW.provider, <now ms>, NEW.account_id, <now ms>);
END;
CREATE TRIGGER active_history_update AFTER UPDATE OF account_id ON active_accounts
  WHEN NEW.account_id IS NOT OLD.account_id BEGIN
  INSERT INTO active_history VALUES (NEW.provider, <now ms>, NEW.account_id, <now ms>);
END;

CREATE TABLE token_state (       -- created by a provider's first refresh write, if absent
  provider          TEXT PRIMARY KEY,
  refreshed_at      INTEGER,         -- the last completed refresh; NULL before one completes
  changed_at        INTEGER,         -- the last batch or prune that added, updated or deleted
                                     -- a response; NULL before one did
  files             INTEGER NOT NULL DEFAULT 0, -- as of the last completed refresh; 0 before
  responses         INTEGER NOT NULL DEFAULT 0,
  skipped_lines     INTEGER NOT NULL DEFAULT 0, -- in the last completed refresh
  unreadable_files  INTEGER NOT NULL DEFAULT 0,
  pruned_at         INTEGER
);

CREATE TABLE token_epoch (       -- one row: when this store first ran a version that records
  at     INTEGER NOT NULL,       -- sightings and launches (§7.2), epoch ms
  seeded INTEGER NOT NULL        -- 1: the migration listed every profile (ruling R1)
);
INSERT INTO token_epoch VALUES (<now ms>, <seeded>);
INSERT INTO active_history SELECT provider, <now ms>, account_id, <now ms> FROM active_accounts;
```

`<now ms>` is `CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)`.

- **Upsert.** A response is written with `ON CONFLICT(key) DO UPDATE SET`
  `at = min(at, excluded.at)`, the maximum for every count and `finalized`
  (`output = max(output, excluded.output)`, …), and `speed = coalesce(excluded.speed, speed)`.
- **No account column.** Attribution is resolved at query time (§7), so evidence recorded
  after a response was indexed applies to it. Neither `home` nor `run_launches.account_id` is a
  foreign key: removing an account keeps its usage and the evidence that places it, shown as a
  removed account (§10).
- **Seeding.** Step 3 runs in the ladder's one transaction (core §6.1), so it lands with the
  version or not at all. Besides its SQL, it records each profile directory under the data
  directory's `sessions/` (core §12.2) as a `run_launches` row:
  - the directory's name as the account, and a NULL `pid`;
  - its birth time as `started_at`, or 0 when the filesystem reports none;
  - the epoch as `ended_at`, unless the profile is in use (core §12.6). Then `ended_at` is
    NULL, and §6.1 closes it. That covers a `run` of the previous version still running.

  `token_epoch.seeded` records whether the migration could list every profile to seed. When it
  could not (a `sessions/` it cannot read, or a raw `Store::open`, which lists none), coverage
  before the epoch (§7.2) is never complete, which can only leave a response unattributed (plan
  ruling R1).
- **Size.** About 100 bytes per response, indexes included: about 150 MB a year at this
  machine's rate (127k responses in 30 days). The plan may normalize `model` into a lookup
  table if that is needed to stay within it. It does (`token_models`, plan ruling R8), and keys
  rows by insertion with the hash `UNIQUE` beside it (ruling R7): about 109 bytes a response,
  and a 30-day range read of a year's rows in 16 ms rather than up to 1.4 s.
- **Retention.** `tokens.retention_days` (default 365, valid 30–3650, core §6.4) prunes, at
  most once a day on the write path (`token_state.pruned_at`) as core §6.1 prunes samples:
  `token_usage` rows older than the window; then sessions with no rows and their sightings;
  then launches that ended before the window. A line older than the window is not inserted.
  A prune that deletes a row advances `token_state.changed_at`.

---

## 5. Index refresh

```rust
impl Engine {
    pub fn refresh_tokens(
        &self,
        provider: &ProviderId,
        wait: Wait,
        budget: Option<Duration>,
        cancel: &Cancel,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<RefreshOutcome, EngineError>;
}

/// `Wait` for the lock (cancellably), or `Try` it and return `Busy` when another refresh holds it.
pub enum Wait { Wait, Try }

pub enum RefreshOutcome { Done(IndexReport), Busy }

pub struct IndexProgress { pub files_done: u64, pub files_total: u64, pub bytes_done: u64, pub bytes_total: u64 }

pub struct IndexReport {
    pub complete: bool,          // false: the budget ran out first; call again to go on
    pub files_read: u64,
    pub responses_added: u64,
    pub responses_updated: u64,
    pub skipped_lines: u64,
    pub evidence_changed: u64,   // evidence rows its sight_sessions added, extended or closed
    pub took_ms: u64,
}
```

### 5.1 The lock

A refresh holds `locks/tokens-<provider>.lock`, an `flock`, so any number of processes may call
it at once and only one reads at a time. With `Wait::Wait` a second refresh waits for the first,
cancellably, then finds little left to do; `tagteam tokens` waits. With `Wait::Try` it returns
`Busy` at once; the TUI's timer and the daemon try. The lock is a leaf (core §4.3): it may be
taken under any other tagteam lock, and nothing is taken while it is held. The one exception
is a full `purge`, below: it holds every affected provider's tokens lock at once, taken in
sorted order. That cannot deadlock, because a refresh takes only one tokens lock (plan
"Rulings (planning)", "Lock waits"). Store transactions are not locks in that order.

`purge` (core §10.5) takes it too, waiting, under its mutation lock and for each provider whose
rows it deletes, so a purge never falls between two batches of a refresh. Sightings and launches
(§6) are written without it: one recorded after a purge describes a session that is still
running, and is kept.

### 5.2 Steps

1. Run `sight_sessions` (§6).
2. List the transcript files and `stat` each. Discovery is not budgeted. It walks two fixed
   levels (`projects/<project>/` and `projects/<project>/<session>/subagents/`): 26 ms for
   2,521 files on this machine. A file whose `(dev, ino)`, size and mtime match those stored
   is skipped. Only a read that consumed the file to its last newline stores its size and
   mtime (step 4), so a file left part-way is never skipped. Any other file is opened, and the open file's own
   `fstat` decides where reading starts:
   - from its stored `offset`, when `(dev, ino)` matches, its size is at least the offset, and
     `head_hash` and `tail_hash` still match the bytes they cover;
   - from 0 otherwise: a new file, another inode, a shorter file, or a changed prefix (a file
     truncated and rewritten in place, or a reused inode). Re-reading is safe: the upsert takes
     maxima.

   A stored file that no longer exists has its `token_files` row removed and its usage kept.
3. Read each file from its start offset in chunks of at most 1 MiB.
   - A line is assembled across chunks until its newline, so a line longer than a chunk still
     advances. A line over 64 MiB is skipped and counted (§5.4); its end is found by scanning.
   - A trailing partial line at the end of the file is left for the next refresh.
   - The prefilter (§3.2) runs on the assembled line, so a long line of another type is
     scanned, never parsed.
4. Parse on scoped threads (core §4.4), at most four, each working through one file at a time,
   in order, holding at most one chunk. One writer commits batches of at most 2,000 responses,
   each transaction holding the write lock for at most about 50 ms.
   - **Offsets advance with every batch.** For each file with lines in a batch, the batch
     commits that file's offset, just past the last whole line it consumed, with its new
     `head_hash` and `tail_hash`. So a file larger than one batch, or one slice, makes progress
     on every call. A chunk that yields no response still advances its file's offset in the
     next batch. The batch that consumes a file to its last newline also stores the size and
     mtime it read. Any earlier batch stores size −1, which keeps the file from being skipped
     (step 2).
   - **Each offset is validated before it commits.** The open file must still have the same
     `(dev, ino)`, a size at least the new offset, and unchanged bytes under the stored
     `head_hash` and under the `tail_hash` that ends where this read started. If any check
     fails, the file was rewritten during the read. Its rows still commit, because every line
     is a whole response and the upsert takes maxima. Its offset is reset to 0 and its stored
     size to −1, which no file matches, so the next refresh reads it again from the start.
   - A batch that adds or updates a response sets `token_state.changed_at` in the same
     transaction.
   - A cancelled refresh stops at the next chunk, commits what it parsed, and loses nothing.
5. On completion, set `token_state.refreshed_at` and the last refresh's statistics. Set
   `token_epoch` to now if it has no row (after a full purge). Prune if due. The first refresh
   to complete also sets `changed_at`, because it ends the index's partial state (§14).

**Budget.** A refresh with a `budget` checks its deadline after each chunk and between
lines. Past it, the workers stop at the next line boundary, the writer commits what was parsed
(with every file's offset as above), and the refresh returns `complete: false`. A slice
therefore overruns its budget by at most one chunk, or one line still being assembled, per
worker, plus one batch. Only a complete refresh sets `refreshed_at`. The next call goes on where this one
stopped. The daemon's scheduler runs one job at a time, so it refreshes in slices of about one
second; a cold index never holds up a provider job for much longer than that.

A refresh with nothing new costs one `stat` per file and one store read.

### 5.3 Who refreshes

`tagteam tokens` (unless `--no-refresh`), the TUI on its timer when no daemon runs, and the
daemon (§12). `statusline`, `list`, `status` and every other command never refresh.

### 5.4 Unreadable lines

An assistant line that does not parse, or lacks a field the parser needs, is skipped and counted
in `skipped_lines`. A refresh never fails on a line. A file that cannot be opened or read is
skipped with a warning and retried on the next refresh. A non-zero skip count is shown with the
report (§11.3), as the first sign of transcript format drift (core §17 R2).

---

## 6. Session evidence

Two kinds of evidence place a session:
- A **sighting** says a session was seen alive in a home from `first_at` to `last_at`.
- A **launch** says `tagteam run` may have had a profile session running for an account over
  an interval.

Each source writes its own rows; writes are idempotent.

```rust
impl Engine {
    /// Scans every home's session records, records what it finds, and closes launches whose
    /// profile is no longer in use. Returns how many rows it added, extended or closed.
    pub fn sight_sessions(&self, provider: &ProviderId) -> Result<u64, EngineError>;
}
```

### 6.1 `run`

- **Launch.** Before it spawns its child, `tagteam run` plans a launch for the account, with
  `started_at` stamped then, and records it open (`ended_at` NULL); once spawned, it adds the
  child's pid. A record that fails is tried again on each poll while the child runs, and once
  more at the close; the launch keeps its planned start, so a late record covers the whole
  session. After exit handling, which releases the reservation `run` itself holds, `run`
  closes the launch if the profile is no longer in use (core §12.6). A background session the
  child started may outlive it; then, as when `run` was killed, the next `sight_sessions` that
  finds the profile no longer in use closes the launch, at that moment. A late close
  over-covers, which can only leave a response unattributed, never misplace it.
- **Poll.** While its child runs, `run` reads the child's record,
  `<profile>/sessions/<child pid>.json`, every 2 seconds on a scoped thread, when its mtime
  changed. For the `sessionId` it sees, it writes the sighting keyed by the record's
  `startedAt` and sets its `last_at` to now (source `run`). When the child exits, it sets
  `last_at` of the sighting it saw last to the exit time.
- A `run` that execs plain `claude` (core §12.1) records neither: §7.2 places that session.

### 6.2 Scan

Every refresh, and the daemon on its own timer (§12), reads every `*.json` in the default
home's `sessions/` and in each profile's (source `scan`). For a record whose process is alive
(core §12.6), the scan writes the sighting `(sessionId, home, startedAt)` and sets `last_at` to
now. An unreadable or partly written record, or one whose process is gone, is skipped.

### 6.3 Statusline

CC runs the statusline command with the session's environment (core Appendix A.7), so
`tagteam statusline` reads `CLAUDE_CODE_SESSION_ID` and `CLAUDE_CONFIG_DIR` from its own
environment. Stdin stays drained and ignored (core §13.5).

- **The home** is the profile named by the marker in a run shell (core §12.8). Otherwise it is
  the default home, when `CLAUDE_CONFIG_DIR` is unset or names it. Otherwise nothing is
  recorded.
- **One indexed lookup** reads the session's latest sighting in that home. When its `last_at`
  is within the last minute, there is nothing to do.
- **Otherwise it finds its execution.** It walks its ancestor processes (at most 8) to the
  first that has a record in the home's `sessions/`: the Claude Code process it runs under. It
  then writes the sighting keyed by that record's `startedAt`, as `run` and the scan do, with
  `last_at` now (source `statusline`). With no such ancestor it records nothing.

  A sighting is only ever extended by the execution it belongs to, so a gap between two
  executions of a session is never bridged.
- **The write** waits at most the 5 ms the live-identity cache already allows
  (`CACHE_WRITE_WAIT`). A busy store skips it; the line is never held up for it.

### 6.4 Writes

- A sighting is one execution of a session in a home, keyed by the process record's
  `startedAt`. Only that execution's observations extend it, at most once a minute per source,
  except `run`'s exit write.
- Every write sets `recorded_at`, which moves `IndexState.changed_at_ms` (§10).

---

## 7. Attribution

Each response is attributed at query time, to an account or to **unattributed**, and the totals
report on what basis (§10): `profile`, `timeline` or `unattributed`. A response is placed only
when the evidence allows exactly one home. Anything else is unattributed; it is never guessed.

### 7.1 Sighted responses

A response at time `t` in session `s` is **sighted** in each home where a sighting of `s` has
`first_at ≤ t ≤ last_at`.
- Sighted in exactly one home: that is its home.
- Sighted in two homes (the session open in both at once): unattributed.

### 7.2 Unsighted responses

A response sighted nowhere is placed by what could have been running. Its candidate homes are:
- the default home;
- the profile of every launch covering `t`;
- any profile, while launch coverage at `t` is **incomplete**.

With the default home as the only candidate, it is the default home's; otherwise it is
unattributed.

**Coverage.**
- From the epoch (`token_epoch`, when the store migrated to step 3) on, coverage is complete:
  `run` records each launch from a start planned before its child starts (§6.1).
- Before the epoch, coverage comes from the seeded launches (§4). It is complete only after
  the provider's last `remove` event before the epoch. An account removed before then may have
  had a profile that left no trace.

### 7.3 The account of a home

- A profile's home is its account, always.
- The default home's account at `t` is the provider's active account at `t`. Evidence for it
  survives `remove`, so a removed account keeps its history:
  - From the epoch on, it comes from `active_history`, which the triggers fill on every change
    and the migration seeded with the account active at the epoch (§4).
  - Before the epoch, it comes from `events` (core §6.1):
    - From the first `switch` on, the `to_id` of the latest switch at or before `t`.
    - Before it, that first switch's `from_id`, from that account's earliest `add` event on.
    - With no switch, the account `active_history` was seeded with, from its earliest `add`
      event on.
  - A `remove` event of the account ends its segment.
  - Outside these segments, or for a NULL account, the response is unattributed.

### 7.4 Assumptions and known limits

The rules above place a response only where the evidence allows one home, under two
assumptions:

- **A1.** A session runs in one execution at a time. Two executions of one session in
  different homes are detected when both are sighted (§7.1). If one is never sighted, its
  responses during the other's sighting are credited to the sighted home.
- **A2.** The default home's login changes only through tagteam. A login changed outside
  tagteam (CC's own `/login`) is credited to the account tagteam had active, until tagteam
  records the change.

Known limits:

- A response copied into another transcript file (a resume or fork copy, §2.2) keeps the
  session and sidechain flag of whichever batch committed it first, because the upsert never
  changes them, and its session's project is the first one known. The counts converge whatever the read order. Session rows can show
  such a response under either session. It is never attributed to an account it could not have
  used: at worst it is unattributed. In the real tree 0.3% of responses have a copy under
  another session id.
- A session CC spawns itself inside a profile writes no record. Its responses fall inside its
  parent's launch, so they are unattributed rather than misplaced.
- `/clear` starts a new session in the same execution (§17 T1). CC rewrites the same record
  under the same `startedAt` with the new `sessionId`, so `run`'s poll sights the new session
  from the record's `startedAt`, before it existed, which covers none of its responses.
- Unattributed usage is larger where evidence is thin: before the epoch while profiles
  existed, and for default-home sessions running alongside a `run` session with neither the
  daemon nor the statusline sighting them.

---

## 8. Cost

### 8.1 Rate cards

Cost is API list price: what the tokens would cost on the API. For a subscription account it is
an equivalent, not a charge; the report calls it "API cost".

```rust
pub struct RateCard {          // USD per million tokens
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

pub struct ModelPrice {
    pub base: RateCard,
    /// A second card for a response whose prompt (input + cache writes + cache read) exceeds
    /// the threshold (Claude Haiku 5.5: above 100K tokens).
    pub long_prompt: Option<(u64, RateCard)>,
    /// The card for `speed: "fast"` responses (Claude Opus 5.5: $8 / $40).
    pub fast: Option<RateCard>,
    pub web_search_per_1k: Option<f64>,
}

pub struct PriceTable { pub as_of: &'static str, pub models: &'static [(&'static str, ModelPrice)] }
```

- The shipped table lives in `tagteam-cc`. It covers every model in §2.6 and every current
  model on Anthropic's pricing page, with the page's rates on the day of the release that ships
  it (`as_of`).
- A model name matches exactly, then with a trailing `-YYYYMMDD` removed.
- A `fast` response whose model has no `fast` card is unpriced. A fast response uses its fast
  card even above the long-prompt threshold; no model has both today.

### 8.2 Overrides

```toml
[provider.claude-code.prices."claude-opus-5-5"]
input = 4.0
output = 20.0
cache_write_5m = 5.0
cache_write_1h = 8.0
cache_read = 0.20
```

- A table may set any field of `base` and `web_search_per_1k`, plus `long_prompt_above`,
  `long_prompt.<field>` and `fast.<field>`.
- A partial table overrides the shipped entry field by field. A complete `base` adds a model
  the shipped table lacks.
- The tables are read forgivingly, like every setting (core §6.4): an invalid value keeps the
  shipped one, with a warning. They are not registry keys, so `tagteam config set` does not
  write them; the user edits the file. The report names the overridden models
  (`price_overrides`).

### 8.3 Computing cost

Cost is computed per response at query time and never stored, so overrides and new tables
apply to the past.

- Each rate converts to nano-dollars per token (USD per MTok × 1000, rounded), and sums are
  integers.
- A response whose model has no price contributes its tokens and no cost.
- A row whose responses are all unpriced has no cost ("—"). A row with some unpriced
  responses has a cost marked partial.

---

## 9. Blocks

A block is a 5-hour window of one account's usage. Unattributed usage gets its own series.

Blocks never overlap, so each response belongs to exactly one: the block whose
`[start, end)` contains its time.

- **Observed blocks.** Each distinct `resets_at` among an account's samples of the provider's
  short window (`WindowKind::Short`, CC's `5h`) gives the block `[resets_at − 5h, resets_at)`.
  - Reset times within 10 minutes of each other count as one, the latest. The API's reset time
    moves by seconds between readings.
  - In time order, an observed block starts no earlier than the previous one ends; its start
    moves up to that end.
- **Inferred blocks** cover usage outside observed blocks, by ccusage's rule:
  - A block starts at the hour floor of the first uncovered response, or at the end of the
    observed block before it if that is later, and ends 5 hours after the hour floor.
  - It ends early where the next observed block starts.
  - The first uncovered response at or after its end starts the next.
- **The active block** is the one containing now. It also reports:
  - `elapsed_s` and `remaining_s`;
  - the burn rate: total tokens per minute and cost per hour, over the time from the block's
    first response to now;
  - the projected tokens and cost at the block's end: current plus rate × remaining.

---

## 10. The engine view

This section is the contract with the TUI: it renders exactly these types, and `--json` (§11.2)
serializes them.

```rust
impl Engine {
    /// Reads only: it never refreshes the index (§5).
    pub fn token_report(&self, q: &TokenQuery) -> Result<TokenReport, EngineError>;
    /// The `index` part of a report alone, from `token_state` and the evidence tables: a few
    /// small reads, for deciding whether to re-query.
    pub fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, EngineError>;
}

pub enum TokenPeriod { Day, Month, Session, Block }

pub enum TokenGrouping { None, Account, Model, AccountModel }

pub enum AccountSel { Account(AccountId), Unattributed }

pub struct TokenQuery {
    pub provider: ProviderId,
    pub period: TokenPeriod,
    pub grouping: TokenGrouping,
    pub since_ms: Option<i64>,      // inclusive
    pub until_ms: Option<i64>,      // exclusive
    pub accounts: Option<Vec<AccountSel>>, // None: every account and unattributed
    pub models: Option<Vec<String>>,       // None: every model; exact names otherwise
    pub session: Option<String>,    // only this session's responses
    pub tz: jiff::tz::TimeZone,     // day and month boundaries; the CLI's default is the system zone
}

pub struct TokenReport {
    pub query: TokenQuery,
    pub time_zone: String,          // the IANA name of `tz`, or "UTC"
    pub rows: Vec<TokenRow>,
    pub total: TokenTotal,
    pub index: IndexState,
    pub unpriced_models: Vec<String>,
    pub prices_as_of: &'static str,
    pub price_overrides: Vec<String>, // models whose price config changed
}

pub struct TokenRow {
    pub key: RowKey,
    pub period: PeriodKey,
    pub account: Option<RowAccount>, // Some when grouped by account
    pub model: Option<String>,       // Some when grouped by model
    pub accounts: Vec<RowAccount>,   // every account present in the row
    pub models: Vec<String>,         // every model present in the row
    pub counts: TokenCounts,
    pub responses: u64,
    pub unfinalized: u64,            // responses whose output is a lower bound (§2.3)
    pub cost: Option<Cost>,
}

pub enum PeriodKey {
    Day { date: jiff::civil::Date },
    Month { year: i16, month: i8 },
    Session { session_id: String, project: Option<String>, first_ms: i64, last_ms: i64 },
    Block { start_ms: i64, end_ms: i64, observed: bool, active: Option<ActiveBlock> },
}

pub struct ActiveBlock {
    pub elapsed_s: i64,
    pub remaining_s: i64,
    pub tokens_per_min: f64,
    pub cost_per_hour: Option<Cost>,
    pub projected_tokens: u64,
    pub projected_cost: Option<Cost>,
}

pub enum RowAccount {
    Account { id: AccountId, position: u32, label: String, alias: Option<String> },
    Removed { id: AccountId },
    Unattributed,
}

/// Stable across refreshes: the TUI keeps a selection by it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RowKey(String);       // e.g. "day:2026-10-14|account:<id>|model:claude-opus-5-5"

pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub web_search: u64,         // requests, not tokens
}
// cache_write() = 5m + 1h; total() = input + output + cache_write() + cache_read

pub struct Cost { pub usd_nanos: u64, pub partial: bool }

pub struct TokenTotal {
    pub counts: TokenCounts,
    pub responses: u64,
    pub unfinalized: u64,
    pub cost: Option<Cost>,
    pub by_basis: [(Basis, u64); 3], // total tokens by basis (§7)
}

pub enum Basis { Profile, Timeline, Unattributed }

pub struct IndexState {
    pub refreshed_at_ms: Option<i64>, // the last completed refresh; None: the index is partial
    pub changed_at_ms: Option<i64>,   // the latest of token_state.changed_at and every evidence
                                      // row's recorded_at (§6.4); it moves when a report may
    pub files: u64,                   // as of the last completed refresh
    pub responses: u64,
    pub skipped_lines: u64,           // in the last completed refresh
    pub unreadable_files: u64,
    pub oldest_ms: Option<i64>,
    pub newest_ms: Option<i64>,
}

impl TokenQuery {
    /// The query one level down from `row`, or None at a session's per-model rows.
    pub fn drill(&self, row: &TokenRow) -> Option<TokenQuery>;
}
```

### 10.1 Rules

- **Order.** Rows are newest first. Within a period, accounts come by position, then removed
  accounts, then unattributed. Models come by name.
- **Periods.**
  - A day or month is a calendar day or month in `tz`.
  - A response belongs to the period containing its time (§2.2).
  - A session row spans its responses inside the query's range.
- **Blocks are per account.** For `Block`, `None` groups as `Account` and `Model` as
  `AccountModel`.
- **Sessions can span accounts.** A session whose responses went to two accounts gives one
  row per account when grouped by account, and one row listing both otherwise.
- **Filters** apply before grouping. `Some` of an empty list matches nothing. An account
  filter naming a removed account's id matches its rows.
- **Drill-down.**
  - From a day, month or block row: `period: Session`, the row's span as `[since, until)`,
    the row's account and model as filters when grouped by them, and the same grouping.
  - From a session row: `session: Some(id)`, `grouping: AccountModel`, and no range.
- **Threads.** `refresh_tokens` and `token_report` take `&self` and do their own store I/O.
  Several processes may refresh at once (§5.1); the TUI tries, and shows `Busy` as catching up
  elsewhere.
  The TUI calls both on a worker thread, with an engine of its own if `Engine` is not `Sync`.
  A query on a current index needs no refresh. `IndexState.refreshed_at_ms` says how current the
  index is, and a `changed_at_ms` that moved says a report may have changed. It follows the
  index and the evidence, and does not move when an account's alias or position changes, or
  when an account other than the active one is removed. Those change row labels and kinds, so a
  consumer that caches rows also re-queries on account changes.

---

## 11. CLI: `tagteam tokens`

```
tagteam tokens [day|month|session|block] [ACCOUNT...] [--unattributed] [--model M]...
               [--since 30d|DATE] [--until DATE] [--by account|model|account,model|none]
               [--tz ZONE] [--csv] [--no-refresh]
```

### 11.1 Arguments

- **The period** is a subcommand, so an alias spelled like one cannot be mistaken for it. When
  no period subcommand matched (for example `tagteam tokens --json month`, where a flag comes
  first), a period word in the first `ACCOUNT` position is the period, and only the first
  position is read so. An alias spelled like a period is named after a period:
  `tagteam tokens day month` (execution ruling, made in review). Bare `tagteam tokens` is
  `tagteam tokens day`.
- **`ACCOUNT`** resolves as core §10.4 does. `--unattributed` adds the unattributed bucket. With
  neither, every account and the unattributed bucket are shown.
- **`--model`** matches model names containing the text, case-insensitively, and is
  repeatable. It resolves to exact names in the index before the query. A `--model` that
  matches none is a usage error (exit 2) that lists the indexed models.
- **`--since` and `--until`** take `Nd`, `Nh` or `Nm` (as `history`), a date `YYYY-MM-DD`
  (midnight in `--tz`), or an ISO 8601 time. The default `--since` is 30d, and for `month` the
  first day of the eleventh month before the current one.
- **`--by`** defaults to `account`. `--tz` takes an IANA name or `UTC`, and defaults to the
  system zone.
- **`--no-refresh`** reports from the index as it is.
- The command refuses for a provider without the `transcripts` capability (core §4.5), with exit 1.
- It creates the store when there is none, as any writing command does.

### 11.2 Output

- **Human.** A table with one row per report row:
  - Columns: period, account (position and label), model when grouped by it, input, output,
    cache write, cache read, total and API cost.
  - Token counts are scaled (`12.4M`) and costs are dollars with cents.
  - A total line closes the table.
  - Footnotes cover unpriced models, unfinalized responses ("output is a lower bound for N
    subagent responses"), skipped lines, and the price table's date.
  - Session rows add the project and first and last times. Block rows add the start and end,
    `observed`, and for the active block the remaining time, burn rate and projection.
  - While a refresh runs longer than one second, progress goes to stderr.
- **`--json`.** One core §13.2 object, the report of §10 in camelCase: `{schemaVersion: 1,
  provider, period, grouping, timeZone, since, until, rows: [...], total, index, unpricedModels,
  prices: {asOf, overridden}}`.
  - Each row is `{key, period: {...}, account?, model?, accounts, models, tokens: {input,
    output, cacheWrite5m, cacheWrite1h, cacheWrite, cacheRead, total, webSearch}, responses,
    unfinalized, costUsd, costPartial}`.
  - `costUsd` is a number or null.
  - An account is `{kind: "account", number, id, email, alias?}`, `{kind: "removed", id}`,
    or `{kind: "unattributed"}`.
  - Times are ISO 8601 UTC; dates and months are in `timeZone`.
- **`--csv`.** One line per report row:
  - Columns: `period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd`.
  - `session` adds `project,first_at,last_at`. `block` adds `start,end,observed,active`.
  - The account is the email, `unattributed`, or `removed:<id>`. A field that is not grouped
    is empty.

### 11.3 Warnings

On stderr: skipped lines since the last refresh, files that could not be read, and invalid price
overrides.

---

## 12. Daemon contract

The daemon (sub-project 2) keeps the index current, so the TUI's queries are cheap:

- `sight_sessions(provider)` on a short timer, about every 5 s. It is a handful of small reads.
- `refresh_tokens(provider, Wait::Try, Some(1 s), cancel, progress)` on its tick, about every
  60 s, or after a filesystem change under `projects/`. An incomplete refresh is due again at
  once, after any other due job. `Busy` means another process is refreshing; the next tick
  tries again.
- Both take their own locks (§5.1) and need neither `auto`'s engine lock nor the mutation lock.
- After each call it reads `token_index_state(provider)`. When `changed_at_ms` moved since its
  last broadcast, it broadcasts the additive `token-index` event on its socket, so the TUI
  re-queries. This catches evidence that `run` or the statusline recorded, not only the
  daemon's own refreshes. The event is the `IndexState` in camelCase, times in ISO 8601 UTC:
  `{provider, refreshedAt, changedAt, files, responses, skippedLines, unreadableFiles, oldest,
  newest}`. This branch merges first, so the daemon's branch emits it. `EventSink` and
  `AutoEvent` are unchanged.

---

## 13. Privacy, export and purge

- **Content.** The index holds counts, model names, ids, times, session ids and working
  directories. It never holds message content. Response ids are kept only as a hash.
- **Logs** (core §14.2):
  - Counts, file counts and timings, at info and debug.
  - Transcript paths and session ids at debug only.
  - Never content, and never an email or label.
- **Export** carries none of it (core §13.3: nothing machine-local).
- **Purge.**
  - A full purge empties these tables with the rest (core §10.5).
  - `purge --provider` deletes that provider's rows from the seven provider-keyed tables;
    `token_epoch` stays.
  - Either holds each affected provider's tokens lock while it deletes (§5.1).
  - `remove` deletes none of them: the account's usage stays, as a removed account.

---

## 14. Errors

- An unreadable transcript is a warning, not an error (§5.4).
- A store error is `EngineError::Store`, with exit 1.
- A cancelled refresh exits 130, keeping what it committed.
- An invalid `--since`, `--until`, `--tz` or `--by` is a usage error, with exit 2.
- `token_report` reports whatever is indexed. Before the first complete refresh it is
  partial, and `IndexState.refreshed_at_ms` is `None`. The CLI's `--no-refresh` says so on
  stderr, and the TUI shows it as catching up.

---

## 15. Testing

- **Core** (pure, with proptest where order matters):
  - The merge rule: any order of a response's lines gives the same row.
  - Cost: cards, the long-prompt tier, fast, unpriced and partial, and nano-dollar sums.
  - The timeline's segments.
  - Blocks: observed resets, collapsing nearby resets, overlapping observed windows, an
    inferred block starting inside the hour an observed block ends in, inferred blocks clipped
    at the next observed one, each response in exactly one block, the active block's
    projection.
  - Day and month bucketing across a DST change in a fixed zone.
- **CC parser.** Synthetic fixtures with no real content:
  - stale and final snapshots, the `iterations` fallback, `<synthetic>`, a missing
    `requestId`;
  - subagent paths and a malformed line;
  - discovery of the flat subagent layout.
- **Engine.** Temp homes and the fake clock:
  - Refresh: append, a partial trailing line, truncation, truncation and regrowth past the
    old offset on the same inode, an inode swap, a deleted file, two concurrent refreshes
    (waiting and trying), cancellation mid-file, a purge waiting for a refresh.
  - Budgeted refresh: a file larger than one slice completes over successive calls and is
    never skipped part-way; a large file of lines without usage yields within one chunk of the
    deadline; a line longer than a chunk, and one over the 64 MiB cap; the first completion
    moves `changed_at`.
  - Evidence:
    - sightings from all three sources, extended at most once a minute;
    - the statusline finding its execution through its ancestors, recording nothing without
      one, and never extending another execution's sighting;
    - a launch recorded before the spawn and closed at exit;
    - a launch closed by `sight_sessions` after its `run` was killed while the child lived on;
    - seeding of idle and in-use profiles at migration;
    - `active_history` rows from switches, from `add`, and from `remove` setting NULL.
  - Attribution end to end:
    - a profile session, and the default home through `active_history` and through `events`;
    - an unsighted response with and without a covering launch, before and after the epoch,
      and before and after a pre-epoch `remove`;
    - a session sighted in two homes at once;
    - a session resumed in another home, with either home unsighted;
    - a removed account, with and without switches, and a removed profile.
  - The view's grouping, filters (including `Some(vec![])`), order and drill-down;
    `changed_at_ms` moving on new evidence alone.
  - Migration from a version-2 store, with profiles to seed.
  - `purge` and `remove`.
- **CLI.** Snapshot tests of the human, `--json` and `--csv` output (core §13.2). `tokens` joins
  core §15.3's byte-identity test.
- **Budgets.**
  - The statusline p95 test runs with a fresh sighting (a lookup alone) and with one due an
    extension (the lookup, the ancestor walk and a write).
  - A benchmark generates a 500 MB synthetic tree and checks the refresh and query budgets
    of §1.1, scaled.
- **Compatibility** (core §15.4). The CC compat suite gains a check that a real session's
  transcript yields usage lines with every field §3.2 reads, and that its session record
  carries `sessionId`.

---

## 16. Amendments to the core spec

Small hunks, in the same branch:

- **core §4.3:** `locks/tokens-<provider>.lock` is a leaf, except that a full purge holds every
  affected provider's at once (§5.1).
- **core §10.5:** `purge` takes each affected provider's tokens lock while it deletes.
- **core §4.5:** the `transcripts` capability and its four methods: `transcript_files`,
  `parse_usage_line`, `price_table` and `default_session_records_dir`.
- **core §6.1:** the tables and triggers of §4 and their retention.
- **core §6.4:** `tokens.retention_days`, and the `[provider.<id>.prices]` tables.
- **core §12.5:** `run` records its launch and polls its child's session record (§6.1).
- **core §13.1:** the `tokens` row, pointing here.
- **core §13.5:** the statusline records its session from its environment.
- **core Appendix C:** `tokens` as new.

---

## 17. Risks and open items

- **T1.** Resolved by the compat check `session-record-session-id` on CC 2.1.299: on
  `/clear` the record file stays, its `sessionId` changes within 10 s, and the next answer is
  written to a new transcript, the one the record then names (§7.4).
- **T2.** Subagent output is a lower bound (§2.3). A later Claude Code may write final lines
  for subagent responses; the merge rule then picks them up with no change.
- **T3.** Transcript format drift (core §17 R2): the skipped-lines count, a warning, and the compat
  check (§15).
- **T4.** Prices change. A release updates the table, and overrides cover the gap (§8.2).
