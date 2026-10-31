//! The production `Http` adapter against a local server (§4.4, §15.1): the User-Agent, the body
//! cap, and which transport failures are `PreSend` and which `Ambiguous`.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use tagteam_engine::net::{Resolver, UreqHttp};
use tagteam_provider::http::{MAX_BODY, USER_AGENT};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Http, HttpError, HttpRequest};

const T: Duration = Duration::from_secs(5);

fn is_pre_send(r: &Result<tagteam_provider::HttpResponse, HttpError>) -> bool {
    matches!(r, Err(HttpError::PreSend(_)))
}

fn is_ambiguous(r: &Result<tagteam_provider::HttpResponse, HttpError>) -> bool {
    matches!(r, Err(HttpError::Ambiguous(_)))
}

#[test]
fn every_request_carries_tagteams_user_agent_and_only_it() {
    let server = MockServer::start();
    server.on(
        "GET",
        "/p",
        MockReply::Raw {
            status: 200,
            headers: vec![
                ("X-Thing".into(), "v".into()),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: b"{}".to_vec(),
        },
    );
    let req =
        HttpRequest::get(format!("{}/p", server.base_url()), T).header("user-agent", "evil/1");
    let resp = UreqHttp::direct().send(&req).unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-thing"), Some("v"));
    assert!(
        resp.headers
            .iter()
            .all(|(k, _)| *k == k.to_ascii_lowercase()),
        "names are lowercased: {:?}",
        resp.headers
    );
    let seen = &server.requests()[0];
    let agents: Vec<&str> = seen
        .headers
        .iter()
        .filter(|(k, _)| k == "user-agent")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(agents, vec![USER_AGENT]);
}

#[test]
fn a_json_post_round_trips_with_its_headers() {
    let server = MockServer::start();
    server.on(
        "POST",
        "/t",
        MockReply::Json {
            status: 200,
            body: json!({"ok": true}),
        },
    );
    let req = HttpRequest::post_json(format!("{}/t", server.base_url()), &json!({"a": 1}), T)
        .bearer("tok");
    let resp = UreqHttp::direct().send(&req).unwrap();
    assert_eq!(resp.json(), Some(json!({"ok": true})));
    let seen = &server.requests()[0];
    assert_eq!(seen.body, br#"{"a":1}"#);
    assert!(
        seen.headers
            .contains(&("authorization".to_owned(), "Bearer tok".to_owned()))
    );
    assert!(
        seen.headers
            .contains(&("content-type".to_owned(), "application/json".to_owned()))
    );
}

#[test]
fn a_4xx_is_a_response_not_an_error() {
    let server = MockServer::start();
    server.on(
        "POST",
        "/t",
        MockReply::Json {
            status: 400,
            body: json!({"error": "invalid_grant"}),
        },
    );
    let req = HttpRequest::post_json(format!("{}/t", server.base_url()), &json!({}), T);
    let resp = UreqHttp::direct().send(&req).unwrap();
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json(), Some(json!({"error": "invalid_grant"})));
}

#[test]
fn redirects_are_returned_not_followed() {
    let server = MockServer::start();
    server.on(
        "GET",
        "/moved",
        MockReply::Raw {
            status: 302,
            headers: vec![(
                "Location".into(),
                format!("{}/elsewhere", server.base_url()),
            )],
            body: vec![],
        },
    );
    let req = HttpRequest::get(format!("{}/moved", server.base_url()), T).bearer("tok");
    let resp = UreqHttp::direct().send(&req).unwrap();
    assert_eq!(resp.status, 302);
    assert_eq!(
        server.hits("GET", "/elsewhere"),
        0,
        "a token never follows a redirect"
    );
}

#[test]
fn a_body_over_one_mebibyte_is_ambiguous() {
    let server = MockServer::start();
    server.on(
        "GET",
        "/big",
        MockReply::Raw {
            status: 200,
            headers: vec![],
            body: vec![b'x'; MAX_BODY + 1],
        },
    );
    let r = UreqHttp::direct().send(&HttpRequest::get(format!("{}/big", server.base_url()), T));
    assert!(is_ambiguous(&r), "{r:?}");
}

#[test]
fn a_connection_closed_without_a_reply_is_ambiguous() {
    let server = MockServer::start();
    server.on("POST", "/t", MockReply::Close);
    let r = UreqHttp::direct().send(&HttpRequest::post_json(
        format!("{}/t", server.base_url()),
        &json!({"refresh_token": "x"}),
        T,
    ));
    assert!(is_ambiguous(&r), "{r:?}");
    assert_eq!(
        server.hits("POST", "/t"),
        1,
        "the request did reach the server"
    );
}

#[test]
fn a_server_that_never_answers_times_out_as_ambiguous() {
    let server = MockServer::start();
    server.on("GET", "/h", MockReply::Hang);
    let t = Instant::now();
    let r = UreqHttp::direct().send(&HttpRequest::get(
        format!("{}/h", server.base_url()),
        Duration::from_millis(300),
    ));
    assert!(is_ambiguous(&r), "{r:?}");
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "the request's own timeout bounds it"
    );
}

