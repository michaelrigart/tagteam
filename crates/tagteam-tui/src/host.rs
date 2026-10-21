//! The action port (TUI spec §4.5). The `tagteam` binary implements `Host` over the CLI's own
//! dispatch, so every action says and does what the same command does on the command line.

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::switch::UsageStrategy;

/// The selected account as the TUI last read it: the position names it to the CLI, and the ID
/// guards the position (`--expect-account`, core §10.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    pub id: AccountId,
    pub provider: ProviderId,
    pub position: u32,
}

/// `switch --strategy`'s values (core §9.3). Not `UsageStrategy::as_str`: that names
/// `switch --json`'s.
fn strategy_arg(strategy: UsageStrategy) -> &'static str {
    match strategy {
        UsageStrategy::Best => "best",
        UsageStrategy::NextAvailable => "next-available",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Switch(AccountRef),
    SwitchBy {
        provider: ProviderId,
        strategy: UsageStrategy,
    },
    Enable(AccountRef),
    Disable(AccountRef),
    SetAlias {
        account: AccountRef,
        alias: String,
    },
    ClearAlias(AccountRef),
    Move {
        account: AccountRef,
        position: u32,
    },
    Remove(AccountRef),
    AddLive {
        provider: ProviderId,
    },
    Run(AccountRef),
    Doctor {
        provider: Option<ProviderId>,
        online: bool,
    },
    /// The daemon's auto-switching for `provider`, on or paused (spec §7.4): its setting
    /// `daemon.autoswitch` (D7).
    Autoswitch {
        provider: ProviderId,
        on: bool,
    },
}

impl Action {
    /// The arguments after `tagteam`, for the CLI's own parser. An account is named by
    /// `--provider P --expect-account ID` and its position, so a `move` made elsewhere since the
    /// TUI last read cannot redirect the action to another account (spec §4.5). Every action but
    /// `Run`, whose output is the agent's own, asks for plain text.
    pub fn args(&self) -> Vec<String> {
        let account = |a: &AccountRef, command: &[&str]| -> Vec<String> {
            let mut args = vec![
                "--no-color".to_owned(),
                "--provider".to_owned(),
                a.provider.to_string(),
                "--expect-account".to_owned(),
                a.id.to_string(),
            ];
            args.extend(command.iter().map(|s| (*s).to_owned()));
            args
        };
        match self {
            Action::Switch(a) => {
                let n = a.position.to_string();
                account(a, &["switch", &n])
            }
            Action::SwitchBy { provider, strategy } => vec![
                "--no-color".into(),
                "--provider".into(),
                provider.to_string(),
                "switch".into(),
                "--strategy".into(),
                strategy_arg(*strategy).into(),
            ],
            Action::Enable(a) => account(a, &["enable", &a.position.to_string()]),
            Action::Disable(a) => account(a, &["disable", &a.position.to_string()]),
            Action::SetAlias { account: a, alias } => {
                account(a, &["alias", &a.position.to_string(), "--", alias])
            }
            Action::ClearAlias(a) => account(a, &["alias", &a.position.to_string(), "--unset"]),
            Action::Move {
                account: a,
                position,
            } => account(a, &["move", &a.position.to_string(), &position.to_string()]),
            Action::Remove(a) => account(a, &["remove", &a.position.to_string()]),
            Action::AddLive { provider } => vec![
                "--no-color".into(),
                "--provider".into(),
                provider.to_string(),
                "add".into(),
            ],
            Action::Run(a) => vec![
                "--provider".into(),
                a.provider.to_string(),
                "--expect-account".into(),
                a.id.to_string(),
                "run".into(),
                a.position.to_string(),
            ],
            Action::Doctor { provider, online } => {
                let mut args = vec!["--no-color".to_owned()];
                if let Some(p) = provider {
                    args.extend(["--provider".to_owned(), p.to_string()]);
                }
                args.push("doctor".into());
                if *online {
                    args.push("--online".into());
                }
                args
            }
            Action::Autoswitch { provider, on } => vec![
                "--no-color".into(),
                "config".into(),
                "set".into(),
                format!("provider.{provider}.daemon.autoswitch"),
                on.to_string(),
            ],
        }
    }

