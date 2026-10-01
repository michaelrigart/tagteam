//! The production `Http` port (§4.4): blocking `ureq` over rustls with the platform verifier,
//! so the OS trust store (and a corporate TLS proxy's root) is honoured.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tagteam_provider::http::{
    Http, HttpError, HttpRequest, HttpResponse, MAX_BODY, Method, USER_AGENT,
};
use ureq::http::Uri;
use ureq::tls::{RootCerts, TlsConfig};
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::time::Duration as WaitFor;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport,
};
use ureq::{Agent, Proxy};

/// Resolves a host and port to addresses. Injectable, so a test can fail a lookup without
/// asking the machine's real resolver (§15.1).
pub type Resolver = fn(&str, u16) -> io::Result<Vec<SocketAddr>>;

fn system_resolver(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    Ok((host, port).to_socket_addrs()?.collect())
}

/// §4.4, §7.3: a suspended process that is resumed makes a Linux socket read with a receive
/// timeout fail with `EINTR` (a stop signal interrupts the call even with no handler; macOS
/// restarts it), and `ureq` surfaces that as an I/O error. Mapped to `Ambiguous`, it would
/// discard a reply the server already sent to a token refresh a stopped holder was waiting on.
/// This wraps the outermost transport, above TLS, and retries only the interrupted read with
/// what is left of its timeout: no byte was consumed, and the request is never re-sent. A
/// timeout already spent gets one `LAST_LOOK`, enough to take a reply that arrived while the
/// process was stopped (`ureq` would turn a zero into a full second); interrupted again, the
/// read times out.
#[derive(Debug)]
struct ResumeInterruptedRead;

const LAST_LOOK: Duration = Duration::from_millis(1);

impl<In: Transport> Connector<In> for ResumeInterruptedRead {
    type Out = ResumingTransport<In>;

    fn connect(
        &self,
        _: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(ResumingTransport))
    }
}

#[derive(Debug)]
struct ResumingTransport<T>(T);

impl<T: Transport> Transport for ResumingTransport<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.0.buffers()
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.0.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let started = Instant::now();
        let mut next = timeout;
        let mut looked = false;
        loop {
            match self.0.await_input(next) {
                Err(ureq::Error::Io(e)) if e.kind() == io::ErrorKind::Interrupted => {
                    if let WaitFor::Exact(d) = timeout.after {
                        let left = d.saturating_sub(started.elapsed());
                        if left.is_zero() {
                            if looked {
                                return Err(ureq::Error::Timeout(timeout.reason));
                            }
                            looked = true;
                        }
                        next.after = WaitFor::Exact(left.max(LAST_LOOK));
                    }
                }
                other => return other,
            }
        }
    }

    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }

    fn is_tls(&self) -> bool {
        self.0.is_tls()
    }
}

pub struct UreqHttp {
    agent: Agent,
    resolver: Resolver,
}

impl UreqHttp {
    /// 4xx and 5xx are responses, not errors; redirects are never followed, so a request that
    /// carries a token is only ever sent where the provider addressed it. Honours the standard
    /// proxy environment variables (§4.4).
    pub fn new() -> Self {
        Self::with_resolver(system_resolver)
    }

    /// `new()`, with the lookup that decides whether a DNS failure is `PreSend` replaced. The
    /// proxy still comes from the environment.
    pub fn with_resolver(resolver: Resolver) -> Self {
        Self::build(resolver, Proxy::try_from_env())
    }

    /// `with_resolver`, with the proxy given explicitly instead of read from the environment:
    /// `None` sends direct. Tests use this so they never inherit the machine's proxy (§15.1).
    pub fn with_proxy(resolver: Resolver, proxy: Option<Proxy>) -> Self {
        Self::build(resolver, proxy)
    }

    /// The system resolver, no proxy: a test's adapter for a local server.
    pub fn direct() -> Self {
        Self::build(system_resolver, None)
    }

    fn build(resolver: Resolver, proxy: Option<Proxy>) -> Self {
        let config = Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(USER_AGENT)
            .proxy(proxy)
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        Self {
            agent: Agent::with_parts(
                config,
                DefaultConnector::new().chain(ResumeInterruptedRead),
                DefaultResolver::default(),
            ),
            resolver,
        }
    }

