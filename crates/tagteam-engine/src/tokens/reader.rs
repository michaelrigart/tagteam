//! One transcript file, read incrementally (spec §5.2 steps 2 to 4): whether to read it, where
//! its read starts, its whole lines chunk by chunk, and the checkpoint a batch commits. It owns
//! no thread and touches no store.

use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};
use tagteam_provider::Cancel;

use crate::store::TokenFileRow;

/// The most bytes read at a time (spec §5.2 step 3).
pub(crate) const CHUNK: usize = 1 << 20;
/// The longest line parsed; a longer one is skipped and counted (spec §5.2 step 3, §5.4).
pub(crate) const MAX_LINE: usize = 64 << 20;
/// The most bytes `head_hash` and `tail_hash` cover (spec §4).
const HASH_SPAN: u64 = 4096;

/// A reader's sizes: the spec's, or smaller ones a test can cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    pub chunk: usize,
    pub max_line: usize,
}

impl Limits {
    pub(crate) const SPEC: Limits = Limits {
        chunk: CHUNK,
        max_line: MAX_LINE,
    };
}

/// The first 8 bytes of the SHA-256 of `bytes` (spec §4).
fn hash8(bytes: &[u8]) -> [u8; 8] {
    let mut out = [0; 8];
    out.copy_from_slice(&Sha256::digest(bytes)[..8]);
    out
}

/// Whether `deadline` has passed; `None` never does.
pub(crate) fn past(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}

/// `(dev, ino, size, mtime_ns)`.
fn stamp(m: &Metadata) -> (u64, u64, u64, i64) {
    let mtime_ns = m
        .mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(m.mtime_nsec());
    (m.dev(), m.ino(), m.len(), mtime_ns)
}

/// Spec §5.2 step 2: a file is skipped when its `(dev, ino)`, size and mtime are the ones a read
/// to its last newline stored. A row a partial read left (size −1) matches no file.
pub(crate) fn unchanged(meta: &Metadata, stored: Option<&TokenFileRow>) -> bool {
    let (dev, ino, size, mtime_ns) = stamp(meta);
    stored.is_some_and(|s| {
        s.size >= 0 && s.size as u64 == size && (s.dev, s.ino, s.mtime_ns) == (dev, ino, mtime_ns)
    })
}

/// `len` bytes of `file` at `at`, or `None` when the file ends first.
fn bytes_at(file: &File, at: u64, len: u64) -> io::Result<Option<Vec<u8>>> {
    let mut buf = vec![0; len as usize];
    match file.read_exact_at(&mut buf, at) {
        Ok(()) => Ok(Some(buf)),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e),
    }
}

/// A transcript opened for reading, and what its read starts from.
#[derive(Debug)]
pub(crate) struct OpenFile {
    file: File,
    path: PathBuf,
    /// The row's key: the path as discovered.
    key: String,
    dev: u64,
    ino: u64,
    /// The open file's own `fstat`: what a read to its last newline stores (spec §5.2 step 4).
    size: u64,
    mtime_ns: i64,
    /// Where reading starts: the stored offset when it still holds, else 0.
    start: u64,
    /// The bytes `start` was trusted on: the first `min(4096, start)`, and the up to 4096 ending
    /// at `start`, with their hash.
    start_head_bytes: Vec<u8>,
    start_tail: [u8; 8],
    start_tail_bytes: Vec<u8>,
}

impl OpenFile {
    pub(crate) fn start(&self) -> u64 {
        self.start
    }

    /// The row of a file whose next read starts over: offset 0, size −1 (spec §5.2 step 4).
    fn restart_row(&self) -> TokenFileRow {
        let empty = hash8(&[]);
        TokenFileRow {
            path: self.key.clone(),
            dev: self.dev,
            ino: self.ino,
            size: -1,
            mtime_ns: self.mtime_ns,
            offset: 0,
            head_hash: empty,
            tail_hash: empty,
        }
    }
}

