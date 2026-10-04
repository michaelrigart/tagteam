//! `capture_logs` itself. It lives in a binary of its own, with one test: the failure it pins
//! needs a single capturing subscriber alive in the process, which a neighbouring test would
//! hide.
mod common;

use common::capture_logs;

fn probe() {
    tracing::info!("a call site first reached from another thread");
}

#[test]
fn a_call_site_first_fired_on_another_thread_is_still_captured() {
    // `tracing` caches a call site's interest when it is first reached. With one subscriber
    // alive it asks only the reaching thread's own, so a thread with none caches "never", and
    // a capture on this thread would see nothing from the site for the rest of the process.
    let ((), lines) = capture_logs(|| {
        std::thread::spawn(probe).join().unwrap();
        probe();
    });
    assert_eq!(
        lines,
        [" INFO a call site first reached from another thread"]
    );
}
