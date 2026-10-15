//! The action port (TUI spec §4.5). The `tagteam` binary implements `Host` over the CLI's own
//! dispatch, so every action says and does what the same command does on the command line.

use tagteam_core::{AccountId, ProviderId};

/// The selected account as the TUI last read it: the position names it to the CLI, and the ID
/// guards the position (`--expect-account`, core §10.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    pub id: AccountId,
    pub provider: ProviderId,
    pub position: u32,
}

/// `switch --strategy`'s values (core §9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Best,
    NextAvailable,
}

impl Strategy {
    fn arg(self) -> &'static str {
        match self {
            Strategy::Best => "best",
            Strategy::NextAvailable => "next-available",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Switch(AccountRef),
    SwitchBy {
        provider: ProviderId,
        strategy: Strategy,
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
                strategy.arg().into(),
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
        }
    }

    /// The command's name as the CLI spells it: "switch", "enable", "disable", "alias", "move",
    /// "remove", "add", "run", "doctor". The footer shows it while the action runs, and says
    /// `busy: <name>` to a second action key (spec §7.2).
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
                strategy: Strategy::Best
            }),
            "--no-color --provider claude-code switch --strategy best"
        );
        assert_eq!(
            args(&Action::SwitchBy {
                provider: provider(),
                strategy: Strategy::NextAvailable
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
                    strategy: Strategy::Best,
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