    /// Whether a proxy resolves `uri`'s host for us: one is configured, `no_proxy` does not
    /// cover the URI, and the proxy does not ask its client to resolve (SOCKS4). Then tagteam
    /// must not look the host up itself, since a proxied network may have no external DNS.
    fn proxy_resolves(&self, uri: &Uri) -> bool {
        self.agent
            .config()
            .proxy()
            .is_some_and(|p| !p.is_no_proxy(uri) && !p.resolve_target())
    }
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

/// An I/O error that can only happen while connecting, before a byte of the request is sent.
fn connect_failure(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::AddrNotAvailable
            | io::ErrorKind::NetworkDown
    )
}

/// §4.4: `PreSend` only for failures that provably precede sending (resolving, connecting, a
/// request that could not even be built); everything else is `Ambiguous`. TLS errors are
/// `Ambiguous` too: rustls can also surface one while the response is being read, after the
/// request left, so a handshake failure is misfiled as `ambiguous`, the harmless direction.
fn classify(e: ureq::Error) -> HttpError {
    use ureq::{Error, Timeout};
    let detail = e.to_string();
    match e {
        Error::HostNotFound
        | Error::ConnectionFailed
        | Error::BadUri(_)
        | Error::Http(_)
        | Error::RequireHttpsOnly(_)
        | Error::InvalidProxyUrl
        | Error::ConnectProxyFailed(_)
        | Error::TlsRequired
        | Error::Timeout(Timeout::Resolve | Timeout::Connect) => HttpError::PreSend(detail),
        Error::Io(ref io) if connect_failure(io.kind()) => HttpError::PreSend(detail),
        _ => HttpError::Ambiguous(detail),
    }
}

/// Resolves the URL's host within `timeout`, so a DNS failure is a certain `PreSend`: `ureq`
/// reports a failed lookup as a plain I/O error, which on its own could also have come after
/// the request left.
fn resolve(uri: &Uri, timeout: Duration, resolver: Resolver) -> Result<(), HttpError> {
    let host = uri
        .host()
        .ok_or_else(|| HttpError::PreSend("the URL has no host".into()))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("http") {
            80
        } else {
            443
        });
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(resolver(&host, port).map(|addrs| addrs.len()));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(n)) if n > 0 => Ok(()),
        Ok(Ok(_)) => Err(HttpError::PreSend("the host did not resolve".into())),
        Ok(Err(e)) => Err(HttpError::PreSend(format!(
            "could not resolve the host: {e}"
        ))),
        Err(_) => Err(HttpError::PreSend("resolving the host timed out".into())),
    }
}

