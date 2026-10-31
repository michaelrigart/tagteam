# The TUI reads its own keys: implementation plan

**Status:** Approved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** A key typed as the terminal is resized is delivered at once: the TUI's input thread reads
fd 0 itself and decodes it with crossterm 0.29.0's vendored decoder, so crossterm's event reader
(a second poll on the same fd) never exists.

**Architecture:** `tagteam-provider::term` gains two safe calls, `bytes_waiting` (`FIONREAD`) and
`read_some` (one `read(2)`), plus two test-only helpers (a raw pty, an fd's status flags), because
`tagteam-tui` forbids `unsafe`. A new `tagteam-tui` module `decode` holds crossterm's `parse.rs`
verbatim and its `Parser`. `input.rs`'s `Terminal` keeps `Wait` and replaces the crossterm calls
with one bounded step per `next`; `Next` gains `Pending`. A workspace `clippy.toml` bans every
call that would start crossterm's reader.

**Tech Stack:** Rust 1.88 (workspace), crossterm `=0.29.0`, mio, libc, ratatui; pty tests through
`crates/tagteam/tests/common/pty.rs`; Linux runs in Docker (`rust:1.88`).

**Spec:** `docs/superpowers/specs/2026-10-31-tagteam-tui-input-reader-design.md` (cited "S§n").
The redesign spec ("redesign §n") stays the authority for everything S does not change. Line
numbers are as of `6691f9af` and will drift.

## Global Constraints

- No new dependency, no crossterm upgrade (`crossterm = "=0.29.0"`), no change to ratatui's use of
  crossterm for drawing, raw mode, the alternate screen and `terminal::size()` (S§2).
- No change to `detect.rs`, `replies.rs`, the pause/stop/hang-up control, R-8 (resizes from the
  thread's own SIGWINCH, the size from `crossterm::terminal::size()`), or M-6 (S§2).
- `O_NONBLOCK` is never set on fd 0 (S§2 anti-goals). The thread never blocks in a read: it reads
  only what `FIONREAD` says is there, once per `next`.
- `tagteam-tui` keeps `#![forbid(unsafe_code)]`. New `unsafe` goes only in
  `crates/tagteam-provider/src/term.rs`, each block with a `// SAFETY:` comment, as its neighbours.
- The vendored decoder has no logic edit (S§3.1). Allowed edits only: imports, the local
  `InternalEvent`, the `bracketed-paste` `cfg`s removed (tagteam's lock builds crossterm with it),
  the two raw-mode calls, visibility, module-level `#[allow(..)]`, and a pointer to the notice.
- Read size `1024` (crossterm's `TTY_BUFFER_SIZE`); `more` is `read == 1024`.
- Commit messages: one plain imperative sentence, no prefix, no attribution.
- Every commit passes `cargo fmt --all --check`, the three clippy runs in CONTRIBUTING.md with
  `-D warnings`, and the focused tests of what it changed. Task 6 runs the full suite.
- pty and socket tests fail inside the sandbox with `openpty: Operation not permitted` or `EPERM`:
  run those unsandboxed. `docker` must be the first word of its Bash call, unpiped, unredirected,
  run in the background.
- If mise has set `RUSTUP_TOOLCHAIN` to another version, run checks with
  `export PATH=$HOME/.rustup/toolchains/1.88.0-aarch64-apple-darwin/bin:$PATH; unset RUSTUP_TOOLCHAIN`
  and confirm `cargo clippy --version` says `0.1.88`.
- Run notes, Docker snapshots and logs go in `.superpowers/research/tui-input-reader-run/`
  (untracked). Its `scripts/loop.sh` runs one pty test `N` times in Docker (env `TIP`, `N`, `TEST`)
  and prints `TIP=… pass=… fail=…`.

## Review Focus

1. **Keys typed while a foreground action owns the terminal** (paused) are not read during the
   pause and come out, in order, after `resume`. Task 4, `a_pause_and_a_stop_are_acknowledged_while_replies_stream_in`.
2. **A UTF-8 character split across two reads** decodes as one key, not two errors. Task 3,
   `a_character_split_across_two_reads_is_one_key`.
3. **A terminal that hangs up with bytes still queued** ends in `Closed` within a bounded number
   of steps, none blocking, and `Closed` stays. Task 4, `a_hang_up_ends_in_closed_without_blocking`.
4. **A read that a signal interrupts** (`EINTR`) loses no byte and returns `Pending`, so the thread
   comes back without a wait. Not reproducible in a test: the reviewer checks the `Interrupted`
   arm in Task 4 and that `read_some` does not retry inside the provider.
5. **A step that decodes no event** (a reply, half a sequence) never sends the thread to its wait
   while `FIONREAD` may be non-zero. Task 4, `a_pending_step_reads_on_without_waiting`.

## Rulings made while planning (inside S)

- **Page keys have no SS3 form.** S§4.3 asks for "arrows and Page keys in both CSI and SS3
  forms". xterm sends PgUp/PgDn only as CSI (`ESC [5~`, `ESC [6~`); the set takes Home and End in
  both forms, and the Page keys in CSI.
- **One more `disallowed-methods` entry.** `ratatui::backend::Backend::get_cursor_position`
  (crossterm's backend calls `crossterm::cursor::position`), so a call through
  `terminal.backend_mut()` cannot bypass the `Terminal` entries (S G2).
- **`FIONREAD` failing with `EIO`** is a hang-up (`Closed`), as a read's `EIO` is in S§3.2 step 4.
- **The size behind `Terminal::size`** is a constructor argument, so the pty tests can see a
  `Msg::Resize` without a controlling terminal; `Terminal::stdin` passes
  `crossterm::terminal::size` (R-8 unchanged).
- **`Wait::closed` stays before every read** (S§7).

## Order

| Task | Depends on | Batch | Tier |
|---|---|---|---|
| 1 Regression test | — | A | sp-standard |
| 2 Provider calls | — | A | sp-mechanical |
| 3 Vendored decoder | — | B | sp-standard |
| 4 The reader | 2, 3 | C | sp-standard |
| 5 The lint | 4 | D | sp-mechanical |
| 6 Verification | 1, 5 | E | controller |

Task 1 runs its test only in Docker on a snapshot, so it never builds Task 2's edits. Task 3 waits
for Task 2 because its tests build `tagteam-provider`.

---

### Task 1: The regression test fails first

**Files:**
- Modify: `crates/tagteam/tests/tui_pty.rs` (a new test after `a_resize_while_idle_redraws_at_the_new_size`, ~line 315)

**Interfaces:** none (uses `Home`, `Pty::resize`, `Pty::type_keys`, `Pty::wait_for`, `accounts`, `LONG`).

- [ ] **Step 1: Write the test**

```rust
#[test]
fn a_key_typed_with_a_resize_is_read() {
    // A key typed as the terminal is resized must not wait for the next key (input reader
    // spec §1). Each round resizes and types at once, without waiting for the redraw.
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the first account selected", |s| {
        accounts(s) && s.contains("SELECTED  1 ")
    });
    let sizes = [(100, 30), (90, 28), (110, 32), (96, 26)];
    for round in 0..40 {
        let (cols, rows) = sizes[round % sizes.len()];
        let (key, want) = if round % 2 == 0 {
            (b"j", "SELECTED  2 ")
        } else {
            (b"k", "SELECTED  1 ")
        };
        tui.resize(cols, rows);
        tui.type_keys(key);
        tui.wait_for(Duration::from_secs(2), &format!("round {round}: {want}"), |s| {
            s.contains(want)
        });
    }
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
}
```

Round 0 types `j` so that every round needs its key (the first account starts selected). Every
size differs from its neighbours and from the pty's 120×40, so every resize raises SIGWINCH.

- [ ] **Step 2: Snapshot the tree and run it 10 times on Linux**

```bash
R=/Users/michael/Code/tagteam/.superpowers/research/tui-input-reader-run
mkdir -p $R/src-before $R/out-before
git -C /Users/michael/Code/tagteam archive HEAD | tar -x -C $R/src-before
cp /Users/michael/Code/tagteam/crates/tagteam/tests/tui_pty.rs $R/src-before/crates/tagteam/tests/
```

Then, in its own background Bash call:

```bash
docker run --rm -v /Users/michael/Code/tagteam/.superpowers/research/tui-input-reader-run/src-before:/src:ro -v /Users/michael/Code/tagteam/.superpowers/research/tui-input-reader-run/scripts:/scripts:ro -v /Users/michael/Code/tagteam/.superpowers/research/tui-input-reader-run/out-before:/out -v tagteam-pty-target:/target -v tagteam-pty-cargo:/usr/local/cargo/registry -e TIP=before -e N=10 -e TEST=a_key_typed_with_a_resize_is_read rust:1.88 bash /scripts/loop.sh
```

Expected: `fail` ≥ 9 of 10, each failing log naming a round's `SELECTED` wait. If fewer fail,
raise the rounds (80, then 160) and run again; the committed test carries the count that reached
9 of 10. Record the counts, the rounds and the failing rounds in `$R/notes.md`.

- [ ] **Step 3: Commit**

```bash
git add crates/tagteam/tests/tui_pty.rs
git commit -m "Test that a key typed with a resize is read at once"
```

---

### Task 2: Safe reads of a terminal's bytes in the provider

**Files:**
- Modify: `crates/tagteam-provider/src/term.rs` (after `read_byte`, ~line 76; tests in its test module)

**Interfaces:**
- Produces:
  - `pub fn bytes_waiting(fd: RawFd) -> io::Result<usize>` — `ioctl(fd, FIONREAD)`; `Err(last_os_error)` on failure.
  - `pub fn read_some(fd: RawFd, buf: &mut [u8]) -> io::Result<usize>` — exactly one `read(2)` of up to `buf.len()`; `Err(last_os_error)` on failure, `EINTR` included (no retry).
  - `#[cfg(any(test, feature = "test-doubles"))] pub fn open_raw_pty() -> io::Result<(OwnedFd, OwnedFd)>` — `(master, slave)` from `openpty`, both `FD_CLOEXEC`, the slave in raw mode (`cfmakeraw`, `tcsetattr(TCSANOW)`): no line buffering, no echo, no signals.
  - `#[cfg(any(test, feature = "test-doubles"))] pub fn status_flags(fd: RawFd) -> io::Result<i32>` — `fcntl(fd, F_GETFL)`.

- [ ] **Step 1: Write the failing tests** (in `term.rs`'s tests; bytes written to a pty master reach
  the slave asynchronously on Linux, so each test waits with `wait_readable(slave, 1000)` first)

  - `bytes_waiting_counts_what_the_terminal_holds`: write `b"abc"` to the master → `bytes_waiting(slave) == 3`; read them all with `read_some` → `bytes_waiting(slave) == 0`.
  - `read_some_takes_at_most_its_buffer_and_leaves_the_rest`: write `b"abcde"`; `read_some(slave, &mut [0; 3]) == 3` and the bytes are `abc`; `bytes_waiting(slave) == 2`.
  - `the_raw_pty_passes_bytes_unbuffered_and_unechoed`: write `b"q"` (no newline) → `read_some` gives `q`; `wait_readable(master, 100) == Readiness::TimedOut` (nothing echoed); `status_flags(slave) & libc::O_NONBLOCK == 0`.
  - `a_hung_up_pty_reads_as_the_end_or_eio`: drop the master; `read_some(slave, ..)` is `Ok(0)` or an error with `raw_os_error() == Some(libc::EIO)` (macOS and Linux differ), and returns within 50 ms.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-provider term::` (unsandboxed: openpty)
Expected: compile errors, `bytes_waiting`, `read_some`, `open_raw_pty`, `status_flags` not found.

- [ ] **Step 3: Implement the four functions** with `libc::ioctl(fd, libc::FIONREAD, &mut n)` (`n: libc::c_int`), `libc::read`, `libc::openpty` + `libc::cfmakeraw` + `libc::tcsetattr`, `libc::fcntl`. Doc comments say what each is for, as `read_byte`'s does; `read_some`'s says it never retries, so its caller sees `Interrupted`.

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p tagteam-provider term::` (unsandboxed)
Expected: PASS. Then `cargo clippy -p tagteam-provider --all-targets --features test-doubles -- -D warnings`: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-provider/src/term.rs
git commit -m "Add safe calls that count and read a terminal's waiting bytes"
```

---

### Task 3: crossterm 0.29.0's decoder, vendored

**Files:**
- Create: `crates/tagteam-tui/src/decode/mod.rs` (notice, `InternalEvent`, `TTY_BUFFER_SIZE`, `Parser`, the fixed-set tests)
- Create: `crates/tagteam-tui/src/decode/parse.rs` (crossterm's `src/event/sys/unix/parse.rs` with its tests)
- Modify: `crates/tagteam-tui/src/lib.rs` (`mod decode;`, in order)
- Modify: `NOTICE` (a paragraph naming the vendored code)

Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/crossterm-0.29.0/` (`LICENSE`,
`src/event/sys/unix/parse.rs`, `src/event/source/unix/mio.rs:161-229`, `src/event.rs:1470-1487`).

**Interfaces:**
- Produces (all `pub(crate)` in `crate::decode`):
  - `enum InternalEvent { Event(Event), CursorPosition(u16, u16), KeyboardEnhancementFlags(KeyboardEnhancementFlags), PrimaryDeviceAttributes }`, crossterm's derives, no `cfg(unix)`.
  - `const TTY_BUFFER_SIZE: usize = 1024;`
  - `struct Parser` with `Default`, `fn advance(&mut self, buffer: &[u8], more: bool)`, and `impl Iterator<Item = InternalEvent>`, all as crossterm's.

- [ ] **Step 1: Copy the two sources and the tests in, with only S§3.1's edits**

  - `parse.rs`: `crate::event::` → `crossterm::event::` (top and test module); `super::super::super::InternalEvent` → `super::InternalEvent`; delete the four `#[cfg(feature = "bracketed-paste")]` lines (the code under them stays); `crate::terminal::sys::is_raw_mode_enabled()` → `matches!(crossterm::terminal::is_raw_mode_enabled(), Ok(true))` at both sites; a first line `//! Vendored from crossterm 0.29.0: see `super` for the source, the licence and the edits.`
  - `mod.rs`: a module doc naming crossterm 0.29.0 and both source paths, why it is vendored (S§1.3, linked), the edit list above, then `Copyright (c) 2019 Timon` and the MIT permission notice verbatim from crossterm's `LICENSE`. The crate is published on its own, so the notice travels in the source, not only in `NOTICE`.
  - `NOTICE`: a paragraph: `crates/tagteam-tui/src/decode/` vendors crossterm 0.29.0's Unix input decoder (https://github.com/crossterm-rs/crossterm), Copyright (c) 2019 Timon, MIT; the licence text is in that module.

- [ ] **Step 2: Write the fixed-set test** in `mod.rs`: `the_decoder_reads_these_bytes_as_crossterm_0_29_0_does`. A table of `(bytes, Vec<InternalEvent>)`, each fed by one `advance(bytes, false)` to a fresh `Parser`, expected values written by hand from `parse.rs`:
  - every key `keys::KEYS` binds (read the whole table): `1` `2` `3`, Tab `\t`, Shift-Tab `\x1b[Z` (`BackTab` with `SHIFT`), `?`, `q`, Esc `\x1b`, `w`, `j`, `k`, and every further row's keys;
  - ↑ ↓ as `\x1b[A` `\x1b[B` and `\x1bOA` `\x1bOB`; Home End as `\x1b[H` `\x1b[F` and `\x1bOH` `\x1bOF`; PgUp PgDn as `\x1b[5~` `\x1b[6~`;
  - Ctrl-C `\x03` and Ctrl-Z `\x1a` (`Char('c')`/`Char('z')` with `CONTROL`), Enter `\r`;
  - Alt+`]` `\x1b]` (`Char(']')` with `ALT`);
  - a bracketed paste `\x1b[200~hi\x1b[201~` → `Event::Paste("hi")`;
  - an OSC 11 reply `\x1b]11;rgb:2828/2c2c/3434\x1b\\` → `Alt+]`, one plain `Char` per byte of `11;rgb:2828/2c2c/3434`, `Alt+\`;
  - a DA1 answer `\x1b[?62;22c` → `PrimaryDeviceAttributes`; `\x1b[1;1R` → `CursorPosition(0, 0)`; `\x1b[?1u` → `KeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)`.

  And `a_character_split_across_two_reads_is_one_key`: `advance(&[0xC3], false)` yields nothing, then `advance(&[0xA9], false)` yields one `Char('é')`.

- [ ] **Step 3: Run the module's tests**

Run: `cargo test -p tagteam-tui decode::`
Expected: PASS, crossterm's vendored tests included (count them against upstream's `#[test]`s in `parse.rs`: the same number). If a vendored test fails, the edit is wrong: no test is changed to pass.

- [ ] **Step 4: Lints**

Run the three clippy commands. Lints the upstream code trips get one module-level `#![allow(..)]` in `parse.rs` (expect `clippy::io_other_error`); a variant field the reader never reads gets `#[allow(dead_code)]` on `InternalEvent`, not a logic change. `decode` has no caller until Task 4: put `#[allow(dead_code)]` on `mod decode;` in `lib.rs`; Task 4 removes it. Then check the edits:

Run: `diff -u ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/crossterm-0.29.0/src/event/sys/unix/parse.rs crates/tagteam-tui/src/decode/parse.rs`
Expected: only the edits listed in Step 1 and the `allow`s. Paste the diff into `$R/notes.md`.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-tui/src/decode crates/tagteam-tui/src/lib.rs NOTICE
git commit -m "Vendor crossterm 0.29.0's key decoder, with its tests and its licence"
```

---

### Task 4: The input thread reads and decodes the terminal itself

**Files:**
- Modify: `crates/tagteam-tui/src/input.rs` (`Next`, `Events::next` doc, `Wait::on` doc, `REPLIES`/`next_of` removed, `Terminal`, `step`, `read`'s comments, `message` and `Pause::hang_up` docs; tests)
- Modify: `crates/tagteam-tui/src/lib.rs` (drop Task 3's `#[allow(dead_code)]` on `mod decode;`)
- Modify: `Cargo.toml` (comments only: `mio`'s "crossterm waits with the same mio", `signal-hook-mio`'s "crossterm registers its own the same way")
- Modify: `CHANGELOG.md` (`Unreleased` / `Fixed`)
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-tui-design.md` (§4.3 input-thread bullet), `docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md` (§4.2's two bullets, §14 PERF-R-9's row)

**Interfaces:**
- Consumes: Task 2's `bytes_waiting`, `read_some`, `open_raw_pty`, `status_flags`; Task 3's `InternalEvent`, `Parser`, `TTY_BUFFER_SIZE`.
- Produces:
  - `pub(crate) enum Next { Event(Event), Pending, Empty, Closed }` — `Pending`: the step read bytes that decoded to no event; the terminal may hold more.
  - `Terminal::on(fd: RawFd, size: fn() -> io::Result<(u16, u16)>) -> io::Result<Terminal>`; `Terminal::stdin()` is `on(libc::STDIN_FILENO, crossterm::terminal::size)`.
  - `Terminal`'s fields: `wait: Wait`, `fd: RawFd`, `decoder: Parser`, `buffer: Box<[u8; TTY_BUFFER_SIZE]>`, `size: fn() -> io::Result<(u16, u16)>`.

- [ ] **Step 1: Write the failing tests** in `input.rs`'s tests. Each pty test holds `alone()` and builds `Terminal::on(slave.as_raw_fd(), || Ok((100, 30)))` on `open_raw_pty()`; it writes to the master through `std::fs::File::from(master)`, and waits with `tagteam_provider::term::wait_readable(slave, 1000)` where it needs the bytes to have landed (`term` in `input.rs` is the TUI's own module). Each test drops its `Input` before its pty.

  - `a_resize_and_a_key_pending_together_both_arrive` (G1): `raise(SIGWINCH)` after `Terminal::on`, write `q`, wait for it to land, then `Input::spawn_with(tx, terminal, None)`: within 2 s both `Msg::Resize { width: 100, height: 30 }` and `Msg::Key('q')` arrive (either order), then nothing for 200 ms.
  - `a_key_split_across_two_reads_is_one_key`: write `\x1b[`, sleep 100 ms, assert nothing came, write `A`: one `Msg::Key(Up)` within 2 s, nothing else.
  - `an_esc_alone_at_the_end_of_a_short_read_is_esc_at_once`: write `\x1b`: `Msg::Key(Esc)` within 2 s.
  - `replies_in_one_read_are_dropped_and_the_key_behind_them_comes_out`: one write of `\x1b[?62;22c\x1b[1;1R\x1b[?1u` + `q`: only `Msg::Key('q')`, then nothing for 200 ms.
  - `more_than_a_read_s_worth_comes_out_whole_and_in_order`: a writer thread writes 3000 bytes cycling `a..=z` (so more than 1024 wait at once on Linux): 3000 `Msg::Key`s, in that order, within 5 s.
  - `a_pause_and_a_stop_are_acknowledged_while_replies_stream_in`: a writer thread writes DA1 answers while `bytes_waiting(slave) < 512` (so a write never blocks) until told to stop. After 100 ms, `pause()` is `true` and returns within `ACK`; nothing arrives for 200 ms; stop and join the writer, write `q`, assert nothing for 200 ms (the paused thread reads nothing), `resume()`: `Msg::Key('q')` within 2 s and no other message before it. Then, on a fresh pty and stream, `drop(input)` returns within `ACK` (acknowledged), and the writer is stopped and joined.
  - `a_hang_up_ends_in_closed_without_blocking`: on a `Terminal` called directly (no thread), write `abc`, wait for it, drop the master; within 10 `next()` calls one returns `Closed`, each call within 50 ms, and two more calls return `Closed` again.
  - `with_nothing_pending_next_is_empty_at_once`: `next()` on a fresh `Terminal` returns `Empty` within 50 ms.
  - `the_terminal_s_file_status_flags_never_change`: `status_flags(slave)` before; write `jk`, call `next()` until `Empty`; drop the master, call `next()` until `Closed`; the flags after are the same and have no `O_NONBLOCK`.
  - `a_pending_step_reads_on_without_waiting` (`Script`): queue `[Next::Pending, Next::Pending, Next::Event(Key('q'))]` before spawning; receive `q`, sleep 100 ms: `(nexts, waits) == (4, 1)` (three, then `Empty`, then one wait). Treating `Pending` as `Empty` gives three waits.

  Delete `next_behind`, `a_key_behind_the_replies_crossterm_keeps_is_read`,
  `only_enough_noes_in_a_row_say_crossterm_holds_nothing` and
  `a_hang_up_between_the_noes_ends_them_before_crossterm_is_called_again`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p tagteam-tui input::` (unsandboxed)
Expected: compile errors: `Next::Pending`, `Terminal::on` not found.

- [ ] **Step 3: Implement**

  - `Terminal::next`, S§3.2 in order:
    1. `take()`: pop the decoder's items; drop `CursorPosition`, `KeyboardEnhancementFlags`, `PrimaryDeviceAttributes`; return the first `Event` as `Next::Event`.
    2. `self.wait.closed()?` → `Closed`.
    3. `bytes_waiting(fd)`: `Err` with `EIO` → `Closed`; other `Err` → `Err`; `0` → `Empty`.
    4. `read_some(fd, &mut buffer[..])`: `Ok(0)` or `EIO` → `Closed`; `Interrupted` → `Pending`; `WouldBlock` → `Empty`; other → `Err`; `Ok(n)` → `decoder.advance(&buffer[..n], n == TTY_BUFFER_SIZE)`.
    5. `take()` again; no event → `Pending`.
  - `step`: `Ok(Next::Pending) => true`, leaving `more` set.
  - `Events::next`'s doc: one bounded step, at most one read and one event, `Pending`, `Empty` only once nothing waits and the decoder holds no event, `Closed` and no read once hung up.
  - Docs and comments that say crossterm reads (`Wait::on`, `Terminal`, `read`'s "crossterm may hold events from before" and "between two events crossterm holds", `message`, `Pause::hang_up`, `stuck_input`, the `Event::Resize` comment in `key_presses_and_resizes_become_messages_and_nothing_else_does`) now say the thread reads and decodes. Rename `a_resize_is_sent_from_the_outer_signal_even_before_crossterm_has_it` to `a_resize_is_sent_from_the_thread_s_own_signal`. Remove `REPLIES` and `next_of`.

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p tagteam-tui` (unsandboxed)
Expected: PASS, the reply-filter, detection and pause tests among them unchanged. Then the three clippy runs: clean.

- [ ] **Step 5: Records**

  - `CHANGELOG.md`, `Unreleased` / `Fixed`: `- \`tagteam tui\` no longer holds a key pressed as the terminal is resized until the next key.`
  - TUI §4.3, after the input-thread bullet's existing amendment: `*(Amended by the [input reader spec](2026-10-31-tagteam-tui-input-reader-design.md) §3: the thread reads the terminal itself and decodes it with crossterm's vendored decoder.)*`
  - Redesign §4.2: on "starts its first crossterm read only after detection" and on "a second key decoder beside crossterm's", the same form of note: the first read is the thread's own, and the decoder is crossterm's, vendored in tagteam; the rules do not change.
  - Redesign §14 PERF-R-9's row: the note that the thread reads and decodes once woken, instead of reading crossterm events.
  - `Cargo.toml`'s two comments: crossterm no longer waits on the terminal or registers SIGWINCH for tagteam (its `events` feature still builds mio, so nothing new is built).

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-tui/src/input.rs crates/tagteam-tui/src/lib.rs Cargo.toml CHANGELOG.md docs/superpowers/specs/2026-10-14-tagteam-tui-design.md docs/superpowers/specs/2026-10-20-tagteam-tui-redesign-design.md
git commit -m "Read and decode the terminal in the input thread, so a key typed with a resize is not held"
```

---

### Task 5: A lint keeps crossterm's event reader unused

**Files:**
- Create: `clippy.toml` (workspace root)

**Interfaces:** none.

- [ ] **Step 1: Write `clippy.toml`**

```toml
# Each of these starts crossterm's event reader: a second poll on the terminal, which loses a key
# typed with a resize. The TUI reads the terminal itself (docs/superpowers/specs/
# 2026-10-31-tagteam-tui-input-reader-design.md §3.3).
disallowed-methods = [
    { path = "crossterm::event::poll", reason = "starts crossterm's event reader (input reader spec §3.3)" },
    { path = "crossterm::event::read", reason = "…" },
    { path = "crossterm::cursor::position", reason = "…" },
    { path = "crossterm::terminal::supports_keyboard_enhancement", reason = "…" },
    { path = "ratatui::Terminal::clear", reason = "asks crossterm for the cursor position (input reader spec §3.3)" },
    { path = "ratatui::Terminal::get_cursor_position", reason = "…" },
    { path = "ratatui::backend::Backend::get_cursor_position", reason = "…" },
]
```

Every `…` is the full reason of its group.

- [ ] **Step 2: Show each entry fires**

For each entry, add a temporary call in a `#[cfg(test)]` fn in `crates/tagteam-tui/src/term.rs`
and run `cargo clippy -p tagteam-tui --all-targets -- -D warnings`.
Expected: `error: use of a disallowed method` naming it. An entry that does not fire, or that
clippy reports it cannot resolve, takes the path that does (ratatui's `Terminal` is defined in
`ratatui_core::terminal::Terminal`, and `Backend` in `ratatui_core::backend::Backend`). Remove the
temporary calls; record each entry's result in `$R/notes.md`.

- [ ] **Step 3: Run the three clippy commands** on the clean tree.
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add clippy.toml
git commit -m "Forbid the calls that start crossterm's event reader"
```

---

### Task 6: Verification (controller)

- [ ] **Step 1: macOS.** `cargo fmt --all --check`, the three clippy runs, and
  `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`, unsandboxed.
  Expected: all pass; report passed/total.
- [ ] **Step 2: Linux, the two pty tests 100 times each** on a snapshot of the final commit
  (`git archive HEAD` into `$R/src-after`, `TIP=after`, `N=100`), one Docker run per test:
  `TEST=a_key_typed_with_a_resize_is_read` and `TEST=a_resize_while_idle_redraws_at_the_new_size`.
  Expected: `pass=100 fail=0` for each.
- [ ] **Step 3: Linux, the whole suite and the clippy runs** in Docker on the same snapshot, as a
  non-root user with a passwd entry and HOME outside /tmp. Expected: all pass.
- [ ] **Step 4: Notes.** Counts before and after, with the commits, in `$R/notes.md`; they go into
  the PR body.
