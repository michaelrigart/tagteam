//! §12.1: what `tagteam run` launches, decided before any lock is taken. A session for an
//! account goes on to `launch` (§12.5), which decides again under its locks. Plain `claude` is
//! an `exec` with the environment `run` was given: the outer home's, inside a run shell (§12.8).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::Provider;
use tagteam_provider::process::{SpawnSpec, find_on_path};
use tagteam_provider::profile::RunShell;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

#[derive(Debug, Clone)]
/// `account` is resolved by the CLI (§10.4, with its ambiguity prompt) before planning;
/// `None` means the mapping decides (§12.1). `provider` is the global `--provider`.
pub struct RunRequest {
    pub account: Option<AccountId>,
    pub provider: Option<ProviderId>,
    pub require_session: bool,
    pub cwd: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // one value per command, and the contract holds the row by value
pub enum RunPlan {
    /// §12.1's plain `claude`: exec `spec` (outer home restored, nothing scrubbed); `warning`
    /// for a mapping whose account went away (Decision 7).
    Plain {
        spec: SpawnSpec,
        warning: Option<String>,
    },
    /// A session for `account`.
    Session {
        account: AccountRow,
        provider: ProviderId,
        launch: PathBuf,
    },
}

/// What a directory's mapping names (§12.7).
#[allow(clippy::large_enum_variant)] // one value per command
enum Mapped {
    Nothing,
    /// The mapping of `path` names an account that was removed after it was read (Decision 7).
    Gone {
        path: String,
    },
    Account(AccountRow),
}

/// `run` for a provider without sessions (§4.5 "Capabilities are explicit").
pub(crate) fn no_sessions(p: &dyn Provider) -> EngineError {
    EngineError::InvalidInput(format!(
        "{} has no `tagteam run` sessions",
        p.display_name()
    ))
}

/// The variables to set and to remove on top of the inherited environment.
type EnvChanges = (Vec<(OsString, OsString)>, Vec<OsString>);

impl Engine {
    /// §12.1. It only reads: no lock, no write, and with no store none is created (§5).
    ///
    /// The named account is resolved first, since its provider is the one whose launch command
    /// runs. That command is looked up next, on the `PATH` the CLI captured into `Env.vars`, before
    /// any mapping or live login is read, so a missing one changes nothing. Then the target: the
    /// named account, or the nearest mapping of `cwd`'s canonical path for the provider. An
    /// API-key account is refused. Plain `claude` runs when nothing is mapped, when the mapped
    /// account went away (with a warning), or when the target is the default home's live login;
    /// under `--require-session` each of those refuses instead.
    pub fn plan_run(&self, req: &RunRequest) -> Result<RunPlan, EngineError> {
        // §12.8: under an unreadable marker the outer home is unknown, so nothing is planned
        // against the engine's own `env`, which is still the run shell's.
        self.refuse_unreadable_run_shell()?;
        let named = match &req.account {
            Some(id) => Some(self.named_account(id)?),
            None => None,
        };
        let provider = match (&named, &req.provider) {
            (Some(row), Some(asked)) if &row.provider != asked => {
                return Err(EngineError::InvalidInput(format!(
                    "position {} is a {} account, not a {asked} one",
                    row.position, row.provider
                )));
            }
            (Some(row), _) => row.provider.clone(),
            (None, Some(asked)) => asked.clone(),
            (None, None) => self.default_provider.clone(),
        };
        let p = self.provider(&provider)?;
        let p = p.as_ref();
        if !p.capabilities().sessions {
            return Err(no_sessions(p));
        }
        let command = p.launch_command();
        let launch = find_on_path(command, self.env.var("PATH")).ok_or_else(|| {
            EngineError::LaunchCommandMissing {
                command: command.to_owned(),
            }
        })?;
        let target = match named {
            Some(row) => row,
            None => match self.mapped_account(&provider, &req.cwd)? {
                Mapped::Account(row) => row,
                Mapped::Nothing => {
                    let why = format!("no mapping applies to {}", req.cwd.display());
                    return self.plain(req, launch, why, None);
                }
                Mapped::Gone { path } => {
                    let why = format!("the account mapped to {path} was removed");
                    let warning = format!("{why}; running plain {command}");
                    return self.plain(req, launch, why, Some(warning));
                }
            },
        };
        if p.kind_traits(&target.kind).managed_key_axis {
            return Err(EngineError::ApiKeyAccount {
                position: target.position,
            });
        }
        if self.is_live_login(p, &target)? {
            let why = format!("position {} is the live login", target.position);
            return self.plain(req, launch, why, None);
        }
        Ok(RunPlan::Session {
            account: target,
            provider,
            launch,
        })
    }

