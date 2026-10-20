use tagteam_core::ProviderId;
use tagteam_engine::EngineError;
use tagteam_engine::doctor::DoctorOptions;
use tagteam_engine::purge::PurgePlan;
use tagteam_engine::settings;

use crate::cli::ConfigAction;
use crate::{config_cmd, doctor_cmd, purge_cmd};

use crate::app::context::fallback_provider;
use crate::app::failure::{EXIT_ERROR, Failure, KIND_NEEDS_CONFIRMATION, cancelled};
use crate::app::{App, display_name};

const PURGE_DATA_NEEDS_YES: &str =
    "purge deletes tagteam's data for good; run it on a terminal to confirm, or pass --yes";

impl App<'_, '_> {
    pub(in crate::app) fn doctor(&mut self, online: bool) -> Result<i32, Failure> {
        let report = self.engine.doctor(DoctorOptions {
            online,
            provider: self.provider_flag.clone(),
            service: Some(self.service()),
        })?;
        let human = doctor_cmd::human(&report, &|p| display_name(&self.engine, p));
        self.print(&human, doctor_cmd::json(&report));
        Ok(if report.ok() { 0 } else { EXIT_ERROR })
    }

    /// §6.4's reads. None touches the store or the Keychain, and none creates anything (§5).
    /// `list` and `get` show a read of their own, so they print that read's warnings, less
    /// those `run` already printed from the engine's read: the two agree unless the file
    /// changed in between, and then the warnings match what is shown.
    pub(in crate::app) fn config(&mut self, action: ConfigAction) -> Result<(), Failure> {
        match action {
            ConfigAction::List => {
                let provider = self.provider();
                let inspection = settings::inspect(
                    self.engine.env(),
                    &provider,
                    &fallback_provider(self.engine.registry()),
                );
                self.read_warnings(&inspection.warnings);
                self.print(
                    &config_cmd::list_human(&inspection),
                    config_cmd::list_json(&inspection, &provider),
                );
            }
            ConfigAction::Get { key } => {
                let resolved = settings::resolve(&key, self.provider_flag.as_ref())
                    .map_err(EngineError::from)?;
                // A provider named in the key is checked as `--provider` is.
                let provider = match resolved.provider {
                    Some(p) => {
                        self.engine.provider(&p)?;
                        p
                    }
                    None => self.provider(),
                };
                let inspection = settings::inspect(
                    self.engine.env(),
                    &provider,
                    &fallback_provider(self.engine.registry()),
                );
                self.read_warnings(&inspection.warnings);
                let state = inspection
                    .keys
                    .iter()
                    .find(|s| s.key.name == resolved.key.name)
                    .expect("inspect reports every registry key");
                self.print(
                    &config_cmd::get_human(state),
                    config_cmd::get_json(state, &provider),
                );
            }
            ConfigAction::Set { key, value } => {
                let change = self
                    .engine
                    .config_set(&key, self.provider_flag.as_ref(), &value)?;
                self.print(
                    &config_cmd::change_human(&change),
                    config_cmd::change_json(&change),
                );
            }
            ConfigAction::Unset { key } => {
                let change = self
                    .engine
                    .config_unset(&key, self.provider_flag.as_ref())?;
                self.print(
                    &config_cmd::change_human(&change),
                    config_cmd::change_json(&change),
                );
            }
            ConfigAction::Path => {
                let path = settings::config_path(self.engine.env());
                let exists = path.exists();
                self.print(
                    &config_cmd::path_human(&path),
                    config_cmd::path_json(&path, exists),
                );
            }
        }
        Ok(())
    }

    /// Prints the warnings of a read `run` did not make (§6.4), less those `run` printed.
    fn read_warnings(&mut self, warnings: &[String]) {
        for w in warnings
            .iter()
            .filter(|w| !self.settings_warnings.contains(w))
        {
            self.io.warn(w);
        }
    }

    /// §10.5: the plan, its summary and a person's yes (or `--yes`), then the purge. The
    /// question comes before any lock is taken, so none is held while the user reads. Without a
    /// terminal, and under `--json`, `--yes` is required. `run_command` has run the usage check,
    /// the run shell's refusal and the Keychain check. `--keychain-orphans` reaches only a
    /// Keychain: on Linux, whose `vault/` a full purge deletes anyway, it asks for nothing more.
    /// Exits 1 if anything could not be deleted, after reporting what was.
    pub(in crate::app) fn purge(
        &mut self,
        yes: bool,
        keychain_orphans: bool,
    ) -> Result<i32, Failure> {
        let plan = PurgePlan {
            keychain_orphans: keychain_orphans && self.keychain.is_some(),
            ..self.engine.purge_plan(self.provider_flag.as_ref())?
        };
        if !yes {
            if !self.can_prompt() {
                return Err(Failure::Message(
                    KIND_NEEDS_CONFIRMATION,
                    PURGE_DATA_NEEDS_YES.into(),
                ));
            }
            let _ = write!(self.io.err, "{}", purge_cmd::summary(&plan));
            let go = self.io.prompter.confirm(purge_cmd::QUESTION, false);
            self.after_prompt()?;
            if !go {
                return Err(cancelled());
            }
        }
        let report = self.engine.purge(&plan)?;
        for w in &report.warnings {
            self.io.warn(w);
        }
        if !self.json {
            for (what, message) in &report.failures {
                let _ = writeln!(self.io.err, "tagteam: could not delete {what}: {message}");
            }
        }
        let provider = plan.provider.as_ref().map(ProviderId::as_str);
        self.print(
            &purge_cmd::human(&report, plan.store_and_log),
            purge_cmd::json(&report, provider),
        );
        Ok(if report.failures.is_empty() {
            0
        } else {
            EXIT_ERROR
        })
    }
}
