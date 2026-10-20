use std::collections::BTreeSet;
use std::path::PathBuf;

use tagteam_core::autoswitch::Strategy;
use tagteam_core::fmt::span;
use tagteam_core::trust::is_future_stamped;
use tagteam_core::usage::WindowKind;
use tagteam_provider::doctor::quoted;
use tagteam_provider::{Check, Liveness, Provider, Read};

use crate::auto::{engine_lock_path, read_holder};
use crate::settings::Settings;
use crate::store::{
    AccountRow, Store, backoff_holds, backoff_is_skewed, plan_is_skewed, replacing_meta_parses,
};
use crate::vault::SERVICE;

use super::files::{entries, readable};
use super::store::restore;
use super::{Run, Secrets, command};

/// `login_expires_at` this close is reported (§13.6).
const EXPIRY_NOTICE_MS: i64 = 7 * 86_400_000;

/// `auto.engine` for an engine lock record naming `pid`: an undetermined liveness is an input
/// doctor could not read, so a warning, as for `switch.interrupted`. `daemon`
/// means the record matches `daemon.lock`'s, so the engine is the daemon's (D10).
fn engine_record_check(pid: u32, liveness: Liveness, daemon: bool) -> Check {
    let who = if daemon {
        format!("the daemon (pid {pid})")
    } else {
        format!("pid {pid}")
    };
    match liveness {
        Liveness::Live if daemon => {
            Check::info("auto.engine", format!("auto-switch runs in {who}"))
        }
        Liveness::Live => Check::info("auto.engine", format!("auto-switch runs as {who}")),
        Liveness::Dead => Check::ok(
            "auto.engine",
            format!("auto-switch is not running (its last engine, {who}, has exited)"),
        ),
        Liveness::Unknown(e) => Check::warn(
            "auto.engine",
            format!(
                "whether auto-switch runs as {who} cannot be told: the process's start time cannot be read ({e})"
            ),
        )
        .fix(format!(
            "check whether pid {pid} is still running a `tagteam {}`, then `tagteam doctor` again",
            if daemon { "daemon" } else { "auto" }
        )),
    }
}

/// §7.4's quarantine reasons, in words.
fn quarantine_words(reason: &str) -> &'static str {
    match reason {
        "invalid_grant" => "the token endpoint refused its refresh token",
        "no_refresh_token" => "it has no refresh token to renew it with",
        "identity_conflict" => "the token endpoint named another account",
        "successor_lost" => "a refreshed login could not be stored anywhere",
        _ => "it cannot be refreshed",
    }
}

