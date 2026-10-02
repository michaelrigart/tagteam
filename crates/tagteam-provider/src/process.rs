use std::io;

/// A pid plus its start time, so a recycled pid is never mistaken for the original (§12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessStamp {
    pub pid: u32,
    pub start: u64,
}

impl ProcessStamp {
    pub fn current() -> io::Result<Self> {
        let pid = std::process::id();
        let start = start_of(pid)?.ok_or_else(|| io::Error::other("own process not found"))?;
        Ok(Self { pid, start })
    }

    /// Exact pid and start-time match. Anything undeterminable counts as live.
    pub fn is_live(&self) -> bool {
        match start_of(self.pid) {
            Ok(Some(start)) => start == self.start,
            Ok(None) => false,
            Err(_) => true,
        }
    }
}

/// `/proc/<pid>/stat` field 22, counted after the last `)`.
#[cfg(target_os = "linux")]
pub(crate) fn start_of(pid: u32) -> io::Result<Option<u64>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let after = stat
        .rfind(')')
        .map(|i| &stat[i + 1..])
        .ok_or_else(|| io::Error::other("bad stat"))?;
    // `after` starts at field 3; field 22 is the 20th item.
    after
        .split_whitespace()
        .nth(19)
        .and_then(|f| f.parse().ok())
        .map(Some)
        .ok_or_else(|| io::Error::other("bad stat"))
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)` start time, in microseconds.
#[cfg(target_os = "macos")]
pub(crate) fn start_of(pid: u32) -> io::Result<Option<u64>> {
    // SAFETY: `proc_bsdinfo` is a C struct of plain integers; the zeroed value is a valid
    // bit pattern for it and is fully overwritten by `proc_pidinfo` below on success.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a correctly sized, writable proc_bsdinfo.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if n == size {
        return Ok(Some(
            info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        ));
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_live_and_a_changed_start_is_not() {
        let me = ProcessStamp::current().unwrap();
        assert!(me.is_live());
        assert!(
            !ProcessStamp {
                start: me.start + 1,
                ..me
            }
            .is_live()
        );
    }

    #[test]
    fn an_exited_child_is_not_live() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let stamp = ProcessStamp {
            pid: child.id(),
            start: 0,
        };
        child.wait().unwrap();
        assert!(!stamp.is_live());
    }
}