/// Spec §5.2 step 2 for a file that is not skipped: opens it, and its own `fstat` decides where
/// reading starts. At the stored offset when `(dev, ino)` match, the file is at least that long,
/// and the bytes under `head_hash` (the first `min(4096, offset)`) and `tail_hash` (the up to
/// 4096 ending at the offset) are unchanged; at 0 otherwise: a new file, another inode, a
/// shorter file, or a changed prefix.
pub(crate) fn open_at(path: &Path, stored: Option<&TokenFileRow>) -> io::Result<OpenFile> {
    let file = File::open(path)?;
    let (dev, ino, size, mtime_ns) = stamp(&file.metadata()?);
    let empty = hash8(&[]);
    let mut open = OpenFile {
        file,
        path: path.to_path_buf(),
        key: path.to_string_lossy().into_owned(),
        dev,
        ino,
        size,
        mtime_ns,
        start: 0,
        start_head_bytes: Vec::new(),
        start_tail: empty,
        start_tail_bytes: Vec::new(),
    };
    let Some(s) =
        stored.filter(|s| s.offset > 0 && (s.dev, s.ino) == (dev, ino) && size >= s.offset)
    else {
        return Ok(open);
    };
    let span = s.offset.min(HASH_SPAN);
    let Some(head) = bytes_at(&open.file, 0, span)? else {
        return Ok(open);
    };
    let Some(tail) = bytes_at(&open.file, s.offset - span, span)? else {
        return Ok(open);
    };
    if hash8(&head) == s.head_hash && hash8(&tail) == s.tail_hash {
        open.start = s.offset;
        open.start_head_bytes = head;
        open.start_tail = s.tail_hash;
        open.start_tail_bytes = tail;
    }
    Ok(open)
}

/// Why a read stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// Read to its end; a trailing partial line is left for the next refresh (spec §5.2 step 3).
    Eof,
    /// The budget's deadline passed (spec §5.2 Budget).
    Deadline,
    /// The cancel token was set (spec §5.2 step 4).
    Cancelled,
}

/// What one `step` came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// A chunk was read and its whole lines handed on: call again.
    More,
    Stop(Stop),
}

/// Keeps the last `HASH_SPAN` bytes of `tail` followed by `bytes`.
fn push_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
    let span = HASH_SPAN as usize;
    if bytes.len() >= span {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - span..]);
    } else {
        tail.extend_from_slice(bytes);
        if tail.len() > span {
            tail.drain(..tail.len() - span);
        }
    }
}

/// Appends to `head` the first of `bytes` that keep `before` plus its length within `HASH_SPAN`.
fn fill_head(head: &mut Vec<u8>, before: usize, bytes: &[u8]) {
    let room = (HASH_SPAN as usize).saturating_sub(before + head.len());
    head.extend_from_slice(&bytes[..bytes.len().min(room)]);
}

/// Spec §5.2 step 3: an open file's whole lines, from its start, one chunk at a time. It holds
/// at most one chunk and one line being assembled, and never a line over the cap: such a line is
/// scanned to its newline, counted, and dropped.
pub(crate) struct LineReader<'f> {
    open: &'f OpenFile,
    limits: Limits,
    /// Bytes read and not yet consumed: one unfinished line, then what followed it.
    buf: Vec<u8>,
    /// The file offset of `buf[0]`.
    buf_at: u64,
    /// Inside a line over the cap: its bytes so far are dropped, the first ones `head` still
    /// lacks kept in `skip_head`, and the last ones in `skip_tail`.
    skipping: bool,
    skip_head: Vec<u8>,
    skip_tail: Vec<u8>,
    /// Just past the last whole line consumed.
    consumed: u64,
    /// The file's first `min(HASH_SPAN, consumed)` bytes, as read: what the new `head_hash`
    /// covers.
    head: Vec<u8>,
    /// Up to `HASH_SPAN` bytes ending at `consumed`, as read: what the new `tail_hash` covers.
    tail: Vec<u8>,
    long_lines: u64,
    chunks: u64,
    peak: usize,
}

