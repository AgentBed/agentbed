//! The operation digest, built exactly as `docs/protocol.md` §4 and §7 freeze it.
//!
//! ```text
//! SHA-256( domain(protocol_version) || JCS({
//!     "operation":         "<name>",
//!     "operation_version": <u32>,
//!     "arguments":         <schema-projected arguments>
//! }) )
//! ```
//!
//! Domain separators are version-specific so identical operations under v1 and
//! v2 never share a digest or approval/replay binding.

use crate::jcs::{self, JcsError};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::{PROTOCOL_VERSION_V1, PROTOCOL_VERSION_V2};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

/// Domain separator for protocol v1 operation digests. Frozen with protocol v1.
pub const OPERATION_DOMAIN_V1: &[u8] = b"agentbed.operation.v1\0";

/// Domain separator for protocol v2 operation digests. Frozen with protocol v2.
pub const OPERATION_DOMAIN_V2: &[u8] = b"agentbed.operation.v2\0";

/// Domain separator for operation digests at protocol v1.
///
/// Kept as an alias so existing Gate 0 call sites stay readable.
pub const OPERATION_DOMAIN: &[u8] = OPERATION_DOMAIN_V1;

/// Return the frozen domain separator for a supported protocol version.
pub fn operation_domain(protocol_version: u8) -> Result<&'static [u8], &'static str> {
    match protocol_version {
        PROTOCOL_VERSION_V1 => Ok(OPERATION_DOMAIN_V1),
        PROTOCOL_VERSION_V2 => Ok(OPERATION_DOMAIN_V2),
        _ => Err("unsupported protocol version for digest"),
    }
}

/// Canonical bytes plus the digest over exactly those bytes.
#[derive(Debug, Clone)]
pub struct OperationDigest {
    canonical_bytes: Vec<u8>,
    digest: Digest,
}

impl OperationDigest {
    /// Build the digest for a validated, schema-projected operation.
    pub fn of(
        protocol_version: u8,
        operation: &str,
        operation_version: u32,
        arguments: &Value,
    ) -> Result<Self, jcs::JcsError> {
        let domain = operation_domain(protocol_version).map_err(|_| JcsError::Format)?;
        let canonical_input = json!({
            "operation": operation,
            "operation_version": operation_version,
            "arguments": arguments,
        });
        let canonical_bytes = jcs::canonicalize(&canonical_input)?;

        let mut hasher = Sha256::new();
        hasher.update(domain);
        hasher.update(&canonical_bytes);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hasher.finalize());

        Ok(OperationDigest {
            canonical_bytes,
            digest: Digest::from_sha256_bytes(out),
        })
    }
}

impl OperationDigest {
    /// The canonical bytes the digest covers, *excluding* the domain separator.
    ///
    /// These are the bytes an approval UI renders and the ledger stores; the
    /// separator is a hashing input, not part of the operation's description.
    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// The digest.
    #[must_use]
    pub fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// Digest of a manifest document, domain-separated from operations.
pub fn manifest_digest(manifest: &Value) -> Result<Digest, jcs::JcsError> {
    /// Frozen alongside the operation separator.
    const MANIFEST_DOMAIN: &[u8] = b"agentbed.manifest.v1\0";

    let canonical = jcs::canonicalize(manifest)?;
    let mut hasher = Sha256::new();
    hasher.update(MANIFEST_DOMAIN);
    hasher.update(&canonical);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    Ok(Digest::from_sha256_bytes(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_frozen_v1_construction_byte_for_byte() {
        let computed =
            OperationDigest::of(PROTOCOL_VERSION_V1, "system.info", 1, &json!({})).unwrap();

        let expected_canonical =
            br#"{"arguments":{},"operation":"system.info","operation_version":1}"#;
        assert_eq!(
            computed.canonical_bytes(),
            expected_canonical,
            "canonical input must be JCS of {{operation, operation_version, arguments}}"
        );

        let mut hasher = Sha256::new();
        hasher.update(b"agentbed.operation.v1\0");
        hasher.update(expected_canonical);
        let expected: [u8; 32] = hasher.finalize().into();
        assert_eq!(computed.digest(), &Digest::from_sha256_bytes(expected));
    }

    #[test]
    fn the_domain_separator_actually_separates() {
        let computed =
            OperationDigest::of(PROTOCOL_VERSION_V1, "system.info", 1, &json!({})).unwrap();
        let undomained: [u8; 32] = Sha256::digest(computed.canonical_bytes()).into();
        assert_ne!(
            computed.digest(),
            &Digest::from_sha256_bytes(undomained),
            "an operation digest must not collide with a bare hash of its canonical bytes"
        );

        let as_manifest = manifest_digest(&json!({
            "arguments": {}, "operation": "system.info", "operation_version": 1
        }))
        .unwrap();
        assert_ne!(
            computed.digest(),
            &as_manifest,
            "domains must not collide with each other"
        );
    }

    #[test]
    fn operation_version_changes_the_digest() {
        let v1 = OperationDigest::of(PROTOCOL_VERSION_V1, "system.info", 1, &json!({})).unwrap();
        let v2 = OperationDigest::of(PROTOCOL_VERSION_V1, "system.info", 2, &json!({})).unwrap();
        assert_ne!(v1.digest(), v2.digest());
    }

    #[test]
    fn operation_name_changes_the_digest() {
        let info = OperationDigest::of(PROTOCOL_VERSION_V1, "system.info", 1, &json!({})).unwrap();
        let other =
            OperationDigest::of(PROTOCOL_VERSION_V1, "system.reboot", 1, &json!({})).unwrap();
        assert_ne!(info.digest(), other.digest());
    }

    #[test]
    fn arguments_are_canonicalized_not_echoed() {
        let a =
            OperationDigest::of(PROTOCOL_VERSION_V1, "x.y", 1, &json!({"b": 1, "a": 2})).unwrap();
        let b =
            OperationDigest::of(PROTOCOL_VERSION_V1, "x.y", 1, &json!({"a": 2, "b": 1})).unwrap();
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn the_frozen_v1_vector_holds() {
        let computed =
            OperationDigest::of(PROTOCOL_VERSION_V1, "system.info", 1, &json!({})).unwrap();
        assert_eq!(
            computed.digest().to_string(),
            "sha256:b407fa812a98601a6a123e5f5f5005e6ddd45f98d48bec9189de22c3df5bcbf2"
        );
    }

    #[test]
    fn v1_and_v2_domains_never_share_a_digest() {
        let v1 = OperationDigest::of(PROTOCOL_VERSION_V1, "system.info", 1, &json!({})).unwrap();
        let v2 = OperationDigest::of(PROTOCOL_VERSION_V2, "system.info", 1, &json!({})).unwrap();
        assert_ne!(v1.digest(), v2.digest());
        assert_eq!(v1.canonical_bytes(), v2.canonical_bytes());
    }

    #[test]
    fn the_v2_domain_separator_is_the_frozen_string() {
        assert_eq!(OPERATION_DOMAIN_V2, b"agentbed.operation.v2\0");
    }
}
