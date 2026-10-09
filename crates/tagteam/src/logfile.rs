//! §14.2's log file: `tagteam.log` in the state directory, private, rotated past 1 MiB to `.1`
//! and `.2`, and shared by every tagteam process. It is opened on the first line that reaches
//! it, and nothing ever waits on it.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use tagteam_provider::FlockGuard;
use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::env::LOG_ROTATIONS;
use tracing_subscriber::fmt::MakeWriter;

/// §14.2: a file over 1 MiB is rotated.
const LIMIT: u64 = 1024 * 1024;

/// Told once why the file was disabled: `--debug`'s notice (§14.2).
pub(crate) type OnDisable = Box<dyn Fn(&Path, &io::Error) + Send + Sync>;

/// One process's log file (Decision 7). Every line is one `write(2)` on an `O_APPEND`
/// descriptor, so the lines of several processes never interleave. The first failure to open,
/// write or rotate it disables it for the rest of the process: logging never fails a command.
pub(crate) struct LogFile {
    dir: PathBuf,
    path: PathBuf,
    lock: PathBuf,
    limit: u64,
    /// The descriptor lines go to: opened on the first one, reopened when the path moves on.
    file: Mutex<Option<File>>,
    disabled: AtomicBool,
    on_disable: Option<OnDisable>,
    /// Every `write(2)` made, so a test can tell that a line was one.
    #[cfg(test)]
    writes: std::sync::atomic::AtomicUsize,
}

