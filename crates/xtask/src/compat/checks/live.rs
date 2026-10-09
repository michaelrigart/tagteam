//! The live phase: tagteam has made the test account the scratch default home's live login, so
//! the default home is where its lineage advances, and plain `claude` runs there. The checks
//! that must see which credential CC sends point it at the local stand-in (`capture`) and
//! spend nothing; a dummy API key and the setup token are the other credentials they switch to.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_cc::locks::acquire_config;
use tagteam_provider::atomic::write_atomic_private;
use tagteam_provider::splice::{get_top_level, remove_top_level, replace_top_level};
use tagteam_provider::{Cancel, Read};

use super::profile::read_name;
use super::{keys, new_record, records};
use crate::compat::capture::{Capture, Sent};
use crate::compat::ctx::{
    ALIAS_API_KEY, ALIAS_OAUTH, ALIAS_SETUP_TOKEN, Ctx, MODEL, PROMPT, dummy_key, generation,
    now_ms,
};
use crate::compat::report::{Outcome, Probe, fingerprint};
use crate::compat::store;
use crate::compat::sys::{HarnessError, harness, pause, wait_until};

const STATUS: [&str; 3] = ["auth", "status", "--json"];
const REQUEST: [&str; 6] = ["-p", PROMPT, "--model", MODEL, "--max-turns", "1"];
const HOLD: Duration = Duration::from_secs(3);
const MCP_SERVER: &str = "tagteam-compat-probe";

fn sent_json(s: Option<Sent>) -> Value {
    s.map_or(Value::Null, |s| s.to_json())
}

/// The access token `credential` carries, fingerprinted as the stand-in records it.
fn bearer(credential: &[u8]) -> Option<Sent> {
    let v: Value = serde_json::from_slice(credential).ok()?;
    v["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(|t| Sent::Bearer(fingerprint(t)))
}

/// §9.5: CC accepts the global config tagteam created on a fresh machine (the activation that
/// opened this phase made it), and CC's next write of it re-renders, byte for byte, as tagteam's
/// splice renders each top-level value: that pins §9.5's rendering against `JSON.stringify`.
pub fn fresh_global_config(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let live = ctx.live();
    let path = ctx.paths(&live).global_config;
    let created = ctx
        .activation_config
        .clone()
        .ok_or_else(|| harness("the activation's global config was not recorded"))?;
    let made: Value = serde_json::from_slice(&created)
        .map_err(|_| harness("tagteam's global config is not JSON"))?;
    p.expect_eq(
        "tagteam created it holding only oauthAccount (§9.5)",
        json!(["oauthAccount"]),
        json!(keys(&made)),
    );
    let ran = ctx.claude(&live, &REQUEST).run(&ctx.roots)?;
    p.expect("CC ran on it", ran.success(), ran.summary());
    let after = fs::read(&path)?;
    p.expect(
        "CC has written it since",
        after != created,
        json!({"bytesBefore": created.len(), "bytesAfter": after.len()}),
    );
    let doc: Value = serde_json::from_slice(&after)
        .map_err(|_| harness("CC left the global config unparseable"))?;
    p.expect(
        "the login is still the account's",
        doc["oauthAccount"]["emailAddress"] == made["oauthAccount"]["emailAddress"],
        json!(null),
    );
    let mut differ = Vec::new();
    for key in keys(&doc) {
        let spliced = replace_top_level(&after, &key, &doc[&key])
            .map_err(|e| harness(format!("splicing {key}: {e}")))?;
        if spliced != after {
            differ.push(key);
        }
    }
    p.expect(
        "every top-level value CC wrote renders as tagteam renders it",
        differ.is_empty(),
        json!(differ),
    );
    if doc["oauthAccount"] == made["oauthAccount"] {
        let same = replace_top_level(&after, "oauthAccount", &made["oauthAccount"])
            .map_err(|e| harness(format!("splicing oauthAccount: {e}")))?
            == after;
        p.expect(
            "the span tagteam spliced is byte-identical",
            same,
            json!(null),
        );
    } else {
        p.note(
            "CC changed oauthAccount itself; its fields",
            json!(keys(&doc["oauthAccount"])),
        );
    }
    let mut corrupt = Vec::new();
    for dir in [ctx.layout.live(), ctx.layout.live().join("backups")] {
        for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.contains("corrupt") {
                corrupt.push(name);
            }
        }
    }
    p.expect(
        "CC set no copy aside as corrupt",
        corrupt.is_empty(),
        json!(corrupt),
    );
    Ok(p.finish("CC took tagteam's file, and its write re-renders as tagteam's"))
}

