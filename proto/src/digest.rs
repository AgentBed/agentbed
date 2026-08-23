//! The wire *rendering* of a digest: `sha256:<64 lowercase hex>`.
//!
//! # This module cannot compute a digest
//!
//! It parses one, formats one, and refuses a malformed one. It does not
//! canonicalize, does not hash, and has no dependency that could.
//!
//! That is deliberate. *Which* bytes a digest covers — the domain separator,
//! the canonical input object, what is excluded — is a security decision, and
//! `docs/protocol.md` §4 freezes it as one. It belongs with the authority that
//! enforces it, so the construction lives in `broker/src/digest.rs` and
//! `broker/src/jcs.rs`. A shared crate that could *produce* a digest would be a
//! shared crate whose bugs are authorization bugs, and both processes link it.
//!
//! The algorithm travels with the value so a future change is a visible,
//! verifiable migration rather than a silent reinterpretation of stored
//! approvals and ledger records.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
    /// Wrap 32 bytes that a hasher elsewhere produced.
    ///
    /// Named for what it is: this crate is carrying someone else's result, not
    /// computing one.
    #[must_use]
    pub fn from_sha256_bytes(bytes: [u8; 32]) -> Self {
        Digest {
            algorithm: DigestAlgorithm::Sha256,
            bytes,
        }
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
        let (algorithm, hex) = raw
            .split_once(':')
            .ok_or_else(|| D::Error::custom("digest missing algorithm tag"))?;
        if algorithm != DigestAlgorithm::Sha256.label() {
            return Err(D::Error::custom("unsupported digest algorithm"));
        }
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(D::Error::custom("malformed digest"));
        }
        let mut bytes = [0u8; 32];
        for (i, slot) in bytes.iter_mut().enumerate() {
            let start = i
                .checked_mul(2)
                .ok_or_else(|| D::Error::custom("malformed digest"))?;
            let end = start
                .checked_add(2)
                .ok_or_else(|| D::Error::custom("malformed digest"))?;
            let pair = hex
                .get(start..end)
                .ok_or_else(|| D::Error::custom("malformed digest"))?;
            *slot = u8::from_str_radix(pair, 16).map_err(D::Error::custom)?;
        }
        Ok(Digest {
            algorithm: DigestAlgorithm::Sha256,
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA-256 of the empty string, as a constant rather than as something this
    /// crate computed.
    const EMPTY_SHA256: [u8; 32] = [
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9,
        0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52,
        0xb8, 0x55,
    ];

    #[test]
    fn renders_and_parses() {
        let d = Digest::from_sha256_bytes(EMPTY_SHA256);
        assert_eq!(
            d.to_string(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Digest>(&json).unwrap(), d);
    }

    #[test]
    fn rejects_untagged_uppercase_or_malformed_digests() {
        assert!(serde_json::from_str::<Digest>("\"deadbeef\"").is_err());
        assert!(serde_json::from_str::<Digest>("\"md5:abc\"").is_err());
        assert!(serde_json::from_str::<Digest>(
            "\"sha256:E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855\""
        )
        .is_err());
        assert!(serde_json::from_str::<Digest>("\"sha256:00\"").is_err());
    }
}
