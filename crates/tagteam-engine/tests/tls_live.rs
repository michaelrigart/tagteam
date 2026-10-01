//! The production `Http` adapter against the real internet, to prove its TLS stack: rustls with
//! the platform verifier (§4.4) trusts the OS roots and rejects a chain that does not end in
//! one. Ignored by default (it needs the network); the release smoke job runs it on the musl
//! targets: `cargo test -p tagteam-engine --test tls_live -- --ignored`.

use std::time::Duration;

use tagteam_engine::net::UreqHttp;
use tagteam_provider::{Http, HttpError, HttpRequest};

const T: Duration = Duration::from_secs(20);

#[test]
#[ignore = "needs the network"]
fn a_public_certificate_chain_is_trusted() {
    let req = HttpRequest::get("https://api.anthropic.com/api/oauth/profile", T);
    let resp = UreqHttp::direct()
        .send(&req)
        .expect("the TLS handshake with api.anthropic.com succeeds, whatever the status");
    assert!(
        (100..600).contains(&resp.status),
        "an HTTP response: {}",
        resp.status
    );
}

#[test]
#[ignore = "needs the network"]
fn an_untrusted_root_is_a_certificate_failure() {
    let req = HttpRequest::get("https://untrusted-root.badssl.com/", T);
    match UreqHttp::direct().send(&req) {
        Err(HttpError::Ambiguous(detail)) => assert!(
            detail.contains("invalid peer certificate"),
            "a certificate-validation failure, not another transport error: {detail}"
        ),
        other => panic!("expected an Ambiguous certificate failure, got {other:?}"),
    }
}
