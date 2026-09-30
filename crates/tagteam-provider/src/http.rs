//! The `Http` port (§4.4): one blocking request, with its own timeout. The engine's `ureq`
//! adapter is the production implementation; `ScriptedHttp` is the tests'. A provider builds
//! every request and parses every response; the engine owns everything around them.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;

/// Sent on every request by the production adapter, whatever the provider asked for (§4.4).
pub const USER_AGENT: &str = concat!("tagteam/", env!("CARGO_PKG_VERSION"));

/// The largest response body the adapter reads (§4.4).
pub const MAX_BODY: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// Renders headers for `Debug`, with the `authorization` value redacted: it carries a token.
fn shown_headers<'a>(headers: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    headers
        .map(|(k, v)| {
            if k.eq_ignore_ascii_case("authorization") {
                format!("{k}: <redacted>")
            } else {
                format!("{k}: {v}")
            }
        })
        .collect()
}

fn shown_body(body: Option<&[u8]>) -> String {
    match body {
        Some(b) => format!("<{} bytes>", b.len()),
        None => "none".into(),
    }
}

/// One request. Its body can carry a refresh token and its `authorization` header an access
/// token, so `Debug` shows neither (§4.4).
#[derive(Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout,
        }
    }

    pub fn post_json(url: impl Into<String>, body: &Value, timeout: Duration) -> Self {
        Self {
            method: Method::Post,
            url: url.into(),
            headers: vec![("content-type", "application/json".into())],
            body: Some(serde_json::to_vec(body).expect("a Value always serializes")),
            timeout,
        }
    }

    pub fn bearer(self, token: &str) -> Self {
        self.header("authorization", format!("Bearer {token}"))
    }

    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field(
                "headers",
                &shown_headers(self.headers.iter().map(|(k, v)| (*k, v.as_str()))),
            )
            .field(
                "body",
                &format_args!("{}", shown_body(self.body.as_deref())),
            )
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// A response, whatever its status: a 4xx or 5xx is a response, not an error. Its body can
/// carry tokens, so `Debug` shows only its length.
#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    /// Names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A response with a JSON body, as fakes and tests build one.
    pub fn json_body(status: u16, body: &Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: serde_json::to_vec(body).expect("a Value always serializes"),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as JSON; `None` when it is not valid JSON.
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body", &format_args!("<{} bytes>", self.body.len()))
            .finish()
    }
}

/// A transport failure (§4.4). `PreSend` only when the request provably never left the
/// machine; anything else is `Ambiguous`, since the server may have acted on it (§7.3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("the request was never sent: {0}")]
    PreSend(String),
    #[error("the request may have been sent, but no response was read: {0}")]
    Ambiguous(String),
}

pub trait Http: Send + Sync {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// Never sends anything: for engines that must make no request at all.
pub struct NoHttp;

impl Http for NoHttp {
    fn send(&self, _req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::PreSend("network disabled".into()))
    }
}

/// A request as `ScriptedHttp` saw it. `Debug` redacts like `HttpRequest`'s.
#[derive(Clone)]
pub struct RecordedRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl fmt::Debug for RecordedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordedRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field(
                "headers",
                &shown_headers(self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
            )
            .field(
                "body",
                &format_args!("{}", shown_body(self.body.as_deref())),
            )
            .finish()
    }
}

type Reply = Result<HttpResponse, HttpError>;

/// The tests' `Http`: replies queued per `(method, url)` and served first in, first out; the
/// last reply queued for a route repeats once it is the only one left. An unscripted route is
/// `PreSend`, as an unreachable host would be. Every request is recorded, scripted or not.
#[derive(Default)]
pub struct ScriptedHttp {
    routes: Mutex<HashMap<(Method, String), VecDeque<Reply>>>,
    log: Mutex<Vec<RecordedRequest>>,
}

impl ScriptedHttp {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, method: Method, url: &str, reply: Reply) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((method, url.to_owned()))
            .or_default()
            .push_back(reply);
    }

    pub fn push_json(&self, method: Method, url: &str, status: u16, body: Value) {
        self.push(method, url, Ok(HttpResponse::json_body(status, &body)));
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn count(&self, method: Method, url: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.method == method && r.url == url)
            .count()
    }

    /// Drops every queued reply and the request log.
    pub fn clear(&self) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

