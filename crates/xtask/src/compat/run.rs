//! One `cargo xtask compat` run: setup, the checks phase by phase, teardown, `--bless`, and the
//! report. Teardown runs once setup has made the scratch directory and every child of the
//! harness has ended: it takes the test account's newest generation back into the vault before
//! it deletes anything. Whatever ends the checks, every daemon a check started is stopped and
//! verified first (`Ctx::stop_daemons`). SIGINT, SIGTERM and SIGHUP stop a run at its next wait
//! instead of where they land: every child and daemon ends, nothing is torn down, the vault is
//! locked again, the run lock is let go of last, and the run exits 128 + the signal's number.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tagteam_provider::Cancel;

use super::CompatArgs;
use super::checks::{CHECKS, metas};
use super::ctx::{ALIAS_API_KEY, ALIAS_OAUTH, ALIAS_SETUP_TOKEN, Account, Ctx, account, now_ms};
use super::guard::Roots;
use super::keychain::VaultKeychain;
use super::layout::{self, Layout, RunLock, make_scratch};
use super::registry::{Flags, Phase, select};
use super::report::{CheckResult, EXIT_HARNESS, Evidence, Outcome, Report};
use super::store;
use super::sys::{
    CAUGHT, HarnessError, Ran, cancel, catch_signals, harness, interrupted, quiesce, tail, which_in,
};
use super::version::{CcVersion, blessed};

/// Claude Code's managed settings, which apply to every home and so to every check.
const MANAGED_SETTINGS: [&str; 2] = [
    "/Library/Application Support/ClaudeCode/managed-settings.json",
    "/etc/claude-code/managed-settings.json",
];

pub fn workspace_root() -> PathBuf {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&here).unwrap_or(here)
}

pub fn tested_version_file(workspace: &Path) -> PathBuf {
    workspace.join("crates/tagteam-cc/compat/tested-cc-version")
}

fn note(label: &str, value: Value) -> Evidence {
    Evidence {
        label: label.to_owned(),
        value,
        ok: None,
    }
}

/// `cargo build -p tagteam --features test-support`, and the binary it made. The path is
/// cargo's own report (`executable` of the `tagteam` bin's `compiler-artifact` record), so it
/// follows `CARGO_TARGET_DIR`, `[build] target-dir` and `[build] target`: a guessed path could
/// name a stale build without `test-support`, which ignores `TAGTEAM_TEST_VAULT_KEYCHAIN`.
pub fn build_tagteam(workspace: &Path) -> Result<PathBuf, HarnessError> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "build",
            "--quiet",
            "--message-format=json-render-diagnostics",
            "--package",
            "tagteam",
            "--features",
            "test-support",
        ])
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| harness(format!("could not run cargo: {e}")))?;
    if !out.status.success() {
        return Err(harness(
            "cargo build -p tagteam --features test-support failed",
        ));
    }
    tagteam_executable(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        harness("cargo build reported no executable for the tagteam binary; refusing to guess one")
    })
}

/// The `executable` of the last `compiler-artifact` record for the `tagteam` bin target in
/// cargo's `--message-format=json` output (one JSON object per line; other lines are ignored).
pub fn tagteam_executable(cargo_json: &str) -> Option<PathBuf> {
    cargo_json
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| {
            m["reason"] == "compiler-artifact"
                && m["target"]["name"] == "tagteam"
                && m["target"]["kind"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|k| k == "bin"))
        })
        .filter_map(|m| m["executable"].as_str().map(PathBuf::from))
        .next_back()
}

/// The user's own Claude Code homes: `~/.claude`, and `CLAUDE_CONFIG_DIR` and
/// `CLAUDE_SECURESTORAGE_CONFIG_DIR` as this process has them, when set and not empty.
pub fn user_homes(home: &Path) -> Vec<PathBuf> {
    let mut out = vec![home.join(".claude")];
    for var in ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"] {
        if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            out.push(PathBuf::from(v));
        }
    }
    out.into_iter()
        .map(|p| fs::canonicalize(&p).unwrap_or(p))
        .collect()
}

