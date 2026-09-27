use std::fmt;

/// `Degraded`: the Keychain lookup failed and the plaintext file covered it, so the bytes may
/// be a superseded generation (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    Fresh,
    Degraded,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    bytes: Vec<u8>,
    provenance: Provenance,
}

impl Credential {
    pub fn fresh(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            provenance: Provenance::Fresh,
        }
    }

    pub fn degraded(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            provenance: Provenance::Degraded,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn provenance(&self) -> Provenance {
        self.provenance
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn into_fresh(self) -> Option<FreshCredential> {
        (self.provenance == Provenance::Fresh).then_some(FreshCredential(self))
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Credential(<{} bytes>, {:?})",
            self.bytes.len(),
            self.provenance
        )
    }
}

/// The only credential the refresh gate accepts. It cannot be built from a degraded read.
#[derive(Debug, Clone)]
pub struct FreshCredential(Credential);

impl FreshCredential {
    pub fn credential(&self) -> &Credential {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_secret_bytes() {
        let c = Credential::fresh(b"sk-ant-ort01-secret".to_vec());
        let shown = format!("{c:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("19 bytes"), "{shown}");
    }

    #[test]
    fn degraded_credentials_cannot_become_fresh() {
        assert!(Credential::degraded(b"x".to_vec()).into_fresh().is_none());
        let fresh = Credential::fresh(b"x".to_vec()).into_fresh().unwrap();
        assert_eq!(fresh.credential().bytes(), b"x");
    }
}