impl Http for ScriptedHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(RecordedRequest {
                method: req.method,
                url: req.url.clone(),
                headers: req
                    .headers
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), v.clone()))
                    .collect(),
                body: req.body.clone(),
            });
        let mut routes = self.routes.lock().unwrap_or_else(PoisonError::into_inner);
        match routes.get_mut(&(req.method, req.url.clone())) {
            Some(queue) if queue.len() > 1 => queue.pop_front().expect("the queue is not empty"),
            Some(queue) => queue
                .front()
                .cloned()
                .expect("a queued route is never empty"),
            None => Err(HttpError::PreSend(format!(
                "no scripted reply for {} {}",
                req.method.as_str(),
                req.url
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    const T: Duration = Duration::from_secs(5);

    #[test]
    fn debug_never_shows_the_bearer_token_or_the_body() {
        let req = HttpRequest::post_json(
            "https://example.test/token",
            &json!({"refresh_token": "sk-ant-ort01-SENTINEL"}),
            T,
        )
        .bearer("sk-ant-oat01-SENTINEL");
        let shown = format!("{req:?}");
        assert!(!shown.contains("SENTINEL"), "{shown}");
        assert!(shown.contains("authorization: <redacted>"), "{shown}");
        assert!(shown.contains("content-type: application/json"), "{shown}");
        assert!(shown.contains("https://example.test/token"), "{shown}");

        let resp = HttpResponse::json_body(200, &json!({"access_token": "sk-ant-SENTINEL"}));
        let shown = format!("{resp:?}");
        assert!(!shown.contains("SENTINEL"), "{shown}");
        assert!(shown.contains("200"), "{shown}");
    }

    #[test]
    fn builders_set_method_headers_and_body() {
        let get = HttpRequest::get("https://example.test/p", T).header("anthropic-beta", "b1");
        assert_eq!(get.method, Method::Get);
        assert_eq!(get.body, None);
        assert_eq!(get.headers, vec![("anthropic-beta", "b1".to_owned())]);
        assert_eq!(get.timeout, T);

        let post =
            HttpRequest::post_json("https://example.test/t", &json!({"a": 1}), T).bearer("tok");
        assert_eq!(post.method, Method::Post);
        assert_eq!(post.body.as_deref(), Some(br#"{"a":1}"#.as_slice()));
        assert_eq!(
            post.headers,
            vec![
                ("content-type", "application/json".to_owned()),
                ("authorization", "Bearer tok".to_owned()),
            ]
        );
    }

    #[test]
    fn response_headers_are_case_insensitive_and_json_is_optional() {
        let resp = HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "30".into())],
            body: b"not json".to_vec(),
        };
        assert_eq!(resp.header("Retry-After"), Some("30"));
        assert_eq!(resp.header("x-missing"), None);
        assert_eq!(resp.json(), None);
        assert_eq!(
            HttpResponse::json_body(200, &json!({"k": [1]})).json(),
            Some(json!({"k": [1]}))
        );
    }

    #[test]
    fn scripted_replies_are_fifo_and_the_last_one_repeats() {
        let http = ScriptedHttp::new();
        let url = "https://example.test/x";
        http.push_json(Method::Get, url, 200, json!({"n": 1}));
        http.push(Method::Get, url, Err(HttpError::Ambiguous("reset".into())));
        http.push_json(Method::Get, url, 500, json!({"n": 3}));
        let req = HttpRequest::get(url, T);
        assert_eq!(http.send(&req).unwrap().json(), Some(json!({"n": 1})));
        assert_eq!(
            http.send(&req).unwrap_err(),
            HttpError::Ambiguous("reset".into())
        );
        assert_eq!(http.send(&req).unwrap().status, 500);
        assert_eq!(
            http.send(&req).unwrap().status,
            500,
            "the last reply repeats"
        );
        assert_eq!(http.count(Method::Get, url), 4);
        assert_eq!(http.count(Method::Post, url), 0);
    }

    #[test]
    fn an_unscripted_route_is_pre_send_and_still_recorded() {
        let http = ScriptedHttp::new();
        http.push_json(Method::Get, "https://example.test/a", 200, json!({}));
        let err = http
            .send(&HttpRequest::post_json(
                "https://example.test/a",
                &json!({}),
                T,
            ))
            .unwrap_err();
        assert_eq!(
            err,
            HttpError::PreSend("no scripted reply for POST https://example.test/a".into())
        );
        assert_eq!(http.count(Method::Post, "https://example.test/a"), 1);
    }

    #[test]
    fn requests_are_recorded_with_headers_and_body_and_clear_drops_everything() {
        let http = ScriptedHttp::default();
        let url = "https://example.test/t";
        http.push_json(Method::Post, url, 200, json!({}));
        http.send(&HttpRequest::post_json(url, &json!({"r": "x"}), T).bearer("tok"))
            .unwrap();
        let log = http.requests();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].method, Method::Post);
        assert_eq!(log[0].url, url);
        assert!(
            log[0]
                .headers
                .contains(&("authorization".to_owned(), "Bearer tok".to_owned()))
        );
        assert_eq!(log[0].body.as_deref(), Some(br#"{"r":"x"}"#.as_slice()));
        let shown = format!("{:?}", log[0]);
        assert!(
            !shown.contains("Bearer tok"),
            "recorded Debug redacts: {shown}"
        );
        assert!(shown.contains("authorization: <redacted>"), "{shown}");

        http.clear();
        assert!(http.requests().is_empty());
        assert!(matches!(
            http.send(&HttpRequest::get(url, T)),
            Err(HttpError::PreSend(_))
        ));
    }

    #[test]
    fn no_http_never_sends() {
        assert_eq!(
            NoHttp
                .send(&HttpRequest::get("https://example.test/", T))
                .unwrap_err(),
            HttpError::PreSend("network disabled".into())
        );
    }

    #[test]
    fn the_response_body_cap_is_one_mebibyte() {
        // The User-Agent is pinned on the wire by Task 3's adapter test, not against its own definition.
        assert_eq!(MAX_BODY, 1_048_576);
    }
}
