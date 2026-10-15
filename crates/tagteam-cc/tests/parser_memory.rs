//! Spec §5.2: a worker's memory stays bounded. Parsing one line holds only a small, fixed amount
//! beyond the line itself, however long its `usage.iterations` array is (spec §2.2: only the
//! first entry counts). Its own test binary, so the counting allocator sees nothing else.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use tagteam_cc::transcripts::parse_usage_line;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; this only counts bytes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract for `layout` is passed through to `System`.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` above with this `layout`.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[test]
fn a_huge_iterations_array_is_skipped_not_collected() {
    let mut line = String::from(concat!(
        r#"{"type":"assistant","requestId":"r","sessionId":"s","#,
        r#""timestamp":"2026-10-14T09:00:00.000Z","isSidechain":false,"#,
        r#""message":{"id":"m","model":"claude-opus-5-5","stop_reason":"end_turn","#,
        r#""usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"#,
        r#""cache_read_input_tokens":0,"iterations":[{"input_tokens":7,"output_tokens":8}"#,
    ));
    for _ in 0..200_000 {
        line.push_str(concat!(
            r#",{"input_tokens":1,"output_tokens":1,"#,
            r#""cache_creation_input_tokens":1,"cache_read_input_tokens":1}"#,
        ));
    }
    line.push_str("]}}}");
    let bytes = line.into_bytes();

    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let parsed = parse_usage_line(&bytes).unwrap().unwrap();
    let held = PEAK.load(Ordering::SeqCst) - before;

    assert_eq!((parsed.counts.input, parsed.counts.output), (7, 8));
    // Collecting the 200,001 entries would hold about 10 MB; skipping them holds a few KB.
    assert!(
        held < 64 * 1024,
        "parsing held {held} bytes beyond the line"
    );
}
