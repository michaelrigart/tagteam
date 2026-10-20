use tagteam_core::ProviderId;
use tagteam_provider::doctor::quoted;
use tagteam_provider::{Check, Liveness, Provider};

use crate::recover::Direction;
use crate::store::{JournalRow, Store};

use super::files::entries;
use super::store::warn_restore;
use super::{Id, Run, Secrets, Stored, command};

impl Run<'_> {
    /// §13.6 Pending storage, `rescue/` (§6.3): a path that is not a listable directory fails;
    /// each pending rescue warns under its account's provider, and one whose account is gone
    /// under tagteam's own.
    pub(super) fn rescues(&mut self, only: Option<&ProviderId>) {
        let dir = self.engine.env.data_dir().join("rescue");
        let paths = match entries(&dir) {
            Ok(paths) => paths,
            Err(e) => {
                self.push(
                    None,
                    Check::fail(
                        Id::PENDING_RESCUE_DIR,
                        format!(
                            "{} is not a directory tagteam can list ({e}), so every account's rescues are unknown: `remove` refuses, and no account can be activated safely",
                            dir.display()
                        ),
                    )
                    .fix(format!(
                        "move {} aside if it is not tagteam's, or make it a directory again; `tagteam purge` deletes it with everything else",
                        quoted(&dir)
                    )),
                );
                return;
            }
        };
        let Some(accounts) = self.all_accounts() else {
            return;
        };
        let files = paths.into_iter().filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.') && n.ends_with(".json"))
        });
        for path in files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let owner = accounts
                .iter()
                .find(|r| name.starts_with(&format!("{}-", r.id)));
            match owner {
                Some(row) => {
                    if only.is_some_and(|p| p != &row.provider) {
                        continue;
                    }
                    let provider = row.provider.clone();
                    self.push(
                        Some(&provider),
                        Check::warn(
                            Id::PENDING_RESCUE,
                            format!(
                                "account {} has a refreshed login in {} that is not in the vault yet",
                                row.position,
                                path.display()
                            ),
                        )
                        .fix("nothing to do: the account's next refresh or activation adopts it; if it stays, check that the vault can be written"),
                    );
                }
                None => self.push(
                    None,
                    Check::warn(
                        Id::PENDING_RESCUE_ORPHAN,
                        format!("{} belongs to no account of this store", path.display()),
                    )
                    .fix(format!(
                        "`tagteam purge` deletes it with everything else, or delete it: rm {}",
                        quoted(&path)
                    )),
                ),
            }
        }
    }

    /// §13.6 Pending storage, `displaced/` (§6.3): its entries, by provider, and a file with no
    /// row or a row with no file, all `info`. Read through M5a's listing, over the read-only
    /// store.
    pub(super) fn displaced(&mut self, only: Option<&ProviderId>) {
        if matches!(self.stored, Stored::Unusable) {
            return;
        }
        let list = match self.engine.displaced_in(self.store_ref()) {
            Ok(list) => list,
            Err(e) => {
                self.push(
                    None,
                    Check::warn(
                        Id::PENDING_DISPLACED,
                        format!("the displaced credentials cannot be listed: {e}"),
                    )
                    .fix(format!(
                        "make {} a directory this user can read, then `tagteam doctor` again",
                        quoted(&self.engine.env.data_dir().join("displaced"))
                    )),
                );
                return;
            }
        };
        let mut counts: Vec<(ProviderId, usize)> = Vec::new();
        for e in &list.entries {
            if let Some(p) = &e.provider {
                if only.is_some_and(|o| o != p) {
                    continue;
                }
                match counts.iter_mut().find(|(q, _)| q == p) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((p.clone(), 1)),
                }
            }
        }
        for (p, n) in counts {
            self.push(
                Some(&p),
                Check::info(
                    Id::PENDING_DISPLACED,
                    format!(
                        "{n} displaced credential(s) in {}: logins a command overwrote, kept for a manual restore",
                        list.dir.display()
                    ),
                )
                .fix("`tagteam displaced` lists them; `tagteam displaced --purge ID` deletes one"),
            );
        }
        for e in &list.entries {
            if only.is_some_and(|o| e.provider.as_ref().is_some_and(|p| p != o)) {
                continue;
            }
            let file = list.dir.join(format!("{}.json", e.id));
            if !e.recorded {
                self.push(
                    None,
                    Check::info(
                        Id::PENDING_DISPLACED_UNRECORDED,
                        format!("{} has no row in the store", file.display()),
                    )
                    .fix(format!("`tagteam displaced --purge {}` deletes it", e.id)),
                );
            } else if !e.file_present {
                self.push(
                    e.provider.as_ref(),
                    Check::info(
                        Id::PENDING_DISPLACED_MISSING,
                        format!("the displaced entry {} has a row but no file", e.id),
                    )
                    .fix(format!(
                        "`tagteam displaced --purge {}` deletes the row",
                        e.id
                    )),
                );
            }
        }
    }

    /// §13.6 Interrupted switch, for `p`: each journal row, by its holder's liveness (§12.6).
    /// A dead holder's row warns when §9.6 can decide it from fingerprints alone, and fails
    /// when it cannot. Doctor recovers nothing, and asks no oracle.
    pub(super) fn journal(&mut self, p: &dyn Provider) {
        let Some(store) = self.store_ref() else {
            return;
        };
        let id = p.id();
        let rows: Vec<JournalRow> = match store.journals() {
            Ok(rows) => rows.into_iter().filter(|r| r.provider == id).collect(),
            Err(e) => {
                self.push(
                    Some(&id),
                    warn_restore(
                        self.engine,
                        Id::SWITCH_INTERRUPTED,
                        format!("the switch journal cannot be read: {e}"),
                    ),
                );
                return;
            }
        };
        let mut found = Vec::new();
        for row in &rows {
            let position = store
                .account(&row.to_id)
                .ok()
                .flatten()
                .map_or_else(|| row.to_id.to_string(), |a| a.position.to_string());
            let force = command(self.engine, &id, &format!("switch {position} --force"));
            let holder = format!("pid {}, started {}", row.holder.pid, row.holder.start);
            found.push(match row.holder.liveness() {
                Liveness::Live => Check::info(
                    Id::SWITCH_INTERRUPTED,
                    format!("a switch to account {position} is under way ({holder})"),
                ),
                Liveness::Unknown(e) => Check::warn(
                    Id::SWITCH_INTERRUPTED,
                    format!(
                        "a switch to account {position} is journaled by {holder}, which reads as live but may be another user's recycled pid ({e})"
                    ),
                )
                .fix(format!("if no tagteam process is running, `{force}`")),
                Liveness::Dead => self.dead_switch(p, store, row, &position, &force),
            });
        }
        if rows.is_empty() {
            found.push(Check::ok(
                Id::SWITCH_INTERRUPTED,
                "no switch was interrupted",
            ));
        }
        for check in found {
            self.push(Some(&id), check);
        }
    }

    /// §9.6's table, from the live login's fingerprints alone, for a row whose holder died.
    fn dead_switch(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &JournalRow,
        position: &str,
        force: &str,
    ) -> Check {
        // `switch N` always takes `MutationGuard`, so it recovers the row first (§9.6), then
        // makes the switch that was being made; `list` takes it only when the live account's
        // usage is due.
        let retry = command(self.engine, &p.id(), &format!("switch {position}"));
        if self.provider_secrets(p) != Secrets::Readable {
            return Check::warn(
                Id::SWITCH_INTERRUPTED,
                format!(
                    "a switch to account {position} was interrupted; whether it can be recovered cannot be read while the Keychain is locked"
                ),
            )
            .fix(format!("unlock the Keychain; `{retry}` then recovers it"));
        }
        let live = p.read_live_auth(&self.engine.env);
        // §13.6: a live login that cannot be read is an input that cannot be read, never an
        // undecidable switch, which `direction` would make of it.
        if let Err(e) = crate::switch::refuse_unsafe_live_reads(&live) {
            return Check::warn(
                Id::SWITCH_INTERRUPTED,
                format!(
                    "a switch to account {position} was interrupted, and the live login cannot be read to judge it: {e}"
                ),
            )
            .fix(format!(
                "make the live login readable again; `{retry}` then recovers it"
            ));
        }
        match self.engine.direction(p, store, row, &live, &[]) {
            Ok(Direction::Undecidable) => Check::fail(
                Id::SWITCH_INTERRUPTED,
                format!(
                    "a switch to account {position} was interrupted, and the live login cannot tell whether it landed: commands that change accounts refuse until it is settled"
                ),
            )
            .fix(format!("`{force}` settles it by activating the account you name")),
            Ok(_) => Check::warn(
                Id::SWITCH_INTERRUPTED,
                format!("a switch to account {position} was interrupted"),
            )
            .fix(format!(
                "`{retry}` recovers it, then makes the switch it was making"
            )),
            Err(e) => Check::warn(
                Id::SWITCH_INTERRUPTED,
                format!("a switch to account {position} was interrupted, and cannot be judged: {e}"),
            )
            .fix(format!("`{retry}` recovers it if it can")),
        }
    }
}