/// Every child's environment but `CLAUDE_CONFIG_DIR`, built from nothing. `HOME` stays the
/// real one: `/usr/bin/security` finds the login keychain from it, for CC and tagteam alike.
pub fn base_env(
    layout: &Layout,
    home: &Path,
    path: &OsStr,
    vault: Option<&VaultKeychain>,
) -> Vec<(OsString, OsString)> {
    let mut v: Vec<(OsString, OsString)> = vec![
        ("HOME".into(), home.into()),
        ("PATH".into(), path.into()),
        ("TERM".into(), "xterm-256color".into()),
        (
            "LANG".into(),
            std::env::var_os("LANG").unwrap_or_else(|| "en_US.UTF-8".into()),
        ),
        // A run never updates the claude it checks.
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
    ];
    for name in ["USER", "LOGNAME", "TMPDIR"] {
        if let Some(value) = std::env::var_os(name) {
            v.push((name.into(), value));
        }
    }
    for (name, dir) in layout.xdg() {
        v.push((name.into(), dir.into()));
    }
    if let Some(vault) = vault {
        v.push((
            "TAGTEAM_TEST_VAULT_KEYCHAIN".into(),
            vault.path.clone().into(),
        ));
    }
    v
}

pub fn run(args: &CompatArgs) -> u8 {
    let flags = Flags {
        ssh: args.ssh.is_some(),
        locked_keychain: args.locked_keychain,
    };
    let selected = match select(&metas(), &args.only, flags) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cargo xtask compat: {e}");
            return EXIT_HARNESS;
        }
    };
    if args.bless && !args.only.is_empty() {
        eprintln!("cargo xtask compat: --bless needs a full run, without --only");
        return EXIT_HARNESS;
    }
    if let Err(e) = catch_signals(cancel(), &CAUGHT) {
        eprintln!("cargo xtask compat: could not catch signals: {e}");
        return EXIT_HARNESS;
    }
    // Taken first and let go last: once every child has ended, the vault is locked again (the
    // `Ctx` goes) and the report is written, whatever ended the run.
    let lock = match take_run_lock() {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("cargo xtask compat: {e}");
            return EXIT_HARNESS;
        }
    };
    let workspace = workspace_root();
    let reports = layout::reports_dir(&workspace, std::env::var_os("CARGO_TARGET_DIR").as_deref());
    let mut report = Report {
        started_at: now_ms() / 1000,
        platform: if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        },
        ..Report::default()
    };
    let mut scratch = None;
    match setup(args, &workspace, &reports, &mut report) {
        Err(e) => report.harness_error = Some(e.0),
        Ok(mut ctx) => {
            scratch = Some(ctx.layout.scratch.clone());
            let quiet = run_checks(&mut ctx, &selected, &mut report);
            // Whatever ended the checks, every daemon one started is stopped and verified.
            let stopped = ctx.stop_daemons();
            let torn = match teardown_refusal(quiet, stopped, &ctx.layout.scratch) {
                Some(refused) => Err(refused),
                // A signal: every child and daemon has ended, and nothing is torn down.
                None if cancel().requested().is_some() => Ok(()),
                None => teardown(&mut ctx, args.keep, &mut report),
            };
            if let Err(e) = torn {
                report.harness_error.get_or_insert(e.0);
            }
            if args.bless && cancel().requested().is_none() {
                bless(&workspace, &mut report);
            }
        }
    }
    conclude(&mut report, cancel(), scratch.as_deref());
    if let Err(e) = report.write(&reports) {
        eprintln!("cargo xtask compat: could not write the report: {e}");
        return EXIT_HARNESS;
    }
    eprintln!(
        "cargo xtask compat: exit {}; the report is {}",
        report.exit_code(),
        reports.join("report.md").display()
    );
    let code = report.exit_code();
    drop(lock);
    code
}

/// Why teardown must not start, when it must not: a process group quiescence found still
/// holding a process, or a daemon that could not be stopped and verified. Either could still
/// write a credential while teardown captures and deletes, so teardown then touches none.
fn teardown_refusal(
    quiet: Result<(), HarnessError>,
    stopped: Result<(), HarnessError>,
    scratch: &Path,
) -> Option<HarnessError> {
    let why = match (quiet, stopped) {
        (Ok(()), Ok(())) => return None,
        (Err(e), Ok(())) | (Ok(()), Err(e)) => e.0,
        (Err(a), Err(b)) => format!("{a}; {b}"),
    };
    Some(harness(format!(
        "teardown refused, since {why}: no credential was touched, and {} is kept",
        scratch.display()
    )))
}