impl Http for UreqHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let started = Instant::now();
        let uri: Uri = req
            .url
            .parse()
            .map_err(|e| HttpError::PreSend(format!("invalid URL: {e}")))?;
        if !self.proxy_resolves(&uri) {
            resolve(&uri, req.timeout, self.resolver)?;
        }
        let remaining = req.timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(HttpError::PreSend(
                "the request timed out before it was sent".into(),
            ));
        }
        // The agent sets tagteam's User-Agent; a provider's own is dropped (§4.4).
        let headers = req
            .headers
            .iter()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("user-agent"));
        let sent = match req.method {
            Method::Get => {
                let mut b = self.agent.get(&req.url);
                for (k, v) in headers {
                    b = b.header(*k, v.as_str());
                }
                b.config().timeout_global(Some(remaining)).build().call()
            }
            Method::Post => {
                let mut b = self.agent.post(&req.url);
                for (k, v) in headers {
                    b = b.header(*k, v.as_str());
                }
                let b = b.config().timeout_global(Some(remaining)).build();
                match &req.body {
                    Some(body) => b.send(body.as_slice()),
                    None => b.send_empty(),
                }
            }
        };
        let mut resp = sent.map_err(classify)?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        // A response was already arriving, so the request was acted on: a body that cannot be
        // read in full is `Ambiguous`, never `PreSend`.
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY as u64)
            .read_to_vec()
            .map_err(|e| HttpError::Ambiguous(format!("reading the response body: {e}")))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use ureq::Timeout;
    use ureq::unversioned::transport::LazyBuffers;

    use super::*;

    #[derive(Debug)]
    struct Scripted {
        buffers: LazyBuffers,
        replies: VecDeque<Result<bool, io::ErrorKind>>,
        timeouts: Vec<NextTimeout>,
    }

    impl Scripted {
        fn new(replies: impl IntoIterator<Item = Result<bool, io::ErrorKind>>) -> Self {
            Self {
                buffers: LazyBuffers::new(64, 64),
                replies: replies.into_iter().collect(),
                timeouts: Vec::new(),
            }
        }
    }

    impl Transport for Scripted {
        fn buffers(&mut self) -> &mut dyn Buffers {
            &mut self.buffers
        }

        fn transmit_output(&mut self, _: usize, _: NextTimeout) -> Result<(), ureq::Error> {
            Ok(())
        }

        fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
            self.timeouts.push(timeout);
            thread::sleep(Duration::from_millis(2));
            self.replies
                .pop_front()
                .expect("the transport was read more often than scripted")
                .map_err(|kind| ureq::Error::Io(io::Error::from(kind)))
        }

        fn is_open(&mut self) -> bool {
            true
        }
    }

    fn wait(secs: u64) -> NextTimeout {
        NextTimeout {
            after: WaitFor::from_secs(secs),
            reason: Timeout::RecvResponse,
        }
    }

    #[test]
    fn an_interrupted_read_is_retried_with_the_time_left() {
        let mut t = ResumingTransport(Scripted::new([
            Err(io::ErrorKind::Interrupted),
            Err(io::ErrorKind::Interrupted),
            Ok(true),
        ]));
        assert!(t.await_input(wait(30)).unwrap());
        let seen = &t.0.timeouts;
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], wait(30));
        for pair in seen.windows(2) {
            assert!(*pair[1].after <= *pair[0].after);
            assert_eq!(pair[1].reason, Timeout::RecvResponse);
        }
        assert!(*seen[2].after < *seen[0].after);
    }

    #[test]
    fn a_read_resumed_past_its_deadline_takes_a_last_look() {
        let mut t = ResumingTransport(Scripted::new([Err(io::ErrorKind::Interrupted), Ok(true)]));
        let timeout = NextTimeout {
            after: WaitFor::from_millis(1),
            reason: Timeout::RecvResponse,
        };
        assert!(t.await_input(timeout).unwrap());
        let last_look = NextTimeout {
            after: WaitFor::Exact(LAST_LOOK),
            ..timeout
        };
        assert_eq!(t.0.timeouts, vec![timeout, last_look]);
    }

    #[test]
    fn an_interrupted_last_look_times_out() {
        let mut t = ResumingTransport(Scripted::new([
            Err(io::ErrorKind::Interrupted),
            Err(io::ErrorKind::Interrupted),
        ]));
        let timeout = NextTimeout {
            after: WaitFor::from_millis(1),
            reason: Timeout::RecvResponse,
        };
        let err = t.await_input(timeout).unwrap_err();
        assert!(
            matches!(err, ureq::Error::Timeout(Timeout::RecvResponse)),
            "{err}"
        );
        assert_eq!(t.0.timeouts.len(), 2);
    }

    #[test]
    fn another_io_error_is_returned_after_one_read() {
        let mut t = ResumingTransport(Scripted::new([Err(io::ErrorKind::ConnectionReset)]));
        let err = t.await_input(wait(30)).unwrap_err();
        assert!(
            matches!(&err, ureq::Error::Io(e) if e.kind() == io::ErrorKind::ConnectionReset),
            "{err}"
        );
        assert_eq!(t.0.timeouts.len(), 1);
    }

    #[test]
    fn a_read_that_made_no_progress_is_passed_through() {
        let mut t = ResumingTransport(Scripted::new([Ok(false)]));
        assert!(!t.await_input(wait(30)).unwrap());
        assert_eq!(t.0.timeouts.len(), 1);
    }

    #[test]
    fn only_failures_before_sending_are_pre_send() {
        use ureq::{Error, Timeout};
        let pre = [
            Error::HostNotFound,
            Error::ConnectionFailed,
            Error::Timeout(Timeout::Resolve),
            Error::Timeout(Timeout::Connect),
            Error::Io(io::Error::from(io::ErrorKind::ConnectionRefused)),
            Error::Io(io::Error::from(io::ErrorKind::NetworkUnreachable)),
        ];
        for e in pre {
            let shown = e.to_string();
            assert!(matches!(classify(e), HttpError::PreSend(_)), "{shown}");
        }
        let ambiguous = [
            Error::Timeout(Timeout::Global),
            Error::Timeout(Timeout::SendBody),
            Error::Timeout(Timeout::RecvResponse),
            Error::Timeout(Timeout::RecvBody),
            Error::Io(io::Error::from(io::ErrorKind::ConnectionReset)),
            Error::Io(io::Error::from(io::ErrorKind::UnexpectedEof)),
            // A TLS error can surface after the request left, so it is never `PreSend`.
            Error::Tls("handshake failed"),
        ];
        for e in ambiguous {
            let shown = e.to_string();
            assert!(matches!(classify(e), HttpError::Ambiguous(_)), "{shown}");
        }
    }
}
