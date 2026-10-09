//! A local stand-in for the Messages API, for the checks that must see which credential `claude`
//! sends without spending anything: `claude` is pointed at it with `ANTHROPIC_BASE_URL`. It
//! keeps only a fingerprint of each credential, and answers every request with a 400
//! `invalid_request_error`, which `claude` neither takes for an authentication failure nor
//! answers by refreshing or wiping its login.

use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

use super::report::fingerprint;
use super::sys::wait_until;

const REPLY: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"tagteam compat: a local stand-in, not the API"}}"#;

/// The credential a request carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    /// `x-api-key`, fingerprinted.
    ApiKey(String),
    /// `Authorization: Bearer`, fingerprinted.
    Bearer(String),
    None,
}

impl Sent {
    pub fn to_json(&self) -> Value {
        match self {
            Sent::ApiKey(fp) => json!({"apiKey": fp}),
            Sent::Bearer(fp) => json!({"bearer": fp}),
            Sent::None => Value::Null,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub sent: Sent,
}

impl Seen {
    fn is_message(&self) -> bool {
        self.method == "POST" && self.path.starts_with("/v1/messages")
    }
}

pub struct Capture {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

fn serve(stream: TcpStream, seen: &Mutex<Vec<Seen>>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();
    let (mut sent, mut length) = (Sent::None, 0usize);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "x-api-key" => sent = Sent::ApiKey(fingerprint(value)),
            "authorization" => {
                if let Some(token) = value.strip_prefix("Bearer ") {
                    sent = Sent::Bearer(fingerprint(token.trim()));
                }
            }
            "content-length" => length = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    let mut body = vec![0u8; length.min(16 << 20)];
    reader.read_exact(&mut body)?;
    seen.lock().unwrap().push(Seen { method, path, sent });
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{REPLY}",
        REPLY.len()
    )?;
    stream.flush()
}

impl Capture {
    /// Listens on `127.0.0.1:0` at once.
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (log, halt) = (seen.clone(), stop.clone());
        let accept = thread::spawn(move || {
            for stream in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let log = log.clone();
                    thread::spawn(move || {
                        let _ = serve(stream, &log);
                    });
                }
            }
        });
        Ok(Self {
            addr,
            seen,
            stop,
            accept: Some(accept),
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// How many Messages requests it has seen.
    pub fn messages(&self) -> usize {
        self.seen().iter().filter(|s| s.is_message()).count()
    }

    /// The credential of the first Messages request after the first `after`, waiting up to
    /// `timeout` for it.
    pub fn next_message(&self, after: usize, timeout: Duration) -> Option<Sent> {
        wait_until(timeout, || self.messages() > after);
        self.seen()
            .into_iter()
            .filter(Seen::is_message)
            .nth(after)
            .map(|s| s.sent)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(c: &Capture, headers: &str) -> String {
        let mut s = TcpStream::connect(c.addr).unwrap();
        write!(
            s,
            "POST /v1/messages?beta=true HTTP/1.1\r\nhost: x\r\n{headers}content-length: 2\r\n\r\n{{}}"
        )
        .unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        reply
    }

    #[test]
    fn it_keeps_a_fingerprint_of_each_credential_and_answers_400() {
        let c = Capture::start().unwrap();
        let reply = send(&c, "x-api-key: sk-ant-api03-k1\r\n");
        assert!(reply.starts_with("HTTP/1.1 400 "), "{reply}");
        assert!(reply.contains("invalid_request_error"));
        send(&c, "Authorization: Bearer at-1\r\n");
        assert_eq!(c.messages(), 2);
        assert_eq!(
            c.next_message(0, Duration::from_secs(1)),
            Some(Sent::ApiKey(fingerprint("sk-ant-api03-k1")))
        );
        assert_eq!(
            c.next_message(1, Duration::from_secs(1)),
            Some(Sent::Bearer(fingerprint("at-1")))
        );
        assert_eq!(c.next_message(2, Duration::from_millis(50)), None);
    }
}
