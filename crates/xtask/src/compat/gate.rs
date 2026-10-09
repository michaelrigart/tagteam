//! The settle gate: before any check, every registered profile is brought to a known state, so
//! that no run inherits the last one's damage. A daemon a previous run left would keep the
//! account session-owned, and tagteam then joins every launch without capturing, relinking or
//! re-seeding (§12.5). The gate stops each profile's daemon, requires it quiescent, runs one
//! quiescent launch, which takes the profile's rotation into the vault, relinks it to this
//! run's scratch home and trusts this run's work directory, and requires the account's lineage
//! to agree: the profile holds the vault's generation. Any failure is a harness error naming
//! what is not settled.

use std::path::Path;

use serde_json::{Value, json};
use tagteam_provider::profile::ProfileMarker;
use tagteam_provider::{Read, SystemProcessProbe};

use super::checks::read_json;
use super::ctx::{ALIAS_OAUTH, ALIAS_SETUP_TOKEN, Ctx};
use super::daemon;
use super::guard::{CONFIG_DIR, SECURE_STORAGE_DIR};
use super::sys::{HarnessError, harness};

fn unsettled(what: &str, why: impl std::fmt::Display) -> HarnessError {
    harness(format!("settle gate: {what}: {why}"))
}

/// Whether a profile's session files name nothing alive (`daemon::owners`).
fn quiet(alias: &str, spelling: &str) -> Result<(), HarnessError> {
    let owners = daemon::owners(Path::new(spelling), &SystemProcessProbe);
    if owners.is_clear() {
        Ok(())
    } else {
        Err(unsettled(
            &format!("the profile of {alias} is not quiescent"),
            owners.describe(),
        ))
    }
}

/// The account's lineage agrees: the profile holds a generation, and it is the vault's.
fn lineage_agrees(lineage: &Value) -> Result<(), HarnessError> {
    if lineage["profile"].is_null() || lineage["profile"] != lineage["vault"] {
        return Err(unsettled(
            "the lineage disagrees after the quiescent launch",
            format!("vault {}, profile {}", lineage["vault"], lineage["profile"]),
        ));
    }
    Ok(())
}

/// The profile's own global config trusts this run's work directory (§12.4 seeds `projects`).
fn trusts_work(dir: &Path, work: &str) -> Result<(), HarnessError> {
    let config = read_json(&dir.join(".claude.json")).unwrap_or(Value::Null);
    if config["projects"][work]["hasTrustDialogAccepted"] == true {
        Ok(())
    } else {
        Err(unsettled(
            "the profile does not trust this run's work directory",
            work,
        ))
    }
}

/// Runs the gate. The evidence it returns names what it stopped and the lineage it found.
pub fn settle(ctx: &Ctx) -> Result<Value, HarnessError> {
    let accounts = std::iter::once((ALIAS_OAUTH, ctx.oauth.id.clone())).chain(
        ctx.setup_token
            .iter()
            .map(|a| (ALIAS_SETUP_TOKEN, a.id.clone())),
    );
    let mut profiles = Vec::new();
    for (alias, id) in accounts {
        let Some((_, spelling)) = ctx.profile_of(&id)? else {
            profiles.push(json!({"alias": alias, "profile": false}));
            continue;
        };
        let stopped = ctx
            .stop_daemon(&spelling)
            .map_err(|e| unsettled(&format!("the daemon of {alias} is not stopped"), e))?;
        quiet(alias, &spelling)?;
        profiles.push(json!({"alias": alias, "profile": true, "daemonStop": stopped}));
    }

    // Only the oauth profile is launched. The setup-token profile is stopped and checked
    // quiescent above, but not launched: its first launch, in `auth-status`, re-marks and
    // relinks it, and a refused launch there is an outcome that check reports, not a reason to
    // stop the run.
    let ran = ctx.run_profile(&["--version"])?;
    if !ran.success() {
        return Err(unsettled(
            "the quiescent launch failed",
            format!("{}", ran.summary()),
        ));
    }
    let (dir, spelling) = ctx
        .profile()?
        .ok_or_else(|| unsettled("the launch made no profile", ALIAS_OAUTH))?;
    quiet(ALIAS_OAUTH, &spelling)?;
    let lineage = ctx.lineage()?;
    lineage_agrees(&lineage)?;
    trusts_work(&dir, &ctx.layout.work().to_string_lossy())?;
    outer_is_this_run(ctx, &dir)?;
    Ok(json!({"profiles": profiles, "lineage": lineage}))
}

