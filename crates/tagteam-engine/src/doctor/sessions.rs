use std::path::Path;
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_provider::{Check, Provider, Read};

use crate::error::{SessionOwner, daemon_advice, supervisor_name};
use crate::session::SessionState;

use super::{Run, command};

impl Run<'_> {
    /// §13.6 Session profiles, for `p`: each profile whose marker names `p`, and the entries of
    /// `p`'s source home on no share list. The profiles need every account and `sessions/`'s
    /// listing; when either cannot be read, `store.accounts` (or `store.skipped`) and
    /// `sessions.profiles` say so, once.
    pub(super) fn sessions(&mut self, p: &dyn Provider) {
        if !p.capabilities().sessions {
            return;
        }
        let id = p.id();
        let mut found = Vec::new();
        let (accounts, profiles) = match (self.all_accounts(), self.profile_dirs()) {
            (Some(accounts), Ok(profiles)) => (accounts, profiles),
            _ => (Vec::new(), Vec::new()),
        };
        for (dir, marker) in profiles {
            // §5: a directory named by a stored account's ID is that account's profile, whatever
            // its marker says, and the account's provider checks it.
            let name = dir.file_name().and_then(|n| n.to_str());
            if let Some(row) = accounts.iter().find(|r| Some(r.id.as_str()) == name) {
                if row.provider != id {
                    continue;
                }
                let why = match &marker {
                    Read::Present(m) if m.account_id == row.id && m.provider == row.provider => {
                        self.profile(p, row, &dir, Some(m), &mut found);
                        continue;
                    }
                    Read::Present(m) => format!("names {} of {} instead", m.account_id, m.provider),
                    Read::Unreadable(e) => format!("cannot be read ({}: {})", e.what, e.detail),
                    Read::Absent => "is missing".to_owned(),
                };
                // §13.6: a marker that cannot be trusted warns, and what needs no marker is
                // still checked.
                let n = row.position;
                found.push(
                    Check::warn(
                        "sessions.marker",
                        format!(
                            "account {n}'s profile marker {why}, so its recorded spelling and outer home are unknown: its credential and provenance are not checked"
                        ),
                    )
                    .fix(format!(
                        "{}; if account {n}'s login then stops working, log in again and run `{}`",
                        Self::move_then_delete(&dir),
                        command(self.engine, &id, &format!("add --position {n}"))
                    )),
                );
                self.profile(p, row, &dir, None, &mut found);
                continue;
            }
            // Any other directory is its marker's provider's; `stray_profiles` reports one
            // with no marker it can read.
            let Read::Present(m) = marker else { continue };
            if m.provider != id {
                continue;
            }
            match accounts
                .iter()
                .find(|r| r.id == m.account_id && r.provider == id)
            {
                Some(row) => found.push(
                    Check::warn(
                        "sessions.marker",
                        format!(
                            "{} holds the marker of account {}'s profile, which lives elsewhere",
                            dir.display(),
                            row.position
                        ),
                    )
                    .fix(Self::move_then_delete(&dir)),
                ),
                None => {
                    found.push(
                        Check::warn(
                            "sessions.orphan",
                            format!(
                                "{} is a profile of an account this store no longer has",
                                dir.display()
                            ),
                        )
                        .fix(format!(
                            "{}; `tagteam purge` deletes it with its Keychain item",
                            Self::move_then_delete(&dir)
                        )),
                    );
                }
            }
        }
        self.unknown_entries(p, &mut found);
        for check in found {
            self.push(Some(&id), check);
        }
    }

    /// §13.6: a live background supervisor in `profile` (its supervisor lock, §12.6), found at
    /// that path, as the refusal names it. `whose` finishes "runs in ...".
    fn daemon_check(p: &dyn Provider, profile: &Path, whose: &str) -> Check {
        Check::info(
            "sessions.daemon",
            format!("{} runs in {whose} until it stops", supervisor_name(p)),
        )
        .fix(daemon_advice(p, profile))
    }

    /// What `state` says about ownership of the profile `whose` names, from the whole aggregate
    /// and through the shared builder: a warning for every input that could not be read, with
    /// its repair, and the daemon, if one is live (§12.6). Nothing is filtered, so the account
    /// checks and the orphan checks report alike. `consequence` finishes the warning.
    fn ownership(
        p: &dyn Provider,
        state: &SessionState,
        whose: &str,
        runs_in: &str,
        consequence: &str,
    ) -> Vec<Check> {
        let mut out = Vec::new();
        if !state.damaged().is_empty() {
            let owner = SessionOwner::of(state, p);
            out.push(
                Check::warn(
                    "sessions.state",
                    format!(
                        "{whose} session state cannot be read ({}), {consequence}",
                        owner.damaged_list()
                    ),
                )
                .fix(owner.repairs().join("; ")),
            );
        }
        if let Some(profile) = state.daemon_profile() {
            out.push(Self::daemon_check(
                p,
                profile,
                &format!("{runs_in}, so it is session-owned"),
            ));
        }
        out
    }

    /// §13.6: ownership of every entry of `sessions/`, walked once and reported through
    /// `ownership`, before and apart from any marker or account classification, each judged
    /// as `purge` judges it: an account's profile by that account's provider, any other entry
    /// (an orphan, a stray, a copy of another's marker) by the provider its marker names when
    /// this build registers it with sessions, else by every registered provider with sessions
    /// (`Engine::judges`). A `sessions/` that cannot be listed is `stray_profiles`'s to report.
    /// With `only`, the entries of providers in scope, and the ones whose provider cannot be
    /// told.
    pub(super) fn ownership_walk(&mut self, only: Option<&ProviderId>) {
        let Ok(profiles) = self.profile_dirs() else {
            return;
        };
        let Some(accounts) = self.all_accounts() else {
            return;
        };
        let in_scope: Vec<ProviderId> = self.scope(only).iter().map(|p| p.id()).collect();
        let every: Vec<ProviderId> = self.engine.registry.all().iter().map(|p| p.id()).collect();
        let mut found: Vec<(Option<ProviderId>, Check)> = Vec::new();
        for (dir, marker) in profiles {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(row) = accounts.iter().find(|r| r.id.as_str() == name) {
                let n = row.position;
                match self.engine.registry.get(&row.provider) {
                    Some(p) if p.capabilities().sessions => {
                        if !in_scope.contains(&row.provider) {
                            continue;
                        }
                        let whose = format!("account {n}'s");
                        let runs_in = format!("account {n}'s profile");
                        let consequence = "so the account counts as in a session: commands that change it refuse, and its baseline and provenance are not checked";
                        let checks = match self.engine.session_state(p.as_ref(), row) {
                            Ok(state) => {
                                Self::ownership(p.as_ref(), &state, &whose, &runs_in, consequence)
                            }
                            Err(e) => {
                                vec![Check::warn(
                                "sessions.state",
                                format!(
                                    "{whose} session state cannot be read ({}), {consequence}",
                                    e.kind()
                                ),
                            )
                            .fix("make what it names readable again")]
                            }
                        };
                        found.extend(checks.into_iter().map(|c| (Some(row.provider.clone()), c)));
                    }
                    Some(_) => {}
                    None => {
                        if only.is_none() {
                            let judges = self.engine.judges(&dir, &every);
                            found.extend(
                                self.orphan_ownership(&judges, &dir)
                                    .into_iter()
                                    .map(|c| (None, c)),
                            );
                        }
                    }
                }
                continue;
            }
            // Not an account's profile: judged as purge judges an orphan, however its marker
            // reads, and whoever the marker names.
            let named = match &marker {
                Read::Present(m) => Some(m.provider.clone()),
                _ => None,
            };
            if only.is_some() && named.as_ref().is_some_and(|id| !in_scope.contains(id)) {
                continue;
            }
            let judges = self.engine.judges(&dir, &every);
            let owner = match (&named, judges.as_slice()) {
                (Some(id), [one]) if one.id() == *id => Some(id.clone()),
                _ => None,
            };
            found.extend(
                self.orphan_ownership(&judges, &dir)
                    .into_iter()
                    .map(|c| (owner.clone(), c)),
            );
        }
        for (id, check) in found {
            self.push(id.as_ref(), check);
        }
    }

    /// `ownership` for the profile at `dir` that no stored account owns: each of `judges` is
    /// asked, as `purge` asks (§10.5 step 6), and every finding is reported once.
    fn orphan_ownership(&self, judges: &[Arc<dyn Provider>], dir: &Path) -> Vec<Check> {
        let whose = format!("the orphaned profile {}'s", dir.display());
        let runs_in = format!("the orphaned profile {}", dir.display());
        let mut out: Vec<Check> = Vec::new();
        for p in judges.iter().filter(|p| p.capabilities().sessions) {
            let state = self.engine.session_state_at(p.as_ref(), dir);
            for c in Self::ownership(
                p.as_ref(),
                &state,
                &whose,
                &runs_in,
                "so it counts as in use: `tagteam purge` refuses it",
            ) {
                if !out.iter().any(|o| o.id == c.id && o.message == c.message) {
                    out.push(c);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_check_names_the_agent() {
        let cc = tagteam_cc::ClaudeCode::new(
            Arc::new(tagteam_provider::FakeKeychain::new()),
            tagteam_provider::Platform::MacOs,
        );
        let c = Run::daemon_check(
            &cc,
            Path::new("/p"),
            "account 1's profile, so it is session-owned",
        );
        assert_eq!(
            c.message,
            "a Claude Code background daemon runs in account 1's profile, so it is session-owned until it stops"
        );
        assert_eq!(
            c.fix.as_deref(),
            Some(
                "stop it with `claude daemon stop --any` run with CLAUDE_CONFIG_DIR set to '/p'; if nothing is running at that pid, delete '/p/daemon.lock'"
            )
        );
    }
}
