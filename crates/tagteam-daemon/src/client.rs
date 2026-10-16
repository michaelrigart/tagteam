//! D5.7: the client of the daemon's socket, used by `tagteam daemon status` and the TUI. It
//! finds the socket from the environment alone (D5.1), and learns in the first exchange
//! whether a daemon is there and speaks this protocol (D1.1 criterion 5).

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_engine::daemon::daemon_socket_path;
use tagteam_provider::Env;

use crate::protocol::{Event, Hello, MAX_LINE, PROTOCOL, Status, parse_event};
use crate::server::MAX_SOCKET_PATH;

/// D5.7: how long `connect` waits for the greeting.
pub const GREETING_TIMEOUT: Duration = Duration::from_millis(250);
/// D5.7: how long `status`, and `subscribe`'s `ok`, may take.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// What `connect` found.
#[derive(Debug)]
pub enum Connect {
    Connected(Client),
    /// No socket file, a path too long to bind, a refused connection, or the end of it before
    /// the greeting: no daemon serves this data directory, and the caller works without one.
    NoDaemon,
    /// The daemon speaks another protocol (D5.6). The connection is closed, and the caller
    /// works without the daemon until it restarts on this binary (D4.5).
    Mismatch {
        server: u32,
        client: u32,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("the daemon answered something this client cannot read: {0}")]
    Protocol(String),
    #[error("the daemon did not answer in time")]
    Timeout,
    #[error("the daemon refused the request ({kind}): {message}")]
    Refused { kind: String, message: String },
}

/// D5.7: the daemon of the store this environment uses, at `daemon_socket_path(env)`.
pub fn connect(env: &Env) -> Result<Connect, ClientError> {
    connect_at(&daemon_socket_path(env))
}

/// D5.7: the daemon at `path`, greeted within `GREETING_TIMEOUT`. A greeting that does not come
/// in time is `Timeout`: something accepted and said nothing, so a daemon is there but does
/// not answer (D8.2). A path over `MAX_SOCKET_PATH` is `NoDaemon` without a connect: no daemon
/// can have bound it (D5.1), and the operating system would answer `InvalidInput`.
pub fn connect_at(path: &Path) -> Result<Connect, ClientError> {
    if path.as_os_str().as_bytes().len() > MAX_SOCKET_PATH {
        return Ok(Connect::NoDaemon);
    }
    let stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(e) if no_daemon(&e) => return Ok(Connect::NoDaemon),
        Err(e) => return Err(e.into()),
    };
    greet(stream)
}

/// The half of `connect_at` after the connection: the greeting, read and checked.
fn greet(stream: UnixStream) -> Result<Connect, ClientError> {
    timeout_set(stream.set_write_timeout(Some(REPLY_TIMEOUT)))?;
    let mut lines = Lines::new(stream);
    let greeting = match lines.read_line(Some(Instant::now() + GREETING_TIMEOUT)) {
        Ok(Some(line)) => line,
        Ok(None) => return Ok(Connect::NoDaemon),
        Err(ClientError::Io(e)) if e.kind() == io::ErrorKind::ConnectionReset => {
            return Ok(Connect::NoDaemon);
        }
        Err(e) => return Err(e),
    };
    let v: Value = serde_json::from_slice(&greeting)
        .map_err(|e| ClientError::Protocol(format!("the greeting is not JSON ({e})")))?;
    if v.get("event").and_then(Value::as_str) != Some("hello") {
        return Err(ClientError::Protocol(
            "the first line is not the greeting".into(),
        ));
    }
    let server = v
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or_else(|| ClientError::Protocol("the greeting names no protocol".into()))?;
    if server != u64::from(PROTOCOL) {
        return Ok(Connect::Mismatch {
            server: u32::try_from(server).unwrap_or(u32::MAX),
            client: PROTOCOL,
        });
    }
    let hello = serde_json::from_value(v)
        .map_err(|e| ClientError::Protocol(format!("the greeting cannot be read ({e})")))?;
    Ok(Connect::Connected(Client {
        lines,
        hello,
        next_id: 1,
    }))
}

/// D5.1: what a connect that finds no daemon fails with. `ENOTSOCK` is macOS's answer for a
/// file that is not a socket, where Linux answers `ECONNREFUSED`.
fn no_daemon(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    ) || e.raw_os_error() == Some(libc::ENOTSOCK)
}

/// macOS refuses `SO_RCVTIMEO` and `SO_SNDTIMEO` with `EINVAL` once the peer has closed. The
/// read or write that follows then answers at once, from the data still queued or with the end
/// of the stream, so that refusal is no failure.
fn timeout_set(set: io::Result<()>) -> io::Result<()> {
    match set {
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => Ok(()),
        other => other,
    }
}

/// A greeted connection.
#[derive(Debug)]
pub struct Client {
    lines: Lines,
    hello: Hello,
    /// The next request's id (D5.3).
    next_id: i64,
}