/// §9.1: CC takes `<global config>.lock` around its own writes of the global config. While
/// tagteam holds it, `claude mcp add --scope user` writes nothing; once released, it writes.
pub fn config_lock(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let live = ctx.live();
    let paths = ctx.paths(&live);
    let before = fs::read(&paths.global_config)?;
    let lock = acquire_config(&paths, Duration::from_secs(9), &Cancel::new())
        .map_err(|e| harness(format!("CC's config lock: {e}")))?;
    let mut running = ctx
        .claude(
            &live,
            &[
                "mcp",
                "add",
                "--scope",
                "user",
                MCP_SERVER,
                "--",
                "/usr/bin/true",
            ],
        )
        .spawn(&ctx.roots)?;
    pause(HOLD);
    let during = fs::read(&paths.global_config)?;
    let finished_early = running.finished();
    drop(lock);
    let ran = running.wait()?;
    let after = fs::read(&paths.global_config)?;
    p.expect(
        "CC wrote nothing while tagteam held the lock",
        during == before,
        json!(null),
    );
    p.note(
        "claude mcp add had finished while it was held",
        json!(finished_early),
    );
    let added = serde_json::from_slice::<Value>(&after)
        .ok()
        .is_some_and(|v| v["mcpServers"].get(MCP_SERVER).is_some());
    p.expect("CC wrote once tagteam released it", added, ran.summary());
    let removed = ctx
        .claude(&live, &["mcp", "remove", "--scope", "user", MCP_SERVER])
        .run(&ctx.roots)?;
    p.note("claude mcp remove", removed.summary());
    Ok(p.finish("CC waited for the config lock tagteam held"))
}

