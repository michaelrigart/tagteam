//! `cargo xtask compat login` (§15.4): the dedicated test account, logged in once by you in a
//! scratch home, and kept in the compat store with a setup token for the same account. Never
//! an account in daily use: compat refreshes its login, which would consume your own copy.

use std::fs;
use std::path::{Path, PathBuf};

use tagteam_cc::{ItemKind, keychain_service};
use tagteam_provider::Cancel;
use tagteam_provider::atomic::ensure_private_dir;

use super::ctx::{ALIAS_OAUTH, ALIAS_SETUP_TOKEN, Account, Ctx, account};
use super::guard::{Roots, cc_env};
use super::keychain::VaultKeychain;
use super::layout::{self, Layout, make_scratch};
use super::report::{EXIT_HARNESS, EXIT_PASS, Redactor};
use super::run::{base_env, build_tagteam, take_run_lock, user_homes, workspace_root};
use super::sys::{CAUGHT, HarnessError, cancel, catch_signals, harness, interrupted, which_in};

pub fn login() -> u8 {
    if let Err(e) = catch_signals(cancel(), &CAUGHT) {
        eprintln!("cargo xtask compat login: could not catch signals: {e}");
        return EXIT_HARNESS;
    }
    // By the time it returns the vault is locked again and the run lock let go of.
    finish(run_login(), cancel())
}

/// What `compat login` prints and exits with: 0 once the store holds both accounts, 128 + n
/// after signal n, 2 after any other failure. A failure's message names the scratch directory
/// it kept (`login_in`).
fn finish(done: Result<(), HarnessError>, token: &Cancel) -> u8 {
    match (done, token.requested()) {
        (Ok(()), None) => {
            eprintln!(
                "cargo xtask compat login: the test account and its setup token are in the compat store"
            );
            EXIT_PASS
        }
        (Ok(()), Some(n)) => {
            eprintln!("cargo xtask compat login: {}", interrupted(n));
            128 + n as u8
        }
        (Err(e), signal) => {
            eprintln!("cargo xtask compat login: {e}");
            signal.map_or(EXIT_HARNESS, |n| 128 + n as u8)
        }
    }
}

fn run_login() -> Result<(), HarnessError> {
    let macos = cfg!(target_os = "macos");
    let home = PathBuf::from(std::env::var_os("HOME").ok_or_else(|| harness("HOME is not set"))?);
    let state = layout::state_dir(&home, std::env::var_os("XDG_STATE_HOME").as_deref());
    if let Some(parent) = state.parent() {
        fs::create_dir_all(parent)?;
    }
    ensure_private_dir(&state)?;
    let _lock = take_run_lock()?;
    let workspace = workspace_root();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let claude = which_in("claude", &path).ok_or_else(|| harness("claude is not on PATH"))?;
    let tagteam = build_tagteam(&workspace)?;
    let layout = Layout {
        reports: layout::reports_dir(&workspace, std::env::var_os("CARGO_TARGET_DIR").as_deref()),
        workspace,
        state: fs::canonicalize(&state)?,
        scratch: make_scratch()?,
    };
    login_in(layout, home, &path, claude, tagteam, macos, cancel())
}

/// The login in its scratch directory, which goes only once the store holds the login and the
/// scratch home's own copy is gone. Interrupted or failed at any point, it keeps the directory:
/// after `/login` its `live/` home holds the only copy of a login no store has captured (on
/// macOS in a Keychain item named from that home, which stays too). The error then names the
/// directory, and the item on macOS, and how to finish. This is the user's terminal, not the
/// log, so the path is printed.
fn login_in(
    layout: Layout,
    home: PathBuf,
    path: &std::ffi::OsStr,
    claude: PathBuf,
    tagteam: PathBuf,
    macos: bool,
    token: &Cancel,
) -> Result<(), HarnessError> {
    let scratch = layout.scratch.clone();
    let live = layout.live().to_string_lossy().into_owned();
    let user = std::env::var("USER").ok();
    let item =
        macos.then(|| keychain_service(&cc_env(&live, &home, user.as_deref()), ItemKind::OAuth));
    match log_in(layout, home, path, claude, tagteam, macos, token) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&scratch);
            Ok(())
        }
        Err(e) => Err(harness(format!("{e}\n{}", kept(&scratch, item.as_deref())))),
    }
}

/// The sentence a login that stopped before its capture leaves with the user.
fn kept(scratch: &Path, item: Option<&str>) -> String {
    let held = item.map_or_else(
        || format!("{}/live/.credentials.json", scratch.display()),
        |service| {
            format!(
                "the Keychain item {service:?}, with {}/live",
                scratch.display()
            )
        },
    );
    format!(
        "Kept {}: it may hold a login no store has captured yet ({held}). To finish, run `cargo xtask compat login` again (or `tagteam add` in that home with the compat store's environment), then delete the directory{}.",
        scratch.display(),
        if item.is_some() { " and that item" } else { "" }
    )
}

