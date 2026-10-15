//! The arguments the TUI builds for each action (`Action::args`), parsed by the CLI's own
//! parser. `tagteam-tui` cannot depend on `tagteam`, so the round trip lives here.

use clap::{CommandFactory, Parser};
use tagteam::cli::{Cli, Command, StrategyArg};
use tagteam_core::{AccountId, ProviderId};
use tagteam_tui::host::{AccountRef, Action, Strategy};

const ID: &str = "0192-abc";

fn account(position: u32) -> AccountRef {
    AccountRef {
        id: AccountId::from_string(ID),
        provider: ProviderId::new("claude-code"),
        position,
    }
}

fn provider() -> ProviderId {
    ProviderId::new("claude-code")
}

/// `tagteam <action.args()>`, parsed.
fn parse(action: &Action) -> Cli {
    let argv = std::iter::once("tagteam".to_owned()).chain(action.args());
    match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => panic!("{:?} does not parse: {e}", action.args()),
    }
}

/// What the flags before the command say, for an action that names an account.
fn guarded(cli: &Cli, no_color: bool) {
    assert_eq!(cli.provider.as_deref(), Some("claude-code"));
    assert_eq!(cli.expect_account.as_deref(), Some(ID));
    assert_eq!(cli.no_color, no_color);
    assert!(!cli.json && !cli.debug);
}

#[test]
fn switch_enable_disable_and_remove_parse_to_their_commands_at_the_position() {
    let cli = parse(&Action::Switch(account(3)));
    guarded(&cli, true);
    assert!(matches!(
        cli.command,
        Some(Command::Switch { account: Some(ref a), force: false, strategy: None, model: None }) if a == "3"
    ));
    let cli = parse(&Action::Enable(account(3)));
    guarded(&cli, true);
    assert!(matches!(cli.command, Some(Command::Enable { ref account }) if account == "3"));
    let cli = parse(&Action::Disable(account(3)));
    guarded(&cli, true);
    assert!(matches!(cli.command, Some(Command::Disable { ref account }) if account == "3"));
    let cli = parse(&Action::Remove(account(3)));
    guarded(&cli, true);
    assert!(matches!(cli.command, Some(Command::Remove { ref account }) if account == "3"));
}

#[test]
fn a_strategy_switch_names_no_account_and_no_guard() {
    for (strategy, expected) in [
        (Strategy::Best, "best"),
        (Strategy::NextAvailable, "next-available"),
    ] {
        let cli = parse(&Action::SwitchBy {
            provider: provider(),
            strategy,
        });
        assert_eq!(cli.provider.as_deref(), Some("claude-code"));
        assert_eq!(cli.expect_account, None);
        assert!(cli.no_color);
        let Some(Command::Switch {
            account: None,
            strategy: Some(parsed),
            ..
        }) = cli.command
        else {
            panic!("not a strategy switch");
        };
        assert!(
            matches!(
                (parsed, strategy),
                (StrategyArg::Best, Strategy::Best)
                    | (StrategyArg::NextAvailable, Strategy::NextAvailable)
            ),
            "{expected}"
        );
    }
}

#[test]
fn an_alias_is_the_name_whatever_it_begins_with() {
    // `--` ends the options, so an alias that looks like a flag, or like `--`, is still a name.
    for alias in [
        "work",
        "-work",
        "--unset",
        "--provider",
        "--",
        "a b",
        "",
        "日本語 😀",
    ] {
        let cli = parse(&Action::SetAlias {
            account: account(2),
            alias: alias.into(),
        });
        guarded(&cli, true);
        let Some(Command::Alias {
            account: Some(position),
            name: Some(name),
            unset: false,
        }) = cli.command
        else {
            panic!("{alias:?} is not an alias to set");
        };
        assert_eq!((position.as_str(), name.as_str()), ("2", alias));
    }
}

#[test]
fn clearing_an_alias_is_unset() {
    let cli = parse(&Action::ClearAlias(account(2)));
    guarded(&cli, true);
    assert!(matches!(
        cli.command,
        Some(Command::Alias { account: Some(ref a), name: None, unset: true }) if a == "2"
    ));
}

#[test]
fn a_move_names_the_position_it_is_at_and_the_one_it_goes_to() {
    let cli = parse(&Action::Move {
        account: account(2),
        position: 5,
    });
    guarded(&cli, true);
    assert!(matches!(
        cli.command,
        Some(Command::Move { ref account, position: 5 }) if account == "2"
    ));
}

#[test]
fn adding_the_live_login_names_only_the_provider() {
    let cli = parse(&Action::AddLive {
        provider: provider(),
    });
    assert_eq!(cli.provider.as_deref(), Some("claude-code"));
    assert_eq!(cli.expect_account, None);
    assert!(cli.no_color);
    assert!(matches!(
        cli.command,
        Some(Command::Add {
            position: None,
            alias: None,
            yes: false
        })
    ));
}

#[test]
fn run_is_launched_with_its_colour_and_the_guard() {
    let cli = parse(&Action::Run(account(1)));
    guarded(&cli, false);
    let Some(Command::Run {
        account: Some(position),
        require_session: false,
        args,
    }) = cli.command
    else {
        panic!("not a plain run");
    };
    assert_eq!(position, "1");
    assert!(args.is_empty(), "nothing is passed to the agent");
}

#[test]
fn doctor_parses_with_and_without_a_provider_and_online() {
    for (provider, online) in [
        (None, false),
        (None, true),
        (Some(provider()), false),
        (Some(provider()), true),
    ] {
        let named = provider.as_ref().map(|p| p.to_string());
        let cli = parse(&Action::Doctor { provider, online });
        assert_eq!(cli.provider, named);
        assert_eq!(cli.expect_account, None);
        assert!(cli.no_color);
        assert!(matches!(
            cli.command,
            Some(Command::Doctor { online: o }) if o == online
        ));
    }
}

#[test]
fn every_action_name_is_a_subcommand_of_the_cli() {
    let command = Cli::command();
    for action in [
        Action::Switch(account(1)),
        Action::SwitchBy {
            provider: provider(),
            strategy: Strategy::Best,
        },
        Action::Enable(account(1)),
        Action::Disable(account(1)),
        Action::SetAlias {
            account: account(1),
            alias: "x".into(),
        },
        Action::ClearAlias(account(1)),
        Action::Move {
            account: account(1),
            position: 2,
        },
        Action::Remove(account(1)),
        Action::AddLive {
            provider: provider(),
        },
        Action::Run(account(1)),
        Action::Doctor {
            provider: None,
            online: false,
        },
    ] {
        assert!(
            command.find_subcommand(action.name()).is_some(),
            "{} is no subcommand",
            action.name()
        );
        // The name is the command the arguments run.
        assert!(
            action.args().iter().any(|a| a == action.name()),
            "{:?} does not run {}",
            action.args(),
            action.name()
        );
    }
}