/// The last teardown step: the vault locked again, after everything else.
const TEARDOWN_LAST: &str = "the vault locked";

/// A run a signal ended, once its `Ctx` has gone: the vault is locked again. The report names
/// the signal and how far teardown got (not begun, interrupted after which steps, or already
/// finished), keeps what else went wrong (a group quiescence found still holding a process,
/// say), and the run exits 128 + the signal's number.
fn conclude(report: &mut Report, token: &Cancel, scratch: Option<&Path>) {
    let Some(n) = token.requested() else {
        return;
    };
    report.interrupted = Some(n);
    let signal = interrupted(n).0;
    let kept = scratch
        .filter(|s| s.exists())
        .map_or_else(String::new, |s| format!("; {} is kept", s.display()));
    let also = report
        .harness_error
        .take()
        .filter(|e| *e != signal)
        .map_or_else(String::new, |e| format!(" ({e})"));
    let torn = match report.teardown.as_deref() {
        None => "nothing was torn down".to_owned(),
        Some([]) => "teardown was interrupted before any step finished; nothing was deleted".into(),
        Some(done) if done.last() == Some(&TEARDOWN_LAST) => {
            format!("teardown had finished ({})", done.join(", "))
        }
        Some(done) => format!(
            "teardown was interrupted after: {}; the next step may be partly done and the rest were not done",
            done.join(", ")
        ),
    };
    report.harness_error = Some(format!(
        "{signal}: {torn}, and the vault is locked again{kept}{also}"
    ));
}

/// `claude --version`'s version, parsed from the raw view: an identity value learned for
/// redaction (an organization named `Claude Code`, say) must not break it. The error shows
/// only the redacted view.
fn claude_version(ran: &Ran) -> Result<CcVersion, HarnessError> {
    ran.parse(|out| CcVersion::from_claude_output(out).ok())
        .ok_or_else(|| {
            harness(format!(
                "`claude --version` printed {:?}, which holds no version",
                tail(&ran.stdout_text(), 200)
            ))
        })
}

/// `RunLock` in the compat store this process's `HOME` and `XDG_STATE_HOME` name.
pub fn take_run_lock() -> Result<RunLock, HarnessError> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or_else(|| harness("HOME is not set"))?);
    RunLock::take(&layout::state_dir(
        &home,
        std::env::var_os("XDG_STATE_HOME").as_deref(),
    ))
}

fn setup(
    args: &CompatArgs,
    workspace: &Path,
    reports: &Path,
    report: &mut Report,
) -> Result<Ctx, HarnessError> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or_else(|| harness("HOME is not set"))?);
    let state = layout::state_dir(&home, std::env::var_os("XDG_STATE_HOME").as_deref());
    let db = state.join("xdg/data/tagteam/tagteam.db");
    if !db.is_file() {
        return Err(harness(format!(
            "no compat store in {}: run `cargo xtask compat login` first",
            state.display()
        )));
    }
    // Before any child runs, so nothing it prints reaches the report unredacted.
    report.redact = store::redactor(&db)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let claude = which_in("claude", &path).ok_or_else(|| harness("claude is not on PATH"))?;
    let tagteam = build_tagteam(workspace)?;
    let layout = Layout {
        workspace: workspace.to_path_buf(),
        reports: reports.to_path_buf(),
        state: fs::canonicalize(&state)?,
        scratch: make_scratch()?,
    };
    let scratch = layout.scratch.clone();
    let made = prepare(args, layout, home, &path, claude, tagteam, report);
    if made.is_err() && !args.keep {
        let _ = fs::remove_dir_all(&scratch);
    }
    made
}

