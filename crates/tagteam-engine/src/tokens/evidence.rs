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
use crate::store::{SightingRow, SightingSource};
use crate::views::CACHE_WRITE_WAIT;

/// A sighting is extended at most once a minute (spec §6.4), and the statusline does nothing
/// while its session's latest sighting in its home is younger than that (spec §6.3).
const SIGHTING_GAP_MS: i64 = 60_000;
/// The most ancestors the statusline walks to find its Claude Code process (spec §6.3).
const MAX_ANCESTORS: usize = 8;

/// One launch `run` recorded (spec §6.1), as `open_launch` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchId {
    pub provider: ProviderId,
    pub account: AccountId,
    pub started_at_ms: i64,
}

/// The poller's own state, owned by `run` (spec §6.1): the child's record's last mtime, and the
/// sighting it wrote last, as `(session id, first_at)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchPoll {
    pub last_mtime_ns: Option<i128>,
    pub last_sighting: Option<(String, i64)>,
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
            for entry in entries {
                // An unreadable or partly written record, or one whose process is gone, is
                // skipped (spec §6.2).
                let RecordEntry::Record(r) = entry else {
                    continue;
                };
                let (Some(session_id), Some(first_at)) = (r.session_id.clone(), r.started_at_ms)
                else {
                    continue;
                };
                if !record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                    continue;
                }
                let now = self.now_ms();
                let row = SightingRow {
                    session_id,
                    home: home.clone(),
                    first_at,
                    last_at: now,
                    source: SightingSource::Scan,
                    recorded_at: now,
                };
                if store.record_sighting(&provider, &row, SIGHTING_GAP_MS)? {
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

    /// Spec §6.1: the launch `run` records before it spawns its child, open from now. `run`
    /// calls it while it holds its launch reservation, so no scan closes it before the child
    /// runs.
    pub fn open_launch(
        &self,
        provider: &ProviderId,
        account: &AccountId,
    ) -> Result<LaunchId, EngineError> {
        let now = self.now_ms();
        self.store()?
            .open_launch(provider, account.as_str(), now, now)?;
        Ok(LaunchId {
            provider: provider.clone(),
            account: account.clone(),
            started_at_ms: now,
        })
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

    /// Spec §6.1's poll, every 2 seconds while the child runs: its record,
    /// `<records dir>/<pid>.json`, is read again only when its mtime moved, and the sighting of
    /// the session it names, keyed by its `startedAt`, is extended to now. The store writes at
    /// most once a minute (spec §6.4). A record caught part-way through its write, or one that is
    /// not this child's, is read again at the next poll.
    pub fn poll_launch(
        &self,
        id: &LaunchId,
        profile: &Path,
        pid: u32,
        state: &mut LaunchPoll,
    ) -> Result<(), EngineError> {
        let p = self.provider(&id.provider)?;
        let record = p.session_records_dir(profile).join(format!("{pid}.json"));
        match fs::metadata(&record) {
            Ok(meta) => {
                let mtime =
                    i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec());
                if state.last_mtime_ns != Some(mtime) {
                    // Only this child's own execution: a record of its pid, started no earlier
                    // than the launch, whose process still runs. One a crashed execution left
                    // behind under a reused pid is skipped, and not remembered.
                    let seen = fs::read(&record)
                        .ok()
                        .and_then(|bytes| parse_session_record(&bytes).ok())
                        .filter(|r| {
                            r.pid == pid
                                && r.started_at_ms.is_some_and(|at| at >= id.started_at_ms)
                                && record_is_live(self.process.as_ref(), r, p.launch_command())
                        })
                        .and_then(|r| r.session_id.zip(r.started_at_ms));
                    if let Some(seen) = seen {
                        state.last_mtime_ns = Some(mtime);
                        state.last_sighting = Some(seen);
                    }
                }
            }
            // Not written yet, or already removed at the child's exit.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let Some((session_id, first_at)) = &state.last_sighting else {
            return Ok(());
        };
        let now = self.now_ms();
        let row = SightingRow {
            session_id: session_id.clone(),
            home: id.account.as_str().to_owned(),
            first_at: *first_at,
            last_at: now,
            source: SightingSource::Run,
            recorded_at: now,
        };
        self.store()?
            .record_sighting(&id.provider, &row, SIGHTING_GAP_MS)?;
        Ok(())
    }

    /// Spec §6.1, at the child's exit: the sighting it saw last ends exactly at `exited_at_ms`,
    /// when the child exited, moved back as well as forward, since a poll in flight as the child
    /// exited may have stamped it later. The launch closes at the moment the profile is found
    /// idle (`session_state_at`), read after that check, unless it is still in use. `run` calls
    /// it after `finish_run`, once its own launch reservation is released, so the profile reads
    /// idle when nothing else runs there. Only the launch may close late, which over-covers; the
    /// sighting never outlasts the child, so exit handling that waits on locks cannot stretch it
    /// over a resume elsewhere. A background session the child started, or another launcher's
    /// reservation, keeps the launch open, and the next scan that finds the profile idle closes
    /// it.
    pub fn close_launch(
        &self,
        id: &LaunchId,
        profile: &Path,
        state: &LaunchPoll,
        exited_at_ms: i64,
    ) -> Result<(), EngineError> {
        let p = self.provider(&id.provider)?;
        let store = self.store()?;
        if let Some((session_id, first_at)) = &state.last_sighting {
            store.set_sighting_last_at(
                &id.provider,
                session_id,
                id.account.as_str(),
                *first_at,
                exited_at_ms,
                self.now_ms(),
            )?;
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

    /// Spec §6.3: the statusline's sighting of `session_id` in its home, which is `profile`, the
    /// run shell's profile, or the default home. Nothing is done while the session's latest
    /// sighting there is under a minute old. Otherwise the Claude Code process the statusline
    /// runs under is found among its ancestors, by its record in the home, and that
    /// execution's sighting is written. The write waits at most `CACHE_WRITE_WAIT`, and a busy
    /// store skips it. The store is never created.
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
        let Some(first_at) = self.execution_started_at(p.as_ref(), &records) else {
            return Ok(());
        };
        let row = SightingRow {
            session_id: session_id.to_owned(),
            home,
            first_at,
            last_at: now,
            source: SightingSource::Statusline,
            recorded_at: now,
        };
        // Evidence, not the line: a failed write leaves the session to the next run. SQLite's
        // result code only, never the error's text (§14.2).
        if let Err(e) =
            store.record_sighting_within(provider, &row, SIGHTING_GAP_MS, CACHE_WRITE_WAIT)
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
    /// runs under.
    fn execution_started_at(&self, p: &dyn Provider, records: &Path) -> Option<i64> {
        let mut pid = std::process::id();
        for _ in 0..MAX_ANCESTORS {
            pid = self.process.parent_pid(pid).filter(|&parent| parent > 1)?;
            let Ok(bytes) = fs::read(records.join(format!("{pid}.json"))) else {
                continue;
            };
            let Ok(r) = parse_session_record(&bytes) else {
                continue;
            };
            if r.pid == pid && record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                return r.started_at_ms;
            }
        }
        None
    }
}
