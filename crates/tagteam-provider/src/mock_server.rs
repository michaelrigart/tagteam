//! A local HTTP/1.1 server for tests (§15.1): scripted replies per route, one request log
//! that every client process shares, and the failure shapes a real network produces (a
//! connection closed with no reply, a server that never answers). Plain `std::net`, one thread
//! per connection, and `Connection: close` on every reply, so each request is its own
//! connection.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The longest `Hang` holds a connection, so a forgotten test cannot pin a thread forever.
const HANG_CAP: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub enum MockReply {
    Json {
        status: u16,
        body: Value,
    },
    Raw {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Waits, then replies.
    Delay(Duration, Box<MockReply>),
    /// Reads the request, then closes the connection without writing a byte: the request may
    /// have been acted on, and no response ever arrives (an `Ambiguous` failure).
    Close,
    /// Reads the request, then never answers; the client times out.
    Hang,
}

#[derive(Debug, Clone)]
pub struct MockRequest {
    pub method: String,
    /// The request target as sent, query string included.
    pub path: String,
    /// Names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Default)]
struct State {
    routes: HashMap<(String, String), VecDeque<MockReply>>,
    log: Vec<MockRequest>,
}

pub struct MockServer {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

/// The route a target belongs to: its path, without the query string.
fn route_path(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn read_request(stream: &TcpStream) -> Option<MockRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            return None;
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    Some(MockRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_response(stream: &mut TcpStream, status: u16, headers: &[(String, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

fn sleep_unless_stopped(total: Duration, stop: &AtomicBool) {
    let until = Instant::now() + total;
    while !stop.load(Ordering::SeqCst) && Instant::now() < until {
        thread::sleep(Duration::from_millis(20));
    }
}

fn respond(mut stream: TcpStream, reply: MockReply, stop: &AtomicBool) {
    match reply {
        MockReply::Json { status, body } => {
            let bytes = serde_json::to_vec(&body).expect("a Value always serializes");
            let headers = [("Content-Type".to_owned(), "application/json".to_owned())];
            write_response(&mut stream, status, &headers, &bytes);
        }
        MockReply::Raw {
            status,
            headers,
            body,
        } => write_response(&mut stream, status, &headers, &body),
        MockReply::Delay(wait, then) => {
            sleep_unless_stopped(wait, stop);
            respond(stream, *then, stop);
        }
        MockReply::Close => {
            let _ = stream.shutdown(Shutdown::Both);
        }
        MockReply::Hang => sleep_unless_stopped(HANG_CAP, stop),
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>, stop: &AtomicBool) {
    let Some(req) = read_request(&stream) else {
        return;
    };
    let reply = {
        let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
        s.log.push(req.clone());
        let key = (req.method.clone(), route_path(&req.path).to_owned());
        match s.routes.get_mut(&key) {
            Some(queue) if queue.len() > 1 => queue.pop_front(),
            Some(queue) => queue.front().cloned(),
            None => None,
        }
    };
    let reply = reply.unwrap_or(MockReply::Json {
        status: 404,
        body: json!({"error": "not_found"}),
    });
    respond(stream, reply, stop);
}

impl MockServer {
    /// Binds `127.0.0.1:0` and starts serving at once.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let addr = listener.local_addr().expect("a bound address");
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let (state, stop) = (state.clone(), stop.clone());
            thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = conn else { continue };
                    let (state, stop) = (state.clone(), stop.clone());
                    thread::spawn(move || serve(stream, &state, &stop));
                }
            })
        };
        Self {
            addr,
            state,
            stop,
            accept: Some(accept),
        }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Queues `reply` for `method` and `path` (without a query string). Replies are served
    /// first in, first out; the last one repeats once it is the only one left.
    pub fn on(&self, method: &str, path: &str, reply: MockReply) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .routes
            .entry((method.to_owned(), path.to_owned()))
            .or_default()
            .push_back(reply);
    }

    /// How many requests reached `method` and `path`, from any process.
    pub fn hits(&self, method: &str, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.method == method && route_path(&r.path) == path)
            .count()
    }

    pub fn requests(&self) -> Vec<MockRequest> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .log
            .clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop, which then sees the flag and returns, dropping the listener.
        let _ = TcpStream::connect(self.addr);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{ErrorKind, Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::*;

    /// Sends `request` raw and reads until the server closes, or until `wait` passes.
    fn raw(server: &MockServer, request: &str, wait: Duration) -> std::io::Result<String> {
        let addr = server.base_url().trim_start_matches("http://").to_owned();
        let mut s = TcpStream::connect(addr)?;
        s.set_read_timeout(Some(wait))?;
        s.write_all(request.as_bytes())?;
        let mut out = Vec::new();
        s.read_to_end(&mut out)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn routes_reply_fifo_then_repeat_the_last_and_every_request_is_logged() {
        let server = MockServer::start();
        server.on(
            "GET",
            "/a",
            MockReply::Json {
                status: 200,
                body: json!({"n": 1}),
            },
        );
        server.on(
            "GET",
            "/a",
            MockReply::Json {
                status: 503,
                body: json!({"n": 2}),
            },
        );
        let first = raw(&server, &get("/a"), WAIT).unwrap();
        assert!(first.starts_with("HTTP/1.1 200 "), "{first}");
        assert!(first.ends_with(r#"{"n":1}"#), "{first}");
        assert!(
            first.to_ascii_lowercase().contains("connection: close"),
            "{first}"
        );
        for _ in 0..2 {
            let again = raw(&server, &get("/a?x=1"), WAIT).unwrap();
            assert!(again.starts_with("HTTP/1.1 503 "), "{again}");
        }
        assert_eq!(
            server.hits("GET", "/a"),
            3,
            "a query string routes by its path"
        );
        assert_eq!(server.hits("POST", "/a"), 0);

        let body = r#"{"k":"v"}"#;
        raw(
            &server,
            &format!(
                "POST /t HTTP/1.1\r\nHost: x\r\nX-Probe: Yes\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
            WAIT,
        )
        .unwrap();
        let log = server.requests();
        let post = log.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(post.path, "/t");
        assert_eq!(post.body, body.as_bytes());
        assert!(
            post.headers
                .contains(&("x-probe".to_owned(), "Yes".to_owned()))
        );
    }

    #[test]
    fn an_unrouted_path_is_404() {
        let server = MockServer::start();
        let out = raw(&server, &get("/nowhere"), WAIT).unwrap();
        assert!(out.starts_with("HTTP/1.1 404 "), "{out}");
        assert!(out.ends_with(r#"{"error":"not_found"}"#), "{out}");
    }

    #[test]
    fn raw_replies_carry_their_own_headers_and_bytes() {
        let server = MockServer::start();
        server.on(
            "GET",
            "/r",
            MockReply::Raw {
                status: 429,
                headers: vec![("Retry-After".into(), "30".into())],
                body: b"slow down".to_vec(),
            },
        );
        let out = raw(&server, &get("/r"), WAIT).unwrap();
        assert!(out.starts_with("HTTP/1.1 429 "), "{out}");
        assert!(out.contains("Retry-After: 30\r\n"), "{out}");
        assert!(out.ends_with("slow down"), "{out}");
    }

    #[test]
    fn delay_waits_before_replying() {
        let server = MockServer::start();
        server.on(
            "GET",
            "/d",
            MockReply::Delay(
                Duration::from_millis(300),
                Box::new(MockReply::Json {
                    status: 200,
                    body: json!({}),
                }),
            ),
        );
        let t = Instant::now();
        let out = raw(&server, &get("/d"), WAIT).unwrap();
        assert!(t.elapsed() >= Duration::from_millis(300));
        assert!(out.starts_with("HTTP/1.1 200 "), "{out}");
    }

    #[test]
    fn close_answers_with_no_bytes_at_all() {
        let server = MockServer::start();
        server.on("POST", "/c", MockReply::Close);
        let out = raw(
            &server,
            "POST /c HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\n{}",
            WAIT,
        );
        match out {
            Ok(s) => assert!(s.is_empty(), "{s}"),
            Err(e) => assert_eq!(e.kind(), ErrorKind::ConnectionReset),
        }
        assert_eq!(
            server.hits("POST", "/c"),
            1,
            "the request was read before closing"
        );
    }

    #[test]
    fn hang_never_answers() {
        let server = MockServer::start();
        server.on("GET", "/h", MockReply::Hang);
        let err = raw(&server, &get("/h"), Duration::from_millis(300)).unwrap_err();
        assert!(
            matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
            "{err:?}"
        );
        assert_eq!(server.hits("GET", "/h"), 1);
    }

    #[test]
    fn dropping_the_server_stops_it() {
        let server = MockServer::start();
        server.on("GET", "/h", MockReply::Hang);
        let addr = server.base_url().trim_start_matches("http://").to_owned();
        let _ = raw(&server, &get("/h"), Duration::from_millis(100));
        drop(server);
        assert!(TcpStream::connect(addr).is_err(), "the listener is gone");
    }
}
