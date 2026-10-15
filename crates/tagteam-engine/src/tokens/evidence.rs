//! Session evidence (spec §6): sightings from the scan, from `run`'s poll of its child's record
//! and from the statusline, and `run`'s launches. Each source writes its own rows, and every
//! write is idempotent.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{
    ProfileMarker, Provider, Read, RecordEntry, parse_session_record, profile_path,
    read_session_records, record_is_live,
};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::store::{SightingRow, SightingSource, Store};
use crate::views::CACHE_WRITE_WAIT;

/// A sighting is extended at most once a minute (spec §6.4), and the statusline does nothing
/// while its session's latest sighting in its home is younger than that (spec §6.3).
const SIGHTING_GAP_MS: i64 = 60_000;
/// The most ancestors the statusline walks to find its Claude Code process (spec §6.3).
const MAX_ANCESTORS: usize = 8;

/// One launch `run` planned (spec §6.1), as `plan_launch` returns it. It is recorded once
/// `record_launch` has written it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchId {
    pub provider: ProviderId,
    pub account: AccountId,
    pub started_at_ms: i64,
}

/// One tenure of the child's session (spec §6.1): the reads of one execution's record (its
/// `startedAt`) that named one session, from the first (`first_at`) to the last read or
/// unchanged `stat` that confirmed it (`confirmed_at`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenure {
    pub session_id: String,
    pub started_at: i64,
    pub first_at: i64,
    pub confirmed_at: i64,
}

/// The poller's own state, owned by `run` (spec §6.1): the record's mtime at the read that named
/// the current tenure's session, that tenure, and the tenures that ended since the last write,
/// oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchPoll {
    pub last_mtime_ns: Option<i128>,
    pub tenure: Option<Tenure>,
    pub ended: Vec<Tenure>,
}

impl Engine {
    /// Spec §6: scans every home's session records, records what it finds, and closes the
    /// launches whose profile is no longer in use. Returns how many rows it added, extended or
    /// closed.
    pub fn sight_sessions(&self, provider: &ProviderId) -> Result<u64, EngineError> {
        let p = self.provider(provider)?;
        if !p.capabilities().transcripts {
            return Err(EngineError::InvalidInput(format!(
                "{} has no transcripts",
                p.display_name()
            )));
        }
        self.scan_sessions(p.as_ref())
    }

