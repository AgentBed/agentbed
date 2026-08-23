//! Compiled JSON Schemas.
//!
//! The schema documents are embedded at build time so a running broker never
//! reads them from a path an agent could influence, and so a schema and the
//! binary that enforces it cannot drift apart on disk.
//!
//! **This crate validates shape only.** ADR §6 is explicit that the semantic
//! checks — `services.control` naming units that exist, a skill's capabilities
//! being a subset of the calling agent's, `min_safety` compared against the
//! host's *reported* vector, class-F diffs rejected at propose, an unbounded
//! `pre_authorized` scope refused — are the broker's, and the broker performs
//! them itself. Passing validation here means "well-formed", never "allowed".

use jsonschema::Validator;
use serde_json::Value;
use std::sync::OnceLock;

/// A schema document embedded in the binary.
struct Embedded {
    id: &'static str,
    source: &'static str,
}

const CAPABILITIES: Embedded = Embedded {
    id: "https://agentbed.dev/schemas/capabilities.schema.json",
    source: include_str!("../capabilities.schema.json"),
};
const SAFETY_VECTOR: Embedded = Embedded {
    id: "https://agentbed.dev/schemas/safety-vector.schema.json",
    source: include_str!("../safety-vector.schema.json"),
};

/// Which schema to validate against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaKind {
    /// `kind: agent` manifest.
    AgentManifest,
    /// `kind: skill` manifest.
    SkillManifest,
    /// `kind: plugin` manifest.
    PluginManifest,
    /// `kind: desktop` manifest.
    DesktopManifest,
    /// A single-use approval record.
    Approval,
    /// An audit ledger record.
    LedgerRecord,
    /// Per-resource safety vector.
    SafetyVector,
    /// `system.info` request parameters.
    SystemInfoRequest,
    /// `system.info` result.
    SystemInfoResponse,
}

impl SchemaKind {
    /// Every schema, for exhaustive tests.
    #[must_use]
    pub fn all() -> &'static [SchemaKind] {
        &[
            SchemaKind::AgentManifest,
            SchemaKind::SkillManifest,
            SchemaKind::PluginManifest,
            SchemaKind::DesktopManifest,
            SchemaKind::Approval,
            SchemaKind::LedgerRecord,
            SchemaKind::SafetyVector,
            SchemaKind::SystemInfoRequest,
            SchemaKind::SystemInfoResponse,
        ]
    }

    fn source(self) -> &'static str {
        match self {
            SchemaKind::AgentManifest => include_str!("../manifest.agent.schema.json"),
            SchemaKind::SkillManifest => include_str!("../manifest.skill.schema.json"),
            SchemaKind::PluginManifest => include_str!("../manifest.plugin.schema.json"),
            SchemaKind::DesktopManifest => include_str!("../manifest.desktop.schema.json"),
            SchemaKind::Approval => include_str!("../approval.schema.json"),
            SchemaKind::LedgerRecord => include_str!("../ledger-record.schema.json"),
            SchemaKind::SafetyVector => SAFETY_VECTOR.source,
            SchemaKind::SystemInfoRequest => {
                include_str!("../tool/system.info.request.schema.json")
            }
            SchemaKind::SystemInfoResponse => {
                include_str!("../tool/system.info.response.schema.json")
            }
        }
    }

    fn slot(self) -> &'static OnceLock<Result<Validator, String>> {
        macro_rules! slots {
            ($($variant:ident),+ $(,)?) => {
                match self {
                    $(SchemaKind::$variant => {
                        static SLOT: OnceLock<Result<Validator, String>> = OnceLock::new();
                        &SLOT
                    })+
                }
            };
        }
        slots!(
            AgentManifest,
            SkillManifest,
            PluginManifest,
            DesktopManifest,
            Approval,
            LedgerRecord,
            SafetyVector,
            SystemInfoRequest,
            SystemInfoResponse,
        )
    }
}

/// A validation failure, rendered as a list of schema paths and messages.
#[derive(Debug)]
pub struct ValidationError(Vec<String>);

impl ValidationError {
    /// The individual problems found.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.0
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.join("; "))
    }
}

impl std::error::Error for ValidationError {}

fn validator(kind: SchemaKind) -> Result<&'static Validator, ValidationError> {
    let compiled = kind.slot().get_or_init(|| {
        let schema: Value = serde_json::from_str(kind.source())
            .map_err(|e| format!("schema is not valid JSON: {e}"))?;
        jsonschema::options()
            .with_retriever(EmbeddedRetriever)
            .build(&schema)
            .map_err(|e| format!("schema does not compile: {e}"))
    });
    match compiled {
        Ok(v) => Ok(v),
        Err(e) => Err(ValidationError(vec![e.clone()])),
    }
}

/// Resolves the two `$ref`-ed documents from the binary rather than the network.
/// A schema fetch over the wire would put a remote party in the validation path.
struct EmbeddedRetriever;

impl jsonschema::Retrieve for EmbeddedRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let source = match uri.as_str() {
            u if u == CAPABILITIES.id => CAPABILITIES.source,
            u if u == SAFETY_VECTOR.id => SAFETY_VECTOR.source,
            other => return Err(format!("refusing to fetch external schema {other}").into()),
        };
        Ok(serde_json::from_str(source)?)
    }
}

/// Validate a JSON value against a schema.
pub fn validate(kind: SchemaKind, value: &Value) -> Result<(), ValidationError> {
    let validator = validator(kind)?;
    let problems: Vec<String> = validator
        .iter_errors(value)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ValidationError(problems))
    }
}

/// Parse a YAML manifest into JSON.
///
/// Manifests are authored in YAML (ADR §6) but validated and digested as JSON,
/// so the conversion happens once, here, and everything downstream — schema
/// validation, the manifest digest, the ledger — sees the same document.
pub fn yaml_to_json(yaml: &str) -> Result<Value, ValidationError> {
    serde_norway::from_str::<Value>(yaml)
        .map_err(|e| ValidationError(vec![format!("manifest is not valid YAML: {e}")]))
}

/// Read the `kind:` discriminator and map it to its schema.
pub fn manifest_kind(value: &Value) -> Result<SchemaKind, ValidationError> {
    match value.get("kind").and_then(Value::as_str) {
        Some("agent") => Ok(SchemaKind::AgentManifest),
        Some("skill") => Ok(SchemaKind::SkillManifest),
        Some("plugin") => Ok(SchemaKind::PluginManifest),
        Some("desktop") => Ok(SchemaKind::DesktopManifest),
        Some(other) => Err(ValidationError(vec![format!(
            "unknown manifest kind: {other}"
        )])),
        None => Err(ValidationError(vec!["manifest has no kind".to_owned()])),
    }
}
