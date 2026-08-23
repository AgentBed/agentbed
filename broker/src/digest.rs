//! The operation digest, built exactly as `docs/protocol.md` §4 freezes it.
//!
//! ```text
//! SHA-256( "agentbed.operation.v1\0" || JCS({
//!     "operation":         "<name>",
//!     "operation_version": <u32>,
//!     "arguments":         <schema-projected arguments>
//! }) )
//! ```
//!
//! # Why each piece is there
//!
//! - **Domain separator.** An unseparated hash over canonical JSON collides
//!   across *kinds* of object: a manifest, a ledger record and an operation
//!   could hash identically given identical bytes. The trailing NUL means no
//!   separator can be a prefix of another.
//! - **`operation_version`.** A future `system.info` v2 taking the same
//!   arguments must not produce the same digest as v1.
//! - **Excluded:** the request id (a caller-chosen correlation label — two
//!   identical operations must digest identically), the credential (a secret
//!   must never enter a value that is logged, shown in an approval UI, or
//!   anchored off-host), and connection metadata. Nothing asserted by the
//!   gateway can be included, because the envelope cannot carry it.
//!
//! # Order is part of the contract
//!
//! [`OperationDigest::of`] takes arguments that have **already** been strictly
//! decoded, projected into the operation's typed parameters, and validated
//! against the published schema. The digest therefore covers the operation as
//! it will be executed, not the bytes a caller sent. Canonical bytes are
//! retained alongside the digest and are never recomputed by re-serializing.

use crate::jcs;
use agentbed_protocol::digest::Digest;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

/// Domain separator for operation digests. Frozen with protocol v1.
pub const OPERATION_DOMAIN: &[u8] = b"agentbed.operation.v1\0";

/// Canonical bytes plus the digest over exactly those bytes.
#[derive(Debug, Clone)]
pub struct OperationDigest {
    canonical_bytes: Vec<u8>,
    digest: Digest,
}

impl OperationDigest {
    /// Build the digest for a validated, schema-projected operation.
    pub fn of(
        operation: &str,
        operation_version: u32,
        arguments: &Value,
    ) -> Result<Self, jcs::JcsError> {
        let canonical_input = json!({
            "operation": operation,
            "operation_version": operation_version,
            "arguments": arguments,
        });
        let canonical_bytes = jcs::canonicalize(&canonical_input)?;

        let mut hasher = Sha256::new();
        hasher.update(OPERATION_DOMAIN);
        hasher.update(&canonical_bytes);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hasher.finalize());

        Ok(OperationDigest {
            canonical_bytes,
            digest: Digest::from_sha256_bytes(out),
        })
    }

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
    fn matches_the_frozen_construction_byte_for_byte() {
        // Recomputed here from the specification in docs/protocol.md §4 rather
        // than from this module's own helper, so the test fails if the
        // construction drifts from the frozen contract.
        let computed = OperationDigest::of("system.info", 1, &json!({})).unwrap();

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
        // Without it, this digest would equal a plain hash of the same bytes.
        let computed = OperationDigest::of("system.info", 1, &json!({})).unwrap();
        let undomained: [u8; 32] = Sha256::digest(computed.canonical_bytes()).into();
        assert_ne!(
            computed.digest(),
            &Digest::from_sha256_bytes(undomained),
            "an operation digest must not collide with a bare hash of its canonical bytes"
        );

        // And an identical document under another domain digests differently.
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
        let v1 = OperationDigest::of("system.info", 1, &json!({})).unwrap();
        let v2 = OperationDigest::of("system.info", 2, &json!({})).unwrap();
        assert_ne!(v1.digest(), v2.digest());
    }

    #[test]
    fn operation_name_changes_the_digest() {
        let info = OperationDigest::of("system.info", 1, &json!({})).unwrap();
        let other = OperationDigest::of("system.reboot", 1, &json!({})).unwrap();
        assert_ne!(info.digest(), other.digest());
    }

    #[test]
    fn arguments_are_canonicalized_not_echoed() {
        // Same arguments, different authoring order: one digest.
        let a = OperationDigest::of("x.y", 1, &json!({"b": 1, "a": 2})).unwrap();
        let b = OperationDigest::of("x.y", 1, &json!({"a": 2, "b": 1})).unwrap();
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn the_frozen_vector_holds() {
        // A frozen contract means this hex is part of the contract: if it ever
        // changes, previously issued approvals and stored ledger records stop
        // verifying, so a change here must be a protocol-version change.
        //
        // Derived independently of this implementation, so it pins the spec
        // rather than the code:
        //
        //   python3 -c 'import hashlib; print(hashlib.sha256(
        //     b"agentbed.operation.v1\0"
        //     b"{\"arguments\":{},\"operation\":\"system.info\",
        //       \"operation_version\":1}").hexdigest())'
        //   -> b407fa812a98601a6a123e5f5f5005e6ddd45f98d48bec9189de22c3df5bcbf2
        let computed = OperationDigest::of("system.info", 1, &json!({})).unwrap();
        assert_eq!(
            computed.digest().to_string(),
            "sha256:b407fa812a98601a6a123e5f5f5005e6ddd45f98d48bec9189de22c3df5bcbf2"
        );
    }
}
