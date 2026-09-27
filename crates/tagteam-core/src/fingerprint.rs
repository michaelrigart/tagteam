use sha2::{Digest, Sha256};

/// The fingerprint of one credential generation (§2 "Generation"):
/// `sha256:<lowercase hex of sha256(secret)>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint(String);

impl Fingerprint {
    pub fn of_secret(secret: &[u8]) -> Self {
        let digest = Sha256::digest(secret);
        Self(format!("sha256:{}", hex::encode(digest.as_slice())))
    }

    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix("sha256:")?;
        let canonical = hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        canonical.then(|| Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 12 hex digits, used in file names (§5).
    pub fn short12(&self) -> &str {
        &self.0[7..19]
    }
}

#[cfg(test)]
mod tests {
    use super::Fingerprint;

    #[test]
    fn fingerprint_is_prefixed_lowercase_sha256_hex() {
        let fp = Fingerprint::of_secret(b"abc");
        assert_eq!(
            fp.as_str(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(fp.short12(), "ba7816bf8f01");
    }

    #[test]
    fn parse_accepts_only_the_canonical_form() {
        let fp = Fingerprint::of_secret(b"rt-1");
        assert_eq!(Fingerprint::parse(fp.as_str()), Some(fp));
        assert_eq!(Fingerprint::parse("sha256:ABC"), None);
        assert_eq!(Fingerprint::parse("sha256:"), None);
        let upper = Fingerprint::of_secret(b"rt-1").as_str().to_uppercase();
        assert_eq!(Fingerprint::parse(&upper), None);
        assert_eq!(
            Fingerprint::parse(
                "md5:a33d8c625833429df4658aa6f6940675ca829051a620ed398517039d4a1fc7ec"
            ),
            None
        );
    }

    #[test]
    fn different_secrets_differ() {
        assert_ne!(Fingerprint::of_secret(b"a"), Fingerprint::of_secret(b"b"));
    }
}