impl Client {
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// D5.4's status, waited for at most `REPLY_TIMEOUT`.
    pub fn status(&mut self) -> Result<Status, ClientError> {
        let result = self.request("status")?;
        serde_json::from_value(result)
            .map_err(|e| ClientError::Protocol(format!("the status cannot be read ({e})")))
    }

    /// D5.5: every event from the daemon's `ok` on.
    pub fn subscribe(mut self) -> Result<Events, ClientError> {
        self.request("subscribe")?;
        Ok(Events {
            lines: self.lines,
            done: false,
        })
    }

    /// Sends `op` with the next id and returns its reply's `result`. Events, and replies to
    /// other ids, are passed over (D5.3).
    fn request(&mut self, op: &str) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.lines.send(&json!({"id": id, "op": op}))?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let want = json!(id);
        loop {
            let Some(line) = self.lines.read_line(Some(deadline))? else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the daemon closed the connection",
                )
                .into());
            };
            let reply: Value = serde_json::from_slice(&line)
                .map_err(|e| ClientError::Protocol(format!("a reply is not JSON ({e})")))?;
            if reply.get("event").is_some() || reply.get("id") != Some(&want) {
                continue;
            }
            return match reply.get("ok").and_then(Value::as_bool) {
                Some(true) => reply
                    .get("result")
                    .cloned()
                    .ok_or_else(|| ClientError::Protocol("a reply has no result".into())),
                Some(false) => Err(ClientError::Refused {
                    kind: reply["error"]["type"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    message: reply["error"]["message"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                }),
                None => Err(ClientError::Protocol("a reply has no ok".into())),
            };
        }
    }
}

/// D5.5's events, typed (D5.7). It waits for as long as the daemon is silent, and ends when the
/// daemon closes the connection: after `stopping`, or when it dropped this subscriber for
/// falling behind, after which the caller re-reads what it shows and subscribes again.
#[derive(Debug)]
pub struct Events {
    lines: Lines,
    done: bool,
}

impl Events {
    /// A handle that ends this stream from another thread: the iterator then ends, as when the
    /// daemon closes the connection. Its read waits for as long as the daemon is silent, so a
    /// caller that finds the daemon no longer answers (the TUI, by `status`) ends it this way.
    pub fn closer(&self) -> io::Result<Closer> {
        self.lines.stream.try_clone().map(Closer)
    }
}

/// Ends an `Events` stream from another thread (`Events::closer`).
#[derive(Debug)]
pub struct Closer(UnixStream);

impl Closer {
    pub fn close(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

impl Iterator for Events {
    type Item = Result<Event, ClientError>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            let line = match self.lines.read_line(None) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(ClientError::Io(e)) if e.kind() == io::ErrorKind::ConnectionReset => break,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            };
            match serde_json::from_slice::<Value>(&line) {
                Ok(v) => {
                    // A line that is no event (a stray reply) is passed over.
                    if let Some(event) = parse_event(&v) {
                        return Some(Ok(event));
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(ClientError::Protocol(format!(
                        "an event is not JSON ({e})"
                    ))));
                }
            }
        }
        self.done = true;
        None
    }
}

/// The connection's lines, each read by a deadline.
#[derive(Debug)]
struct Lines {
    stream: UnixStream,
    /// Bytes read that are not yet a whole line.
    buf: Vec<u8>,
}

impl Lines {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            buf: Vec::new(),
        }
    }

    /// One request, as one line.
    fn send(&mut self, v: &Value) -> io::Result<()> {
        let mut line = v.to_string();
        line.push('\n');
        self.stream.write_all(line.as_bytes())
    }

    /// The next whole line, its newline left off. `None` at the end of the connection; a last
    /// line cut short goes with it. `Timeout` once `deadline` passes; with no deadline it waits
    /// for as long as it takes. A line over `MAX_LINE` is `Protocol`, whether its newline came
    /// in the read that passed the limit or in a later one.
    fn read_line(&mut self, deadline: Option<Instant>) -> Result<Option<Vec<u8>>, ClientError> {
        let mut chunk = [0u8; 8192];
        loop {
            let newline = self.buf.iter().position(|b| *b == b'\n');
            if newline.unwrap_or(self.buf.len()) > MAX_LINE {
                return Err(ClientError::Protocol(
                    "the daemon sent a line over 64 KiB".into(),
                ));
            }
            if let Some(end) = newline {
                let mut line: Vec<u8> = self.buf.drain(..=end).collect();
                line.pop();
                return Ok(Some(line));
            }
            let wait = match deadline {
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => Some(left),
                    _ => return Err(ClientError::Timeout),
                },
                None => None,
            };
            timeout_set(self.stream.set_read_timeout(wait))?;
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(None),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_closed_before_the_timeouts_are_set_is_no_daemon() {
        // The race `fake(&path, drop)` and a full daemon run: the peer is gone before
        // `connect_at` sets its timeouts, which macOS refuses with EINVAL.
        let (client, server) = UnixStream::pair().unwrap();
        drop(server);
        assert!(matches!(greet(client), Ok(Connect::NoDaemon)));
    }
}