impl<'f> LineReader<'f> {
    pub(crate) fn new(open: &'f OpenFile, limits: Limits) -> Self {
        Self {
            open,
            limits,
            buf: Vec::new(),
            buf_at: open.start,
            skipping: false,
            skip_head: Vec::new(),
            skip_tail: Vec::new(),
            consumed: open.start,
            head: open.start_head_bytes.clone(),
            tail: open.start_tail_bytes.clone(),
            long_lines: 0,
            chunks: 0,
            peak: 0,
        }
    }

    /// Just past the last whole line consumed: where the next read of the file starts.
    pub(crate) fn consumed(&self) -> u64 {
        self.consumed
    }

    /// Lines over the cap, skipped (spec §5.4).
    pub(crate) fn long_lines(&self) -> u64 {
        self.long_lines
    }

    /// The most bytes the reader held at once.
    pub(crate) fn peak(&self) -> usize {
        self.peak
    }

    /// Reads one chunk and hands each whole line now complete to `on_line`, newline stripped.
    /// The cancel token is checked before the chunk and between lines; the deadline after a
    /// chunk, at a line boundary, and between lines. So past the deadline it stops at the next
    /// line boundary, finishing a line still being assembled, and every call that reads a chunk
    /// with a newline in it consumes at least one line (spec §5.2 Budget).
    pub(crate) fn step(
        &mut self,
        deadline: Option<Instant>,
        cancel: &Cancel,
        on_line: &mut dyn FnMut(&[u8]),
    ) -> io::Result<Step> {
        if cancel.requested().is_some() {
            return Ok(Step::Stop(Stop::Cancelled));
        }
        let boundary = self.buf.is_empty() && !self.skipping;
        if self.chunks > 0 && boundary && past(deadline) {
            return Ok(Step::Stop(Stop::Deadline));
        }
        let have = self.buf.len();
        if self.read_chunk()? == 0 {
            return Ok(Step::Stop(Stop::Eof));
        }
        self.chunks += 1;
        // `from` is the first byte not yet consumed; bytes before `scan` hold no newline.
        let (mut from, mut scan) = (0, have);
        let mut stop = None;
        while let Some(i) = self.buf[scan..].iter().position(|&b| b == b'\n') {
            let nl = scan + i;
            if self.skipping {
                // The end of a line over the cap, whose earlier bytes were dropped.
                self.skipping = false;
                self.long_lines += 1;
                self.head.append(&mut self.skip_head);
                push_tail(&mut self.tail, &self.skip_tail);
                self.skip_tail.clear();
            } else if nl - from > self.limits.max_line {
                self.long_lines += 1;
            } else {
                on_line(&self.buf[from..nl]);
            }
            from = nl + 1;
            scan = from;
            self.consumed = self.buf_at + from as u64;
            if cancel.requested().is_some() {
                stop = Some(Stop::Cancelled);
                break;
            }
            if past(deadline) {
                stop = Some(Stop::Deadline);
                break;
            }
        }
        fill_head(&mut self.head, 0, &self.buf[..from]);
        push_tail(&mut self.tail, &self.buf[..from]);
        self.buf.drain(..from);
        self.buf_at += from as u64;
        if let Some(stop) = stop {
            return Ok(Step::Stop(stop));
        }
        // What is left is one unfinished line. Past the cap it is dropped as it is read.
        if self.skipping || self.buf.len() > self.limits.max_line {
            self.skipping = true;
            fill_head(&mut self.skip_head, self.head.len(), &self.buf);
            push_tail(&mut self.skip_tail, &self.buf);
            self.buf_at += self.buf.len() as u64;
            self.buf.clear();
            self.buf.shrink_to(self.limits.chunk);
        }
        Ok(Step::More)
    }