impl Run<'_> {
    /// §13.6 Accounts, orphaned vault items: on Linux a `vault/` file naming no account; on
    /// macOS any item of service `tagteam` while the store has no account, by an attributes-only
    /// probe that never prompts. Every data directory on a Mac shares that service, so such
    /// items may be another data directory's.
    pub(super) fn vault_orphans(&mut self) {
        let Some(accounts) = self.all_accounts() else {
            return;
        };
        let ids: BTreeSet<&str> = accounts.iter().map(|r| r.id.as_str()).collect();
        if let Some(dir) = self.engine.vault.dir() {
            let dir = dir.to_path_buf();
            let orphans: Vec<PathBuf> = match entries(&dir) {
                Ok(paths) => paths
                    .into_iter()
                    .filter(|p| {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        let stem = name
                            .strip_suffix(".json")
                            .filter(|_| !name.starts_with('.'));
                        stem.is_some_and(|s| !ids.contains(s.strip_suffix(".prev").unwrap_or(s)))
                    })
                    .collect(),
                Err(e) => {
                    self.push(
                        None,
                        Check::warn(
                            "accounts.orphans",
                            format!(
                                "{} cannot be listed ({}), so whether every vault file names an account cannot be told",
                                dir.display(),
                                e.kind()
                            ),
                        )
                        .fix(readable(&dir)),
                    );
                    return;
                }
            };
            if orphans.is_empty() {
                self.push(
                    None,
                    Check::ok("accounts.orphans", "every vault file names an account"),
                );
            }
            for path in orphans {
                self.push(
                    None,
                    Check::warn(
                        "accounts.orphans",
                        format!("{} names no account of this store", path.display()),
                    )
                    .fix(format!(
                        "rm {}, which no account uses; `tagteam purge` deletes it with everything else",
                        quoted(&path)
                    )),
                );
            }
            return;
        }
        let Some(keychain) = self.engine.vault.keychain() else {
            return;
        };
        if !accounts.is_empty() {
            return;
        }
        let check = match keychain.service_has_items(SERVICE) {
            Read::Present(true) => Check::warn(
                "accounts.orphans",
                format!(
                    "the Keychain holds items of service `{SERVICE}` while this store has no account; they may belong to another tagteam data directory on this Mac"
                ),
            )
            .fix("once no other tagteam data directory uses them, `tagteam purge --keychain-orphans` deletes them"),
            Read::Present(false) | Read::Absent => Check::ok(
                "accounts.orphans",
                format!("the Keychain holds no item of service `{SERVICE}`"),
            ),
            Read::Unreadable(e) => Check::warn(
                "accounts.orphans",
                format!("whether the Keychain holds items of service `{SERVICE}` cannot be told: {e}"),
            )
            .fix("run `tagteam doctor` again; if it persists, `security show-keychain-info` shows the Keychain's state"),
        };
        self.push(None, check);
    }

    /// §13.6 Accounts, for `p`: each vault entry readable, quarantines, a login near its
    /// expiry, pending replacements, and a stale-marked live store.
    pub(super) fn accounts(&mut self, p: &dyn Provider) {
        let Some(store) = self.store_ref() else {
            return;
        };
        let id = p.id();
        let rows = match store.accounts(&id) {
            Ok(rows) => rows,
            Err(e) => {
                let check = Check::warn(
                    "accounts.vault",
                    format!("the store's accounts cannot be read: {e}"),
                )
                .fix(restore(self.engine));
                self.push(Some(&id), check);
                return;
            }
        };
        let mut found = Vec::new();
        let readable = !rows.is_empty() && self.secrets() == Secrets::Readable;
        if readable {
            let mut broken = 0;
            for row in &rows {
                let remove = command(self.engine, &id, &format!("remove {}", row.position));
                match self.engine.vault.read(&row.id) {
                    Read::Present(_) => {}
                    Read::Absent => {
                        broken += 1;
                        found.push(
                            Check::fail(
                                "accounts.vault",
                                format!(
                                    "account {} has no vault entry, as a purge that stopped part way leaves one; it can never be switched to, refreshed or launched",
                                    row.position
                                ),
                            )
                            .fix(format!("`{remove}` finishes deleting it")),
                        );
                    }
                    Read::Unreadable(e) => {
                        broken += 1;
                        found.push(
                            Check::fail(
                                "accounts.vault",
                                format!("account {}'s vault entry cannot be read: {e}", row.position),
                            )
                            .fix(format!(
                                "make the entry readable again; if it is lost, `{remove}` and add the account again"
                            )),
                        );
                    }
                }
            }
            if broken == 0 && !rows.is_empty() {
                found.push(Check::ok(
                    "accounts.vault",
                    format!("the vault holds every account's login ({})", rows.len()),
                ));
            }
        }
        let mut stale_unread = None;
        for row in &rows {
            let add = command(
                self.engine,
                &id,
                &format!("add --position {}", row.position),
            );
            if let Some(reason) = &row.quarantine_reason {
                found.push(
                    Check::warn(
                        "accounts.quarantined",
                        format!(
                            "account {} is quarantined ({reason}): {}",
                            row.position,
                            quarantine_words(reason)
                        ),
                    )
                    .fix(format!(
                        "log in as that account with `{}`, then `{add}`",
                        p.launch_command()
                    )),
                );
            }
            if let Some(at) = row.login_expires_at {
                let left = at - self.now_ms;
                if left <= EXPIRY_NOTICE_MS {
                    let when = if left > 0 {
                        format!("expires in {}", span(left / 1000))
                    } else {
                        format!("expired {} ago", span(-left / 1000))
                    };
                    found.push(
                        Check::warn(
                            "accounts.login-expiry",
                            format!("account {}'s login {when}", row.position),
                        )
                        .fix(format!(
                            "log in as that account with `{}`, then `{add}`",
                            p.launch_command()
                        )),
                    );
                }
            }
            if let Some(fp) = &row.replacing_fp {
                found.push(self.replacement(p, store, row, fp));
            }
            match store.live_store_stale(row) {
                Ok(true) => found.push(
                    Check::warn(
                        "accounts.live-stale",
                        format!(
                            "{} still runs the login an explicit command replaced for account {}",
                            p.display_name(),
                            row.position
                        ),
                    )
                    .fix(format!(
                        "`{}` activates the new login",
                        command(
                            self.engine,
                            &id,
                            &format!("switch {} --force", row.position)
                        )
                    )),
                ),
                Ok(false) => {}
                Err(e) => {
                    stale_unread.get_or_insert(e);
                }
            }
        }
        if let Some(e) = stale_unread {
            found.push(
                Check::warn(
                    "accounts.live-stale",
                    format!(
                        "whether {} still runs a login an explicit command replaced cannot be told: the active account cannot be read ({e})",
                        p.display_name()
                    ),
                )
                .fix(restore(self.engine)),
            );
        }
        for check in found {
            self.push(Some(&id), check);
        }
    }

    /// §12.5: a replacement whose replacer died. When its vault write landed but its recorded
    /// metadata cannot be parsed, every lock holder but `remove` and `purge` refuses the
    /// account (`replacement-unreadable`), which fails; otherwise the next lock holder settles
    /// it. Nothing is reconciled here.
    fn replacement(&self, p: &dyn Provider, store: &Store, row: &AccountRow, fp: &str) -> Check {
        let id = p.id();
        let remove = command(self.engine, &id, &format!("remove {}", row.position));
        // A NULL `replacing_meta` is cleared, never refused (§12.5): only recorded
        // details that do not parse block. A column that cannot be read is a check whose input
        // cannot be read (§13.6).
        let parses = match store.replacing_meta(&row.id) {
            Ok(meta) => meta.is_none_or(|m| replacing_meta_parses(&m)),
            Err(e) => {
                return Check::warn(
                    "accounts.replacement",
                    format!(
                        "a replacement of account {} was interrupted, and the login details recorded with it cannot be read from the store ({e})",
                        row.position
                    ),
                )
                .fix(format!(
                    "run `tagteam doctor` again; if it reached the vault and the details stay unreadable, `{remove}` is the way out"
                ));
            }
        };
        let landed = match self.secrets() {
            Secrets::Readable => match self.engine.vault.read(&row.id) {
                Read::Present(b) => Some(p.fingerprint(&b).is_some_and(|f| f.as_str() == fp)),
                Read::Absent => Some(false),
                Read::Unreadable(_) => None,
            },
            Secrets::Locked | Secrets::Unknown => None,
        };
        match (landed, parses) {
            (Some(true), false) => Check::fail(
                "accounts.replacement",
                format!(
                    "account {}'s replacement reached the vault, but the login details recorded with it cannot be read, so every command but `remove` and `purge` refuses the account",
                    row.position
                ),
            )
            .fix(format!("`{remove}`, then add the account again")),
            (None, false) => Check::warn(
                "accounts.replacement",
                format!(
                    "a replacement of account {} was interrupted and the login details recorded with it cannot be read; whether it reached the vault cannot be read now",
                    row.position
                ),
            )
            .fix(format!(
                "run `tagteam doctor` again once the vault can be read; if it reached the vault, `{remove}` is the way out"
            )),
            _ => Check::warn(
                "accounts.replacement",
                format!("a replacement of account {} was interrupted", row.position),
            )
            .fix(format!(
                "nothing to do: the next command that refreshes, switches to or adds account {} finishes or undoes it",
                row.position
            )),
        }
    }

    /// §13.6 Usage, for `p`: backoff, the hourly budget (§8.6), and stamps a skewed clock left
    /// (§8.4).
    pub(super) fn usage(&mut self, p: &dyn Provider) {
        let Some(store) = self.store_ref() else {
            return;
        };
        let id = p.id();
        // `accounts.vault` reports the same read when it fails.
        let Ok(rows) = store.accounts(&id) else {
            return;
        };
        let now_s = self.now_ms.div_euclid(1000);
        let budget = p.poll_budget();
        let mut found = Vec::new();
        let mut identities = BTreeSet::new();
        for row in &rows {
            if identities.insert(row.identity_key.clone()) {
                let since = now_s - budget.count_window_s;
                match store.usage_request_count(&id, &row.identity_key, since) {
                    Ok(n) if n >= budget.hourly_requests => found.push(
                        Check::warn(
                            "usage.budget",
                            format!(
                                "account {} has used all {} usage requests of the past hour",
                                row.position, budget.hourly_requests
                            ),
                        )
                        .fix("wait: tagteam sends no more until the oldest leaves the hour; a second `tagteam auto`, or another tool polling the account, uses it up"),
                    ),
                    Ok(_) => {}
                    Err(e) => found.push(
                        Check::warn(
                            "usage.budget",
                            format!(
                                "account {}'s usage requests of the past hour cannot be counted: {e}",
                                row.position
                            ),
                        )
                        .fix(restore(self.engine)),
                    ),
                }
            }
            let state = match store.usage_state(&row.id) {
                Ok(Some(state)) => state,
                Ok(None) => continue,
                Err(e) => {
                    found.push(
                        Check::warn(
                            "usage.state",
                            format!("account {}'s usage state cannot be read: {e}", row.position),
                        )
                        .fix(restore(self.engine)),
                    );
                    continue;
                }
            };
            if let Some(until) = state
                .backoff_until
                .filter(|u| backoff_holds(Some(*u), now_s))
            {
                found.push(
                    Check::info(
                        "usage.backoff",
                        format!(
                            "account {}'s usage is backed off after {}; the next try is in {}",
                            row.position,
                            state.last_error.as_deref().unwrap_or("a failure"),
                            span(until - now_s)
                        ),
                    )
                    .fix("nothing to do: tagteam tries again then"),
                );
            }
            let skewed: Vec<(&str, i64)> = [
                state
                    .fetched_at
                    .filter(|t| is_future_stamped(*t, now_s))
                    .map(|t| ("reading", t)),
                state
                    .next_poll_at
                    .filter(|t| plan_is_skewed(*t, now_s, &budget))
                    .map(|t| ("poll plan", t)),
                state
                    .backoff_until
                    .filter(|t| backoff_is_skewed(*t, now_s))
                    .map(|t| ("backoff", t)),
            ]
            .into_iter()
            .flatten()
            .collect();
            for (what, at) in skewed {
                found.push(
                    Check::warn(
                        "usage.clock-skew",
                        format!(
                            "account {}'s {what} is stamped {} ahead of now: the clock is, or was, skewed",
                            row.position,
                            span(at - now_s)
                        ),
                    )
                    .fix("check the system clock; tagteam ignores the stamp, and the next fetch replaces it"),
                );
            }
        }
        if found.is_empty() && !rows.is_empty() {
            found.push(Check::ok(
                "usage.state",
                "no account is backed off, over its budget or stamped by a skewed clock",
            ));
        }
        for check in found {
            self.push(Some(&id), check);
        }
    }

    /// §13.6 Auto-switch, for `p`: whether an engine runs, by its lock record and an exact pid
    /// and start-time match (§11.1); unhealthy ticks; and the settings a tick would warn about.
    pub(super) fn auto(&mut self, p: &dyn Provider) {
        let id = p.id();
        let lock = engine_lock_path(&self.engine.env, &id);
        let running = match read_holder(&lock) {
            Read::Absent => Check::ok("auto.engine", "auto-switch is not running"),
            Read::Present(stamp) => {
                engine_record_check(stamp.pid, stamp.liveness(), self.held_by_daemon(&stamp))
            }
            Read::Unreadable(e) => Check::warn(
                "auto.engine",
                format!("whether auto-switch runs cannot be told: {e}"),
            )
            .fix(readable(&lock)),
        };
        self.push(Some(&id), running);
        if let Some(store) = self.store_ref() {
            let unhealthy = match store.autoswitch_state(&id) {
                Ok(state) if state.unhealthy_ticks > 0 => Some(
                    Check::warn(
                        "auto.unhealthy",
                        format!(
                            "the last {} auto-switch tick(s) could not read the live account's usage",
                            state.unhealthy_ticks
                        ),
                    )
                    .fix("`tagteam list` shows why the live account's usage cannot be read"),
                ),
                Ok(_) => None,
                Err(e) => Some(
                    Check::warn(
                        "auto.unhealthy",
                        format!("the auto-switch state cannot be read: {e}"),
                    )
                    .fix(restore(self.engine)),
                ),
            };
            if let Some(check) = unhealthy {
                self.push(Some(&id), check);
            }
        }
        let (settings, _) = Settings::load(&self.engine.env, &id);
        if settings.strategy == Strategy::ConsumeFirst && p.primary_long_window().is_none() {
            self.push(
                Some(&id),
                Check::warn(
                    "auto.strategy",
                    format!(
                        "autoswitch.strategy is consume-first, but {} has no long usage window to rank by, so auto-switch runs best",
                        p.display_name()
                    ),
                )
                .fix(format!(
                    "`tagteam config set provider.{id}.autoswitch.strategy best`"
                )),
            );
        }
        self.models(p, &settings);
    }

    /// §11.2 step 3's model check, as the tick's `config-warning` makes it: each
    /// `autoswitch.models` name should be a scoped window some account's last reading reports.
    /// Nothing is said until some account has a reading.
    fn models(&mut self, p: &dyn Provider, settings: &Settings) {
        let Some(store) = self.store_ref() else {
            return;
        };
        let id = p.id();
        let names: Vec<&String> = settings
            .models
            .iter()
            .filter(|m| !m.eq_ignore_ascii_case("all"))
            .collect();
        if names.is_empty() {
            return;
        }
        let (mut read_any, mut scoped, mut unread) = (false, BTreeSet::new(), None);
        // The accounts `accounts.vault` read: it reports them when they cannot be read.
        for row in store.accounts(&id).unwrap_or_default() {
            match store.usage_state(&row.id) {
                Ok(state) => {
                    if let Some(windows) = state.and_then(|s| s.last_good) {
                        read_any = true;
                        scoped.extend(
                            windows
                                .iter()
                                .filter(|w| w.kind == WindowKind::Scoped)
                                .map(|w| w.label.to_lowercase()),
                        );
                    }
                }
                Err(e) => {
                    unread = Some((row.position, e));
                    break;
                }
            }
        }
        if let Some((n, e)) = unread {
            let check = Check::warn(
                "auto.models",
                format!(
                    "whether autoswitch.models names a model no account's usage reports cannot be told: account {n}'s usage state cannot be read ({e})"
                ),
            )
            .fix(restore(self.engine));
            self.push(Some(&id), check);
            return;
        }
        if !read_any {
            return;
        }
        let missing: Vec<String> = names
            .into_iter()
            .filter(|n| !scoped.contains(&n.to_lowercase()))
            .cloned()
            .collect();
        for name in missing {
            self.push(
                Some(&id),
                Check::warn(
                    "auto.models",
                    format!(
                        "autoswitch.models names {name:?}, but no account's usage reports a window for that model"
                    ),
                )
                .fix("check the name against `tagteam list`, then `tagteam config set autoswitch.models …`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tagteam_provider::CheckStatus;

    #[test]
    fn every_quarantine_reason_has_its_words() {
        for reason in [
            "invalid_grant",
            "no_refresh_token",
            "identity_conflict",
            "successor_lost",
        ] {
            assert_ne!(
                quarantine_words(reason),
                quarantine_words("other"),
                "{reason}"
            );
        }
    }

    #[test]
    fn an_engine_whose_liveness_cannot_be_told_is_a_warning_with_a_fix() {
        let c = engine_record_check(4242, Liveness::Unknown("permission denied".into()), false);
        assert_eq!(c.id, "auto.engine");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(c.message.contains("4242") && c.message.contains("permission denied"));
        assert!(c.fix.as_deref().is_some_and(|f| f.contains("pid 4242")));
        assert_eq!(
            engine_record_check(4242, Liveness::Live, false).status,
            CheckStatus::Info
        );
        assert_eq!(
            engine_record_check(4242, Liveness::Dead, false).status,
            CheckStatus::Ok
        );
        // D10: a record that is daemon.lock's names the daemon.
        assert_eq!(
            engine_record_check(4242, Liveness::Live, true).message,
            "auto-switch runs in the daemon (pid 4242)"
        );
        let c = engine_record_check(4242, Liveness::Unknown("permission denied".into()), true);
        assert_eq!(
            c.message,
            "whether auto-switch runs as the daemon (pid 4242) cannot be told: the process's start time cannot be read (permission denied)"
        );
        assert_eq!(
            c.fix.as_deref(),
            Some(
                "check whether pid 4242 is still running a `tagteam daemon`, then `tagteam doctor` again"
            )
        );
    }
}