/// Setup once the scratch directory exists: the guard over every home a run will use, the
/// vault unlocked, `claude`'s version, the scratch default home seeded, and the account found.
fn prepare(
    args: &CompatArgs,
    layout: Layout,
    home: PathBuf,
    path: &OsStr,
    claude: PathBuf,
    tagteam: PathBuf,
    report: &mut Report,
) -> Result<Ctx, HarnessError> {
    let macos = cfg!(target_os = "macos");
    let roots = Roots {
        scratch: layout.scratch.clone(),
        state: layout.state.clone(),
        users: user_homes(&home),
        home: home.clone(),
        user: std::env::var("USER").ok(),
    };
    let live = layout.live().to_string_lossy().into_owned();
    let items = roots.services(&live)?;
    report.setup.push(note(
        "the scratch default home",
        json!({"home": live, "items": items}),
    ));
    let vault = if macos {
        Some(VaultKeychain::open(&layout.vault_keychain(), &layout.vault_password())?.unlock()?)
    } else {
        None
    };
    let base = base_env(&layout, &home, path, vault.as_deref());
    let mut ctx = Ctx {
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
        ssh: args.ssh.clone(),
        redact: report.redact.clone(),
        cancel: cancel().clone(),
        daemons: Vec::new(),
    };

    let ran = ctx
        .claude(&live, &["--version"])
        .timeout(Duration::from_secs(30))
        .run(&ctx.roots)?;
    let version = claude_version(&ran)?;
    let tested_text = fs::read_to_string(tested_version_file(&ctx.layout.workspace))?;
    let tested = CcVersion::from_tested_file(&tested_text).map_err(harness)?;
    report.claude = Some(version.to_string());
    report.tested = Some(tested.to_string());
    report.setup.push(note(
        "claude",
        json!({"path": ctx.claude, "newerThanTested": version > tested}),
    ));
    let managed: Vec<&str> = MANAGED_SETTINGS
        .into_iter()
        .filter(|p| Path::new(p).exists())
        .collect();
    report.setup.push(note(
        "managed settings, which apply to every check",
        json!(managed),
    ));

    // The default home's own copies of the files a profile links to (§12.2).
    let dir = ctx.layout.live();
    fs::write(dir.join("settings.json"), "{}\n")?;
    fs::write(
        dir.join("history.jsonl"),
        format!(
            "{}\n",
            json!({"display": "tagteam compat", "pastedContents": {}, "timestamp": now_ms(),
                   "project": ctx.layout.work()})
        ),
    )?;
    fs::write(dir.join("CLAUDE.md"), "# tagteam compat\n")?;
    fs::write(dir.join("keybindings.json"), "{\"bindings\": []}\n")?;
    ctx.seed_trust(&live)?;

    let list = ctx.list()?;
    ctx.oauth = account(&list, ALIAS_OAUTH).ok_or_else(|| {
        harness("the compat store has no compat-oauth account: run `cargo xtask compat login`")
    })?;
    ctx.setup_token = account(&list, ALIAS_SETUP_TOKEN);
    for a in std::iter::once(&ctx.oauth).chain(ctx.setup_token.as_ref()) {
        ctx.roots
            .services(&ctx.layout.profile(&a.id).to_string_lossy())?;
    }
    let row = list["accounts"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["alias"] == ALIAS_OAUTH))
        .cloned()
        .unwrap_or(Value::Null);
    report.setup.push(note(
        "the account",
        json!({"position": ctx.oauth.position, "usageStatus": row["usageStatus"],
               "setupToken": ctx.setup_token.is_some()}),
    ));
    report
        .setup
        .push(note("the lineage at the start", ctx.lineage()?));
    Ok(ctx)
}

/// The checks phase by phase, then quiescence (`quiesce`): teardown starts only once every
/// child they started has ended, so none can write a credential while it captures and deletes.
/// A signal stops it before the next check, and quiescence still runs.
fn run_checks(ctx: &mut Ctx, selected: &[usize], report: &mut Report) -> Result<(), HarnessError> {
    'phases: for phase in [Phase::Standalone, Phase::Profile, Phase::Live] {
        let here: Vec<usize> = selected
            .iter()
            .copied()
            .filter(|&i| CHECKS[i].meta.phase == phase)
            .collect();
        if here.is_empty() {
            continue;
        }
        if phase == Phase::Live {
            if let Err(e) = enter_live(ctx, report) {
                report.harness_error = Some(e.0);
                break;
            }
        }
        for i in here {
            if cancel().requested().is_some() {
                break 'phases;
            }
            let check = &CHECKS[i];
            let started = Instant::now();
            let outcome = if check.meta.macos_only && !ctx.macos {
                Outcome::skip("needs the macOS Keychain")
            } else {
                eprintln!("cargo xtask compat: {}", check.meta.id);
                match catch_unwind(AssertUnwindSafe(|| (check.run)(ctx))) {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(e)) => Outcome::error(e.0, Vec::new()),
                    Err(_) => Outcome::error("the check panicked", Vec::new()),
                }
            };
            report.checks.push(CheckResult {
                id: check.meta.id,
                title: check.meta.title,
                // Redacted as it is stored, not only when the files are written.
                outcome: ctx.redact.outcome(&outcome),
                seconds: started.elapsed().as_secs_f64(),
            });
        }
    }
    quiesce()
}