fn log_in(
    layout: Layout,
    home: PathBuf,
    path: &std::ffi::OsStr,
    claude: PathBuf,
    tagteam: PathBuf,
    macos: bool,
    token: &Cancel,
) -> Result<(), HarnessError> {
    let vault = if macos {
        let file = layout.vault_keychain();
        Some(if file.exists() {
            VaultKeychain::open(&file, &layout.vault_password())?.unlock()?
        } else {
            VaultKeychain::create(&file, &layout.vault_password())?
        })
    } else {
        None
    };
    let roots = Roots {
        scratch: layout.scratch.clone(),
        state: layout.state.clone(),
        users: user_homes(&home),
        home: home.clone(),
        user: std::env::var("USER").ok(),
    };
    let base = base_env(&layout, &home, path, vault.as_deref());
    let ctx = Ctx {
        layout,
        roots,
        tagteam,
        claude,
        macos,
        oauth: Account::default(),
        setup_token: None,
        vault,
        base,
        activation_config: None,
        live_active: false,
        ssh: None,
        // Nothing login captures is written anywhere: token-shaped runs only.
        redact: Redactor::default(),
        cancel: token.clone(),
        daemons: Vec::new(),
    };
    let live = ctx.live();
    ctx.roots.services(&live)?;
    ctx.seed_trust(&live)?;

    eprintln!(
        "\nLog in with the dedicated test account: type /login, finish in the browser, then /exit.\n\
         Never use an account you work with: compat refreshes its login.\n"
    );
    ctx.claude(&live, &[]).attached(&ctx.roots)?;
    if ctx
        .tagteam(&["add", "--alias", ALIAS_OAUTH])
        .attached(&ctx.roots)?
        != Some(0)
    {
        return Err(harness(
            "tagteam add failed; if the store holds another test account, delete the compat store and log in again",
        ));
    }

    eprintln!(
        "\nNow a setup token for the same account: `claude setup-token` opens the browser and prints it.\n"
    );
    ctx.claude(&live, &["setup-token"]).attached(&ctx.roots)?;
    let _ = ctx.tagteam(&["remove", ALIAS_SETUP_TOKEN]).run(&ctx.roots);
    eprintln!("\nPaste the token at tagteam's prompt; it is not echoed.\n");
    if ctx
        .tagteam(&["add-token", "--alias", ALIAS_SETUP_TOKEN])
        .attached(&ctx.roots)?
        != Some(0)
    {
        return Err(harness("tagteam add-token failed"));
    }

    let list = ctx.list()?;
    let (Some(oauth), Some(setup)) = (
        account(&list, ALIAS_OAUTH),
        account(&list, ALIAS_SETUP_TOKEN),
    ) else {
        return Err(harness(
            "the compat store lacks compat-oauth or compat-setup-token",
        ));
    };
    // The login goes only once compat's own vault is seen to hold it. A `tagteam` that ignored
    // the vault hook (a build without `test-support`) would have stored it in the default
    // keychain instead, and the scratch home's copy would then be the only other one.
    for (alias, id) in [(ALIAS_OAUTH, &oauth.id), (ALIAS_SETUP_TOKEN, &setup.id)] {
        if let Err(e) = ctx.vault_credential(id) {
            return Err(harness(format!(
                "compat's vault does not hold {alias} ({e}); the stored login may be in the default keychain (is `tagteam` built with test-support?)"
            )));
        }
    }
    // The login's copy in the scratch home goes: the vault holds it now.
    ctx.forget(&live)?;
    if let Some(v) = &ctx.vault {
        v.lock()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::Duration;

    use super::*;
    use crate::compat::keychain::random_hex;
    use crate::compat::sys::wait_until;

    /// A new directory under the temporary directory, by its canonical path.
    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xtask-login-{tag}-{}",
            &random_hex().unwrap()[..12]
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A fake `claude` and `tagteam` in `bin`. A bare `claude` stands for `/login`: it writes the
    /// home's credential, says so in `bin/ready`, and waits for `bin/go` (5 s at most). `tagteam
    /// add` exits `add_exit`; `tagteam list --json` names both compat accounts.
    fn fakes(bin: &Path, add_exit: i32, vault: bool) -> (PathBuf, PathBuf) {
        // What a `tagteam` that honours the vault hook leaves in the compat vault (the Linux
        // file vault: these tests run with `macos` false).
        let store = |id: &str| {
            if vault {
                format!(
                    r#"mkdir -p "$XDG_DATA_HOME/tagteam/vault" && printf '%s' '{{}}' > "$XDG_DATA_HOME/tagteam/vault/{id}.json""#
                )
            } else {
                String::new()
            }
        };
        let claude = script(
            bin,
            "claude",
            &format!(
                r#"if [ "$#" -eq 0 ]; then
    printf '%s' '{{"claudeAiOauth":{{"refreshToken":"rt-login"}}}}' > "$CLAUDE_CONFIG_DIR/.credentials.json"
    : > '{ready}'
    i=0; while [ ! -e '{go}' ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
fi
exit 0
"#,
                ready = bin.join("ready").display(),
                go = bin.join("go").display(),
            ),
        );
        let tagteam = script(
            bin,
            "tagteam",
            &format!(
                r#"case "$1" in
add) [ {add_exit} -eq 0 ] && {store1}; exit {add_exit} ;;
add-token) {store2}; exit 0 ;;
list) printf '%s' '{{"accounts":[{{"alias":"compat-oauth","id":"1","position":1}},{{"alias":"compat-setup-token","id":"2","position":2}}]}}' ;;
esac
exit 0
"#,
                store1 = if vault { store("1") } else { ":".into() },
                store2 = if vault { store("2") } else { ":".into() },
            ),
        );
        (claude, tagteam)
    }

    /// `login_in` with the fakes in `bin`, in a new scratch directory: its result, and the
    /// directory.
    fn login_with(
        bin: &Path,
        add_exit: i32,
        vault: bool,
        token: &Cancel,
    ) -> (Result<(), HarnessError>, PathBuf) {
        let (claude, tagteam) = fakes(bin, add_exit, vault);
        let (state, home) = (temp("state"), temp("home"));
        let layout = Layout {
            workspace: PathBuf::from("/w"),
            reports: PathBuf::from("/w/target/compat"),
            state: state.clone(),
            scratch: make_scratch().unwrap(),
        };
        let scratch = layout.scratch.clone();
        let path = std::ffi::OsStr::new("/bin:/usr/bin");
        let done = login_in(layout, home.clone(), path, claude, tagteam, false, token);
        for dir in [state, home] {
            fs::remove_dir_all(dir).unwrap();
        }
        (done, scratch)
    }

    #[test]
    fn a_login_s_scratch_directory_goes_only_once_the_login_is_captured() {
        let _serial = crate::compat::sys::serial();
        // `tagteam add` fails after `/login`: the directory and its login stay, and are named.
        let bin = temp("bin");
        fs::write(bin.join("go"), "").unwrap();
        let token = Cancel::new();
        let (done, scratch) = login_with(&bin, 1, true, &token);
        let message = done.clone().unwrap_err().0;
        assert!(
            message.contains("tagteam add failed")
                && message.contains(&format!("Kept {}", scratch.display())),
            "{message}"
        );
        assert!(scratch.join("live/.credentials.json").is_file());
        assert_eq!(finish(done, &token), EXIT_HARNESS);
        fs::remove_dir_all(&scratch).unwrap();

        // Ctrl-C after `/login`, before `tagteam add`: the same, and exit 130.
        let waiting = temp("bin");
        let token = Cancel::new();
        let (signaller, watched) = (token.clone(), waiting.clone());
        let interrupt = std::thread::spawn(move || {
            let ready = wait_until(Duration::from_secs(10), || watched.join("ready").exists());
            signaller.request(2);
            fs::write(watched.join("go"), "").unwrap();
            ready
        });
        let (done, scratch) = login_with(&waiting, 0, true, &token);
        assert!(interrupt.join().unwrap(), "the fake claude wrote its login");
        let message = done.clone().unwrap_err().0;
        assert!(
            message.starts_with("interrupted by SIGINT")
                && message.contains(&format!("Kept {}", scratch.display())),
            "{message}"
        );
        assert!(scratch.join("live/.credentials.json").is_file());
        assert_eq!(finish(done, &token), 130);
        fs::remove_dir_all(&scratch).unwrap();

        // Captured: the scratch home's copy goes, then the directory.
        let token = Cancel::new();
        let (done, scratch) = login_with(&bin, 0, true, &token);
        assert_eq!(done, Ok(()));
        assert!(!scratch.exists());
        assert_eq!(finish(done, &token), EXIT_PASS);
        for dir in [bin, waiting] {
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn a_login_the_vault_does_not_hold_keeps_its_scratch_directory() {
        let _serial = crate::compat::sys::serial();
        // A `tagteam` that ignored the vault hook added both accounts somewhere else (the
        // default keychain): the store lists them, compat's vault holds nothing.
        let bin = temp("bin");
        fs::write(bin.join("go"), "").unwrap();
        let token = Cancel::new();
        let (done, scratch) = login_with(&bin, 0, false, &token);
        let message = done.clone().unwrap_err().0;
        assert!(
            message.contains("compat's vault does not hold compat-oauth")
                && message.contains("default keychain")
                && message.contains(&format!("Kept {}", scratch.display())),
            "{message}"
        );
        assert!(
            scratch.join("live/.credentials.json").is_file(),
            "the scratch home's copy is not deleted"
        );
        assert_eq!(finish(done, &token), EXIT_HARNESS);
        fs::remove_dir_all(&scratch).unwrap();
        fs::remove_dir_all(&bin).unwrap();
    }
}