/// §7.5 and §9.1: when CC and tagteam both find the live access token expired, CC's credential
/// locks serialize them, whichever starts first, and no generation is refreshed twice.
/// - CC first: CC refreshes holding the refresh lock. `tagteam list` then takes one of two
///   paths, both correct, and which one is a race real `claude` cannot be held to: it reads the
///   token still expired, waits on CC's credential locks (§9.1) and adopts CC's rotation into
///   the vault without a request (§7.5 steps 3 and 4), or it reads CC's fresh token and leaves
///   the active token to CC (§7.5). So only what holds on both is expected: both succeed, the
///   live store holds CC's new generation, tagteam sent no refresh of its own (resending the
///   generation CC consumed would fail, R9, and show as a refresh failure), and the vault holds
///   CC's generation or the one it held before. Which path ran is recorded.
/// - tagteam first: tagteam stops at its `active-before-request` point with the locks held
///   (`TAGTEAM_TEST_PAUSE_AT`); CC waits, then sends with the generation tagteam wrote, so both
///   end on one generation.
///
/// `tagteam list` runs the active refresh only inside a collection, and §8.3's floor and the
/// plan the last collection or switch recorded would hold it back, so each order first makes
/// the account due (`store::make_due`).
pub fn refresh_lock_interop(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let live = ctx.live();
    let paths = ctx.paths(&live);
    let db = ctx.layout.data_dir().join("tagteam.db");
    let vault_generation = |ctx: &Ctx| -> Result<Option<String>, HarnessError> {
        Ok(generation(&ctx.vault_credential(&ctx.oauth.id)?))
    };
    let live_generation = |ctx: &Ctx| -> Result<Option<String>, HarnessError> {
        Ok(ctx
            .read_credential(&ctx.live())?
            .and_then(|(b, _)| generation(&b)))
    };

    // CC first.
    store::make_due(&db, &ctx.oauth.id, now_ms() / 1000)?;
    let expired = ctx.expire(&live)?;
    let consumed = expired["generation"].clone();
    p.note("expired", expired);
    let vault_before = vault_generation(ctx)?;
    let cc = ctx.claude(&live, &REQUEST).spawn(&ctx.roots)?;
    let locked = wait_until(Duration::from_secs(60), || paths.refresh_lock.is_dir());
    let still_held = paths.refresh_lock.is_dir();
    let live_at_start = live_generation(ctx)?;
    let tt = ctx.tagteam(&["list", "--json"]).run(&ctx.roots)?;
    let cc = cc.wait()?;
    p.expect("CC first: CC took the refresh lock", locked, json!(null));
    p.expect("CC first: claude -p succeeded", cc.success(), cc.summary());
    p.expect(
        "CC first: tagteam list succeeded",
        tt.success(),
        tt.summary(),
    );
    let live_after = live_generation(ctx)?;
    let vault_after = vault_generation(ctx)?;
    p.expect(
        "CC first: the live store holds CC's new generation, not the one its refresh consumed (§7.5)",
        live_after.is_some() && json!(live_after) != consumed,
        json!({"consumed": consumed, "live": live_after}),
    );
    let row = tt
        .json()
        .and_then(|v| {
            v["accounts"]
                .as_array()?
                .iter()
                .find(|r| r["alias"] == ALIAS_OAUTH)
                .cloned()
        })
        .unwrap_or(Value::Null);
    let refresh_failed =
        row["usageError"] == "refresh-failed" || row["usageStatus"] == "relogin_required";
    p.expect(
        "CC first: tagteam sent no refresh of its own: its row shows no refresh failure, which resending the consumed generation would be (R9, §7.5 step 4)",
        !refresh_failed,
        ctx.redact
            .value(&json!({"usageStatus": row["usageStatus"], "usageError": row["usageError"]})),
    );
    p.expect(
        "CC first: the vault holds CC's generation or the one it held before, never an older one over a newer (§7.5 step 3)",
        vault_after.is_some() && (vault_after == live_after || vault_after == vault_before),
        json!({"before": vault_before, "after": vault_after, "live": live_after}),
    );
    let path = if vault_after == live_after && vault_before != live_after {
        "tagteam waited for CC's locks and adopted CC's generation into the vault, sending nothing (§7.5 steps 3 and 4)"
    } else if vault_after == vault_before {
        "tagteam found CC's fresh token and left it to CC: no active refresh ran (§7.5)"
    } else {
        "neither"
    };
    p.note(
        "CC first: the path tagteam took",
        json!({"path": path, "lockHeldAtStart": still_held, "liveAtStart": live_at_start}),
    );

    // tagteam first.
    p.note("expired again", ctx.expire(&live)?);
    let pause_dir = ctx.layout.scratch.join("pause");
    let _ = fs::remove_dir_all(&pause_dir);
    fs::create_dir(&pause_dir)?;
    store::make_due(&db, &ctx.oauth.id, now_ms() / 1000)?;
    let mut tt = ctx
        .tagteam(&["list", "--json"])
        .var("TAGTEAM_TEST_PAUSE_AT", "active-before-request")
        .var("TAGTEAM_TEST_PAUSE_DIR", &pause_dir)
        .timeout(Duration::from_secs(120))
        .spawn(&ctx.roots)?;
    wait_until(Duration::from_secs(60), || {
        pause_dir.join("paused").exists() || tt.finished()
    });
    let paused = pause_dir.join("paused").exists();
    if !paused {
        let why = if tt.finished() {
            tt.wait()?.summary()
        } else {
            json!("it was still running after 60 s")
        };
        return Err(harness(format!(
            "tagteam first: tagteam list never reached active-before-request, so this order proves nothing: {why}"
        )));
    }
    let locked = paths.refresh_lock.is_dir();
    let mut cc = ctx.claude(&live, &REQUEST).spawn(&ctx.roots)?;
    pause(HOLD);
    let cc_early = cc.finished();
    fs::write(pause_dir.join("resume"), b"")?;
    let tt = tt.wait()?;
    let cc = cc.wait()?;
    p.expect(
        "tagteam first: tagteam stopped holding CC's refresh lock",
        paused && locked,
        json!({"paused": paused, "locked": locked}),
    );
    p.expect("tagteam first: CC waited for it", !cc_early, json!(null));
    p.expect(
        "tagteam first: tagteam list succeeded",
        tt.success(),
        tt.summary(),
    );
    p.expect(
        "tagteam first: claude -p succeeded",
        cc.success(),
        cc.summary(),
    );
    let (vault, held) = (vault_generation(ctx)?, live_generation(ctx)?);
    p.expect(
        "tagteam first: the live store and the vault hold one generation",
        vault.is_some() && vault == held,
        json!({"vault": vault, "live": held}),
    );
    Ok(p.finish("CC's credential locks kept one refresh at a time, both ways"))
}

