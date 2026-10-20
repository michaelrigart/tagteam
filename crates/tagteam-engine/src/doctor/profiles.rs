use std::io;
use std::path::{Path, PathBuf};

use tagteam_core::{ProvenanceVerdict, ProviderId, provenance};
use tagteam_provider::doctor::quoted;
use tagteam_provider::profile::{ProfileMarker, Seed, canonical_profile_path, launch_reservations};
use tagteam_provider::{Check, Env, LockProbe, Provenance, Provider, Read, holders_of};

use crate::profiles::{Held, allowlist, held, is_private, resolved};
use crate::provenance::identity_drifted;
use crate::session::SessionState;
use crate::settings::Settings;
use crate::store::AccountRow;

use super::files::{entries, readable, subdirs};
use super::{Id, Run, Secrets, command};

/// `sessions.reservation` for a reservation `path` still held after its tagteam `parent` died.
/// `holders` is `holders_of`'s answer; `None` means either that this OS cannot tell, or (when
/// `still_there` is false) that the lock file went away between the probe and the scan. (The
/// scan's `stat` of the file could hang on a stalled NFS mount; a known gap.)
fn held_reservation(
    path: &Path,
    parent: u32,
    n: u32,
    launch: &str,
    holders: Option<Vec<u32>>,
    still_there: bool,
) -> Check {
    if holders.is_none() && !still_there {
        return Check::info(
            Id::SESSIONS_RESERVATION,
            format!(
                "{} was held although its tagteam (pid {parent}) is gone, but the lock went away while doctor looked, so account {n}'s state is not known to have stayed the same",
                path.display()
            ),
        )
        .fix("run `tagteam doctor` again");
    }
    let holders = match holders {
        Some(pids) if !pids.is_empty() => format!(
            "held by pid {}",
            pids.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(_) => "held by a process this user cannot see".to_owned(),
        None => "this OS cannot tell which process holds it".to_owned(),
    };
    Check::info(
        Id::SESSIONS_RESERVATION,
        format!(
            "{} is still held although its tagteam (pid {parent}) is gone ({holders}): a process its `{launch}` started still runs, so account {n} stays in a session",
            path.display()
        ),
    )
    .fix("end that process to release the account")
}

impl Run<'_> {
    /// The fix for a profile directory doctor cannot place: real history first, never deleted
    /// (§12.2), then the directory.
    pub(super) fn move_then_delete(dir: &Path) -> String {
        format!(
            "move any real `projects/` or `history.jsonl` in {} into the default home's, then delete it once no session runs in it",
            quoted(dir)
        )
    }

    /// Every directory under `sessions/`, with its marker as read (§12.2); an error when
    /// `sessions/` cannot be listed, which `stray_profiles` reports.
    pub(super) fn profile_dirs(&self) -> io::Result<Vec<(PathBuf, Read<ProfileMarker>)>> {
        let dirs = subdirs(&self.engine.env.data_dir().join("sessions"))?;
        Ok(dirs
            .into_iter()
            .map(|d| {
                let m = ProfileMarker::read(&d);
                (d, m)
            })
            .collect())
    }

    /// §13.6 Session profiles that belong to no provider in scope, found from `sessions/`
    /// itself: a marker that cannot be read, one naming a provider this build lacks or one out
    /// of scope (a registered provider with no account), and a directory with no marker, each
    /// in a directory no stored account owns. An account's own profile, marker or not, is its
    /// provider's to report (`sessions`). Each names what to delete. A `sessions/` that cannot
    /// be listed warns here, once, for every provider.
    pub(super) fn stray_profiles(&mut self, only: Option<&ProviderId>) {
        let profiles = match self.profile_dirs() {
            Ok(profiles) => profiles,
            Err(e) => {
                let dir = self.engine.env.data_dir().join("sessions");
                self.push(
                    None,
                    Check::warn(
                        Id::SESSIONS_PROFILES,
                        format!(
                            "{} cannot be listed ({}), so no session profile is checked",
                            dir.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&dir)),
                );
                return;
            }
        };
        let Some(accounts) = self.all_accounts() else {
            return;
        };
        // The providers whose `sessions` runs, and reports their own orphans (§13.1's scope).
        let in_scope: Vec<ProviderId> = self.scope(only).iter().map(|p| p.id()).collect();
        let mut found = Vec::new();
        for (dir, marker) in profiles {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if accounts.iter().any(|r| r.id.as_str() == name) {
                continue;
            }
            let delete = Self::move_then_delete(&dir);
            match marker {
                Read::Unreadable(e) => found.push(
                    Check::warn(
                        Id::SESSIONS_MARKER,
                        format!(
                            "the profile marker {} cannot be read ({}), so whose profile {} is cannot be told",
                            e.what,
                            e.detail,
                            dir.display()
                        ),
                    )
                    .fix(Self::move_then_delete(&dir)),
                ),
                Read::Present(m) if self.engine.registry.get(&m.provider).is_none() => {
                    if only.is_none() {
                        found.push(
                            Check::warn(
                                Id::SESSIONS_ORPHAN,
                                format!(
                                    "{} is a profile of {}, which this tagteam does not have",
                                    dir.display(),
                                    m.provider
                                ),
                            )
                            .fix(delete),
                        );
                    }
                }
                // A registered provider with no account is out of scope, so no `sessions` run
                // finds its orphans.
                Read::Present(m) if !in_scope.contains(&m.provider) => {
                    if only.is_none() {
                        found.push(
                            Check::warn(
                                Id::SESSIONS_ORPHAN,
                                format!(
                                    "{} is a profile of {}, which has no account in this store",
                                    dir.display(),
                                    m.provider
                                ),
                            )
                            .fix(format!(
                                "{delete}; `tagteam purge` deletes it with its Keychain item"
                            )),
                        );
                    }
                }
                Read::Present(_) => {}
                Read::Absent => found.push(
                    Check::warn(
                        Id::SESSIONS_ORPHAN,
                        format!(
                            "{} has no profile marker and names no account",
                            dir.display()
                        ),
                    )
                    .fix(delete),
                ),
            }
        }
        for check in found {
            self.push(None, check);
        }
    }

    /// One profile of account `row` (§12.2–§12.6), read and probed only. With no marker, its
    /// splits are judged against the default home, as `remove` judges such a profile, and what
    /// needs the recorded spelling (the spelling itself, the credential and the provenance) is
    /// skipped: the caller's `sessions.marker` warning says so.
    pub(super) fn profile(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        dir: &Path,
        marker: Option<&ProfileMarker>,
        found: &mut Vec<Check>,
    ) {
        let n = row.position;
        let env = &self.engine.env;
        if let Some(marker) = marker {
            self.spelling(p, dir, marker, n, found);
        }
        match marker.map(|m| p.apply_outer_home(env, &m.outer)) {
            Some(Ok(outer)) => self.splits(p, &outer, dir, found),
            Some(Err(e)) => found.push(
                Check::warn(
                    Id::SESSIONS_MARKER,
                    format!("account {n}'s profile marker records an outer home that cannot be read: {e}"),
                )
                .fix(format!("nothing to do: account {n}'s next `tagteam run` records it again")),
            ),
            None => self.splits(p, env, dir, found),
        }
        // Ownership is reported once for every `sessions/` entry, by `ownership_walk`; this
        // only gates the checks that need a quiescent profile.
        let quiescent = matches!(
            self.engine.session_state(p, row),
            Ok(SessionState::Quiescent { .. })
        );
        self.reservations(p, dir, n, found);
        if quiescent && p.has_baseline(dir) {
            found.push(
                Check::info(
                    Id::SESSIONS_BASELINE,
                    format!("account {n}'s profile has a config baseline awaiting merge-back"),
                )
                .fix(format!(
                    "nothing to do: account {n}'s next `tagteam run` merges it back"
                )),
            );
        }
        let seed = Seed::read(dir);
        match &seed {
            Read::Present(s) if s.login_epoch != row.login_epoch || s.needs_bootstrap => {
                let why = if s.login_epoch != row.login_epoch {
                    "is stale-marked: an explicit command replaced the account's login"
                } else {
                    "needs a bootstrap: its last login check found it invalid"
                };
                found.push(
                    Check::info(
                        Id::SESSIONS_BOOTSTRAP,
                        format!("account {n}'s profile {why}"),
                    )
                    .fix(format!(
                        "nothing to do: account {n}'s next `tagteam run` bootstraps it"
                    )),
                );
            }
            Read::Unreadable(e) => found.push(
                Check::warn(
                    Id::SESSIONS_SEED,
                    format!(
                        "account {n}'s profile seed {} cannot be read: {}",
                        e.what, e.detail
                    ),
                )
                .fix(format!(
                    "nothing to do: account {n}'s next `tagteam run` bootstraps it"
                )),
            ),
            _ => {}
        }
        // The credential's Keychain item is named from the recorded spelling (§12.2).
        let Some(marker) = marker else { return };
        if self.provider_secrets(p) != Secrets::Readable {
            return;
        }
        let cannot = format!(
            "account {n}'s profile credential cannot be read, so the account cannot switch or launch until it can"
        );
        let held = match p.read_profile_credential(env, dir, &marker.config_dir) {
            Read::Unreadable(e) => {
                found.push(
                    Check::warn(Id::SESSIONS_CREDENTIAL, format!("{cannot}: {e}"))
                        .fix("make it readable again (on macOS, unlock the login keychain)"),
                );
                return;
            }
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                found.push(
                    Check::warn(
                        Id::SESSIONS_CREDENTIAL,
                        format!("{cannot}: only its file could be read, which may be out of date"),
                    )
                    .fix("make its Keychain item readable again (unlock the login keychain)"),
                );
                return;
            }
            Read::Present(c) => c.bytes().to_vec(),
            Read::Absent => return,
        };
        let Read::Present(seed) = seed else { return };
        if !quiescent {
            return;
        }
        // §12.5: only a login that names another account is drift, which is ignored. An absent
        // identity decides nothing: the table runs, and only a rotation needs one, as `apply_provenance` has it.
        let identity_absent = match p.profile_identity(env, dir) {
            Read::Present(login) if !identity_drifted(&login, row) => false,
            Read::Present(_) => return,
            Read::Absent => true,
            Read::Unreadable(e) => {
                found.push(
                    Check::warn(
                        Id::SESSIONS_PROVENANCE,
                        format!(
                            "whether account {n}'s profile and the vault both moved cannot be told: the profile's identity cannot be read ({e})"
                        ),
                    )
                    .fix("make the file it names readable again"),
                );
                return;
            }
        };
        let Some(p_fp) = p.fingerprint(&held).filter(|_| p.has_refresh_token(&held)) else {
            return;
        };
        // The vault's Keychain can be locked while the provider's is not.
        if self.secrets() != Secrets::Readable {
            return;
        }
        // An entry that cannot be read is `accounts.vault`'s `fail`, in the same run.
        let Read::Present(vault) = self.engine.vault.read(&row.id) else {
            return;
        };
        let Some(v_fp) = p.fingerprint(&vault) else {
            return;
        };
        let stale = seed.login_epoch != row.login_epoch;
        let replace = format!(
            "log in as account {n} with `{}`, then `{}`: an explicit replacement settles it",
            p.launch_command(),
            command(self.engine, &p.id(), &format!("add --position {n}"))
        );
        match provenance(p_fp.as_str(), v_fp.as_str(), &seed.seed_fp, stale) {
            ProvenanceVerdict::Conflict => found.push(
                Check::fail(
                    Id::SESSIONS_PROVENANCE,
                    format!(
                        "account {n}'s profile and the vault both moved since they last agreed: nothing is captured, refreshed or launched for the account"
                    ),
                )
                .fix(replace),
            ),
            // The profile rotated the vault's generation, but names no identity that says the
            // rotation is the account's: no holder of its lock captures it, so it warns.
            ProvenanceVerdict::Capture if identity_absent => found.push(
                Check::warn(
                    Id::SESSIONS_PROVENANCE,
                    format!(
                        "account {n}'s profile holds a login it rotated but names no identity, so it cannot be told to be the account's and is not captured"
                    ),
                )
                .fix(replace),
            ),
            _ => {}
        }
    }

    /// §13.6: a recorded spelling (§12.2 "One spelling") that is no longer the profile's
    /// canonical path, or a path that cannot be resolved.
    fn spelling(
        &self,
        p: &dyn Provider,
        dir: &Path,
        marker: &ProfileMarker,
        n: u32,
        found: &mut Vec<Check>,
    ) {
        match canonical_profile_path(dir) {
            Ok(canonical) => {
                let now = p.profile_spelling(&canonical);
                if now != marker.config_dir {
                    found.push(
                        Check::warn(
                            Id::SESSIONS_SPELLING,
                            format!(
                                "account {n}'s profile is recorded as {} but is now {now}: the data directory moved",
                                marker.config_dir
                            ),
                        )
                        .fix(format!("nothing to do: account {n}'s next `tagteam run` bootstraps it")),
                    );
                }
            }
            Err(e) => found.push(
                Check::warn(
                    Id::SESSIONS_SPELLING,
                    format!(
                        "whether account {n}'s profile is still at {} cannot be told: {} cannot be resolved ({})",
                        marker.config_dir,
                        dir.display(),
                        e.kind()
                    ),
                )
                .fix(readable(dir)),
            ),
        }
    }

    /// §12.2: a must-share entry the profile holds as a real copy, or as a link that resolves
    /// elsewhere, fails; another shared entry held as a real copy warns. Read-only: link sync's
    /// own allowlist and matcher, with the provider's `run.share_extra`.
    fn splits(&self, p: &dyn Provider, outer: &Env, dir: &Path, found: &mut Vec<Check>) {
        let policy = p.share_policy(outer);
        let (settings, _) = Settings::load(&self.engine.env, &p.id());
        for w in allowlist(&policy, &settings.share_extra, &mut Vec::new()) {
            let (src, dst) = (policy.source.join(&w.name), dir.join(&w.name));
            let untold = |path: &Path, e: io::Error| {
                Check::warn(
                    Id::SESSIONS_SPLIT,
                    format!(
                        "whether {} is split from {} cannot be told: {} cannot be read ({})",
                        dst.display(),
                        src.display(),
                        path.display(),
                        e.kind()
                    ),
                )
                .fix(readable(path))
            };
            let target = match resolved(&src) {
                Ok(target) => target,
                Err(e) => {
                    found.push(untold(&src, e));
                    continue;
                }
            };
            let split = match held(&dst) {
                Ok(Held::Real) => w.must.is_some() || target.is_some(),
                Ok(Held::Link(_)) if w.must.is_some() && target.is_some() => match resolved(&dst) {
                    Ok(now) => now != target,
                    Err(e) => {
                        found.push(untold(&dst, e));
                        continue;
                    }
                },
                Ok(Held::Link(_) | Held::Nothing) => false,
                Err(e) => {
                    found.push(untold(&dst, e));
                    continue;
                }
            };
            if !split {
                continue;
            }
            found.push(if w.must.is_some() {
                Check::fail(
                    Id::SESSIONS_SPLIT,
                    format!(
                        "{} is not the link to {} it must be: memory or history is split, and `tagteam run` refuses the account",
                        dst.display(),
                        src.display()
                    ),
                )
                .fix(format!(
                    "merge {} into {} by hand, then remove {}",
                    quoted(&dst),
                    quoted(&src),
                    quoted(&dst)
                ))
            } else {
                Check::warn(
                    Id::SESSIONS_SPLIT,
                    format!(
                        "{} is a real copy where {} should be linked: the two have split",
                        dst.display(),
                        src.display()
                    ),
                )
                .fix(format!(
                    "merge them by hand and remove {}; the next launch links it",
                    quoted(&dst)
                ))
            });
        }
    }

    /// §12.5: a reservation whose `tagteam` is gone but whose lock is still held, with the
    /// processes that hold it where the OS can tell. Probed, never waited on, never created.
    fn reservations(&self, p: &dyn Provider, dir: &Path, n: u32, found: &mut Vec<Check>) {
        let list = match launch_reservations(dir) {
            Read::Present(list) => list,
            // The session state reads the same reservations, and `sessions.state` reports
            // them when they cannot be read.
            Read::Absent | Read::Unreadable(_) => return,
        };
        for (path, probe) in list {
            if probe != LockProbe::Held {
                continue;
            }
            let Some(parent) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if self.engine.process.exists(parent) != Some(false) {
                continue;
            }
            let holders = holders_of(&path);
            let still_there = holders.is_some() || path.try_exists().unwrap_or(true);
            found.push(held_reservation(
                &path,
                parent,
                n,
                p.launch_command(),
                holders,
                still_there,
            ));
        }
    }

    /// §12.2 "Unknown entries": the entries of `p`'s source home on neither its share lists
    /// nor its known-private list, which each profile keeps its own of.
    pub(super) fn unknown_entries(&self, p: &dyn Provider, found: &mut Vec<Check>) {
        let policy = p.share_policy(&self.engine.env);
        let (settings, _) = Settings::load(&self.engine.env, &p.id());
        let wanted = allowlist(&policy, &settings.share_extra, &mut Vec::new());
        // A source home that does not exist holds nothing to report.
        let listing = match policy.source.try_exists() {
            Ok(false) => return,
            Ok(true) => entries(&policy.source),
            Err(e) => Err(e),
        };
        let paths = match listing {
            Ok(paths) => paths,
            Err(e) => {
                found.push(
                    Check::warn(
                        Id::SESSIONS_UNKNOWN_ENTRIES,
                        format!(
                            "{} cannot be listed ({}), so whether it holds entries on no share list cannot be told",
                            policy.source.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&policy.source)),
                );
                return;
            }
        };
        let unknown: Vec<String> = paths
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !is_private(&policy, n) && !wanted.iter().any(|w| &w.name == n))
            .collect();
        if unknown.is_empty() {
            found.push(Check::ok(
                Id::SESSIONS_UNKNOWN_ENTRIES,
                format!(
                    "every entry of {} is on a share list",
                    policy.source.display()
                ),
            ));
            return;
        }
        found.push(
            Check::warn(
                Id::SESSIONS_UNKNOWN_ENTRIES,
                format!(
                    "{} holds entries on none of {}'s share lists, so each profile keeps its own: {}",
                    policy.source.display(),
                    p.display_name(),
                    unknown.join(", ")
                ),
            )
            .fix("to share one with every profile, `tagteam config set run.share_extra <names>`"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_whose_lock_went_away_is_not_reported_as_unknowable() {
        let path = Path::new("/d/.tagteam-launch/42.lock");
        let gone = held_reservation(path, 42, 1, "claude", None, false);
        assert!(gone.message.contains("lock went away"), "{}", gone.message);
        assert_eq!(gone.fix.as_deref(), Some("run `tagteam doctor` again"));
        let blind = held_reservation(path, 42, 1, "claude", None, true);
        assert!(
            blind.message.contains("this OS cannot tell"),
            "{}",
            blind.message
        );
        let seen = held_reservation(path, 42, 1, "claude", Some(vec![7, 9]), true);
        assert!(
            seen.message.contains("held by pid 7, 9"),
            "{}",
            seen.message
        );
    }

    #[test]
    fn a_held_reservation_names_the_agent() {
        let held = held_reservation(
            Path::new("/d/.tagteam-launch/42.lock"),
            42,
            1,
            "claude",
            Some(vec![7, 9]),
            true,
        );
        assert_eq!(
            held.message,
            "/d/.tagteam-launch/42.lock is still held although its tagteam (pid 42) is gone (held by pid 7, 9): a process its `claude` started still runs, so account 1 stays in a session"
        );
    }
}