/// The outer home the marker records is exactly what compat exports: the scratch default home
/// as `CLAUDE_CONFIG_DIR`, and compat's own `CLAUDE_SECURESTORAGE_CONFIG_DIR` (none, so null).
/// A quiescent launch re-marks it (§12.5), so anything else is not settled: null, a missing key,
/// another home, or a marker that cannot be read.
fn outer_is_this_run(ctx: &Ctx, dir: &Path) -> Result<(), HarnessError> {
    let secure = ctx
        .base
        .iter()
        .find(|(k, _)| k == SECURE_STORAGE_DIR)
        .map(|(_, v)| json!(v.to_string_lossy()))
        .unwrap_or(Value::Null);
    let expected = json!({CONFIG_DIR: ctx.live(), SECURE_STORAGE_DIR: secure});
    match ProfileMarker::read(dir) {
        Read::Present(m) if m.outer == expected => Ok(()),
        Read::Present(m) => Err(unsettled(
            "the profile still records another outer home",
            format!("its marker's outer is {}, not {expected}", m.outer),
        )),
        Read::Absent => Err(unsettled("the profile has no marker", dir.display())),
        Read::Unreadable(e) => Err(unsettled("the profile's marker cannot be read", e)),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use super::*;
    use crate::compat::ctx::Account;
    use crate::compat::layout::make_scratch;
    use crate::compat::report::Redactor;

    const ID: &str = "01a";

    fn credential(refresh: &str) -> String {
        json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": refresh, "expiresAt": 1}})
            .to_string()
    }

    /// A compat store holding the account's profile and vault entry, a scratch directory, and a
    /// fake `tagteam` that exits `launch_exit` (printing a `--json`-style error when it fails).
    struct Fixture {
        ctx: Ctx,
        state: PathBuf,
        profile: PathBuf,
    }

    impl Fixture {
        fn new(launch_exit: i32) -> Self {
            let scratch = make_scratch().unwrap();
            let state = std::env::temp_dir().join(format!(
                "xtask-gate-{}",
                crate::compat::keychain::random_hex().unwrap()
            ));
            fs::create_dir_all(&state).unwrap();
            let state = fs::canonicalize(state).unwrap();
            let fake = scratch.join("tagteam");
            fs::write(
                &fake,
                format!("#!/bin/sh\n[ {launch_exit} -eq 0 ] || echo '{{\"error\":{{\"type\":\"session-owned\"}}}}'\nexit {launch_exit}\n"),
            )
            .unwrap();
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
            let mut ctx = Ctx::offline(&scratch, &state, fake, Redactor::default());
            ctx.oauth = Account {
                id: ID.into(),
                position: 1,
                email: "t@x.co".into(),
            };
            let profile = ctx.layout.profile(ID);
            fs::create_dir_all(&profile).unwrap();
            let profile = fs::canonicalize(profile).unwrap();
            fs::write(
                profile.join(MARKER),
                json!({"format": "tagteam-profile", "version": 1, "provider": "claude-code",
                       "accountId": ID, "configDir": profile,
                       "outer": {"CLAUDE_CONFIG_DIR": ctx.live(), "CLAUDE_SECURESTORAGE_CONFIG_DIR": null}})
                .to_string(),
            )
            .unwrap();
            fs::write(profile.join(".credentials.json"), credential("rt-1")).unwrap();
            fs::write(
                profile.join(".claude.json"),
                json!({"projects": {ctx.layout.work().to_string_lossy(): {"hasTrustDialogAccepted": true}}})
                    .to_string(),
            )
            .unwrap();
            let vault = ctx.layout.data_dir().join("vault");
            fs::create_dir_all(&vault).unwrap();
            fs::write(vault.join(format!("{ID}.json")), credential("rt-1")).unwrap();
            Self {
                ctx,
                state,
                profile,
            }
        }

        fn done(self) {
            fs::remove_dir_all(&self.ctx.layout.scratch).unwrap();
            let _ = fs::remove_dir_all(&self.state);
        }
    }

    const MARKER: &str = ".tagteam-profile.json";

    fn refused(f: &Fixture) -> String {
        settle(&f.ctx).unwrap_err().0
    }

    #[test]
    fn a_quiescent_profile_in_step_with_the_vault_settles() {
        let _serial = crate::compat::sys::serial();
        let f = Fixture::new(0);
        let evidence = settle(&f.ctx).unwrap();
        assert_eq!(evidence["lineage"]["profile"], evidence["lineage"]["vault"]);
        assert_eq!(evidence["profiles"][0]["alias"], "compat-oauth");
        f.done();
    }

    #[test]
    fn the_gate_refuses_what_is_not_settled_and_names_it() {
        let _serial = crate::compat::sys::serial();

        // A session still alive in the profile.
        let f = Fixture::new(0);
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 60 >/dev/null 2>&1 & echo $!"])
            .output()
            .unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        fs::create_dir_all(f.profile.join("sessions")).unwrap();
        fs::write(
            f.profile.join("sessions/1.json"),
            json!({"pid": pid, "kind": "interactive"}).to_string(),
        )
        .unwrap();
        let e = refused(&f);
        crate::compat::sys::signal(pid, "KILL", false);
        assert!(
            e.starts_with("settle gate: ")
                && e.contains("not quiescent")
                && e.contains(&format!("pid {pid}")),
            "{e}"
        );
        f.done();

        // The launch fails: its stdout (the --json error) is in the message.
        let f = Fixture::new(1);
        let e = refused(&f);
        assert!(
            e.contains("the quiescent launch failed") && e.contains("session-owned"),
            "{e}"
        );
        f.done();

        // The profile is ahead of the vault: the lineage disagrees.
        let f = Fixture::new(0);
        fs::write(f.profile.join(".credentials.json"), credential("rt-2")).unwrap();
        let e = refused(&f);
        assert!(e.contains("the lineage disagrees"), "{e}");
        f.done();

        // The profile does not trust this run's work directory.
        let f = Fixture::new(0);
        fs::write(
            f.profile.join(".claude.json"),
            r#"{"projects":{"/old/work":{"hasTrustDialogAccepted":true}}}"#,
        )
        .unwrap();
        let e = refused(&f);
        assert!(
            e.contains("does not trust this run's work directory"),
            "{e}"
        );
        f.done();
    }

    #[test]
    fn the_marker_s_outer_must_be_exactly_what_compat_exports() {
        let _serial = crate::compat::sys::serial();
        let write = |f: &Fixture, outer: Value| {
            let mut marker: Value =
                serde_json::from_slice(&fs::read(f.profile.join(MARKER)).unwrap()).unwrap();
            marker["outer"] = outer;
            fs::write(f.profile.join(MARKER), marker.to_string()).unwrap();
        };
        let exact = |f: &Fixture| json!({"CLAUDE_CONFIG_DIR": f.ctx.live(), "CLAUDE_SECURESTORAGE_CONFIG_DIR": null});

        let f = Fixture::new(0);
        settle(&f.ctx).expect("the exact match settles");
        f.done();

        for (what, outer) in [
            ("null", Some(Value::Null)),
            ("a missing key", Some(json!({"CLAUDE_CONFIG_DIR": "LIVE"}))),
            (
                "another string",
                Some(json!({"CLAUDE_CONFIG_DIR": "/tmp/tagteam-compat.old/live",
                            "CLAUDE_SECURESTORAGE_CONFIG_DIR": null})),
            ),
            (
                "a secure-storage dir compat does not export",
                Some(json!({"CLAUDE_CONFIG_DIR": "LIVE", "CLAUDE_SECURESTORAGE_CONFIG_DIR": "/x"})),
            ),
            (
                "an extra key",
                Some(json!({"CLAUDE_CONFIG_DIR": "LIVE",
                            "CLAUDE_SECURESTORAGE_CONFIG_DIR": null, "HOME": "/h"})),
            ),
        ] {
            let f = Fixture::new(0);
            let outer = outer.unwrap().to_string().replace("LIVE", &f.ctx.live());
            write(&f, serde_json::from_str(&outer).unwrap());
            let e = refused(&f);
            // A marker with no outer object is unreadable to tagteam itself, and refused as such.
            assert!(
                e.contains("another outer home") || e.contains("it has no outer record"),
                "{what}: {e}"
            );
            f.done();
        }

        // An unreadable marker.
        let f = Fixture::new(0);
        fs::write(f.profile.join(MARKER), "not json").unwrap();
        let e = refused(&f);
        assert!(
            e.contains("the profile marker") && e.contains("unreadable"),
            "{e}"
        );
        f.done();

        // The exact value is what the other tests' fixture writes.
        let f = Fixture::new(0);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(f.profile.join(MARKER)).unwrap()).unwrap()["outer"],
            exact(&f)
        );
        f.done();
    }
}