#[test]
fn a_refused_connection_is_pre_send() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let r = UreqHttp::direct().send(&HttpRequest::get(format!("http://127.0.0.1:{port}/"), T));
    assert!(is_pre_send(&r), "{r:?}");
}

/// A lookup that fails, injected so the machine's real resolver is never asked (§15.1).
fn no_such_host(_: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
    Err(io::Error::new(io::ErrorKind::NotFound, "no such host"))
}

/// A lookup that answers with no address at all.
fn no_addresses(_: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
    Ok(vec![])
}

#[test]
fn a_host_that_does_not_resolve_is_pre_send() {
    let server = MockServer::start();
    server.on(
        "GET",
        "/p",
        MockReply::Json {
            status: 200,
            body: json!({}),
        },
    );
    for resolver in [no_such_host as Resolver, no_addresses] {
        let r = UreqHttp::with_proxy(resolver, None)
            .send(&HttpRequest::get(format!("{}/p", server.base_url()), T));
        assert!(is_pre_send(&r), "{r:?}");
    }
    assert_eq!(server.hits("GET", "/p"), 0, "nothing was sent");
}

/// The proxy resolves the host, so tagteam must not: the request reaches a proxy that answers,
/// though the injected resolver cannot resolve the host it names.
#[test]
fn a_proxy_resolves_the_host_so_tagteam_does_not() {
    let proxy = MockServer::start();
    // ureq asks an HTTP proxy to CONNECT; whatever it answers, the request reached it.
    proxy.on(
        "CONNECT",
        "unresolvable.invalid:80",
        MockReply::Raw {
            status: 502,
            headers: vec![],
            body: vec![],
        },
    );
    let proxy_url = ureq::Proxy::new(&proxy.base_url()).unwrap();
    let r = UreqHttp::with_proxy(no_such_host, Some(proxy_url))
        .send(&HttpRequest::get("http://unresolvable.invalid/p", T));
    assert_eq!(proxy.hits("CONNECT", "unresolvable.invalid:80"), 1, "{r:?}");
    let not_resolve = match &r {
        Err(HttpError::PreSend(m)) => !m.contains("resolve"),
        _ => true,
    };
    assert!(not_resolve, "the host was resolved by tagteam: {r:?}");
}

/// A host the proxy is told not to handle is resolved by tagteam as usual.
#[test]
fn a_host_covered_by_no_proxy_is_still_resolved_by_tagteam() {
    let proxy = MockServer::start();
    let proxy_url = ureq::Proxy::builder(ureq::ProxyProtocol::Http)
        .host("127.0.0.1")
        .port(
            proxy
                .base_url()
                .rsplit(':')
                .next()
                .unwrap()
                .parse()
                .unwrap(),
        )
        .no_proxy("unresolvable.invalid")
        .build()
        .unwrap();
    let r = UreqHttp::with_proxy(no_such_host, Some(proxy_url))
        .send(&HttpRequest::get("http://unresolvable.invalid/p", T));
    assert!(is_pre_send(&r), "{r:?}");
    assert_eq!(proxy.requests().len(), 0, "the proxy was never asked");
}

