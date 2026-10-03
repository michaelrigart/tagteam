//! §5: on an unset, empty or relative `HOME`, tagteam refuses to run before anything is
//! created, and the status bar's line prints nothing instead. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

use common::std_cmd;
use serde_json::{Value, json};

/// What a relative `HOME` is set to.
const RELATIVE: &str = "rel";
/// Where the old fallback for an unset `HOME` put tagteam's state.
const UNDER_ROOT: [&str; 3] = [
    "/.local/share/tagteam",
    "/.config/tagteam",
    "/.local/state/tagteam",
];
/// Commands that read or write state under `HOME`. On a fresh machine `add-token` creates the
/// store and the vault, so a fallback would show in the working directory.
const COMMANDS: [&[&str]; 5] = [
    &["list"],
    &["status"],
    &["switch"],
    &["history"],
    &["add-token", "sk-ant-api03-key"],
];

#[derive(Clone, Copy, Debug)]
enum Home {
    Unset,
    Empty,
    Relative,
}

impl Home {
    const ALL: [Home; 3] = [Home::Unset, Home::Empty, Home::Relative];

    /// The refusal, verbatim: its wording tells the user what to do.
    fn message(self) -> &'static str {
        match self {
            Home::Unset => "HOME is not set; set it to the absolute path of your home directory",
            Home::Empty => "HOME is empty; set it to the absolute path of your home directory",
            Home::Relative => {
                "HOME is \"rel\", which is not an absolute path; set it to the absolute path of your home directory"
            }
        }
    }

    /// Where the old fallback read `~/.claude.json`, relative to the working directory. `None`
    /// for an unset `HOME`, whose fallback was `/`.
    fn claude_json(self) -> Option<&'static str> {
        match self {
            Home::Unset => None,
            Home::Empty => Some(".claude.json"),
            Home::Relative => Some("rel/.claude.json"),
        }
    }
}

/// `tagteam <args>` under `home`, run from the working directory `cwd`.
fn run(root: &Path, cwd: &Path, home: Home, args: &[&str]) -> Output {
    let mut c = std_cmd(root);
    match home {
        Home::Unset => c.env_remove("HOME"),
        Home::Empty => c.env("HOME", ""),
        Home::Relative => c.env("HOME", RELATIVE),
    };
    c.current_dir(cwd).args(args).output().unwrap()
}

fn under_root() -> Vec<bool> {
    UNDER_ROOT.iter().map(|p| Path::new(p).exists()).collect()
}

fn is_empty(dir: &Path) -> bool {
    fs::read_dir(dir).unwrap().next().is_none()
}

#[test]
fn an_unusable_home_refuses_every_command_and_creates_nothing() {
    let root = tempfile::tempdir().unwrap();
    let before = under_root();
    for home in Home::ALL {
        for args in COMMANDS {
            let cwd = tempfile::tempdir().unwrap();
            let out = run(root.path(), cwd.path(), home, args);
            assert_eq!(out.status.code(), Some(1), "{home:?} {args:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "",
                "{home:?} {args:?}"
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stderr),
                format!("tagteam: {}\n", home.message()),
                "{home:?} {args:?}"
            );
            assert!(
                is_empty(cwd.path()),
                "{home:?} {args:?}: something was created in the working directory"
            );
        }
        // The parse comes first: a usage error and the version are as before.
        let cwd = tempfile::tempdir().unwrap();
        assert_eq!(
            run(root.path(), cwd.path(), home, &["frobnicate"])
                .status
                .code(),
            Some(2)
        );
        assert_eq!(
            run(root.path(), cwd.path(), home, &["--version"])
                .status
                .code(),
            Some(0)
        );
    }
    assert_eq!(under_root(), before, "something was created under /");
    assert!(
        !root.path().join("keychain").exists(),
        "a vault item was written"
    );
}

#[test]
fn under_json_the_refusal_is_one_env_error() {
    let root = tempfile::tempdir().unwrap();
    for home in Home::ALL {
        for args in [
            &["list", "--json"][..],
            &["--json", "add-token", "sk-ant-api03-key"][..],
            // `--json` is no status bar: it gets the error object, not silence.
            &["statusline", "--json"][..],
        ] {
            let cwd = tempfile::tempdir().unwrap();
            let out = run(root.path(), cwd.path(), home, args);
            assert_eq!(out.status.code(), Some(1), "{home:?} {args:?}");
            assert_eq!(
                serde_json::from_slice::<Value>(&out.stdout).unwrap(),
                json!({"schemaVersion": 1, "error": {"type": "env", "message": home.message()}}),
                "{home:?} {args:?}"
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stderr),
                "",
                "{home:?} {args:?}"
            );
            assert!(is_empty(cwd.path()), "{home:?} {args:?}");
        }
    }
}

#[test]
fn the_status_bar_line_prints_nothing_and_print_config_is_refused() {
    let root = tempfile::tempdir().unwrap();
    for home in Home::ALL {
        let cwd = tempfile::tempdir().unwrap();
        // A login where the old fallback would have found it, so the old code printed a line.
        if let Some(path) = home.claude_json() {
            let path = cwd.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                r#"{"oauthAccount":{"emailAddress":"stranger@x.co","organizationUuid":"","accountUuid":"uuid-s"}}"#,
            )
            .unwrap();
        }
        let out = run(root.path(), cwd.path(), home, &["statusline"]);
        assert_eq!(
            (
                out.status.code(),
                out.stdout.as_slice(),
                out.stderr.as_slice()
            ),
            (Some(0), &b""[..], &b""[..]),
            "{home:?}"
        );
        let out = run(
            root.path(),
            cwd.path(),
            home,
            &["statusline", "--print-config"],
        );
        assert_eq!(out.status.code(), Some(1), "{home:?}");
        assert_eq!(out.stdout, b"", "{home:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("tagteam: {}\n", home.message()),
            "{home:?}"
        );
    }
}