/// An interactive `claude` in the default home that talks to `capture`, started and ready.
fn session_on(
    ctx: &Ctx,
    capture: &Capture,
) -> Result<Option<crate::compat::sys::Pty>, HarnessError> {
    let live = ctx.live();
    let sessions = Path::new(&live).join("sessions");
    let before = records(&sessions);
    let pty = ctx
        .claude(&live, &["--model", MODEL])
        .var("ANTHROPIC_BASE_URL", capture.base_url())
        .pty(&ctx.roots)?;
    if new_record(&sessions, &before, Duration::from_secs(60)).is_none() {
        pty.finish(Duration::from_secs(5))?;
        return Ok(None);
    }
    pause(Duration::from_secs(2));
    Ok(Some(pty))
}

/// One prompt in `pty`; the credential its request carried.
fn prompt(
    pty: &mut crate::compat::sys::Pty,
    capture: &Capture,
) -> Result<Option<Sent>, HarnessError> {
    let n = capture.messages();
    pty.line(PROMPT)?;
    Ok(capture.next_message(n, Duration::from_secs(60)))
}

/// Appendix A.3 and §9.4's hint: a running `claude` takes up a switch at once when the
/// credential file's mtime moves, and from the Keychain within 30 s. The switch goes from the
/// account to its setup token and back, so the lineage never leaves the default home.
pub fn hot_reload(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let Some(st) = ctx.setup_token.clone() else {
        p.expect(
            "a setup-token account to switch to",
            false,
            json!("run `cargo xtask compat login` again to add it"),
        );
        return Ok(p.finish(""));
    };
    let live = ctx.live();
    ctx.seed_trust(&live)?;
    let setup = bearer(&ctx.vault_credential(&st.id)?)
        .ok_or_else(|| harness("the setup token's vault entry has no access token"))?;
    let capture = Capture::start()?;
    let file = ctx.paths(&live).credentials_file;
    let modes: &[(&str, bool)] = if ctx.macos {
        &[("from the Keychain", false), ("by the file's mtime", true)]
    } else {
        &[("by the file's mtime", true)]
    };
    for &(label, with_file) in modes {
        ctx.switch(ALIAS_OAUTH)?;
        let (bytes, _) = ctx
            .read_credential(&live)?
            .ok_or_else(|| harness("the default home holds no credential"))?;
        if ctx.macos {
            // tagteam rewrites the file after a Keychain write only if it exists (A.3).
            if with_file {
                write_atomic_private(&file, &bytes, 0o600)?;
            } else if file.exists() {
                fs::remove_file(&file)?;
            }
        }
        let Some(mut pty) = session_on(ctx, &capture)? else {
            p.expect(&format!("{label}: the session started"), false, json!(null));
            continue;
        };
        let first = prompt(&mut pty, &capture)?;
        p.expect_eq(
            &format!("{label}: before the switch, the account's token"),
            sent_json(bearer(&bytes)),
            sent_json(first),
        );
        p.note(&format!("{label}: switch"), ctx.switch(ALIAS_SETUP_TOKEN)?);
        let switched = Instant::now();
        if with_file {
            let next = prompt(&mut pty, &capture)?;
            p.expect_eq(
                &format!("{label}: the next message carries the setup token"),
                setup.to_json(),
                sent_json(next),
            );
        } else {
            pause(Duration::from_secs(2));
            let early = prompt(&mut pty, &capture)?;
            p.note(&format!("{label}: 2 s after the switch"), sent_json(early));
            pause(Duration::from_secs(31).saturating_sub(switched.elapsed()));
            let late = prompt(&mut pty, &capture)?;
            p.expect_eq(
                &format!("{label}: 31 s after the switch, the setup token"),
                setup.to_json(),
                sent_json(late),
            );
        }
        let _ = pty.line("/exit");
        pty.finish(Duration::from_secs(30))?;
        ctx.switch(ALIAS_OAUTH)?;
    }
    if ctx.macos && file.exists() {
        fs::remove_file(&file)?;
    }
    Ok(p.finish("a running claude took up each switch as tagteam's hint says"))
}

