# tagteam — The TUI reads its own keys

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/54
**Date:** 2026-10-31
**Scope:** The TUI's input thread (`crates/tagteam-tui/src/input.rs`) and a new vendored decoder
module. It stops calling crossterm's event reader and reads the terminal itself, so a key typed as
the terminal is resized is no longer held until the next key. Nothing else changes: no screen, key,
setting, CLI command or `--json` output.
**Builds on:**
- **TUI spec:** [`2026-10-14-tagteam-tui-design.md`](2026-10-14-tagteam-tui-design.md), cited as
  "TUI §n".
- **Redesign spec:** [`2026-10-20-tagteam-tui-redesign-design.md`](2026-10-20-tagteam-tui-redesign-design.md),
  cited as "redesign §n". Its §4.2 (reply filter, detection hand-off, M-6) and §14 PERF-R-9 (the
  input thread's wait) stay the authority for everything this spec does not change.

Line numbers are as of `0d046eeb` and will drift.

---

## 1. Purpose

### 1.1 The bug

A key typed in the same instant as a terminal resize is not delivered. It waits in the terminal
until another key arrives, and then both arrive. A user sees a keypress that does nothing until
they press something else.

CI shows it as `tui_pty::a_resize_while_idle_redraws_at_the_new_size` hanging: the test resizes,
waits for the redraw, types `q`, and the TUI is still running 20 s later. It failed on both
ubuntu runs of `bc64744f` on 2026-10-31 (main's push run and PR #52's), and passed on the re-run.
In Linux Docker (`rust:1.88`), it fails in 2 of 20 runs at `bc64744f` and 3 of 40 at `eda9605a`,
so it predates PR #50. It came with the test in `df090735`. macOS CI has not failed it, but the
same code path runs there (§1.2).

### 1.2 The cause

The input thread waits on fd 0 with its own mio `Poll` (`Wait`, `input.rs:68-170`), on its own
SIGWINCH registration and on a waker. Once woken, it asks crossterm for events with
`crossterm::event::poll(Duration::ZERO)` and `crossterm::event::read` (`input.rs:222-234`).

crossterm's event reader keeps a second mio `Poll` on the same fd, with its own SIGWINCH pipe.
mio is edge-triggered. In crossterm 0.29.0's `UnixInternalEventSource::try_read`
(`event/source/unix/mio.rs:92-136`), when one poll reports both the SIGWINCH pipe and the
terminal, and the signal comes first in the batch, it returns `Event::Resize` at once. The
terminal's readiness from that batch is then dropped. The key bytes stay unread in the kernel,
crossterm's poll never reports them again (no new edge), and crossterm says it holds nothing.
`Wait` has already spent its own edge for the same bytes, so the thread waits for the next byte.

An instrumented run confirmed this order of events. tagteam's SIGWINCH woke the thread (9 ms),
and crossterm had nothing yet because signal-hook had not yet written crossterm's pipe. The `q`
arrived (22 ms) and `Wait` returned `Input`. crossterm then returned only its own
`Resize(100, 30)` and said empty, and the `q` was never read. In passing runs, crossterm picked up
its SIGWINCH one wake before the key.

crossterm master still returns early on the signal, and 0.29.0 is the newest release.

### 1.3 Why this design

- **Read the terminal ourselves**, with crossterm's own decoder vendored. Nothing creates
  crossterm's event reader any more, so the second poll and the second SIGWINCH pipe are gone, and
  the race cannot happen. The same bytes decode to the same events, so key handling, the reply
  filter, detection's hand-off and M-6 are unchanged.
- **Rejected: crossterm's `use-dev-tty` feature.** Its source uses level-triggered `poll(2)` and
  reads the terminal before the signal pipe. But its `try_read` loops only `while` time is left
  (`event/source/unix/tty.rs:121`), so `event::poll(Duration::ZERO)` never polls and never
  returns an event. The input thread's drain depends on zero-timeout polls.
- **Rejected: a patched or forked crossterm.** A `[patch]` or git dependency does not reach
  `cargo install tagteam` from crates.io, and publishing a fork is out of proportion.
- **Rejected: waiting briefly for crossterm's `Resize` after our own SIGWINCH.** It shrinks the
  window to the microseconds between signal-hook's two pipe writes, but does not close it, and it
  reverses R-8 (crossterm's `Resize` is not waited for).
- **Rejected: quarantining the test.** The bug is real, and Michael chose to fix it first.

## 2. Goals

- **G1.** A key is delivered whatever SIGWINCH arrives with it.
- **G2.** Nothing in the TUI calls crossterm's event reader, and a lint keeps it that way.
- **G3.** Every byte sequence decodes to the events crossterm 0.29.0 gives today, from the same
  reads.

### Non-goals

- No crossterm upgrade, no new dependency, no change to ratatui's use of crossterm for drawing,
  raw mode, the alternate screen and `terminal::size()`.
- M-6 (redesign §4.2: a late DA1 reply cut short holds every key until a `c` or `u`) stays as
  accepted. The decoder still holds an unfinished CSI. Owning the decoder makes a later fix
  possible, but it is not this one.
- No change to detection (`detect.rs`), the reply filter (`replies.rs`), the pause, stop and
  hang-up control, or how resizes are seen (R-8: the thread's own SIGWINCH, the size from
  `crossterm::terminal::size()`).

### Anti-goals

- A key read differently from today, or a reply surfacing as keys. G3 guards this.
- The input thread blocking in a read, so a pause or stop waits on the user.
- `O_NONBLOCK` set on fd 0. The open file description is shared with the shell and with
  foreground actions (TUI §7.3), and the flag would outlive the TUI.

## 3. Design

### 3.1 The decoder (new module)

A new module, `crates/tagteam-tui/src/keys_decode.rs` (the builder may choose the name), holds:

- **crossterm 0.29.0's Unix decoder**, `src/event/sys/unix/parse.rs`, verbatim. Allowed edits:
  - imports: crossterm's public event types instead of `crate::event`;
  - a local `InternalEvent` with the four Unix variants of crossterm's (`event.rs:1475-1487`):
    `Event(Event)`, `CursorPosition(u16, u16)`, `KeyboardEnhancementFlags(..)`,
    `PrimaryDeviceAttributes`;
  - `cfg` attributes for features tagteam's crossterm build has, resolved to what that build
    compiles. The `bracketed-paste` feature is in crossterm's defaults; the builder confirms
    which features tagteam's lock resolves.
  - the two calls to crossterm's private `crate::terminal::sys::is_raw_mode_enabled()`
    (`parse.rs:99`, `:552`, the `\n` rule of issue #371), each replaced by
    `matches!(crossterm::terminal::is_raw_mode_enabled(), Ok(true))`. The public function wraps
    the same private one (`terminal.rs:107-111`) and reads the same static, so `\n` decodes as
    today in both places;
  - visibility, and module-level `#[allow(..)]` for lints the upstream code trips. No logic edit.
- **crossterm's `Parser`** (`event/source/unix/mio.rs:161-229`): the sequence buffer and the
  queue of decoded events, with `advance(bytes, more)`, verbatim.
- **crossterm's tests** for the decoder (`parse.rs:864-1506`), vendored with it, adjusted only
  for the same imports.
- **The MIT notice.** crossterm is MIT, as is tagteam. The module's header names its source
  (crossterm 0.29.0, the two file paths), carries crossterm's copyright line and licence text or
  a pointer to a vendored copy of it, and says what was edited. The builder checks how the
  repository records third-party notices (none exists yet) and adds the copy if a pointer is not
  enough for MIT.

### 3.2 The reader (`Terminal` in `input.rs`)

`Terminal` keeps `Wait` and gains the decoder. Its `Events::next` becomes one bounded step:

1. Take decoded items in order. A reply (`CursorPosition`, `KeyboardEnhancementFlags`,
   `PrimaryDeviceAttributes`) is dropped: nothing in tagteam asks for one. The first `Event` is
   returned.
2. `Wait::closed()` (unchanged). Hung up: `Closed`.
3. Ask the terminal how many bytes it holds (`FIONREAD` on fd 0). None: `Empty`.
4. Read once, up to 1024 bytes (crossterm's `TTY_BUFFER_SIZE`), from fd 0. The read cannot
   block, because the bytes are there. Feed them to `advance(bytes, read == 1024)`, which is
   crossterm's rule for whether more input follows, so a lone ESC at the end of a short read is
   the Esc key just as today.
   - A read of 0 bytes, or `EIO`: `Closed`. `EINTR`: `Pending` (below). `EAGAIN`: `Empty`. Any
     other error: returned as today's errors are (the thread rests a slice, `input.rs:489-493`).
5. Take decoded items as in step 1. An `Event` is returned. If the read decoded to no event
   (only replies, or a sequence still unfinished), return `Pending`.

`Pending` is a new `Next` variant: the step did work and the terminal may hold more. `step`
treats it as it treats an event, keeping `more` set, so the thread looks at its control and then
calls `next` again (`input.rs:446-473`, `479-495`). Only `Empty` sends the thread back to its
wait. So each `next` call does at most one read and returns at most one event. A stream of bytes
that never decodes to an event (replies, or a bracketed paste without its end) cannot keep the
thread from its control, and no pending byte is left behind a wait that has already spent its
edge: `Empty` comes only once `FIONREAD` says zero and the decoder holds no event. The thread still
looks at its control between any two events. Bytes read before a pause stay in the decoder and come
out after `resume`, as crossterm's buffered events do today ("crossterm may hold events from
before", `input.rs:444`). The thread reads nothing while paused or after a stop, so it never reads
a foreground action's or the shell's input.

`REPLIES`, `next_of` and their doc comments (`input.rs:172-204`) go. They worked around
crossterm's poll saying "no" when it handed itself a reply, and step 5 drops replies directly. The
tests that pin them (`a_key_behind_the_replies_crossterm_keeps_is_read`,
`only_enough_noes_in_a_row_say_crossterm_holds_nothing`,
`a_hang_up_between_the_noes_ends_them_before_crossterm_is_called_again`) are replaced by tests of
`next` (§4.2).

`Events`, `Ready`, `Wait`, `read`, `Keys`, `message` and the `Script` test double keep their
shapes; `Next` gains `Pending`, and `step` handles it as above. The doc comments that say
"crossterm reads" are updated to say the thread reads and decodes.

### 3.3 Nothing calls crossterm's event reader

crossterm creates its reader lazily, on the first call that needs it (`event.rs:149-160`). After
this change nothing in tagteam makes that call. A workspace `clippy.toml` `disallowed-methods`
list keeps it so, each entry with a reason pointing at this spec:

- `crossterm::event::poll`, `crossterm::event::read`;
- `crossterm::cursor::position`;
- `crossterm::terminal::supports_keyboard_enhancement`;
- `ratatui::Terminal::clear` and `ratatui::Terminal::get_cursor_position`, which ask crossterm for
  the cursor position. `term::repaint` (`term.rs:167-176`) already avoids `clear` for this reason.

The builder confirms each entry fires (a temporary call fails `cargo clippy`) and lists any path
clippy cannot resolve, with the nearest path that works. Test code is not exempt.

## 4. Tests

### 4.1 The regression test (fails first)

A pty test in `crates/tagteam/tests/tui_pty.rs`, `a_key_typed_with_a_resize_is_read`. In each of
40 rounds, resize the pty to a new size (cycling through sizes of at least 80×24) and type a key
at once, without waiting for the redraw. Each key must take visible effect within 2 s before the
next round starts. The keys alternate `k` and `j`, moving the selection between the fixture's two
accounts (`SELECTED  1 ` and `SELECTED  2 `), or anything else with a visible effect that needs
no data. The test then ends with `q` and exit code 0.

A 20-round spike of this test on `0d046eeb` failed 8 of 10 runs on Linux, each time on a key that
never arrived. Before the fix, the builder shows the 40-round test failing on Linux (Docker,
`rust:1.88`, as a non-root user with a passwd entry and HOME outside /tmp) in at least 9 of 10
runs, and records the counts. If it does not fail that often, the builder raises the rounds and
reports what it took. After the fix it must pass 100 of 100 runs there, and
`a_resize_while_idle_redraws_at_the_new_size` must pass 100 of 100 too.

### 4.2 The reader

Unit tests in `input.rs` through an in-process pty (`openpty`, with `Terminal` built on its slave
fd rather than fd 0; the builder adds that constructor):

- a SIGWINCH and a key both pending before the first `wait`: the thread sends `Resize` and the key
  (G1);
- a key split across two reads, and an ESC alone at the end of a short read (Esc, as today);
- a DA1 answer, a cursor position report and keyboard flags in one read with a key behind them:
  only the key comes out;
- more than 1024 bytes pending: every key comes out, in order;
- a pause, and a stop, asked while the terminal keeps sending bytes that decode to no event (DA1
  answers written in a loop): acknowledged within `ACK`, and keys sent after the stream are read
  after `resume`;
- a hang-up (the master closed): `Closed`, and no read blocks;
- nothing pending: `Empty` without blocking;
- fd 0's file status flags are the same before and after (`O_NONBLOCK` never set).

### 4.3 The decoder

crossterm's vendored tests pass unchanged (§3.1). A test feeds a fixed set of byte strings and
checks the events, so the vendored decoder can be compared against a later crossterm. The set:
every key the `KEYS` table binds (`keys.rs`; redesign §10), arrows and Page keys in both CSI and
SS3 forms, Alt+`]`, a bracketed paste, an OSC 11 reply and a DA1 answer.

### 4.4 What must still pass

- The whole workspace suite and the three clippy runs (CI's `check` job) on macOS and Linux.
- The reply-filter, detection and pause tests in `input.rs`, `replies.rs` and `detect.rs`,
  unchanged.

## 5. Records

- **TUI §4.3** ("The input thread turns crossterm events into `Msg`s"): an amendment note saying
  the thread reads the terminal and decodes it with crossterm's vendored decoder, with a link to
  this spec.
- **Redesign §4.2:** a note on "a replay would need a second key decoder beside crossterm's" and
  on "the input thread starts its first crossterm read only after detection has finished". The
  decoder is now tagteam's (crossterm's, vendored), and the first read is the thread's own. The
  rules themselves do not change.
- **Redesign §14 PERF-R-9:** a note that the thread reads and decodes once it is woken, instead of
  reading crossterm events.
- **CHANGELOG `Unreleased` / Fixed:** a key pressed as the terminal is resized is no longer held
  until the next key.

## 6. Order and risks

- **Branch:** `fix/tui-input-reader`, from main `0d046eeb` (#50 and #52 merged). Merge it before
  #53 and #51 rebase, so their CI runs on a fixed main.
- **#53** moves the toolchain to Rust 1.99.1, whose clippy adds lints. The vendored module may
  need more module-level `allow`s on #53's rebase; that is #53's work, and still no logic edit.
- **Release:** the next release waits for this fix, #53 and #51 (Michael, 2026-10-31).

## 7. Left to the builder

- The decoder module's name and where the reader's `openpty` constructor lives.
- How the regression test picks its keys and sizes, within §4.1.
- Whether `Wait::closed` stays before every read, or a read of 0 bytes or `EIO` alone serves.
  Either way a hung-up terminal must never be read in a loop.
