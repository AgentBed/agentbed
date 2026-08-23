//! Versioned digests over canonical bytes.
//!
//! Every digest carries its algorithm on the wire (`sha256:<hex>`) so a future
//! algorithm change is a visible, verifiable migration rather than a silent
//! reinterpretation of stored ledger and approval records.

use crate::jcs;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};
use std::fmt;

/// Digest algorithms this protocol version understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestAlgorithm {
    /// SHA-256, the only algorithm at protocol version 1.
    Sha256,
}

impl DigestAlgorithm {
    #[must_use]
    fn label(self) -> &'static str {
        match self {
            DigestAlgorithm::Sha256 => "sha256",
        }
    }
}

/// An algorithm-tagged digest, rendered as `sha256:<64 lowercase hex>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    algorithm: DigestAlgorithm,
    bytes: [u8; 32],
}

impl Digest {
    /// SHA-256 of arbitrary bytes.
    #[must_use]
    pub fn sha256(input: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(input);
        let out = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&out);
        Digest { algorithm: DigestAlgorithm::Sha256, bytes }
    }

    /// The algorithm this digest was produced with.
    #[must_use]
    pub fn algorithm(&self) -> DigestAlgorithm {
        self.algorithm
    }

    /// Raw digest bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.algorithm.label())?;
        for byte in self.bytes {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let raw = String::deserialize(deserializer)?;
        let (algorithm, hex) =
            raw.split_once(':').ok_or_else(|| D::Error::custom("digest missing algorithm tag"))?;
        if algorithm != DigestAlgorithm::Sha256.label() {
            return Err(D::Error::custom("unsupported digest algorithm"));
        }
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(D::Error::custom("malformed digest"));
        }
        let mut bytes = [0u8; 32];
        for (i, slot) in bytes.iter_mut().enumerate() {
            let start = i.checked_mul(2).ok_or_else(|| D::Error::custom("malformed digest"))?;
            let end = start.checked_add(2).ok_or_else(|| D::Error::custom("malformed digest"))?;
            let pair = hex.get(start..end).ok_or_else(|| D::Error::custom("malformed digest"))?;
            *slot = u8::from_str_radix(pair, 16).map_err(D::Error::custom)?;
        }
        Ok(Digest { algorithm: DigestAlgorithm::Sha256, bytes })
    }
}

/// Canonical bytes plus the digest over exactly those bytes.
///
/// The two travel together on purpose: `docs/effects.md` §1 requires that every
/// consumer use the *same* bytes, never a re-serialization of the value.
#[derive(Debug, Clone)]
pub struct CanonicalDigest {
    canonical_bytes: Vec<u8>,
    digest: Digest,
}

impl CanonicalDigest {
    /// Canonicalize (RFC 8785) and hash.
    pub fn of(value: &serde_json::Value) -> Result<Self, jcs::JcsError> {
        let canonical_bytes = jcs::canonicalize(value)?;
        let digest = Digest::sha256(&canonical_bytes);
        Ok(CanonicalDigest { canonical_bytes, digest })
    }

    /// The canonical bytes themselves.
    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// The digest over [`Self::canonical_bytes`].
    #[must_use]
    pub fn digest(&self) -> &Digest {
        &self.digest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_renders_and_parses() {
        let d = Digest::sha256(b"");
        assert_eq!(
            d.to_string(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let json = serde_json::to_string(&d).unwrap();
        let back: Digest = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn rejects_untagged_or_malformed_digests() {
        assert!(serde_json::from_str::<Digest>("\"deadbeef\"").is_err());
        assert!(serde_json::from_str::<Digest>("\"md5:abc\"").is_err());
        assert!(serde_json::from_str::<Digest>(
            "\"sha256:E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855\""
        )
        .is_err());
    }

    #[test]
    fn canonical_digest_is_over_canonical_bytes() {
        let value = serde_json::json!({"b": 1, "a": 2});
        let cd = CanonicalDigest::of(&value).unwrap();
        assert_eq!(cd.canonical_bytes(), br#"{"a":2,"b":1}"#);
        assert_eq!(cd.digest(), &Digest::sha256(br#"{"a":2,"b":1}"#));
    }
}
