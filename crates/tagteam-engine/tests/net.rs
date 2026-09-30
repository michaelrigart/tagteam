//! The production `Http` adapter against a local server (§4.4, §15.1): the User-Agent, the body
//! cap, and which transport failures are `PreSend` and which `Ambiguous`.

use std::io;
use std::net::{SocketAddr, TcpListener};
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
    let resp = UreqHttp::new().send(&req).unwrap();
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
    let resp = UreqHttp::default().send(&req).unwrap();
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
    let resp = UreqHttp::new().send(&req).unwrap();
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
    let resp = UreqHttp::new().send(&req).unwrap();
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
    let r = UreqHttp::new().send(&HttpRequest::get(format!("{}/big", server.base_url()), T));
    assert!(is_ambiguous(&r), "{r:?}");
}

#[test]
fn a_connection_closed_without_a_reply_is_ambiguous() {
    let server = MockServer::start();
    server.on("POST", "/t", MockReply::Close);
    let r = UreqHttp::new().send(&HttpRequest::post_json(
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
    let r = UreqHttp::new().send(&HttpRequest::get(
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
    let r = UreqHttp::new().send(&HttpRequest::get(format!("http://127.0.0.1:{port}/"), T));
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
        let r = UreqHttp::with_resolver(resolver)
            .send(&HttpRequest::get(format!("{}/p", server.base_url()), T));
        assert!(is_pre_send(&r), "{r:?}");
    }
    assert_eq!(server.hits("GET", "/p"), 0, "nothing was sent");
}