/// `primaryApiKey` in the home's global config, set or removed, with a set key's last 20
/// characters approved as tagteam approves its own (§9.4 step 7). Under CC's config lock.
fn set_primary_api_key(ctx: &Ctx, key: Option<&str>) -> Result<(), HarnessError> {
    let paths = ctx.paths(&ctx.live());
    let _lock = acquire_config(&paths, Duration::from_secs(9), &Cancel::new())
        .map_err(|e| harness(format!("CC's config lock: {e}")))?;
    let doc = fs::read(&paths.global_config)?;
    let err = |e| harness(format!("splicing the global config: {e}"));
    let doc = match key {
        None => remove_top_level(&doc, "primaryApiKey").map_err(err)?,
        Some(k) => {
            let mut responses = get_top_level(&doc, "customApiKeyResponses")
                .map_err(err)?
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({"approved": [], "rejected": []}));
            let tail = &k[k.len().saturating_sub(20)..];
            if !responses["approved"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == tail))
            {
                if !responses["approved"].is_array() {
                    responses["approved"] = json!([]);
                }
                responses["approved"]
                    .as_array_mut()
                    .expect("an array")
                    .push(json!(tail));
            }
            let doc = replace_top_level(&doc, "customApiKeyResponses", &responses).map_err(err)?;
            replace_top_level(&doc, "primaryApiKey", &json!(k)).map_err(err)?
        }
    };
    write_atomic_private(&paths.global_config, &doc, 0o600)?;
    Ok(())
}

/// §9.4 step 7: activating an API key drops the credential entry's account-scoped keys and keeps
/// its machine-shared ones, and CC then runs on the key: it sends it, to the stand-in.
pub fn api_key_entry(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let live = ctx.live();
    let (bytes, place) = ctx
        .read_credential(&live)?
        .ok_or_else(|| harness("the default home holds no credential"))?;
    let mut v: Value =
        serde_json::from_slice(&bytes).map_err(|_| harness("the live credential is not JSON"))?;
    v["mcpOAuth"] = json!({"tagteam-compat|0": {
        "serverName": "tagteam-compat",
        "serverUrl": "http://127.0.0.1:9/mcp",
        "accessToken": "tagteam-compat-mcp-dummy",
        "expiresAt": 0
    }});
    ctx.write_credential(
        &live,
        &place,
        &serde_json::to_vec(&v).expect("a Value serializes"),
    )?;
    let key = ctx.add_dummy_api_key()?;
    p.note("switch", ctx.switch(ALIAS_API_KEY)?);
    match ctx.read_credential(&live)? {
        Some((b, at)) => {
            let entry: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
            p.expect_eq(
                "the entry keeps only its machine-shared keys",
                json!(["mcpOAuth"]),
                json!(keys(&entry)),
            );
            p.note("the entry", at.describe());
        }
        None => {
            p.expect(
                "the entry is kept for its machine-shared key",
                false,
                json!(null),
            );
        }
    }
    if ctx.macos {
        let stored = ctx.item(&live, ItemKind::ManagedKey)?.read();
        p.expect(
            "the key is in the managed-key item",
            matches!(&stored, Read::Present(b) if b.as_slice() == key.as_bytes()),
            json!(read_name(&stored)),
        );
    } else {
        let stored = get_top_level(&fs::read(ctx.paths(&live).global_config)?, "primaryApiKey")
            .ok()
            .flatten();
        p.expect(
            "the key is primaryApiKey",
            stored == Some(json!(key)),
            json!(stored.is_some()),
        );
    }
    let v = ctx
        .claude(&live, &STATUS)
        .run(&ctx.roots)?
        .json()
        .unwrap_or(Value::Null);
    p.expect_eq(
        "auth status: authMethod",
        "api_key",
        v["authMethod"].clone(),
    );
    p.note(
        "auth status: apiKeySource",
        ctx.redact.value(&v["apiKeySource"]),
    );
    let capture = Capture::start()?;
    let ran = ctx
        .claude(&live, &REQUEST)
        .var("ANTHROPIC_BASE_URL", capture.base_url())
        .timeout(Duration::from_secs(120))
        .run(&ctx.roots)?;
    p.note("claude -p against the stand-in", ran.summary());
    p.expect_eq(
        "CC sent the key",
        Sent::ApiKey(fingerprint(&key)).to_json(),
        sent_json(capture.next_message(0, Duration::from_secs(5))),
    );
    ctx.drop_dummy_api_key()?;
    if let Some((b, at)) = ctx.read_credential(&live)? {
        let mut entry: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
        if let Some(o) = entry.as_object_mut() {
            o.remove("mcpOAuth");
        }
        ctx.write_credential(
            &live,
            &at,
            &serde_json::to_vec(&entry).expect("a Value serializes"),
        )?;
    }
    Ok(p.finish("the entry kept its machine-shared key and CC ran on the key"))
}