/// The live phase: no daemon left in a profile, then the account activated into the scratch
/// default home as on a fresh machine (no global config at all, §9.5). The switch takes a
/// rotation a quiescent profile holds into the vault first (§9.2), so the default home gets the
/// newest generation; if it did not, the phase is not entered, since the default home would
/// hold a consumed one.
fn enter_live(ctx: &mut Ctx, report: &mut Report) -> Result<(), HarnessError> {
    if ctx.live_active {
        return Ok(());
    }
    for a in std::iter::once(ctx.oauth.clone()).chain(ctx.setup_token.clone()) {
        if let Some((_, spelling)) = ctx.profile_of(&a.id)? {
            ctx.stop_daemon(&spelling)?;
        }
    }
    let ahead = match ctx.profile()? {
        Some((dir, spelling)) => ctx.uncaptured(&dir, &spelling)?,
        None => None,
    };
    let live = ctx.live();
    let config = ctx.paths(&live).global_config;
    if config.exists() {
        fs::remove_file(&config)?;
    }
    report
        .setup
        .push(note("the activation", ctx.switch(ALIAS_OAUTH)?));
    ctx.activation_config = Some(fs::read(&config)?);
    let lineage = ctx.lineage()?;
    report
        .setup
        .push(note("the lineage once live", lineage.clone()));
    if let Some(rotated) = ahead {
        if lineage["live"] != json!(rotated) {
            return Err(harness(
                "the activation did not take the profile's rotation into the vault; the live phase is not entered",
            ));
        }
    }
    ctx.live_active = true;
    Ok(())
}

/// The directory `claude`'s daemon keeps its sockets in for the home `spelling` (Appendix A.7).
fn daemon_sockets(uid: u32, spelling: &str) -> PathBuf {
    let hash = hex::encode(Sha256::digest(spelling.as_bytes()));
    PathBuf::from(format!("/tmp/cc-daemon-{uid}/{}", &hash[..8]))
}