/// A proxy that cannot be reached is `PreSend`: nothing was sent.
#[test]
fn an_unreachable_proxy_is_pre_send() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let proxy = ureq::Proxy::new(&format!("http://127.0.0.1:{port}")).unwrap();
    let r = UreqHttp::with_proxy(no_such_host, Some(proxy))
        .send(&HttpRequest::get("http://unresolvable.invalid/p", T));
    assert!(is_pre_send(&r), "{r:?}");
}

/// How long `reply_to_an_interrupted_read`'s signal handler holds the client's read.
const STALL_MS: libc::c_int = 1500;

/// Sends a GET with `timeout` from a client thread, interrupts its wait for the reply with a
/// signal whose handler stalls for `STALL_MS`, and answers `answer_after` the signal. On Linux a
/// stop interrupts a socket read with a timeout the same way; a handler installed without
/// `SA_RESTART` does it on both platforms.
fn reply_to_an_interrupted_read(
    timeout: Duration,
    answer_after: Duration,
) -> Result<tagteam_provider::HttpResponse, HttpError> {
    extern "C" fn stall(_: libc::c_int) {
        // SAFETY: `poll` is async-signal-safe; with no descriptors it only waits.
        unsafe { libc::poll(std::ptr::null_mut(), 0, STALL_MS) };
    }
    // SAFETY: the handler only waits, and only a client thread below is sent the signal.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = stall as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        assert_eq!(
            libc::sigaction(libc::SIGUSR2, &action, std::ptr::null_mut()),
            0
        );
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/p", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let client = thread::spawn(move || {
        // SAFETY: `pthread_self` has no preconditions.
        tx.send(unsafe { libc::pthread_self() }).unwrap();
        UreqHttp::direct().send(&HttpRequest::get(url, timeout))
    });
    let reader = rx.recv().unwrap();
    let deadline = Instant::now() + T;
    let mut conn = loop {
        match listener.accept() {
            Ok((conn, _)) => break conn,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if client.is_finished() {
                    panic!("the client gave up before connecting: {:?}", client.join());
                }
                assert!(Instant::now() < deadline, "the client never connected");
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    conn.set_nonblocking(false).unwrap();
    conn.set_read_timeout(Some(T)).unwrap();
    let mut request = Vec::new();
    let mut buf = [0; 1024];
    while !request.ends_with(b"\r\n\r\n") {
        let n = conn.read(&mut buf).unwrap();
        assert!(n > 0, "the request ended early");
        request.extend_from_slice(&buf[..n]);
    }
    // The client now waits for the reply.
    thread::sleep(Duration::from_millis(50));
    // SAFETY: `reader` is the client thread, which is joined below, so its id stays valid.
    assert_eq!(unsafe { libc::pthread_kill(reader, libc::SIGUSR2) }, 0);
    thread::sleep(answer_after);
    conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
        .unwrap();
    client.join().unwrap()
}

/// §4.4, §7.3: a holder resumed after its timeout must not discard the reply to a token refresh
/// that the server sent while it was stopped. The read takes it at its last look.
#[test]
fn a_reply_that_arrived_while_the_read_was_interrupted_past_its_timeout_is_taken() {
    let r = reply_to_an_interrupted_read(Duration::from_millis(1000), Duration::from_millis(100));
    assert!(matches!(&r, Ok(reply) if reply.status == 200), "{r:?}");
}

/// A stop shorter than the timeout: the read resumes with the time left, and takes a reply that
/// comes only after the resume.
#[test]
fn a_read_interrupted_within_its_timeout_resumes_and_takes_a_later_reply() {
    let after_the_stall = Duration::from_millis(STALL_MS as u64 + 500);
    let r = reply_to_an_interrupted_read(Duration::from_secs(5), after_the_stall);
    assert!(matches!(&r, Ok(reply) if reply.status == 200), "{r:?}");
}
