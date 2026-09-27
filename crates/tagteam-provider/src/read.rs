use std::fmt;

/// Every read of a credential, config, roster or session record (§4.3). `Unreadable` is
/// never collapsed into `Absent` or into an empty value.
#[derive(Debug, Clone)]
pub enum Read<T> {
    Present(T),
    Absent,
    Unreadable(ReadError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    pub what: String,
    pub detail: String,
}

impl ReadError {
    pub fn new(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is unreadable: {}", self.what, self.detail)
    }
}

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
}
