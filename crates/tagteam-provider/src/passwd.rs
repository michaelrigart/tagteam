use std::ffi::CStr;

/// The effective user's passwd name, through the re-entrant `getpwuid_r`: tagteam's threads may
/// resolve Keychain names at the same time, and `getpwuid` answers into one shared static buffer.
pub fn user_name() -> Option<String> {
    // SAFETY: `sysconf` only reads a configuration value.
    let hint = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut size = usize::try_from(hint)
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(1024);
    loop {
        let mut buf: Vec<libc::c_char> = vec![0; size];
        // SAFETY: `passwd` holds only integers and pointers, for which all-zero is valid; it
        // is an out-parameter that `getpwuid_r` fills.
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer is to memory this frame owns, `buf.len()` is its true length,
        // and `getpwuid_r` writes only within them.
        let rc = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                &mut pw,
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            )
        };
        if rc == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc != 0 || found.is_null() {
            return None;
        }
        // SAFETY: on success `pw_name` points to a NUL-terminated string inside `buf`, which
        // lives until the end of this iteration.
        let name = unsafe { CStr::from_ptr(pw.pw_name) };
        return name.to_str().ok().map(str::to_owned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_name_is_this_user_s_from_any_thread() {
        let _fork = crate::fork_guard();
        let id = std::process::Command::new("id")
            .arg("-un")
            .output()
            .unwrap();
        let expected = String::from_utf8(id.stdout).unwrap().trim().to_owned();
        let names: Vec<Option<String>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8).map(|_| s.spawn(user_name)).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for name in names {
            assert_eq!(name.as_deref(), Some(expected.as_str()));
        }
    }
}
