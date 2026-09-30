//! §15.2 concurrency, through the real binary: the refresh gate is single-flight per account,
//! and a suspended holder is never preempted (§7.3 step 1, B.38). Needs `--features
//! test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{cmd, expire_vault, live_email, std_cmd, two_accounts};
use serde_json::{Value, json};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{FileKeychain, Keychain};

const API_BASE: &str = "TAGTEAM_TEST_API_BASE";
const TOKEN: &str = "/v1/oauth/token";

/// A spawned `tagteam`. However the test ends, dropping it resumes the process (SIGCONT, in
/// case it is stopped), kills it if it is still running, and reaps it, so a failed assertion
/// never leaves a stopped or orphaned child behind.
struct Spawned(Option<Child>);

impl Spawned {
    fn child(&self) -> &Child {
        self.0.as_ref().expect("the child is still owned")
    }

    fn signal(&self, sig: libc::c_int) {
        // SAFETY: `kill` only sends a signal to our own child, whose pid is still ours to use:
        // it has not been waited for.
        assert_eq!(
            unsafe { libc::kill(self.child().id() as libc::pid_t, sig) },
            0
        );
    }

    fn stop(&self) {
        self.signal(libc::SIGSTOP);
    }

    fn resume(&self) {
        self.signal(libc::SIGCONT);
    }

    /// Waits for the process and returns (succeeded, stdout as JSON, stderr).
    fn json_out(mut self) -> (bool, Value, String) {
        let out = self.0.take().unwrap().wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let v = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}\n{stderr}", String::from_utf8_lossy(&out.stdout)));
        (out.status.success(), v, stderr)
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            // SAFETY: as in `signal`; the child has not been waited for.
            unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGCONT) };
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// `a@x.co` at position 1 and `b@x.co` at 2, `b` live; `a`'s access token inside the freshen
/// window (§7.2), so `switch 1` refreshes it through the gate first. Returns `a`'s id.
fn expiring_target(root: &Path) -> String {
    let (a, _) = two_accounts(root);
    expire_vault(root, &a, 60_000);
    a
}

fn token_reply(rt: &str) -> MockReply {
    MockReply::Json {
        status: 200,
        body: json!({"access_token": format!("at-{rt}"), "refresh_token": rt,
                     "expires_in": 28800, "scope": "user:inference user:profile"}),
    }
}

fn spawn_switch(root: &Path, server: &MockServer) -> Spawned {
    Spawned(Some(
        std_cmd(root)
            .env(API_BASE, server.base_url())
            .args(["switch", "1", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ))
}

/// Returns once the server has read the token request: the sender's gate now holds the
/// account lock and is waiting for the reply (§7.3 steps 1 and 5).
fn wait_for_token_request(server: &MockServer) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.hits("POST", TOKEN) == 0 {
        assert!(Instant::now() < deadline, "the token request never arrived");
        thread::sleep(Duration::from_millis(20));
    }
}

fn vault_refresh_token(root: &Path, id: &str) -> String {
    let kc = FileKeychain::new(root.join("keychain"));
    let v: Value = serde_json::from_slice(&kc.find(SERVICE, id).present().unwrap()).unwrap();
    v["claudeAiOauth"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_switch_that_finds_the_target_refreshing_waits_and_activates_its_successor() {
    // §7.2's `Busy` row: B proceeds, waits for the account lock, and its locked re-read picks
    // up A's refresh. One request in all.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let a = expiring_target(root);
    let server = MockServer::start();
    server.on(
        "POST",
        TOKEN,
        MockReply::Delay(Duration::from_secs(3), Box::new(token_reply("rt-a2"))),
    );

    let holder = spawn_switch(root, &server);
    wait_for_token_request(&server);
    holder.stop();
    let waiter = spawn_switch(root, &server);
    // B finds the lock held (`Busy`), takes the mutation lock, and waits for the account lock.
    thread::sleep(Duration::from_secs(1));
    // Well inside A's 10 s request timeout; the reply arrives 3 s after the request.
    holder.resume();

    let (b_ok, b, b_err) = waiter.json_out();
    let (a_ok, a_out, a_err) = holder.json_out();
    assert!(a_ok && b_ok, "A: {a_out} {a_err}\nB: {b} {b_err}");
    assert_eq!(server.hits("POST", TOKEN), 1, "single flight");
    assert_eq!(vault_refresh_token(root, &a), "rt-a2");
    assert_eq!(live_email(root), "a@x.co");
    // Whichever took the mutation lock first switched; the other found the work done.
    let reasons: BTreeSet<String> = [&a_out, &b]
        .iter()
        .map(|v| v["reason"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        reasons,
        BTreeSet::from(["already-active".to_owned(), "switched".to_owned()])
    );
}

#[test]
#[ignore = "waits out the full 15 s account-lock timeout; run with --ignored"]
fn a_refresh_holder_stopped_past_every_timeout_is_never_preempted() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let a = expiring_target(root);
    let server = MockServer::start();
    // The reply comes long after A's own 10 s request timeout: A never receives a successor.
    server.on(
        "POST",
        TOKEN,
        MockReply::Delay(Duration::from_secs(60), Box::new(token_reply("rt-a2"))),
    );

    let holder = spawn_switch(root, &server);
    wait_for_token_request(&server);
    holder.stop();

    // B: `Busy`, then the account lock A still holds, for all of `AccountLock::WAIT`.
    let started = Instant::now();
    let out = cmd(root)
        .env(API_BASE, server.base_url())
        .args(["switch", "1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["error"]["type"], "lock-timeout", "{v}");
    assert!(started.elapsed() >= Duration::from_secs(15));
    assert_eq!(
        server.hits("POST", TOKEN),
        1,
        "B never sends the generation A holds"
    );

    holder.resume();
    let (ok, v, stderr) = holder.json_out();
    // A's request was sent and its reply never read (`ambiguous`): the switch proceeds with
    // the vault's generation and one warning (§7.2).
    assert!(ok, "{v}\n{stderr}");
    assert_eq!(v["switched"], true);
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(server.hits("POST", TOKEN), 1);
    assert_eq!(vault_refresh_token(root, &a), "rt-a");
    assert_eq!(live_email(root), "a@x.co");
}