impl LogFile {
    /// The log at `path` (`Env::log_file`, §5), rotated past 1 MiB. Nothing is touched before
    /// a line arrives.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self::at(path, LIMIT)
    }

    /// `new` with a rotation limit small enough for a test to cross.
    #[cfg(test)]
    pub(crate) fn with_limit(path: PathBuf, limit: u64) -> Self {
        Self::at(path, limit)
    }

    fn at(path: PathBuf, limit: u64) -> Self {
        Self {
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            lock: suffixed(&path, ".lock"),
            path,
            limit,
            file: Mutex::new(None),
            disabled: AtomicBool::new(false),
            on_disable: None,
            #[cfg(test)]
            writes: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Calls `report` with the path and the cause when the file is disabled.
    pub(crate) fn on_disable(mut self, report: OnDisable) -> Self {
        self.on_disable = Some(report);
        self
    }

    /// The `n`th rotation: the path with `LOG_ROTATIONS[n - 1]` appended (§14.2).
    fn rotated(&self, n: u8) -> PathBuf {
        suffixed(&self.path, LOG_ROTATIONS[usize::from(n) - 1])
    }

    /// Hands one finished line to the file. It never waits on another process and never
    /// reports an error: the first failure disables the file, and `on_disable` hears why.
    fn append(&self, line: &[u8]) {
        if line.is_empty() || self.disabled.load(Ordering::Acquire) {
            return;
        }
        let mut slot = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        // Another thread may have disabled it while this one waited.
        if self.disabled.load(Ordering::Acquire) {
            return;
        }
        if let Err(e) = self.append_to(&mut slot, line) {
            *slot = None;
            self.disabled.store(true, Ordering::Release);
            if let Some(report) = &self.on_disable {
                report(&self.path, &e);
            }
        }
    }

    /// Before the write, the path must still name the descriptor, and a file over the limit is
    /// rotated unless another process is rotating it (§14.2). Then the one write.
    fn append_to(&self, slot: &mut Option<File>, line: &[u8]) -> io::Result<()> {
        let mut file = self.current(slot.take())?;
        if file.metadata()?.len() > self.limit {
            self.rotate()?;
            file = self.current(Some(file))?;
        }
        let written = self.write_once(&file, line);
        *slot = Some(file);
        written
    }

    /// `held` while the path still names it (the same device and inode); otherwise the path,
    /// opened afresh. A path that is gone counts as moved on.
    fn current(&self, held: Option<File>) -> io::Result<File> {
        if let Some(file) = held {
            let named = match fs::metadata(&self.path) {
                Ok(m) => Some((m.dev(), m.ino())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            let ours = file.metadata()?;
            if named == Some((ours.dev(), ours.ino())) {
                return Ok(file);
            }
        }
        self.open()
    }

    /// The path, for appending: its directory created 0700 and the file 0600 when they are
    /// new (§5). An existing file keeps its mode.
    fn open(&self) -> io::Result<File> {
        ensure_private_dir(&self.dir)?;
        OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)
    }

    /// `.1` becomes `.2` and the file `.1`, under a try-only lock, and only if the file is still
    /// over the limit once the lock is held: another process may have rotated it meanwhile. A
    /// lock someone holds means they are rotating, so this process writes on without. Nothing
    /// else is taken while it is held (§4.3).
    fn rotate(&self) -> io::Result<()> {
        let Some(_rotating) = FlockGuard::try_lock(&self.lock)? else {
            return Ok(());
        };
        match fs::metadata(&self.path) {
            Ok(m) if m.len() > self.limit => {}
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
        rename_present(&self.rotated(1), &self.rotated(2))?;
        rename_present(&self.path, &self.rotated(1))
    }

    /// One `write(2)` of the whole line. A write a signal interrupted before it wrote anything
    /// is made again; a short write is a failure, since the rest of the line could no longer
    /// join its start.
    fn write_once(&self, mut file: &File, line: &[u8]) -> io::Result<()> {
        loop {
            #[cfg(test)]
            self.writes.fetch_add(1, Ordering::SeqCst);
            match file.write(line) {
                Ok(n) if n == line.len() => return Ok(()),
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "a line was written only in part",
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

/// Renames `from` to `to`; a `from` that is already gone is no error.
fn rename_present(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `path` with `suffix` appended to its last component.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// One event's line, gathered as the formatter writes it and handed to the file whole when it
/// is dropped (Decision 7).
pub(crate) struct EventWriter<'a> {
    log: &'a LogFile,
    line: Vec<u8>,
}

impl Write for EventWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EventWriter<'_> {
    fn drop(&mut self) {
        self.log.append(&self.line);
    }
}

impl<'a> MakeWriter<'a> for LogFile {
    type Writer = EventWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            log: self,
            line: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};

    use super::*;

    /// The log's name, and its rotation lock's, in the directories the tests make.
    const FILE_NAME: &str = "tagteam.log";
    const LOCK_NAME: &str = "tagteam.log.lock";

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// The file's text; empty when it does not exist.
    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap_or_default()
    }

    /// The directory's entries, by name, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// The kept lines, oldest first: `.2`, then `.1`, then the file.
    fn kept(dir: &Path) -> String {
        ["tagteam.log.2", "tagteam.log.1", FILE_NAME]
            .iter()
            .map(|n| read(&dir.join(n)))
            .collect()
    }

    /// One event's line, handed over as a formatter hands it: in two pieces, then dropped.
    fn log(file: &LogFile, line: &str) {
        let mut w = file.make_writer();
        let (head, tail) = line.split_at(line.len() / 2);
        w.write_all(head.as_bytes()).unwrap();
        w.write_all(tail.as_bytes()).unwrap();
    }

    #[test]
    fn nothing_is_created_before_the_first_line() {
        // §14.2: the file is opened on the first event, so a command that logs nothing
        // creates nothing (§5).
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("state/tagteam");
        let file = LogFile::new(dir.join(FILE_NAME));
        drop(file.make_writer());
        assert!(!d.path().join("state").exists());
        log(&file, "one\n");
        assert_eq!(read(&dir.join(FILE_NAME)), "one\n");
    }

    #[test]
    fn the_file_is_0600_in_a_0700_directory() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("state/tagteam");
        let file = LogFile::with_limit(dir.join(FILE_NAME), 8);
        log(&file, "first line\n");
        log(&file, "second line\n"); // the first is over the limit: it rotates to `.1`
        assert_eq!(mode(&d.path().join("state")), 0o700);
        assert_eq!(mode(&dir), 0o700);
        for name in [FILE_NAME, "tagteam.log.1", LOCK_NAME] {
            assert_eq!(mode(&dir.join(name)), 0o600, "{name}");
        }
    }

    #[test]
    fn each_line_is_one_write_however_it_was_formatted() {
        // Decision 7: the pieces a formatter writes are gathered, and the line reaches the
        // file as one write(2), which O_APPEND keeps whole among other processes' lines.
        let d = tempfile::tempdir().unwrap();
        let file = LogFile::new(d.path().join(FILE_NAME));
        for n in 0..5 {
            log(&file, &format!("line {n}\n"));
        }
        assert_eq!(file.writes.load(Ordering::SeqCst), 5);
        assert_eq!(
            read(&d.path().join(FILE_NAME)),
            "line 0\nline 1\nline 2\nline 3\nline 4\n"
        );
    }

    #[test]
    fn rotation_keeps_the_file_and_two_older_ones() {
        // §14.2: `.1` becomes `.2`, the file becomes `.1`, and the oldest is dropped.
        let d = tempfile::tempdir().unwrap();
        let file = LogFile::with_limit(d.path().join(FILE_NAME), 100);
        // 30 bytes each: four fill a file past 100, and the fifth rotates it.
        let lines: Vec<String> = (0..40)
            .map(|n| format!("line {n:02} {}\n", "x".repeat(21)))
            .collect();
        for line in &lines {
            log(&file, line);
        }
        assert_eq!(
            names(d.path()),
            [FILE_NAME, "tagteam.log.1", "tagteam.log.2", LOCK_NAME]
        );
        assert_eq!(
            kept(d.path()),
            lines[28..].concat(),
            "the last 12, in order"
        );
        for old in ["tagteam.log.1", "tagteam.log.2"] {
            assert_eq!(read(&d.path().join(old)).len(), 120, "{old}");
        }
    }

    #[test]
    fn a_process_that_cannot_take_the_rotation_lock_does_not_rotate() {
        // §14.2: rotation is only tried. Whoever holds the lock is rotating, and nothing waits.
        let d = tempfile::tempdir().unwrap();
        let file = LogFile::with_limit(d.path().join(FILE_NAME), 10);
        log(&file, "over the limit already\n");
        // Its own open file description: to `flock`, another process.
        let held = FlockGuard::try_lock(&d.path().join(LOCK_NAME))
            .unwrap()
            .unwrap();
        log(&file, "second\n");
        assert!(!d.path().join("tagteam.log.1").exists());
        assert_eq!(
            read(&d.path().join(FILE_NAME)),
            "over the limit already\nsecond\n"
        );
        drop(held);
        log(&file, "third\n");
        assert_eq!(
            read(&d.path().join("tagteam.log.1")),
            "over the limit already\nsecond\n"
        );
        assert_eq!(read(&d.path().join(FILE_NAME)), "third\n");
    }

    #[test]
    fn a_file_moved_away_is_noticed_and_the_path_reopened() {
        // §14.2: another process rotated it, or a person moved or deleted it. The inode check
        // reopens the path, so the next line starts a new file.
        let d = tempfile::tempdir().unwrap();
        let file = LogFile::new(d.path().join(FILE_NAME));
        log(&file, "before\n");
        fs::rename(d.path().join(FILE_NAME), d.path().join("tagteam.log.1")).unwrap();
        log(&file, "after\n");
        assert_eq!(read(&d.path().join("tagteam.log.1")), "before\n");
        assert_eq!(read(&d.path().join(FILE_NAME)), "after\n");
        assert_eq!(mode(&d.path().join(FILE_NAME)), 0o600);
        fs::remove_file(d.path().join(FILE_NAME)).unwrap();
        log(&file, "recreated\n");
        assert_eq!(read(&d.path().join(FILE_NAME)), "recreated\n");
    }

    #[test]
    fn a_log_that_cannot_be_opened_is_disabled_once_and_quietly() {
        // §14.2: logging never fails a command. A state directory under a regular file can
        // never be created, by root either.
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("blocker"), "").unwrap();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&reports);
        let file = LogFile::new(d.path().join("blocker/tagteam").join(FILE_NAME)).on_disable(
            Box::new(move |path, _| seen.lock().unwrap().push(path.to_path_buf())),
        );
        log(&file, "one\n");
        log(&file, "two\n");
        assert_eq!(
            *reports.lock().unwrap(),
            [d.path().join("blocker/tagteam/tagteam.log")],
            "reported once, and never tried again"
        );
        assert!(file.disabled.load(Ordering::SeqCst));
        assert_eq!(file.writes.load(Ordering::SeqCst), 0);
    }

    /// Writer `w`'s line `n`, 100 bytes, padded with `w` itself: a line cut short, or spliced
    /// with the other writer's, no longer parses.
    fn numbered(w: char, n: usize) -> String {
        format!("{w} {n:05} {}\n", w.to_string().repeat(91))
    }

    /// The writer and number of a whole `numbered` line; a panic naming any other line.
    fn parse(line: &str) -> (char, usize) {
        let parts: Vec<&str> = line.split(' ').collect();
        let whole = parts.len() == 3
            && parts[0].chars().count() == 1
            && parts[1].len() == 5
            && parts[2] == parts[0].repeat(91);
        assert!(whole, "a line was cut or interleaved: {line:?}");
        (parts[0].chars().next().unwrap(), parts[1].parse().unwrap())
    }

    #[test]
    fn two_processes_logging_across_rotations_keep_every_line_whole_and_in_order() {
        // §15.2 and Review Focus 4. Each `LogFile` has its own descriptor, and tries the
        // rotation lock through its own open file description, as two processes do.
        const LINES: usize = 2000;
        let d = tempfile::tempdir().unwrap();
        let start = Arc::new(Barrier::new(2));
        let writers: Vec<_> = ['a', 'b']
            .into_iter()
            .map(|w| {
                let (dir, start) = (d.path().to_path_buf(), Arc::clone(&start));
                std::thread::spawn(move || {
                    let file = LogFile::with_limit(dir.join(FILE_NAME), 64 * 1024);
                    start.wait();
                    for n in 0..LINES {
                        log(&file, &numbered(w, n));
                    }
                    assert!(!file.disabled.load(Ordering::SeqCst), "{w} was disabled");
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert!(
            d.path().join("tagteam.log.2").exists(),
            "it rotated, more than once"
        );
        let lines: Vec<(char, usize)> = kept(d.path()).lines().map(parse).collect();
        assert!(!lines.is_empty());
        for w in ['a', 'b'] {
            let seen: Vec<usize> = lines
                .iter()
                .filter(|(c, _)| *c == w)
                .map(|(_, n)| *n)
                .collect();
            // A writer whose lines were all rotated out keeps none; any it keeps run without
            // a gap from the first kept to its last.
            if let Some(&first) = seen.first() {
                assert_eq!(seen, (first..LINES).collect::<Vec<_>>(), "{w}");
            }
        }
    }

    #[test]
    fn a_failing_rotation_disables_the_file_once() {
        // §14.2: a file that cannot be rotated is disabled for the rest of the process.
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("tagteam.log.1"), "older\n").unwrap();
        let blocked = d.path().join("tagteam.log.2");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("keep"), "").unwrap();
        let reports = Arc::new(Mutex::new(0));
        let seen = Arc::clone(&reports);
        let file =
            LogFile::with_limit(d.path().join(FILE_NAME), 8).on_disable(Box::new(move |_, _| {
                *seen.lock().unwrap() += 1;
            }));
        log(&file, "first line\n"); // over the limit, but nothing has rotated it yet
        log(&file, "second line\n"); // rotates: `.1` cannot become `.2`
        assert_eq!(*reports.lock().unwrap(), 1);
        assert!(file.disabled.load(Ordering::SeqCst));
        let written = file.writes.load(Ordering::SeqCst);
        let before = read(&d.path().join(FILE_NAME));
        log(&file, "third line\n");
        assert_eq!(*reports.lock().unwrap(), 1, "reported once");
        assert_eq!(file.writes.load(Ordering::SeqCst), written);
        assert_eq!(read(&d.path().join(FILE_NAME)), before);
        assert_eq!(read(&d.path().join("tagteam.log.1")), "older\n");
    }

    #[test]
    fn a_disable_callback_may_log_into_the_same_file_without_deadlocking() {
        // Task 6's `--debug` callback prints through tracing, which reaches this file again.
        // The file is marked disabled before the callback runs, so the re-entrant line returns
        // at once instead of waiting for the mutex the disabling call holds.
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("blocker"), "").unwrap();
        let slot: Arc<std::sync::OnceLock<Arc<LogFile>>> = Arc::default();
        let calls = Arc::new(Mutex::new(0));
        let (cb_slot, cb_calls) = (Arc::clone(&slot), Arc::clone(&calls));
        let file = Arc::new(
            LogFile::new(d.path().join("blocker/tagteam").join(FILE_NAME)).on_disable(Box::new(
                move |_, _| {
                    *cb_calls.lock().unwrap() += 1;
                    if let Some(file) = cb_slot.get() {
                        log(file, "from the callback\n");
                    }
                },
            )),
        );
        slot.set(Arc::clone(&file)).ok().unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let worker = Arc::clone(&file);
        std::thread::spawn(move || {
            log(&worker, "one\n");
            done.send(()).ok();
        });
        finished
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the disable callback deadlocked on the file it disabled");
        assert_eq!(*calls.lock().unwrap(), 1);
        assert!(file.disabled.load(Ordering::SeqCst));
    }
}