    /// `sight_sessions`, and a refresh's step 1 (spec §5.2): the scan of spec §6.2, then every
    /// open launch whose profile is no longer in use is closed at the moment it is found so
    /// (spec §6.1). Each time is read after the check it records, never at the scan's start: a
    /// child of a killed `run` that answers once more while the scan runs, then exits before
    /// its profile is checked, leaves no response past its launch's end. A late close
    /// over-covers, which can only leave a response unattributed, never misplace it.
    pub(crate) fn scan_sessions(&self, p: &dyn Provider) -> Result<u64, EngineError> {
        let provider = p.id();
        let store = self.store()?;
        let mut changed = 0;
        for (home, dir) in self.homes(p) {
            let Read::Present(entries) = read_session_records(&dir) else {
                continue;
            };
            // The home's records were read now. A sighting is dated at its read, not after the
            // liveness checks and the writes that follow, which take time in which the session
            // may leave (spec §6.4).
            let read_at = self.now_ms();
            hooks::point(self, "scan-records-read")?;
            for entry in entries {
                // An unreadable or partly written record, or one whose process is gone, is
                // skipped (spec §6.2).
                let RecordEntry::Record(r) = entry else {
                    continue;
                };
                let (Some(session_id), Some(started_at)) = (r.session_id.clone(), r.started_at_ms)
                else {
                    continue;
                };
                if !record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                    continue;
                }
                // The read names the session then, and says nothing of when it entered the
                // execution: the latest tenure goes on, or one starts at the read (spec §6.4).
                let row = SightingRow {
                    session_id,
                    home: home.clone(),
                    started_at,
                    first_at: read_at,
                    last_at: read_at,
                    source: SightingSource::Scan,
                    recorded_at: self.now_ms(),
                };
                if store.continue_sighting(&provider, &row, SIGHTING_GAP_MS)? {
                    changed += 1;
                }
            }
        }
        for launch in store.launches(&provider)? {
            if launch.ended_at.is_some() {
                continue;
            }
            let account = AccountId::from_string(launch.account_id.as_str());
            hooks::point(self, "scan-launch-check")?;
            if !self
                .session_state_at(p, &profile_path(&self.env, &account))
                .owned()
            {
                let now = self.now_ms();
                store.close_launch(&provider, &launch.account_id, launch.started_at, now, now)?;
                changed += 1;
            }
        }
        tracing::debug!(
            provider = self.registered_id(&provider),
            changed,
            "scanned the session records"
        );
        Ok(changed)
    }

    /// The homes the scan reads (spec §6.2): the default home (`""`), then each of `p`'s
    /// profiles under the data directory's `sessions/`, by its account. A profile whose marker
    /// names another provider, or cannot be read, is not `p`'s to place.
    fn homes(&self, p: &dyn Provider) -> Vec<(String, PathBuf)> {
        let mut homes = Vec::new();
        if let Some(dir) = p.default_session_records_dir(&self.env) {
            homes.push((String::new(), dir));
        }
        if !p.capabilities().sessions {
            return homes;
        }
        let Ok(listing) = fs::read_dir(self.env.data_dir().join("sessions")) else {
            return homes;
        };
        for entry in listing.flatten() {
            let Some(account) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let profile = entry.path();
            let ours = entry.file_type().is_ok_and(|t| t.is_dir())
                && matches!(ProfileMarker::read(&profile), Read::Present(m) if m.provider == p.id());
            if ours {
                homes.push((account, p.session_records_dir(&profile)));
            }
        }
        homes.sort();
        homes
    }

    /// Spec §6.1: the launch `run` plans before it spawns its child, started now. Nothing is
    /// written: `record_launch` writes it, as often as it takes. Attribution reads launches at
    /// query time, so one recorded late with this start covers the whole session.
    pub fn plan_launch(&self, provider: &ProviderId, account: &AccountId) -> LaunchId {
        LaunchId {
            provider: provider.clone(),
            account: account.clone(),
            started_at_ms: self.now_ms(),
        }
    }

    /// Spec §6.1: records a planned launch, open from its own start. Idempotent: a launch
    /// already recorded at that key is kept as it is, so `run` can try again until it succeeds.
    pub fn record_launch(&self, id: &LaunchId) -> Result<(), EngineError> {
        self.store()?.open_launch(
            &id.provider,
            id.account.as_str(),
            id.started_at_ms,
            self.now_ms(),
        )?;
        Ok(())
    }

    /// Spec §6.1: `plan_launch`, then `record_launch`, for a caller with no use for the gap
    /// between them. `run` calls the two apart, while it holds its launch reservation, so no scan
    /// closes the launch before the child runs.
    pub fn open_launch(
        &self,
        provider: &ProviderId,
        account: &AccountId,
    ) -> Result<LaunchId, EngineError> {
        let id = self.plan_launch(provider, account);
        self.record_launch(&id)?;
        Ok(id)
    }

    /// The child's pid, once it is spawned (spec §6.1).
    pub fn attach_launch_pid(&self, id: &LaunchId, pid: u32) -> Result<(), EngineError> {
        Ok(self.store()?.attach_launch_pid(
            &id.provider,
            id.account.as_str(),
            id.started_at_ms,
            pid,
            self.now_ms(),
        )?)
    }

    /// Spec §6.1's check, on every 50 ms tick while the child runs; it never touches the store.
    /// The child's record, `<records dir>/<pid>.json`, is `stat`ed. An mtime unchanged since the
    /// read that named the current tenure's session confirms that tenure now: Claude Code
    /// rewrites the record when its session changes (spec §7.4 A3). A moved mtime is read. This
    /// child's own record naming the tenure's session in the same execution confirms it; naming
    /// another, it ends the tenure at its last confirmation and starts the next one. Each is
    /// dated at the read, before the liveness check that follows it. Any other read, of a record
    /// caught part-way through its write, of one that is not this child's, or of none, confirms
    /// nothing, and its mtime is not remembered, so the next tick reads it again.
    pub fn observe_launch(
        &self,
        id: &LaunchId,
        profile: &Path,
        pid: u32,
        state: &mut LaunchPoll,
    ) -> Result<(), EngineError> {
        let p = self.provider(&id.provider)?;
        let record = p.session_records_dir(profile).join(format!("{pid}.json"));
        let mtime = match fs::metadata(&record) {
            Ok(meta) => i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
            // Not written yet, or already removed at the child's exit.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        if state.last_mtime_ns == Some(mtime) {
            if let Some(t) = &mut state.tenure {
                t.confirmed_at = self.now_ms();
            }
            return Ok(());
        }
        // Only this child's own execution: a record of its pid, started no earlier than the
        // launch, whose process still runs. One a crashed execution left behind under a reused
        // pid is skipped.
        let read = fs::read(&record).ok();
        let read_at = self.now_ms();
        hooks::point(self, "launch-record-read")?;
        let named = read
            .and_then(|bytes| parse_session_record(&bytes).ok())
            .filter(|r| {
                r.pid == pid
                    && r.started_at_ms.is_some_and(|at| at >= id.started_at_ms)
                    && record_is_live(self.process.as_ref(), r, p.launch_command())
            })
            .and_then(|r| r.session_id.zip(r.started_at_ms));
        let Some((session_id, started_at)) = named else {
            return Ok(());
        };
        state.last_mtime_ns = Some(mtime);
        match &mut state.tenure {
            Some(t) if t.session_id == session_id && t.started_at == started_at => {
                t.confirmed_at = read_at;
            }
            current => {
                let next = Tenure {
                    session_id,
                    started_at,
                    first_at: read_at,
                    confirmed_at: read_at,
                };
                if let Some(ended) = current.replace(next) {
                    state.ended.push(ended);
                }
            }
        }
        Ok(())
    }

    /// Spec §6.1's write, every 2 seconds while the child runs: the current tenure, to its last
    /// confirmation, extended at most once a minute (spec §6.4); then each tenure that ended
    /// since, newest first, exactly to its last confirmation. A tenure leaves the queue only once
    /// every later one is stored, so a scan or a statusline that reads its session again finds
    /// the execution seen naming another since, and starts a new tenure rather than bridging the
    /// gap (spec §6.4). A write that fails leaves the rest queued for the next poll.
    pub fn poll_launch(&self, id: &LaunchId, state: &mut LaunchPoll) -> Result<(), EngineError> {
        if state.tenure.is_none() && state.ended.is_empty() {
            return Ok(());
        }
        let store = self.store()?;
        if let Some(t) = &state.tenure {
            store.record_sighting(
                &id.provider,
                &self.tenure_row(id, t, t.confirmed_at),
                SIGHTING_GAP_MS,
            )?;
        }
        while let Some(t) = state.ended.last() {
            self.end_tenure(&store, id, t, t.confirmed_at)?;
            state.ended.pop();
        }
        Ok(())
    }

    /// Spec §6.1, at the child's exit: the child's tenures end at their last confirming reads,
    /// the current one first, then those the poll had not stored, newest first; not at the exit,
    /// since the child may have left its session after its last read. None ends past
    /// `exited_at_ms`, when the child exited: a check after it, of a record a crash left behind,
    /// confirms nothing. Each end is set back as well as forward, below a later stamp too, such
    /// as a scan's read between the last check and the exit. The launch closes at the moment the
    /// profile is found idle (`session_state_at`), read after that check, unless it is still in
    /// use. `run` calls it after `finish_run`, once its own launch reservation is released, so
    /// the profile reads idle when nothing else runs there. Only the launch may close late, which
    /// over-covers. A background session the child started, or another launcher's reservation,
    /// keeps the launch open, and the next scan that finds the profile idle closes it.
    pub fn close_launch(
        &self,
        id: &LaunchId,
        profile: &Path,
        state: &LaunchPoll,
        exited_at_ms: i64,
    ) -> Result<(), EngineError> {
        let p = self.provider(&id.provider)?;
        let store = self.store()?;
        for t in state.tenure.iter().chain(state.ended.iter().rev()) {
            self.end_tenure(&store, id, t, t.confirmed_at.min(exited_at_ms))?;
        }
        hooks::point(self, "close-launch-check")?;
        if !self.session_state_at(p.as_ref(), profile).owned() {
            let now = self.now_ms();
            store.close_launch(
                &id.provider,
                id.account.as_str(),
                id.started_at_ms,
                now,
                now,
            )?;
        }
        Ok(())
    }

    /// `run`'s sighting of the tenure `t` in its launch's profile, ending at `last_at`.
    fn tenure_row(&self, id: &LaunchId, t: &Tenure, last_at: i64) -> SightingRow {
        SightingRow {
            session_id: t.session_id.clone(),
            home: id.account.as_str().to_owned(),
            started_at: t.started_at,
            first_at: t.first_at,
            last_at,
            source: SightingSource::Run,
            recorded_at: self.now_ms(),
        }
    }

    /// Ends `run`'s tenure `t` exactly at `last_at` (spec §6.1): stored first, when no write has
    /// stored it yet, then its end set, back as well as forward, and every other tenure of its
    /// session in that execution that began by then clamped to it (spec §6.4).
    fn end_tenure(
        &self,
        store: &Store,
        id: &LaunchId,
        t: &Tenure,
        last_at: i64,
    ) -> Result<(), EngineError> {
        store.end_sighting(&id.provider, &self.tenure_row(id, t, last_at))?;
        Ok(())
    }

    /// Spec §6.3: the statusline's sighting of `session_id` in its home, which is `profile`, the
    /// run shell's profile, or the default home. Nothing is done while the session's latest
    /// sighting there is under a minute old. Otherwise the Claude Code process the statusline
    /// runs under is found among its ancestors, by its record in the home: the session's latest
    /// tenure in that execution goes on to that read, or one starts at it (spec §6.4). The write
    /// waits at most `CACHE_WRITE_WAIT`, and a busy store skips it. The store is never created.
    pub fn statusline_sighting(
        &self,
        provider: &ProviderId,
        session_id: &str,
        profile: Option<&Path>,
    ) -> Result<(), EngineError> {
        let p = self.provider(provider)?;
        if !p.capabilities().transcripts {
            return Ok(());
        }
        let (home, records) = match profile {
            Some(dir) => match ProfileMarker::read(dir) {
                Read::Present(m) if &m.provider == provider => {
                    (m.account_id.as_str().to_owned(), p.session_records_dir(dir))
                }
                _ => return Ok(()),
            },
            None => match p.default_session_records_dir(&self.env) {
                Some(dir) => (String::new(), dir),
                None => return Ok(()),
            },
        };
        let Some(store) = self.existing_store()? else {
            return Ok(());
        };
        let now = self.now_ms();
        let fresh = store
            .latest_sighting(provider, session_id, &home)?
            .is_some_and(|s| s.last_at > now.saturating_sub(SIGHTING_GAP_MS));
        if fresh {
            return Ok(());
        }
        let Some((started_at, read_at)) = self.execution_started_at(p.as_ref(), &records) else {
            return Ok(());
        };
        let row = SightingRow {
            session_id: session_id.to_owned(),
            home,
            started_at,
            first_at: read_at,
            last_at: read_at,
            source: SightingSource::Statusline,
            recorded_at: self.now_ms(),
        };
        // Evidence, not the line: a failed write leaves the session to the next run. SQLite's
        // result code only, never the error's text (§14.2).
        if let Err(e) =
            store.continue_sighting_within(provider, &row, SIGHTING_GAP_MS, CACHE_WRITE_WAIT)
        {
            tracing::debug!(
                code = e.sqlite_code().map(tracing::field::debug),
                "the statusline's session sighting was not written"
            );
        }
        Ok(())
    }

    /// The `startedAt` of the first of this process's ancestors, at most `MAX_ANCESTORS` up,
    /// with a live record of its own in `records` (spec §6.3): the execution the statusline
    /// runs under. With it, when that record was read, which dates the sighting (spec §6.4).
    fn execution_started_at(&self, p: &dyn Provider, records: &Path) -> Option<(i64, i64)> {
        let mut pid = std::process::id();
        for _ in 0..MAX_ANCESTORS {
            pid = self.process.parent_pid(pid).filter(|&parent| parent > 1)?;
            hooks::point(self, "statusline-ancestor").ok()?;
            let Ok(bytes) = fs::read(records.join(format!("{pid}.json"))) else {
                continue;
            };
            let read_at = self.now_ms();
            let Ok(r) = parse_session_record(&bytes) else {
                continue;
            };
            if r.pid == pid && record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                return r.started_at_ms.map(|at| (at, read_at));
            }
        }
        None
    }
}
