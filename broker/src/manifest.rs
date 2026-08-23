//! Agent manifests, loaded and validated **in the broker**.
//!
//! `docs/threat-model.md` boundary 2: every call is re-checked against the
//! manifest *in the broker*; the gateway is untrusted. So the broker reads the
//! manifest itself, from its own configured directory, and never accepts one
//! over the wire.
//!
//! The digest is SHA-256 over the RFC 8785 canonical bytes of the manifest as
//! JSON. Approvals and ledger records bind that digest (`docs/effects.md` §1),
//! so it must be computed the same way everywhere — canonicalize once, hash the
//! canonical bytes, never re-serialize.

use crate::policy::OperationPolicy;
use crate::safety::MinSafety;
use agentbed_protocol::digest::{CanonicalDigest, Digest};
use agentbed_protocol::wire::EffectClass;
use agentbed_schemas::{manifest_kind, validate, yaml_to_json, SchemaKind};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// A loaded, validated agent manifest.
#[derive(Debug, Clone)]
pub struct AgentManifest {
    name: String,
    digest: Digest,
    capabilities: Capabilities,
}

impl AgentManifest {
    /// Manifest name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Digest over the manifest's canonical bytes.
    #[must_use]
    pub fn digest(&self) -> &Digest {
        &self.digest
    }

    /// The capabilities block.
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// The explicit policy for an operation, if the manifest carries one.
    #[must_use]
    pub fn operation_policy(&self, op: &str) -> Option<&OperationPolicy> {
        self.capabilities.operations.get(op)
    }

    /// Highest class invocable without approval (stage 4).
    #[must_use]
    pub fn max_unapproved_class(&self) -> Option<EffectClass> {
        self.capabilities.risk.max_unapproved_class
    }

    /// Per-resource minimums (stage 2).
    #[must_use]
    pub fn min_safety(&self) -> &MinSafety {
        &self.capabilities.risk.min_safety
    }

    /// Daily call ceiling (stage 5).
    #[must_use]
    pub fn calls_per_day(&self) -> Option<u64> {
        self.capabilities.quotas.calls_per_day
    }
}

/// The subset of the capabilities block the Gate 0 broker evaluates.
///
/// Unknown keys are tolerated *here* because the schema has already rejected
/// the ones it does not know: this struct is a projection of a validated
/// document, not the validation itself.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Capabilities {
    /// Explicit per-operation policy (stage 3).
    #[serde(default)]
    pub operations: HashMap<String, OperationPolicy>,
    /// Quotas (stage 5).
    #[serde(default)]
    pub quotas: Quotas,
    /// Risk settings (stages 2 and 4).
    #[serde(default)]
    pub risk: Risk,
}

/// Quota settings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Quotas {
    /// D/M transactions per day.
    #[serde(default)]
    pub tx_per_day: Option<u64>,
    /// Authorized calls per day, class R included.
    #[serde(default)]
    pub calls_per_day: Option<u64>,
}

/// Risk settings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Risk {
    /// Class ceiling for stage 4.
    #[serde(default)]
    pub max_unapproved_class: Option<EffectClass>,
    /// Per-resource minimums for stage 2.
    #[serde(default)]
    pub min_safety: MinSafety,
}

/// Why a manifest could not be loaded.
#[derive(Debug)]
pub enum ManifestError {
    /// The reference names something outside the manifest directory.
    UnsafeReference,
    /// The file could not be read.
    Unreadable(String),
    /// The document is not valid YAML/JSON, or fails its schema.
    Invalid(String),
    /// The document is well-formed but semantically refused by the broker.
    SemanticallyRefused(&'static str),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::UnsafeReference => {
                f.write_str("manifest reference is not a plain file name")
            }
            ManifestError::Unreadable(e) | ManifestError::Invalid(e) => f.write_str(e),
            ManifestError::SemanticallyRefused(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for ManifestError {}

/// Loads and caches agent manifests from a directory.
#[derive(Debug)]
pub struct ManifestStore {
    dir: std::path::PathBuf,
}

impl ManifestStore {
    /// Point the store at a directory of manifests.
    #[must_use]
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        ManifestStore { dir: dir.into() }
    }

    /// Load the manifest a token entry refers to.
    ///
    /// The reference comes from the broker's own token store, not from the
    /// wire — but it is still constrained to a plain file name. Config files
    /// get edited by scripts, and "the input was trusted" is how directory
    /// traversal keeps being rediscovered.
    pub fn load(&self, manifest_ref: &str) -> Result<AgentManifest, ManifestError> {
        if manifest_ref.is_empty()
            || manifest_ref.contains('/')
            || manifest_ref.contains('\\')
            || manifest_ref.contains("..")
        {
            return Err(ManifestError::UnsafeReference);
        }
        let path = self.dir.join(manifest_ref);
        load_agent_manifest(&path)
    }
}

/// Load, schema-validate, semantically check and digest one agent manifest.
pub fn load_agent_manifest(path: &Path) -> Result<AgentManifest, ManifestError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ManifestError::Unreadable(format!("cannot read manifest: {e}")))?;
    let value = yaml_to_json(&raw).map_err(|e| ManifestError::Invalid(e.to_string()))?;

    let kind = manifest_kind(&value).map_err(|e| ManifestError::Invalid(e.to_string()))?;
    if kind != SchemaKind::AgentManifest {
        return Err(ManifestError::SemanticallyRefused("not an agent manifest"));
    }
    validate(kind, &value).map_err(|e| ManifestError::Invalid(e.to_string()))?;

    let name = value
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or(ManifestError::SemanticallyRefused("manifest has no name"))?
        .to_owned();

    let capabilities: Capabilities = value
        .get("capabilities")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| ManifestError::Invalid(format!("capabilities: {e}")))?
        .unwrap_or_default();

    semantic_checks(&capabilities)?;

    let canonical = CanonicalDigest::of(&value)
        .map_err(|e| ManifestError::Invalid(format!("manifest does not canonicalize: {e}")))?;

    Ok(AgentManifest {
        name,
        digest: canonical.digest().clone(),
        capabilities,
    })
}

/// The checks ADR §6 assigns to the compiler rather than to JSON Schema.
fn semantic_checks(capabilities: &Capabilities) -> Result<(), ManifestError> {
    for policy in capabilities.operations.values() {
        // ADR §5: "operations whose credential-reflection or downstream-fetch
        // behaviour cannot be bounded are rejected at manifest-compile time".
        // An unbounded pre-authorization is the degenerate case of that.
        if policy.is_pre_authorized() && !policy.has_bounds() {
            return Err(ManifestError::SemanticallyRefused(
                "pre_authorized operation without bounds",
            ));
        }
    }
    if capabilities.risk.max_unapproved_class == Some(EffectClass::F) {
        return Err(ManifestError::SemanticallyRefused(
            "max_unapproved_class may never be F",
        ));
    }
    Ok(())
}
