use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_core::Fingerprint;
use tagteam_core::pace::ProjectionMethod;
use tagteam_core::usage::WindowKind;
use tagteam_fake::{
    FAKE_AGENT, FakeAgent, FakePaths, KIND_STATIC, KIND_TOKEN, LOGIN_EXPIRES, credential_json,
    identity_json, login,
};
use tagteam_provider::http::{HttpError, HttpResponse, Method, RecordedRequest, ScriptedHttp};
use tagteam_provider::provider::TransientKind;
use tagteam_provider::{
    Capabilities, Credential, EntryKind, Env, LiveChange, LockError, MustShare, MutationGuard,
    Pace, PollBudget, Provenance, Provider, ProviderError, Read, SecretStore, SharePolicy,
    StoredLogin, UsageResult, Window, Written,
};

struct Fx {
    _d: tempfile::TempDir,
    env: Env,
    fake: FakeAgent,
}

fn fx() -> Fx {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    Fx {
        _d: d,
        env,
        fake: FakeAgent::new().with_lock_budget(Duration::from_secs(1)),
    }
}

fn file_json(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// A fallback hook that saves nothing: FakeAgent never falls back, so it is never called.
fn save_nothing(_: &[u8]) -> Result<(), ProviderError> {
    Ok(())
}

#[test]
fn its_identity_has_no_email_and_its_shapes_are_its_own() {
    let f = fx();
    assert_eq!(f.fake.id().as_str(), FAKE_AGENT);
    assert!(
        matches!(f.fake.live_identity(&f.env), Read::Absent),
        "no login yet"
    );
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let id = f.fake.live_identity(&f.env).present().unwrap();
    assert_eq!(
        (
            id.label.as_str(),
            id.email.as_deref(),
            id.org_uuid.as_str(),
            id.account_uuid.as_deref()
        ),
        ("alice@ws", None, "ws", Some("uid-alice"))
    );
    assert_eq!(id.raw, identity_json("alice", "ws", "uid-alice"));
    assert_eq!(f.fake.identity_key(&id).as_str(), "alice\nws");

    let auth = f.fake.read_live_auth(&f.env);
    assert!(
        matches!(auth.managed_key, Read::Absent),
        "no managed-key axis"
    );
    let bytes = auth.credential.present().unwrap().bytes().to_vec();
    assert_eq!(f.fake.classify(&bytes), KIND_TOKEN);
    assert_eq!(
        f.fake.fingerprint(&bytes),
        Some(Fingerprint::of_secret(b"renew-a"))
    );
    assert_eq!(
        f.fake.access_fingerprint(&bytes),
        Some(Fingerprint::of_secret(b"tok-a"))
    );
    assert_eq!(f.fake.access_expires_at(&bytes), Some(LOGIN_EXPIRES));
    assert_eq!(f.fake.login_expires_at(&bytes), None);
    assert!(f.fake.has_refresh_token(&bytes));

    let (kind, secret) = f.fake.token_secret("  static-1 ");
    assert_eq!(kind, KIND_STATIC);
    assert_eq!(f.fake.classify(&secret), KIND_STATIC);
    assert_eq!(
        f.fake.fingerprint(&secret),
        Some(Fingerprint::of_secret(b"static-1"))
    );
    assert!(!f.fake.has_refresh_token(&secret));
    assert!(f.fake.is_wiped(br#"{"fa":{"token":"","renew":""}}"#));
    assert!(!f.fake.is_wiped(&secret));
    let t = f.fake.token_identity("fa-static-3@token.local");
    assert_eq!(
        (
            t.label.as_str(),
            t.email.as_deref(),
            t.account_uuid.as_deref()
        ),
        ("fa-static-3@token.local", None, None)
    );
}

#[test]
fn kinds_capabilities_endpoints_and_surface() {
    let f = fx();
    assert_eq!(f.fake.credential_kinds(), &[KIND_TOKEN, KIND_STATIC]);
    assert_eq!(
        f.fake.capabilities(),
        Capabilities {
            usage: true,
            refresh: true,
            sessions: true,
            ..Capabilities::default()
        }
    );
    assert!(f.fake.kind_traits(KIND_TOKEN).refreshable);
    let st = f.fake.kind_traits(KIND_STATIC);
    assert_eq!(
        (
            st.refreshable,
            st.managed_key_axis,
            st.default_email_prefix,
            st.display
        ),
        (false, false, Some("fa-static"), Some("static"))
    );
    let s = f.fake.identity_surface(&f.env);
    let p = FakePaths::resolve(&f.env);
    assert_eq!(
        s.json_keys,
        vec![(p.identity.clone(), vec!["identity".to_string()])]
    );
    assert_eq!(s.credential_files, vec![p.credential.clone()]);
    assert!(s.credential_items.is_empty() && s.owned_items.is_empty());
    assert_eq!(s.machine_shared_keys, vec!["device"]);
    assert_eq!(s.create_only, vec![p.dir.join("journal.log")]);
    assert_eq!(f.fake.renew_url(), "https://fake-agent.invalid/fa/renew");
    assert_eq!(f.fake.usage_url(), "https://fake-agent.invalid/usage");
    assert_eq!(
        FakeAgent::new()
            .with_endpoint_base("http://127.0.0.1:9/")
            .whoami_url(),
        "http://127.0.0.1:9/fa/whoami"
    );
}

#[test]
fn a_write_keeps_the_machines_device_key_and_undoes_exactly() {
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    // The machine's device key moved on since any stored credential was captured.
    let mut live = file_json(&p.credential);
    live["device"] = json!({"id": "machine-2"});
    fs::write(&p.credential, serde_json::to_vec(&live).unwrap()).unwrap();
    let before_cred = fs::read(&p.credential).unwrap();
    let before_identity = fs::read(&p.identity).unwrap();

    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.fake.lock_live(&f.env, &g).unwrap();
    assert!(p.lock.is_dir(), "its one live lock");
    let bob = StoredLogin {
        kind: KIND_TOKEN.into(),
        secret: serde_json::to_vec(&credential_json("tok-b", Some("renew-b"), Some(1))).unwrap(),
        identity: f
            .fake
            .parse_identity(&identity_json("bob", "ws", "uid-bob"))
            .unwrap(),
    };
    let doomed = f.fake.doomed(&f.env, &locks, LiveChange::Write(KIND_TOKEN));
    assert_eq!(
        doomed.len(),
        1,
        "the credential file, and nothing on another axis"
    );
    assert_eq!(doomed[0].bytes.clone().present(), Some(before_cred.clone()));
    assert!(
        f.fake
            .doomed(&f.env, &locks, LiveChange::ClearOther(KIND_TOKEN))
            .is_empty()
    );

    let Written { undo, stored_in } = f
        .fake
        .write_credential(&f.env, &locks, &bob, &mut save_nothing)
        .unwrap();
    assert_eq!(stored_in, SecretStore::File(p.credential.clone()));
    let identity_undo = f
        .fake
        .write_identity(&f.env, &locks, Some(&bob.identity))
        .unwrap();
    assert_eq!(
        file_json(&p.credential),
        json!({"fa": {"token": "tok-b", "renew": "renew-b", "expires": 1}, "device": {"id": "machine-2"}})
    );
    assert_eq!(
        fs::metadata(&p.credential).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let doc = file_json(&p.identity);
    assert_eq!(doc["identity"], identity_json("bob", "ws", "uid-bob"));
    assert_eq!(
        doc["prefs"],
        json!({"theme": "x"}),
        "the rest of identity.json stays"
    );
    assert_eq!(
        f.fake.live_identity(&f.env).present().unwrap().label,
        "bob@ws"
    );

    f.fake
        .clear_other_axis(&f.env, &locks, KIND_TOKEN)
        .unwrap()
        .undo(&locks)
        .unwrap();
    identity_undo.undo(&locks).unwrap();
    undo.undo(&locks).unwrap();
    assert_eq!(fs::read(&p.credential).unwrap(), before_cred);
    assert_eq!(fs::read(&p.identity).unwrap(), before_identity);
    drop(locks);
    assert!(!p.lock.exists(), "released on drop");
}

#[test]
fn an_unreadable_live_credential_is_never_overwritten() {
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    fs::remove_file(&p.credential).unwrap();
    fs::create_dir(&p.credential).unwrap(); // reading it fails: neither present nor absent
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.fake.lock_live(&f.env, &g).unwrap();
    let bob = StoredLogin {
        kind: KIND_TOKEN.into(),
        secret: serde_json::to_vec(&credential_json("tok-b", Some("renew-b"), None)).unwrap(),
        identity: f
            .fake
            .parse_identity(&identity_json("bob", "ws", "uid-bob"))
            .unwrap(),
    };
    assert!(matches!(
        f.fake
            .write_credential(&f.env, &locks, &bob, &mut save_nothing),
        Err(ProviderError::Unreadable(_))
    ));
    assert!(p.credential.is_dir(), "left exactly as found");
}

#[test]
fn its_one_live_lock_times_out_within_its_budget_and_is_left_alone() {
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    fs::create_dir(&p.lock).unwrap(); // another FakeAgent process holds it, freshly
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert!(matches!(
        f.fake.lock_live(&f.env, &g),
        Err(ProviderError::Lock(LockError::Timeout(_)))
    ));
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(p.lock.is_dir());
}

/// Any signal number: the token only carries it.
const SIGTERM: i32 = 15;

#[test]
fn its_live_lock_wait_ends_when_the_token_is_set() {
    // §14.1: FakeAgent's lock wait is a cancellation point like Claude Code's.
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    fs::create_dir(&p.lock).unwrap(); // another FakeAgent process holds it, freshly
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    f.env.cancel.request(SIGTERM);
    let start = Instant::now();
    assert!(matches!(
        f.fake.lock_live(&f.env, &g),
        Err(ProviderError::Lock(LockError::Interrupted {
            signal: SIGTERM,
            ..
        }))
    ));
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "not the 1 s budget"
    );
    assert!(p.lock.is_dir(), "the other holder's lock is left alone");
}

#[test]
fn a_torn_identity_file_is_unreadable_and_never_replaced() {
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    fs::write(&p.identity, b"{\"identity\": {\"handle\": ").unwrap();
    assert!(matches!(f.fake.live_identity(&f.env), Read::Unreadable(_)));
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.fake.lock_live(&f.env, &g).unwrap();
    let id = f
        .fake
        .parse_identity(&identity_json("bob", "ws", "uid-bob"))
        .unwrap();
    match f.fake.write_identity(&f.env, &locks, Some(&id)) {
        Err(e @ ProviderError::ConfigUnsplicable { .. }) => {
            assert!(e.to_string().contains("repair or remove it"), "{e}")
        }
        other => panic!("expected ConfigUnsplicable, got {:?}", other.err()),
    }
    assert_eq!(
        fs::read(&p.identity).unwrap(),
        b"{\"identity\": {\"handle\": ".to_vec()
    );
}

/// A day and 30 days after 1_790_000_000 s: FakeAgent's `renews` are epoch seconds.
const RENEWS_DAY: i64 = 1_790_086_400;
const RENEWS_MONTH: i64 = 1_792_592_000;

fn meter_window(key: &str, kind: WindowKind, pct: f64, renews: i64, period_s: i64) -> Window {
    Window {
        key: key.into(),
        label: key.into(),
        kind,
        pct,
        resets_at: Some(renews),
        period_s: Some(period_s),
        detail: None,
    }
}

/// One usage fetch with `fa-tok`, against a fresh `ScriptedHttp` that answers with `reply`.
fn fetch_once(reply: Result<HttpResponse, HttpError>) -> (UsageResult, Vec<RecordedRequest>) {
    fetch_with_expiry(None, reply)
}

/// As `fetch_once`, with the credential's `expires` (epoch ms) set.
fn fetch_with_expiry(
    expires: Option<i64>,
    reply: Result<HttpResponse, HttpError>,
) -> (UsageResult, Vec<RecordedRequest>) {
    let fake = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push(Method::Get, &fake.usage_url(), reply);
    let token = Credential::fresh(
        credential_json("fa-tok", Some("fa-renew"), expires)
            .to_string()
            .into_bytes(),
    );
    (fake.fetch_usage(&http, &token), http.requests())
}

#[test]
fn fake_agent_meters_are_fractions_under_its_own_keys() {
    let body = json!({"meters": [
        {"id": "daily", "used": 0.42, "renews": RENEWS_DAY},
        {"id": "monthly", "used": 0.1, "renews": RENEWS_MONTH},
        {"id": "hourly", "used": 0.9, "renews": RENEWS_DAY}
    ]});
    let (r, sent) = fetch_once(Ok(HttpResponse::json_body(200, &body)));
    assert_eq!(
        r,
        UsageResult::Windows(vec![
            meter_window("daily", WindowKind::Short, 42.0, RENEWS_DAY, 86_400),
            meter_window("monthly", WindowKind::Long, 10.0, RENEWS_MONTH, 2_592_000),
        ]),
        "an unknown meter is ignored"
    );
    assert_eq!(sent.len(), 1);
    assert_eq!(
        (sent[0].method, sent[0].url.as_str()),
        (Method::Get, "https://fake-agent.invalid/usage")
    );
    assert_eq!(
        sent[0].headers,
        vec![("authorization".to_owned(), "Fake fa-tok".to_owned())]
    );
    assert!(
        !format!("{:?}", sent[0]).contains("fa-tok"),
        "the token never reaches Debug"
    );
}

#[test]
fn fake_agent_usage_verdicts() {
    let failed = |kind, retry_after_s| UsageResult::Failed {
        kind,
        retry_after_s,
    };
    assert_eq!(
        fetch_once(Ok(HttpResponse::json_body(401, &json!({})))).0,
        UsageResult::Unauthorized
    );
    let limited = HttpResponse {
        status: 429,
        headers: vec![("retry-after".into(), "60".into())],
        body: vec![],
    };
    assert_eq!(
        fetch_once(Ok(limited)).0,
        failed(TransientKind::Http(429), Some(60.0))
    );
    assert_eq!(
        fetch_once(Ok(HttpResponse::json_body(200, &json!({"meters": "none"})))).0,
        failed(TransientKind::BadResponse, None)
    );
    assert_eq!(
        fetch_once(Ok(HttpResponse::json_body(200, &json!({"meters": []})))).0,
        UsageResult::Windows(vec![])
    );
    assert_eq!(
        fetch_once(Err(HttpError::Ambiguous("reset".into()))).0,
        failed(TransientKind::Ambiguous, None)
    );
    // The same table as Claude Code's, since both delegate to `UsageResult::from_reply`.
    assert_eq!(
        fetch_once(Err(HttpError::PreSend("no route".into()))).0,
        failed(TransientKind::PreSend, None)
    );
    let html = HttpResponse {
        status: 200,
        headers: vec![],
        body: b"<html>not json</html>".to_vec(),
    };
    assert_eq!(
        fetch_once(Ok(html)).0,
        failed(TransientKind::BadResponse, None),
        "a 200 that is not JSON"
    );
    assert_eq!(
        fetch_once(Ok(HttpResponse::json_body(500, &json!({})))).0,
        failed(TransientKind::Http(500), None)
    );
    let busy = HttpResponse {
        status: 503,
        headers: vec![("retry-after".into(), "7".into())],
        body: vec![],
    };
    assert_eq!(
        fetch_once(Ok(busy)).0,
        failed(TransientKind::Http(503), Some(7.0))
    );
}

#[test]
fn fake_agent_fetches_with_an_expired_token_and_never_refreshes() {
    // §8.1: the usage fetch ignores expiry; a stale token is the gate's and the 401's business.
    let fake = FakeAgent::new();
    let body = json!({"meters": [{"id": "daily", "used": 0.5, "renews": RENEWS_DAY}]});
    let (r, sent) = fetch_with_expiry(Some(1), Ok(HttpResponse::json_body(200, &body)));
    assert_eq!(
        r,
        UsageResult::Windows(vec![meter_window(
            "daily",
            WindowKind::Short,
            50.0,
            RENEWS_DAY,
            86_400
        )])
    );
    assert_eq!(sent.len(), 1, "one request, and nothing else");
    assert_eq!(
        (sent[0].method, sent[0].url.as_str()),
        (Method::Get, fake.usage_url().as_str())
    );
    assert_ne!(
        sent[0].url,
        fake.renew_url(),
        "the renew endpoint is never called"
    );
    assert_eq!(
        sent[0].headers,
        vec![("authorization".to_owned(), "Fake fa-tok".to_owned())],
        "the expired token is sent as it is"
    );
}

#[test]
fn fake_agent_without_a_token_sends_nothing() {
    let fake = FakeAgent::new();
    let http = ScriptedHttp::new();
    let renew_only = Credential::fresh(br#"{"fa": {"renew": "r"}}"#.to_vec());
    assert_eq!(
        fake.fetch_usage(&http, &renew_only),
        UsageResult::NoAccessToken
    );
    assert!(http.requests().is_empty());
}

#[test]
fn fake_agent_renders_its_own_shape_and_names_its_identity_file() {
    let f = fx();
    let paced = Pace {
        rate_per_hour: Some(2.0),
        method: Some(ProjectionMethod::Regression),
        ..Pace::default()
    };
    let windows = vec![
        (
            meter_window("daily", WindowKind::Short, 42.0, RENEWS_DAY, 86_400),
            Pace::default(),
        ),
        (
            meter_window("monthly", WindowKind::Long, 10.0, RENEWS_MONTH, 2_592_000),
            paced,
        ),
    ];
    assert_eq!(
        f.fake.render_usage(&windows),
        json!({"meters": {"daily": 42.0, "monthly": 10.0}})
    );
    assert_eq!(f.fake.poll_budget(), PollBudget::STANDARD);
    assert_eq!(
        f.fake.live_identity_source(&f.env),
        Some(FakePaths::resolve(&f.env).identity)
    );
}

#[test]
fn fake_agent_describes_its_own_meter_keys() {
    let f = fx();
    let bare = |key: &str, kind, period_s| {
        Some(Window {
            key: key.into(),
            label: key.into(),
            kind,
            pct: 0.0,
            resets_at: None,
            period_s: Some(period_s),
            detail: None,
        })
    };
    assert_eq!(
        f.fake.describe_window("daily"),
        bare("daily", WindowKind::Short, 86_400)
    );
    assert_eq!(
        f.fake.describe_window("monthly"),
        bare("monthly", WindowKind::Long, 2_592_000)
    );
    for key in ["hourly", "Daily", "", "5h", "7d", "spend", "scoped:Fable"] {
        assert_eq!(f.fake.describe_window(key), None, "{key:?}");
    }
}

#[test]
fn fake_agent_offers_consume_first_no_long_window() {
    // §4.5, §11.5: without a primary long window a consume-first strategy runs `best`, and
    // FakeAgent is the provider that exercises that path. Its `monthly` meter is a `Long`
    // window for pace (§8.7), but it is not offered for ranking.
    let f = fx();
    assert_eq!(f.fake.primary_long_window(), None);
    assert_eq!(
        f.fake.describe_window("monthly").map(|w| w.kind),
        Some(WindowKind::Long)
    );
}

/// `f.env` with `FAKEAGENT_HOME` set to `v`.
fn with_home(f: &Fx, v: &str) -> Env {
    let mut env = f.env.clone();
    env.vars.insert("FAKEAGENT_HOME".into(), v.into());
    env
}

#[test]
fn fakeagent_home_moves_its_state_and_an_empty_one_is_unset() {
    let f = fx();
    let home = f.env.home.join("elsewhere");
    let p = FakePaths::resolve(&with_home(&f, home.to_str().unwrap()));
    assert_eq!(
        (p.dir.clone(), p.credential, p.identity, p.lock),
        (
            home.clone(),
            home.join("credential.json"),
            home.join("identity.json"),
            home.join(".live.lock")
        )
    );
    assert_eq!(
        FakePaths::resolve(&with_home(&f, "")).dir,
        f.env.home.join(".fakeagent")
    );
    assert_eq!(
        FakePaths::resolve(&f.env).dir,
        f.env.home.join(".fakeagent")
    );
}

#[test]
fn its_session_facts_are_deliberately_unlike_claude_code_s() {
    let f = fx();
    assert_eq!(f.fake.launch_command(), "fakeagent");
    assert_eq!(f.fake.session_dir_var(), Some("FAKEAGENT_HOME"));
    assert_eq!(f.fake.session_dir(&f.env), None);
    assert_eq!(f.fake.session_dir(&with_home(&f, "")), None);
    assert_eq!(
        f.fake.session_dir(&with_home(&f, "/p/0193")),
        Some(std::path::PathBuf::from("/p/0193"))
    );
    assert_eq!(
        f.fake.session_records_dir(std::path::Path::new("/p/0193")),
        std::path::Path::new("/p/0193/procs")
    );
    assert_eq!(
        f.fake
            .profile_spelling(std::path::Path::new("/p/cafe\u{301}")),
        "/p/cafe\u{301}",
        "the path as it is: no NFC"
    );
    let mut cc_shaped = with_home(&f, "/p/0193");
    cc_shaped.vars.insert("CLAUDECODE".into(), "1".into());
    cc_shaped.claude_config_dir = Some("/p/0193".into());
    assert!(!f.fake.invoked_by(&cc_shaped));
    assert_eq!(
        f.fake.share_policy(&f.env),
        SharePolicy {
            source: f.env.home.join(".fakeagent"),
            shared: vec!["notes", "prefs.json"],
            must_share: vec![MustShare {
                name: "journal.log",
                kind: EntryKind::File
            }],
            private: vec![
                "credential.json",
                "identity.json",
                "procs",
                ".live.lock",
                ".tagteam-*"
            ],
        }
    );
}

#[test]
fn its_outer_home_round_trips_unset_set_and_empty() {
    let f = fx();
    let inside = with_home(&f, "/data/sessions/0193");
    for home in [None, Some("/h/.fakeagent-custom"), Some("")] {
        let outer_env = match home {
            Some(h) => with_home(&f, h),
            None => f.env.clone(),
        };
        let outer = f.fake.outer_home(&outer_env);
        assert_eq!(outer, json!({"FAKEAGENT_HOME": home}), "{home:?}");
        let restored = f.fake.apply_outer_home(&inside, &outer).unwrap();
        assert_eq!(
            restored.var("FAKEAGENT_HOME"),
            home.map(std::ffi::OsStr::new),
            "{home:?}"
        );
    }
    for outer in [
        json!({}),
        json!({"FAKEAGENT_HOME": 7}),
        json!({"CLAUDE_CONFIG_DIR": "SENTINEL"}),
    ] {
        match f.fake.apply_outer_home(&inside, &outer) {
            Err(ProviderError::Invalid(msg)) => assert!(!msg.contains("SENTINEL"), "{msg}"),
            other => panic!("{outer}: {other:?}"),
        }
    }
}

#[test]
fn its_profile_credential_and_identity_are_read_in_the_profile_s_directory() {
    let f = fx();
    let profile = f.env.data_dir().join("sessions/0193");
    fs::create_dir_all(&profile).unwrap();
    let spelling = profile.to_str().unwrap();
    // Decision 19: a spelling the data directory has moved away from names nothing FakeAgent
    // reads, since it keeps no item.
    let gone = f.env.home.join("moved-from/sessions/0193");
    let gone = gone.to_str().unwrap();
    login(&f.env, "alice", "ws", "tok-live", "renew-live");
    assert!(matches!(
        f.fake.read_profile_credential(&f.env, &profile, spelling),
        Read::Absent
    ));
    assert!(matches!(
        f.fake.profile_identity(&f.env, &profile),
        Read::Absent
    ));
    login(&with_home(&f, spelling), "bob", "ws2", "tok-p", "renew-p");
    for recorded in [spelling, gone] {
        let c = f
            .fake
            .read_profile_credential(&f.env, &profile, recorded)
            .present()
            .unwrap();
        assert_eq!(c.provenance(), Provenance::Fresh, "{recorded}");
        assert_eq!(
            f.fake.fingerprint(c.bytes()),
            Some(tagteam_core::Fingerprint::of_secret(b"renew-p")),
            "{recorded}"
        );
    }
    assert!(
        f.fake
            .read_profile_credential(&f.env, std::path::Path::new(gone), gone)
            .present()
            .is_none(),
        "nothing is where the profile was"
    );
    let id = f.fake.profile_identity(&f.env, &profile).present().unwrap();
    assert_eq!(id.label, "bob@ws2");
    f.fake
        .delete_profile_credential(&f.env, &profile, spelling)
        .unwrap();
    assert!(
        profile.join("credential.json").exists(),
        "FakeAgent keeps nothing outside the directory to delete"
    );
    fs::write(profile.join("identity.json"), b"{\"identity\": {").unwrap();
    assert!(matches!(
        f.fake.profile_identity(&f.env, &profile),
        Read::Unreadable(_)
    ));
}