    /// §12.1: whether `row` is the default home's live login, so that a session would give its
    /// rotating token a second copy. The live login is the default home's inside a run shell
    /// too (§12.8). An unreadable live identity is an error, never taken for another login.
    pub(crate) fn is_live_login(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<bool, EngineError> {
        Ok(self
            .read_live_identity(p)?
            .is_some_and(|i| p.identity_key(&i).as_str() == row.identity_key))
    }

    /// The account the CLI resolved. With no store there is none, and none is created (§5).
    fn named_account(&self, id: &AccountId) -> Result<AccountRow, EngineError> {
        let row = match self.existing_store()? {
            Some(store) => store.account(id)?,
            None => None,
        };
        row.ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))
    }

    /// §12.7: the mapping of `cwd`'s canonical path, or of its nearest mapped ancestor, for
    /// `provider`. `map` stores `fs::canonicalize`'s result rebuilt from its components, and
    /// `nearest_mapping` rebuilds the same way, so a `cwd` reached through a symlink, a `.` or a
    /// trailing `/` finds what `map` stored. A directory that cannot be resolved is looked up as
    /// given.
    fn mapped_account(&self, provider: &ProviderId, cwd: &Path) -> Result<Mapped, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(Mapped::Nothing);
        };
        let dir = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let Some(mapping) = store.nearest_mapping(&dir, provider)? else {
            return Ok(Mapped::Nothing);
        };
        // Decision 7: a mapping goes with its account (ON DELETE CASCADE), so a row missing
        // now was removed between the two reads.
        Ok(match store.account(&mapping.account_id)? {
            Some(row) => Mapped::Account(row),
            None => Mapped::Gone { path: mapping.path },
        })
    }

    /// §12.1's plain `claude`, or `--require-session`'s refusal of it, naming `why`.
    fn plain(
        &self,
        req: &RunRequest,
        launch: PathBuf,
        why: String,
        warning: Option<String>,
    ) -> Result<RunPlan, EngineError> {
        if req.require_session {
            return Err(EngineError::RequiresSession { why });
        }
        let (set, remove) = self.outer_home_vars()?;
        Ok(RunPlan::Plain {
            spec: SpawnSpec {
                program: launch,
                args: req.args.clone(),
                set,
                remove,
                cwd: None,
            },
            warning,
        })
    }

    /// The variables that give plain `claude` the outer home back inside a run shell (§12.1,
    /// §12.8): the marker's `outer` record, whose keys are the marker provider's home variables
    /// (§4.5 `outer_home`). A string is set, and `null` (undefined) is removed. A
    /// defined-but-empty session variable is removed too, since tagteam treats it as unset and
    /// never exports one (Appendix A.1); every other value is restored as recorded, `""`
    /// included. Nothing is scrubbed (§12.5). Outside a run shell, nothing changes.
    fn outer_home_vars(&self) -> Result<EnvChanges, EngineError> {
        let marker = match self.run_shell() {
            RunShell::Outside => return Ok((Vec::new(), Vec::new())),
            RunShell::Inside { marker, .. } => marker,
            RunShell::Unreadable { marker, detail } => {
                return Err(EngineError::RunShellUnreadable {
                    marker: marker.clone(),
                    detail: detail.clone(),
                });
            }
        };
        let session_var = self.provider(&marker.provider)?.session_dir_var();
        let (mut set, mut remove) = (Vec::new(), Vec::new());
        if let Value::Object(vars) = &marker.outer {
            for (name, value) in vars {
                match value {
                    Value::String(s) if !(s.is_empty() && session_var == Some(name.as_str())) => {
                        set.push((OsString::from(name), OsString::from(s)));
                    }
                    _ => remove.push(OsString::from(name)),
                }
            }
        }
        Ok((set, remove))
    }
}
