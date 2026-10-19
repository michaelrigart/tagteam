use std::fmt;

/// Every read of a credential, config, roster or session record (§4.3). `Unreadable` is
/// never collapsed into `Absent` or into an empty value.
#[derive(Clone)]
pub enum Read<T> {
    Present(T),
    Absent,
    Unreadable(ReadError),
}

/// Hand-written so a secret payload (e.g. `Read<Vec<u8>>` from a Keychain or vault read)
/// never reaches `Debug`, regardless of whether `T` implements it.
impl<T> fmt::Debug for Read<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Read::Present(_) => write!(f, "Present(..)"),
            Read::Absent => write!(f, "Absent"),
            Read::Unreadable(e) => write!(f, "Unreadable({e:?})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    pub what: String,
    pub detail: String,
    cause: Cause,
}

/// Why a read failed, where a caller acts on it: a locked keychain is not retried, and an
/// interrupted read is no failure at all (H4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cause {
    Other,
    Locked,
    Interrupted(i32),
}

impl ReadError {
    /// A read that failed for any reason but a lock or an interruption.
    pub fn new(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::with(what, detail, Cause::Other)
    }

    /// A read the locked keychain refused (rc 36).
    pub fn locked(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::with(what, detail, Cause::Locked)
    }

    /// A read a signal cut short, or never started, inside an interruptible scope (H4, §14.1).
    pub fn interrupted(what: impl Into<String>, signal: i32) -> Self {
        Self::with(
            what,
            format!("interrupted by signal {signal}"),
            Cause::Interrupted(signal),
        )
    }

    fn with(what: impl Into<String>, detail: impl Into<String>, cause: Cause) -> Self {
        Self {
            what: what.into(),
            detail: detail.into(),
            cause,
        }
    }

    /// The signal that interrupted the read, if one did.
    pub fn interruption(&self) -> Option<i32> {
        match self.cause {
            Cause::Interrupted(signal) => Some(signal),
            Cause::Other | Cause::Locked => None,
        }
    }

    /// Whether the locked keychain refused the read.
    pub fn is_locked(&self) -> bool {
        self.cause == Cause::Locked
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is unreadable: {}", self.what, self.detail)
    }
}

impl std::error::Error for ReadError {}

impl<T> Read<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Read<U> {
        match self {
            Read::Present(v) => Read::Present(f(v)),
            Read::Absent => Read::Absent,
            Read::Unreadable(e) => Read::Unreadable(e),
        }
    }

    pub fn as_ref(&self) -> Read<&T> {
        match self {
            Read::Present(v) => Read::Present(v),
            Read::Absent => Read::Absent,
            Read::Unreadable(e) => Read::Unreadable(e.clone()),
        }
    }

    pub fn is_present(&self) -> bool {
        matches!(self, Read::Present(_))
    }

    pub fn present(self) -> Option<T> {
        match self {
            Read::Present(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_is_never_absent() {
        let r: Read<u8> = Read::Unreadable(ReadError::new("keychain", "rc 36"));
        assert!(!r.is_present());
        assert!(matches!(r.map(|v| v + 1), Read::Unreadable(e) if e.detail == "rc 36"));
        let a: Read<u8> = Read::Absent;
        assert!(a.present().is_none());
        assert_eq!(Read::Present(2).map(|v| v * 2).present(), Some(4));
    }

    #[test]
    fn debug_never_prints_the_payload() {
        let shown = format!("{:?}", Read::Present(b"sk-ant-secret".to_vec()));
        assert!(!shown.contains("sk-ant"), "{shown}");
        assert!(!shown.contains("115, 107"), "{shown}");
        for byte in b"sk-ant-secret" {
            assert!(!shown.contains(&byte.to_string()), "{shown}");
        }

        let wrapped = format!(
            "{:?}",
            Ok::<Read<Vec<u8>>, ()>(Read::Present(b"sk".to_vec()))
        );
        assert!(wrapped.contains("Present(..)"), "{wrapped}");
    }

    #[test]
    fn an_error_says_whether_it_was_a_lock_or_an_interruption() {
        let plain = ReadError::new("keychain", "rc 1");
        assert_eq!((plain.is_locked(), plain.interruption()), (false, None));
        let locked = ReadError::locked("keychain", "rc 36: the keychain is locked");
        assert_eq!((locked.is_locked(), locked.interruption()), (true, None));
        let cut = ReadError::interrupted("keychain", 15);
        assert_eq!((cut.is_locked(), cut.interruption()), (false, Some(15)));
        assert_eq!(
            cut.to_string(),
            "keychain is unreadable: interrupted by signal 15"
        );
    }
}