/// §9.4: with the managed-key item and `primaryApiKey` both set, which CC sends; with the item
/// gone, `primaryApiKey`; and a running session's next message after `primaryApiKey` changes,
/// as the file-store hint ("active on your next message") says.
pub fn managed_key_precedence(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let live = ctx.live();
    ctx.seed_trust(&live)?;
    let k1 = ctx.add_dummy_api_key()?;
    p.note("switch", ctx.switch(ALIAS_API_KEY)?);
    let k2 = dummy_key("k2")?;
    set_primary_api_key(ctx, Some(&k2))?;
    let capture = Capture::start()?;
    let request = |ctx: &Ctx| -> Result<Option<Sent>, HarnessError> {
        let n = capture.messages();
        ctx.claude(&ctx.live(), &REQUEST)
            .var("ANTHROPIC_BASE_URL", capture.base_url())
            .timeout(Duration::from_secs(120))
            .run(&ctx.roots)?;
        Ok(capture.next_message(n, Duration::from_secs(5)))
    };
    p.expect_eq(
        "both set: CC sends the managed-key item's key",
        Sent::ApiKey(fingerprint(&k1)).to_json(),
        sent_json(request(ctx)?),
    );
    ctx.item(&live, ItemKind::ManagedKey)?.delete()?;
    p.expect_eq(
        "the item gone: CC sends primaryApiKey",
        Sent::ApiKey(fingerprint(&k2)).to_json(),
        sent_json(request(ctx)?),
    );
    match session_on(ctx, &capture)? {
        None => {
            p.expect("a session: it started", false, json!(null));
        }
        Some(mut pty) => {
            p.expect_eq(
                "a session: its first message carries primaryApiKey",
                Sent::ApiKey(fingerprint(&k2)).to_json(),
                sent_json(prompt(&mut pty, &capture)?),
            );
            let k3 = dummy_key("k3")?;
            set_primary_api_key(ctx, Some(&k3))?;
            p.expect_eq(
                "a session: the next message carries the changed primaryApiKey",
                Sent::ApiKey(fingerprint(&k3)).to_json(),
                sent_json(prompt(&mut pty, &capture)?),
            );
            let _ = pty.line("/exit");
            pty.finish(Duration::from_secs(30))?;
        }
    }
    set_primary_api_key(ctx, None)?;
    ctx.drop_dummy_api_key()?;
    Ok(p.finish("CC read the managed keys in the order and at the time tagteam assumes"))
}