    /// The command's name as the CLI spells it: "switch", "enable", "disable", "alias", "move",
    /// "remove", "add", "run", "doctor", "config". The finishing line, the result panel's title
    /// and the outcome toasts use it; the spinner and the busy toast use `label`.
    pub fn name(&self) -> &'static str {
        match self {
            Action::Switch(_) | Action::SwitchBy { .. } => "switch",
            Action::Enable(_) => "enable",
            Action::Disable(_) => "disable",
            Action::SetAlias { .. } | Action::ClearAlias(_) => "alias",
            Action::Move { .. } => "move",
            Action::Remove(_) => "remove",
            Action::AddLive { .. } => "add",
            Action::Run(_) => "run",
            Action::Doctor { .. } => "doctor",
            Action::Autoswitch { .. } => "config",
        }
    }

    /// The UI's words while the action runs (redesign §9.4, T2-17), without the `…`: the
    /// spinner shows them, and the busy toast says `still <label>`. `named` gives an account's
    /// short name (`words::short_name`) and `display` the provider's display name.
    pub fn label(&self, named: &dyn Fn(&AccountRef) -> String, display: &str) -> String {
        match self {
            Action::Switch(a) => format!("switching to {}", named(a)),
            Action::SwitchBy {
                strategy: UsageStrategy::Best,
                ..
            } => "switching to the best account".into(),
            Action::SwitchBy {
                strategy: UsageStrategy::NextAvailable,
                ..
            } => "switching to the next account".into(),
            Action::Enable(a) => format!("enabling {}", named(a)),
            Action::Disable(a) => format!("disabling {}", named(a)),
            Action::SetAlias { .. } | Action::ClearAlias(_) => "setting the alias".into(),
            Action::Move { account, .. } => format!("moving {}", named(account)),
            Action::Remove(a) => format!("removing {}", named(a)),
            Action::AddLive { .. } => format!("adding {display}'s login"),
            Action::Run(a) => format!("starting {display} as {}", named(a)),
            Action::Doctor { .. } => "running doctor".into(),
            Action::Autoswitch { on: false, .. } => "pausing auto".into(),
            Action::Autoswitch { on: true, .. } => "resuming auto".into(),
        }
    }

    /// `Run` only: it always runs suspended (spec §7.3).
    pub fn needs_terminal(&self) -> bool {
        matches!(self, Action::Run(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Done {
        exit: i32,
        out: String,
        err: String,
    },
    /// The command needs the terminal itself, and changed nothing (spec §4.5).
    NeedsTerminal,
}

/// The questions an action may ask: the CLI's own prompts, answered by the TUI's dialogs.
pub trait Ask {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
}

pub trait Host: Send + Sync {
    /// Runs `action` inside the TUI, on the actions lane. Prompts arrive through `ask`.
    fn act(&self, action: &Action, ask: &mut dyn Ask) -> ActionOutcome;
    /// Runs `action` on the real terminal, on the UI thread, after the TUI has suspended. Then
    /// prints `Press Enter to return to tagteam` and waits for Enter, or for a signal.
    fn act_foreground(&self, action: &Action) -> ActionOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(position: u32) -> AccountRef {
        AccountRef {
            id: AccountId::from_string("0192-abc"),
            provider: ProviderId::new("claude-code"),
            position,
        }
    }

    fn provider() -> ProviderId {
        ProviderId::new("claude-code")
    }

    fn args(a: &Action) -> String {
        a.args().join(" ")
    }

    #[test]
    fn each_action_names_its_account_by_provider_id_and_position() {
        let g = "--no-color --provider claude-code --expect-account 0192-abc";
        for (action, tail) in [
            (Action::Switch(account(3)), "switch 3"),
            (Action::Enable(account(3)), "enable 3"),
            (Action::Disable(account(3)), "disable 3"),
            (Action::Remove(account(3)), "remove 3"),
            (Action::ClearAlias(account(3)), "alias 3 --unset"),
            (
                Action::Move {
                    account: account(3),
                    position: 1,
                },
                "move 3 1",
            ),
            (
                Action::SetAlias {
                    account: account(3),
                    alias: "work".into(),
                },
                "alias 3 -- work",
            ),
        ] {
            assert_eq!(args(&action), format!("{g} {tail}"), "{action:?}");
        }
    }

    #[test]
    fn the_actions_that_name_no_account_carry_no_guard() {
        assert_eq!(
            args(&Action::SwitchBy {
                provider: provider(),
                strategy: UsageStrategy::Best
            }),
            "--no-color --provider claude-code switch --strategy best"
        );
        assert_eq!(
            args(&Action::SwitchBy {
                provider: provider(),
                strategy: UsageStrategy::NextAvailable
            }),
            "--no-color --provider claude-code switch --strategy next-available"
        );
        assert_eq!(
            args(&Action::AddLive {
                provider: provider()
            }),
            "--no-color --provider claude-code add"
        );
    }

    #[test]
    fn pausing_and_resuming_auto_switch_is_config_set_of_the_provider_s_daemon_setting() {
        // Spec §7.4, D7: the socket stays read-only; the setting is written as the CLI writes
        // it.
        let set = |on| {
            args(&Action::Autoswitch {
                provider: provider(),
                on,
            })
        };
        assert_eq!(
            set(false),
            "--no-color config set provider.claude-code.daemon.autoswitch false"
        );
        assert_eq!(
            set(true),
            "--no-color config set provider.claude-code.daemon.autoswitch true"
        );
        let pause = Action::Autoswitch {
            provider: provider(),
            on: false,
        };
        assert_eq!(pause.name(), "config");
        assert!(!pause.needs_terminal());
    }

    #[test]
    fn run_keeps_its_colour_because_the_agent_output_is_not_ours() {
        assert_eq!(
            args(&Action::Run(account(2))),
            "--provider claude-code --expect-account 0192-abc run 2"
        );
    }

    #[test]
    fn doctor_names_a_provider_only_when_it_has_one() {
        let doctor = |provider, online| args(&Action::Doctor { provider, online });
        assert_eq!(doctor(None, false), "--no-color doctor");
        assert_eq!(doctor(None, true), "--no-color doctor --online");
        assert_eq!(
            doctor(Some(provider()), false),
            "--no-color --provider claude-code doctor"
        );
        assert_eq!(
            doctor(Some(provider()), true),
            "--no-color --provider claude-code doctor --online"
        );
    }

    #[test]
    fn an_alias_is_one_argument_after_the_double_dash_whatever_it_holds() {
        for alias in ["-work", "--unset", "--", "a b", "", "日本語 😀"] {
            let args = Action::SetAlias {
                account: account(1),
                alias: alias.into(),
            }
            .args();
            let (head, tail) = args.split_at(args.len() - 1);
            assert_eq!(&head[head.len() - 3..], ["alias", "1", "--"], "{alias:?}");
            assert_eq!(tail, [alias], "one argument, untouched");
        }
    }

    #[test]
    fn names_are_the_clis_command_names() {
        let a = || account(1);
        for (action, name) in [
            (Action::Switch(a()), "switch"),
            (
                Action::SwitchBy {
                    provider: provider(),
                    strategy: UsageStrategy::Best,
                },
                "switch",
            ),
            (Action::Enable(a()), "enable"),
            (Action::Disable(a()), "disable"),
            (
                Action::SetAlias {
                    account: a(),
                    alias: "x".into(),
                },
                "alias",
            ),
            (Action::ClearAlias(a()), "alias"),
            (
                Action::Move {
                    account: a(),
                    position: 2,
                },
                "move",
            ),
            (Action::Remove(a()), "remove"),
            (
                Action::AddLive {
                    provider: provider(),
                },
                "add",
            ),
            (Action::Run(a()), "run"),
            (
                Action::Doctor {
                    provider: None,
                    online: false,
                },
                "doctor",
            ),
        ] {
            assert_eq!(action.name(), name, "{action:?}");
        }
    }

    #[test]
    fn every_action_has_its_label() {
        // Redesign §9.4's eleven, and T2-17's two (`R`, clearing an alias), and `enable`, which
        // neither lists.
        let a = || account(3);
        let named = |r: &AccountRef| format!("{} spare", r.position);
        let label = |action: Action| action.label(&named, "Claude Code");
        let by = |strategy| Action::SwitchBy {
            provider: provider(),
            strategy,
        };
        let auto = |on| Action::Autoswitch {
            provider: provider(),
            on,
        };
        for (action, want) in [
            (Action::Switch(a()), "switching to 3 spare"),
            (by(UsageStrategy::Best), "switching to the best account"),
            (
                by(UsageStrategy::NextAvailable),
                "switching to the next account",
            ),
            (Action::Disable(a()), "disabling 3 spare"),
            (
                Action::SetAlias {
                    account: a(),
                    alias: "work".into(),
                },
                "setting the alias",
            ),
            (
                Action::Move {
                    account: a(),
                    position: 1,
                },
                "moving 3 spare",
            ),
            (Action::Remove(a()), "removing 3 spare"),
            (
                Action::AddLive {
                    provider: provider(),
                },
                "adding Claude Code's login",
            ),
            (
                Action::Doctor {
                    provider: None,
                    online: true,
                },
                "running doctor",
            ),
            (auto(false), "pausing auto"),
            (auto(true), "resuming auto"),
            (Action::Run(a()), "starting Claude Code as 3 spare"),
            (Action::ClearAlias(a()), "setting the alias"),
            (Action::Enable(a()), "enabling 3 spare"),
        ] {
            assert_eq!(label(action.clone()), want, "{action:?}");
        }
    }

    #[test]
    fn only_run_needs_the_terminal() {
        let a = || account(1);
        assert!(Action::Run(a()).needs_terminal());
        for action in [
            Action::Switch(a()),
            Action::Enable(a()),
            Action::Disable(a()),
            Action::Remove(a()),
            Action::ClearAlias(a()),
            Action::AddLive {
                provider: provider(),
            },
            Action::Doctor {
                provider: None,
                online: true,
            },
        ] {
            assert!(!action.needs_terminal(), "{action:?}");
        }
    }
}