/// Puts everything back. The account's newest generation goes into the vault first: the
/// account is made the default home's live login again (which captures a rotated profile),
/// and `tagteam add` takes the default home's login (§10.1). Nothing is deleted unless the
/// vault then holds the default home's generation. Then, unless `--keep`, once teardown's own
/// children have ended: every profile is dropped, every scratch home's credential deleted
/// (`Ctx::forget`), the daemons' socket directories and the scratch directory removed. The
/// vault is locked last.
fn teardown(ctx: &mut Ctx, keep: bool, report: &mut Report) -> Result<(), HarnessError> {
    let mut spellings = vec![ctx.live()];
    for a in std::iter::once(ctx.oauth.clone()).chain(ctx.setup_token.clone()) {
        if let Some((_, spelling)) = ctx.profile_of(&a.id)? {
            spellings.push(spelling);
        }
    }
    let step = |report: &mut Report, name: &'static str| {
        report.teardown.get_or_insert_with(Vec::new).push(name);
    };
    report.teardown.get_or_insert_with(Vec::new);
    for spelling in &spellings {
        ctx.stop_daemon(spelling)?;
    }
    step(report, "daemons stopped");
    enter_live(ctx, report)?;
    ctx.switch(ALIAS_OAUTH)?;
    step(report, "account activated into the default home");
    let _ = ctx.tagteam(&["remove", ALIAS_API_KEY]).run(&ctx.roots);
    let added = ctx.tagteam(&["add", "--json"]).run(&ctx.roots)?;
    let lineage = ctx.lineage()?;
    report
        .setup
        .push(note("the lineage at the end", lineage.clone()));
    if !added.success() || lineage["live"] != lineage["vault"] {
        return Err(harness(format!(
            "the vault does not hold the default home's generation ({}); nothing was deleted, and {} is kept",
            added.summary(),
            ctx.layout.scratch.display()
        )));
    }
    step(report, "default home's generation taken into the vault");
    if keep {
        eprintln!(
            "cargo xtask compat: kept {} and the profiles; never run claude in them, since the vault holds their generation",
            ctx.layout.scratch.display()
        );
        report
            .setup
            .push(note("kept", json!({"scratch": ctx.layout.scratch})));
    } else {
        // Teardown's own children too.
        quiesce()?;
        for a in std::iter::once(ctx.oauth.clone()).chain(ctx.setup_token.clone()) {
            if let Some((dir, spelling)) = ctx.profile_of(&a.id)? {
                ctx.drop_profile(&dir, &spelling)?;
            }
        }
        step(report, "profiles dropped");
        let mut homes = vec![ctx.live()];
        for e in fs::read_dir(ctx.layout.homes())?.flatten() {
            homes.push(e.path().to_string_lossy().into_owned());
        }
        for spelling in &homes {
            ctx.forget(spelling)?;
        }
        step(report, "credentials deleted");
        let uid = fs::metadata(&ctx.layout.scratch)?.uid();
        for spelling in spellings.iter().chain(&homes) {
            let sockets = daemon_sockets(uid, spelling);
            if sockets.is_dir() {
                fs::remove_dir_all(sockets)?;
            }
        }
        step(report, "daemon sockets removed");
        fs::remove_dir_all(&ctx.layout.scratch)?;
        step(report, "scratch directory deleted");
    }
    if let Some(v) = &ctx.vault {
        v.lock()?;
    }
    step(report, TEARDOWN_LAST);
    Ok(())
}

