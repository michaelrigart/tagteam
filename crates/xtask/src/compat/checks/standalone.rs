//! The checks that need no account.

use std::time::Instant;

use serde_json::json;
use tagteam_provider::keychain::Keychain as _;

use super::profile::read_name;
use crate::compat::ctx::Ctx;
use crate::compat::keychain::ThrowawayKeychain;
use crate::compat::report::{Outcome, Probe};
use crate::compat::sys::{HarnessError, harness};

/// Not a Claude Code service: B.70 does not reach it.
const PROBE_SERVICE: &str = "tagteam-compat-probe";

/// Appendix A.3: on a locked keychain file the existence probe (`find-generic-password` without
/// `-w`) answers rc 0 for a present item and rc 44 for an absent one, at once and without a
/// prompt, which is what lets a file-fallback activation commit. A throwaway keychain file.
pub fn locked_file_probe(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let kc = ThrowawayKeychain::create(&ctx.layout.scratch.join("probe.keychain-db"))?;
    let cli = kc.cli();
    cli.upsert(PROBE_SERVICE, "present", b"x")
        .map_err(|e| harness(format!("writing the probe item: {e}")))?;
    kc.lock()?;
    for (account, want) in [("present", "present"), ("absent", "absent")] {
        let t = Instant::now();
        let r = cli.exists(PROBE_SERVICE, account);
        let seconds = t.elapsed().as_secs_f64();
        p.expect(
            &format!("the probe of an {want} item"),
            read_name(&r) == want && seconds < 5.0,
            json!({"read": read_name(&r), "seconds": seconds}),
        );
    }
    p.note(
        "the lock check on the locked file",
        json!(format!("{:?}", cli.lock_state())),
    );
    Ok(p.finish("rc 0 and rc 44 on a locked keychain file, without a prompt"))
}