    /// Appends up to one chunk to `buf`; 0 at the end of the file.
    fn read_chunk(&mut self) -> io::Result<usize> {
        let have = self.buf.len();
        self.buf.reserve_exact(self.limits.chunk);
        self.buf.resize(have + self.limits.chunk, 0);
        self.peak = self.peak.max(self.buf.len());
        let at = self.buf_at + have as u64;
        let read = loop {
            match self.open.file.read_at(&mut self.buf[have..], at) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                other => break other,
            }
        };
        let n = match read {
            Ok(n) => n,
            Err(e) => {
                self.buf.truncate(have);
                return Err(e);
            }
        };
        self.buf.truncate(have + n);
        Ok(n)
    }

    /// The row a batch commits for this file, just past the last whole line consumed (spec §5.2
    /// step 4). `Valid` while the path still names the open file, the file is at least that
    /// long, and the bytes its start was trusted on and the first and last bytes read are all
    /// unchanged; its hashes are of the bytes read. Its size and mtime are the ones it was opened
    /// with when `at_eof`; otherwise its size is −1, so a file left part-way is never skipped. If
    /// a check fails the file was rewritten during the read: `Rewritten` restarts it from 0.
    pub(crate) fn checkpoint(&self, at_eof: bool) -> io::Result<Checkpoint> {
        let open = self.open;
        if !self.still_holds()? {
            return Ok(Checkpoint::Rewritten(open.restart_row()));
        }
        let size = if at_eof {
            i64::try_from(open.size).unwrap_or(i64::MAX)
        } else {
            -1
        };
        Ok(Checkpoint::Valid(TokenFileRow {
            path: open.key.clone(),
            dev: open.dev,
            ino: open.ino,
            size,
            mtime_ns: open.mtime_ns,
            offset: self.consumed,
            head_hash: hash8(&self.head),
            tail_hash: hash8(&self.tail),
        }))
    }

    /// Spec §5.2 step 4's checks, and that the first and last bytes read are still there. The
    /// first ones begin with the bytes under the stored `head_hash`.
    fn still_holds(&self) -> io::Result<bool> {
        let open = self.open;
        let same =
            fs::metadata(&open.path).is_ok_and(|m| (m.dev(), m.ino()) == (open.dev, open.ino));
        if !same || open.file.metadata()?.len() < self.consumed {
            return Ok(false);
        }
        if open.start > 0 {
            let span = open.start.min(HASH_SPAN);
            let tail = bytes_at(&open.file, open.start - span, span)?.map(|b| hash8(&b));
            if tail != Some(open.start_tail) {
                return Ok(false);
            }
        }
        let still = |at: u64, bytes: &[u8]| -> io::Result<bool> {
            Ok(bytes_at(&open.file, at, bytes.len() as u64)?.as_deref() == Some(bytes))
        };
        Ok(still(0, &self.head)? && still(self.consumed - self.tail.len() as u64, &self.tail)?)
    }
}