/// `--bless`: after a full pass, the version of `claude` that passed into
/// `compat/tested-cc-version` (§15.4), the only way the tested version advances.
fn bless(workspace: &Path, report: &mut Report) {
    let required: Vec<&str> = CHECKS
        .iter()
        .filter(|c| c.meta.opt_in.is_none())
        .map(|c| c.meta.id)
        .collect();
    if !report.passed_all(&required) {
        eprintln!("cargo xtask compat: not blessed: every check must run and pass");
        return;
    }
    let Some(version) = report.claude.as_deref().and_then(CcVersion::parse) else {
        return;
    };
    let path = tested_version_file(workspace);
    let written = fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| blessed(&text, version))
        .and_then(|text| fs::write(&path, text).map_err(|e| e.to_string()));
    match written {
        Ok(()) => report.blessed = Some(version.to_string()),
        Err(e) => {
            report.harness_error = Some(format!("--bless could not write {}: {e}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(only: &[&str], bless: bool) -> CompatArgs {
        CompatArgs {
            command: None,
            keep: false,
            ssh: None,
            locked_keychain: false,
            only: only.iter().map(|s| (*s).to_owned()).collect(),
            bless,
        }
    }

    #[test]
    fn a_selection_error_stops_the_run_before_anything_is_made() {
        assert_eq!(run(&args(&["no-such-check"], false)), EXIT_HARNESS);
        assert_eq!(run(&args(&["ssh-keychain-read"], false)), EXIT_HARNESS);
        assert_eq!(
            run(&args(&["auth-status"], true)),
            EXIT_HARNESS,
            "--bless with --only"
        );
    }

    #[test]
    fn a_signal_ends_the_children_then_locks_the_vault_then_lets_go_of_the_lock() {
        let _serial = crate::compat::sys::serial();
        use crate::compat::keychain::{Relock, Unlocked};
        use crate::compat::sys::{Cmd, signal, wait_until};
        use std::cell::Cell;
        use std::rc::Rc;

        struct Vault(Rc<Cell<u32>>);
        impl Relock for Vault {
            fn relock(&self) -> Result<(), HarnessError> {
                self.0.set(self.0.get() + 1);
                Ok(())
            }
        }
        let state = std::env::temp_dir().join(format!("xtask-signal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        let roots = Roots {
            scratch: PathBuf::from("/tmp/tagteam-compat.test"),
            state: PathBuf::from("/nonexistent/state"),
            users: vec![],
            home: std::env::temp_dir(),
            user: None,
        };
        // The run's own token stands in for the process's, which a real SIGTERM sets
        // (`a_signal_sets_the_cancel_token` pins that wiring).
        let token = Cancel::new();
        let locks = Rc::new(Cell::new(0));
        let mut report = Report::default();
        let lock = RunLock::take(&state).unwrap();
        let group = {
            let _vault = Unlocked::new(Vault(locks.clone()));
            let child = Cmd::new(
                Path::new("/bin/sh"),
                vec![(
                    "CLAUDE_CONFIG_DIR".into(),
                    "/tmp/tagteam-compat.test/live".into(),
                )],
                Path::new("/"),
            )
            .args(["-c", "sleep 30 & wait"])
            .cancel(&token)
            .spawn(&roots)
            .unwrap();
            let group = child.pid();
            let signaller = token.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                signaller.request(15);
            });
            let stopped = child.wait().unwrap_err();
            assert_eq!(stopped.0, "interrupted by SIGTERM");
            assert!(
                !signal(group, "0", true),
                "the child's group ended and was reaped"
            );
            assert_eq!(
                locks.get(),
                0,
                "the vault is still unlocked while the run unwinds"
            );
            group
        };
        assert_eq!(locks.get(), 1, "then the vault is locked again");
        assert!(RunLock::take(&state).is_err(), "the run lock is still held");
        conclude(&mut report, &token, None);
        drop(lock);
        assert!(
            wait_until(Duration::from_secs(1), || RunLock::take(&state).is_ok()),
            "and let go of last"
        );
        assert_eq!(report.exit_code(), 143);
        assert!(
            report
                .harness_error
                .as_deref()
                .is_some_and(|e| e.starts_with("interrupted by SIGTERM")),
            "{report:?}"
        );
        assert!(!signal(group, "0", true));
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn the_version_is_read_from_the_raw_view_and_shown_redacted() {
        let _serial = crate::compat::sys::serial();
        use crate::compat::report::Redactor;
        use crate::compat::sys::Cmd;
        let roots = Roots {
            scratch: PathBuf::from("/tmp/tagteam-compat.test"),
            state: PathBuf::from("/nonexistent/state"),
            users: vec![],
            home: std::env::temp_dir(),
            user: None,
        };
        let mut redact = Redactor::default();
        redact.learn("Claude Code", "<account 1 org>".into());
        redact.learn("t@x.co", "<account 1>".into());
        let ran = |script: &str| {
            Cmd::new(
                Path::new("/bin/sh"),
                vec![(
                    "CLAUDE_CONFIG_DIR".into(),
                    "/tmp/tagteam-compat.test/live".into(),
                )],
                Path::new("/"),
            )
            .args(["-c", script])
            .redact(&redact)
            .run(&roots)
            .unwrap()
        };
        let good = ran("echo '2.1.286 (Claude Code)'");
        assert_eq!(claude_version(&good).unwrap().to_string(), "2.1.286");
        assert_eq!(good.stdout_text(), "2.1.286 (<account 1 org>)\n");
        let bad = ran("echo 'Claude Code: log in as t@x.co'");
        let refused = claude_version(&bad).unwrap_err();
        let report = Report {
            claude: Some(claude_version(&good).unwrap().to_string()),
            harness_error: Some(refused.0),
            setup: vec![note("claude --version", good.summary())],
            redact,
            ..Report::default()
        };
        let dir = std::env::temp_dir().join(format!("xtask-version-{}", std::process::id()));
        report.write(&dir).unwrap();
        for name in ["report.json", "report.md"] {
            let written = fs::read_to_string(dir.join(name)).unwrap();
            assert!(!written.contains("Claude Code"), "{name}: {written}");
            assert!(!written.contains("t@x.co"), "{name}: {written}");
            assert!(written.contains("2.1.286"), "{name}: {written}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_daemon_s_sockets_are_named_by_its_home_s_hash() {
        assert_eq!(
            daemon_sockets(501, "/tmp/tagteam-r1-spike"),
            PathBuf::from("/tmp/cc-daemon-501/ba6c431d")
        );
    }

    #[test]
    fn every_child_gets_the_real_home_the_compat_xdg_and_no_cc_variable() {
        let layout = Layout {
            workspace: PathBuf::from("/w"),
            reports: PathBuf::from("/w/target/compat"),
            state: PathBuf::from("/s"),
            scratch: PathBuf::from("/t"),
        };
        let env = base_env(&layout, Path::new("/home/t"), OsStr::new("/bin"), None);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("HOME"), Some("/home/t".into()));
        assert_eq!(get("XDG_DATA_HOME"), Some("/s/xdg/data".into()));
        assert_eq!(get("DISABLE_AUTOUPDATER"), Some("1".into()));
        assert!(
            env.iter().all(|(n, _)| {
                let n = n.to_string_lossy();
                !n.starts_with("CLAUDE") && !n.starts_with("ANTHROPIC")
            }),
            "{env:?}"
        );
    }

    #[test]
    fn the_binary_is_the_one_cargo_reports_for_the_tagteam_bin() {
        let out = [
            r#"{"reason":"compiler-artifact","target":{"name":"tagteam_cc","kind":["lib"]},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"tagteam","kind":["lib"]},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"xtask","kind":["bin"]},"executable":"/t/debug/xtask"}"#,
            "not json at all",
            r#"{"reason":"compiler-artifact","target":{"name":"tagteam","kind":["bin"]},"executable":"/elsewhere/aarch64-apple-darwin/debug/tagteam"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        assert_eq!(
            tagteam_executable(&out),
            Some(PathBuf::from(
                "/elsewhere/aarch64-apple-darwin/debug/tagteam"
            ))
        );
        // No record for the bin, or one without an executable: no path is guessed.
        assert_eq!(tagteam_executable(""), None);
        assert_eq!(
            tagteam_executable(
                r#"{"reason":"compiler-artifact","target":{"name":"tagteam","kind":["bin"]},"executable":null}"#
            ),
            None
        );
    }

    #[test]
    fn a_signal_mid_teardown_says_how_far_teardown_got() {
        let token = Cancel::new();
        token.request(2);
        let concluded = |teardown: Option<Vec<&'static str>>| {
            let mut report = Report {
                teardown,
                ..Report::default()
            };
            conclude(&mut report, &token, None);
            assert_eq!(report.exit_code(), 130);
            report.harness_error.unwrap()
        };
        assert_eq!(
            concluded(None),
            "interrupted by SIGINT: nothing was torn down, and the vault is locked again"
        );
        assert_eq!(
            concluded(Some(vec![])),
            "interrupted by SIGINT: teardown was interrupted before any step finished; nothing was deleted, and the vault is locked again"
        );
        let mid = concluded(Some(vec![
            "daemons stopped",
            "default home's generation taken into the vault",
        ]));
        assert!(
            mid.contains(
                "teardown was interrupted after: daemons stopped, default home's generation taken into the vault; the next step may be partly done and the rest were not done"
            ) && !mid.contains("nothing was torn down"),
            "{mid}"
        );
        let done = concluded(Some(vec!["daemons stopped", TEARDOWN_LAST]));
        assert!(done.contains("teardown had finished"), "{done}");
    }

    #[test]
    fn teardown_refuses_while_a_process_or_a_daemon_remains() {
        let scratch = Path::new("/tmp/tagteam-compat.test");
        assert!(teardown_refusal(Ok(()), Ok(()), scratch).is_none());
        for (quiet, stopped, named) in [
            (
                Err(harness("sleep 30 (process group 7) still holds a process")),
                Ok(()),
                "process group 7",
            ),
            (
                Ok(()),
                Err(harness("a daemon in /h still runs (pid 9)")),
                "pid 9",
            ),
        ] {
            let refused = teardown_refusal(quiet, stopped, scratch).unwrap().0;
            assert!(
                refused.starts_with("teardown refused, since ")
                    && refused.contains(named)
                    && refused.contains("no credential was touched")
                    && refused.contains("/tmp/tagteam-compat.test is kept"),
                "{refused}"
            );
        }
        let both = teardown_refusal(Err(harness("a")), Err(harness("b")), scratch).unwrap();
        assert!(both.0.contains("a; b"), "{both}");
    }
}