/// What a batch commits for a file (spec §5.2 step 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Checkpoint {
    /// The file still holds what was read: its offset moves just past it.
    Valid(TokenFileRow),
    /// It was rewritten during the read: offset 0 and size −1, so the next refresh reads it
    /// again from the start. The lines already read still commit (spec §5.2 step 4).
    Rewritten(TokenFileRow),
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::Duration;

    use super::*;

    const SMALL: Limits = Limits {
        chunk: 8,
        max_line: 64,
    };

    fn append(path: &Path, bytes: &[u8]) {
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    /// Every line of `open` from its start, why the read stopped, and the reader.
    fn read_lines<'f>(
        open: &'f OpenFile,
        limits: Limits,
        deadline: Option<Instant>,
        cancel: &Cancel,
    ) -> (Vec<String>, Stop, LineReader<'f>) {
        let mut reader = LineReader::new(open, limits);
        let mut lines = Vec::new();
        let stop = loop {
            let step = reader
                .step(deadline, cancel, &mut |line: &[u8]| {
                    lines.push(String::from_utf8(line.to_vec()).unwrap())
                })
                .unwrap();
            if let Step::Stop(stop) = step {
                break stop;
            }
        };
        (lines, stop, reader)
    }

    /// Reads `path` to its end from where `stored` lets it start: its lines, and the row a
    /// batch commits for it.
    fn index(path: &Path, stored: Option<&TokenFileRow>) -> (Vec<String>, TokenFileRow) {
        let open = open_at(path, stored).unwrap();
        let (lines, stop, reader) = read_lines(&open, Limits::SPEC, None, &Cancel::new());
        assert_eq!(stop, Stop::Eof);
        match reader.checkpoint(true).unwrap() {
            Checkpoint::Valid(row) => (lines, row),
            Checkpoint::Rewritten(row) => panic!("rewritten: {row:?}"),
        }
    }

    fn meta(path: &Path) -> Metadata {
        fs::metadata(path).unwrap()
    }

    #[test]
    fn an_unchanged_file_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (lines, row) = index(&path, None);
        assert_eq!(lines, ["one", "two"]);
        assert_eq!((row.offset, row.size), (8, 8));
        assert!(unchanged(&meta(&path), Some(&row)));
        assert!(!unchanged(&meta(&path), None), "a file with no row is read");
    }

    #[test]
    fn a_partial_checkpoint_is_never_skipped() {
        // A read the budget stopped stores size -1 (spec §5.2 step 4): the next refresh reads
        // the file again, from the offset it reached.
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\nthree\n").unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) =
            read_lines(&open, Limits::SPEC, Some(Instant::now()), &Cancel::new());
        assert_eq!((lines, stop), (vec!["one".to_owned()], Stop::Deadline));
        let Checkpoint::Valid(row) = reader.checkpoint(false).unwrap() else {
            panic!("nothing changed");
        };
        assert_eq!((row.offset, row.size), (4, -1));
        assert!(!unchanged(&meta(&path), Some(&row)));
        let (rest, row) = index(&path, Some(&row));
        assert_eq!(rest, ["two", "three"]);
        assert_eq!((row.offset, row.size), (14, 14));
    }

    #[test]
    fn an_append_resumes_at_the_offset() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\n").unwrap();
        let (_, row) = index(&path, None);
        append(&path, b"two\nthree\n");
        assert!(!unchanged(&meta(&path), Some(&row)));
        assert_eq!(open_at(&path, Some(&row)).unwrap().start(), 4);
        let (lines, row) = index(&path, Some(&row));
        assert_eq!(lines, ["two", "three"]);
        assert_eq!(row.offset, 14);
    }

    #[test]
    fn a_shrunk_or_replaced_file_restarts() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (_, row) = index(&path, None);
        // Shorter than the offset, on the same inode.
        fs::write(&path, b"x\n").unwrap();
        assert_eq!(open_at(&path, Some(&row)).unwrap().start(), 0);
        // Another inode in its place, with the same first bytes.
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (_, row) = index(&path, None);
        let other = d.path().join("other");
        fs::write(&other, b"one\ntwo\nthree\n").unwrap();
        fs::rename(&other, &path).unwrap();
        assert_ne!(meta(&path).ino(), row.ino);
        assert_eq!(open_at(&path, Some(&row)).unwrap().start(), 0);
        let (lines, _) = index(&path, Some(&row));
        assert_eq!(lines, ["one", "two", "three"]);
    }

    #[test]
    fn a_rewrite_with_regrowth_on_the_same_inode_restarts() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (_, row) = index(&path, None);
        // Truncated and rewritten in place, past the old offset.
        let mut f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        f.write_all(b"uno\ndos\ntres\n").unwrap();
        drop(f);
        assert_eq!(meta(&path).ino(), row.ino, "the same inode");
        assert_eq!(open_at(&path, Some(&row)).unwrap().start(), 0);
        let (lines, _) = index(&path, Some(&row));
        assert_eq!(lines, ["uno", "dos", "tres"], "read again from the start");
    }

    #[test]
    fn a_rewrite_during_the_read_is_caught_by_the_checkpoint() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        // A resumed read: the bytes its start was trusted on change before the batch commits.
        fs::write(&path, b"one\ntwo\n").unwrap();
        let (_, row) = index(&path, None);
        append(&path, b"three\n");
        let open = open_at(&path, Some(&row)).unwrap();
        assert_eq!(open.start(), 8);
        let (lines, _, reader) = read_lines(&open, Limits::SPEC, None, &Cancel::new());
        assert_eq!(lines, ["three"]);
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all_at(b"ONE", 0)
            .unwrap();
        match reader.checkpoint(true).unwrap() {
            Checkpoint::Rewritten(row) => assert_eq!((row.offset, row.size), (0, -1)),
            other => panic!("{other:?}"),
        }
        // A read from the start: the bytes it read change under it.
        fs::write(&path, b"one\ntwo\n").unwrap();
        let open = open_at(&path, None).unwrap();
        let (_, _, reader) = read_lines(&open, Limits::SPEC, None, &Cancel::new());
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all_at(b"TWO", 4)
            .unwrap();
        assert!(matches!(
            reader.checkpoint(true).unwrap(),
            Checkpoint::Rewritten(_)
        ));
    }

    #[test]
    fn a_head_rewrite_during_a_first_read_is_caught() {
        // A read from 0: the file's first bytes change after they were parsed, and its last ones
        // stay. The head the checkpoint stores is the bytes parsed, so the change is caught; were
        // it hashed from the file, the next refresh would resume past the rewritten response.
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        let line = format!("{}\n", "x".repeat(99));
        fs::write(&path, line.repeat(100)).unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) = read_lines(&open, Limits::SPEC, None, &Cancel::new());
        assert_eq!((lines.len(), stop), (100, Stop::Eof));
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all_at(b"y", 0)
            .unwrap();
        assert!(matches!(
            reader.checkpoint(true).unwrap(),
            Checkpoint::Rewritten(_)
        ));
    }

    #[test]
    fn lines_assemble_across_chunks() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        let text = "a line longer than a chunk\nshort\nanother line that spans chunks\n";
        fs::write(&path, text).unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) = read_lines(&open, SMALL, None, &Cancel::new());
        assert_eq!(
            lines,
            [
                "a line longer than a chunk",
                "short",
                "another line that spans chunks"
            ]
        );
        assert_eq!((stop, reader.consumed()), (Stop::Eof, text.len() as u64));
        assert!(matches!(
            reader.checkpoint(true).unwrap(),
            Checkpoint::Valid(_)
        ));
    }

    #[test]
    fn a_trailing_partial_line_waits() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\npart").unwrap();
        let (lines, row) = index(&path, None);
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(
            (row.offset, row.size),
            (8, 12),
            "read to its last newline, with the size it was opened at"
        );
        assert!(
            unchanged(&meta(&path), Some(&row)),
            "nothing new while the line stays partial"
        );
        append(&path, b"ial\n");
        let (lines, row) = index(&path, Some(&row));
        assert_eq!(lines, ["partial"]);
        assert_eq!(row.offset, 16);
    }

    #[test]
    fn a_huge_line_is_skipped_and_counted_without_holding_the_file() {
        // Review Focus 1: chunks of 16 bytes and a cap of 64 stand in for 1 MiB and 64 MiB.
        let limits = Limits {
            chunk: 16,
            max_line: 64,
        };
        let mut text = b"before\n".to_vec();
        text.extend(vec![b'x'; 1_000]);
        text.push(b'\n');
        // Exactly the cap: parsed.
        text.extend(vec![b'y'; 64]);
        text.push(b'\n');
        // One over it: skipped.
        text.extend(vec![b'z'; 65]);
        text.push(b'\n');
        text.extend(b"after\n");
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, &text).unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) = read_lines(&open, limits, None, &Cancel::new());
        assert_eq!(
            lines,
            ["before".to_owned(), "y".repeat(64), "after".to_owned()]
        );
        assert_eq!((stop, reader.long_lines()), (Stop::Eof, 2));
        assert_eq!(
            reader.consumed(),
            text.len() as u64,
            "a skipped line is consumed"
        );
        assert!(
            reader.peak() <= limits.chunk + limits.max_line,
            "held {} bytes",
            reader.peak()
        );
        // The tail it hashes is the bytes it read, the skipped ones included.
        assert!(matches!(
            reader.checkpoint(true).unwrap(),
            Checkpoint::Valid(row) if row.offset == text.len() as u64
        ));
    }

    #[test]
    fn a_resume_past_the_hash_window_skips_an_over_cap_line_and_keeps_the_rest_in_order() {
        let limits = Limits {
            chunk: 16,
            max_line: 64,
        };
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        let first: String = (0..60)
            .map(|i| format!("{i:04}{}\n", "x".repeat(95)))
            .collect();
        fs::write(&path, &first).unwrap();
        let (lines, row) = index(&path, None);
        assert_eq!((lines.len(), row.offset), (60, 6_000));
        assert!(row.offset > HASH_SPAN, "past the hash window");
        append(
            &path,
            format!("k1\n{}\nk2\nk3\n", "z".repeat(200)).as_bytes(),
        );
        let open = open_at(&path, Some(&row)).unwrap();
        assert_eq!(open.start(), 6_000, "the stored offset still holds");
        let (lines, stop, reader) = read_lines(&open, limits, None, &Cancel::new());
        assert_eq!(
            (lines, stop),
            (vec!["k1".into(), "k2".into(), "k3".into()], Stop::Eof)
        );
        assert_eq!(reader.long_lines(), 1);
        let len = meta(&path).len();
        let Checkpoint::Valid(row) = reader.checkpoint(true).unwrap() else {
            panic!("nothing changed");
        };
        assert_eq!(row.offset, len);
        append(&path, b"k4\n");
        let (lines, row) = index(&path, Some(&row));
        assert_eq!(
            lines,
            ["k4"],
            "each line once: the checkpoint's hashes held"
        );
        assert_eq!(row.offset, len + 3);
    }

    #[test]
    fn the_deadline_stops_between_lines() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\ntwo\nthree\n").unwrap();
        let open = open_at(&path, None).unwrap();
        // Past it from the start: one line, never none.
        let (lines, stop, reader) =
            read_lines(&open, Limits::SPEC, Some(Instant::now()), &Cancel::new());
        assert_eq!((lines, stop), (vec!["one".to_owned()], Stop::Deadline));
        assert_eq!(reader.consumed(), 4);
        // A line still being assembled is finished first.
        let long = "x".repeat(40);
        fs::write(&path, format!("{long}\nnext\n")).unwrap();
        let open = open_at(&path, None).unwrap();
        let (lines, stop, reader) = read_lines(
            &open,
            SMALL,
            Some(Instant::now() - Duration::from_millis(1)),
            &Cancel::new(),
        );
        assert_eq!((lines, stop), (vec![long], Stop::Deadline));
        assert_eq!(reader.consumed(), 41);
    }

    #[test]
    fn a_cancelled_read_stops_before_its_next_chunk() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("s.jsonl");
        fs::write(&path, b"one\n").unwrap();
        let open = open_at(&path, None).unwrap();
        let cancel = Cancel::new();
        cancel.request(2);
        let (lines, stop, reader) = read_lines(&open, Limits::SPEC, None, &cancel);
        assert_eq!((lines, stop), (Vec::<String>::new(), Stop::Cancelled));
        assert_eq!(reader.consumed(), 0);
    }
}
