//! Request/response envelopes.
//!
//! # The shape *is* the security property
//!
//! A request carries exactly four things: protocol version, correlation id,
//! which operation, and the caller's bearer token, plus that operation's
//! parameters. It carries **no** `agent_id`, no manifest digest, no effect set,
//! no canonical bytes or digest, and no gateway authorization verdict. Those
//! are broker outputs, and the broker derives every one of them from its own
//! inputs (`docs/threat-model.md`, boundary 2).
//!
//! This is enforced structurally, not by a check the broker might forget:
//! [`Request`] is `deny_unknown_fields`, so a frame containing `"agent_id"` is
//! refused during deserialization — before any handler, any identity lookup, or
//! any policy evaluation runs. There is no "advisory identity hint" field to
//! get promoted to authoritative by a later refactor.

use crate::digest::Digest;
use crate::dto::system_info::SystemInfo;
use crate::PROTOCOL_VERSION;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Maximum length of a correlation id, in bytes.
pub const MAX_REQUEST_ID_BYTES: usize = 64;

/// Caller-chosen correlation id, echoed back on the response.
///
/// Constrained to printable ASCII without whitespace so it can be embedded in
/// an audit line without escaping games.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestId(String);

impl RequestId {
    /// Validate and wrap.
    pub fn new(raw: impl Into<String>) -> Result<Self, &'static str> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err("request id is empty");
        }
        if raw.len() > MAX_REQUEST_ID_BYTES {
            return Err("request id too long");
        }
        if !raw.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("request id contains non-graphic ASCII");
        }
        Ok(RequestId(raw))
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let raw = String::deserialize(deserializer)?;
        RequestId::new(raw).map_err(D::Error::custom)
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A bearer token presented by the caller.
///
/// `Debug` is redacted: tokens must not reach logs, panic messages, or error
/// strings. The gateway merely relays this value; only the broker can turn it
/// into an identity.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(String);

impl Token {
    /// Wrap a raw token string.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Token(raw.into())
    }

    /// Expose the secret for verification. Callers must not log the result.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(redacted)")
    }
}

/// Authentication material. One variant, deliberately: an authenticated caller
/// presents a token, full stop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    /// The per-agent bearer token.
    pub token: Token,
}

/// The closed set of operations. Gate 0 exposes exactly one.
///
/// Adding an operation is a code change here plus a match arm in the broker's
/// dispatcher — there is no registry, no string lookup, and no dynamic
/// dispatch, so an unknown `op` fails during deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpName {
    /// `system.info` — read-only host facts, adapter state, safety vector.
    #[serde(rename = "system.info")]
    SystemInfo,
}

impl OpName {
    /// The wire name, for audit lines.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            OpName::SystemInfo => "system.info",
        }
    }
}

/// Parameters of `system.info`. No arguments at Gate 0; the empty-but-strict
/// struct means `{"params":{"anything":1}}` is refused rather than ignored.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemInfoParams {}

/// A broker request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Protocol version; must equal [`PROTOCOL_VERSION`].
    pub v: u8,
    /// Correlation id, echoed on the response.
    pub id: RequestId,
    /// Which operation.
    pub op: OpName,
    /// Version of that operation's own contract (`docs/protocol.md` §2).
    ///
    /// Absent means 1. Within protocol v1 every operation is at version 1, so
    /// requiring the field would add one with a single legal value; having it
    /// at all means a future operation revision is *refusable* rather than
    /// silently reinterpreted, and it is part of the digest input.
    #[serde(default = "default_op_version")]
    pub op_version: u32,
    /// Caller credential.
    pub auth: Auth,
    /// Operation parameters, typed by the broker per `op`.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Operation version assumed when the field is absent.
fn default_op_version() -> u32 {
    1
}

impl Request {
    /// Check the protocol version. Kept separate from deserialization so the
    /// broker can answer a version mismatch with a structured error instead of
    /// a parse failure.
    #[must_use]
    pub fn version_supported(&self) -> bool {
        self.v == PROTOCOL_VERSION
    }
}

/// Machine-readable failure code. Prose belongs in the broker's log, not on the
/// wire: error text returned to a caller is an information-disclosure channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Malformed frame, bad version, unknown field, unknown operation.
    InvalidRequest,
    /// No valid agent identity could be derived from the presented credential.
    Unauthenticated,
    /// Identity established, but policy refused the call.
    Denied,
    /// Refused by the quota veto (`docs/effects.md` §1 stage 5).
    QuotaExhausted,
    /// The call requires an approval that does not exist. Approvals land at
    /// Gate 2; at Gate 0 this is a terminal refusal, never a prompt.
    ApprovalRequired,
    /// The operation exists but the requested `op_version` is not one this
    /// broker implements. Refused rather than reinterpreted as a version it
    /// does know (`docs/protocol.md` §2).
    UnsupportedOperation,
    /// Broker-side failure. Never carries detail.
    Internal,
}

/// Which stage of the `docs/effects.md` §1 precedence ladder decided.
///
/// Reported so a refusal is auditable and testable as *the stage the design
/// says should fire*, not merely "denied".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStage {
    /// Stage 1 — class F, or a globally forbidden operation. Terminal.
    ForbiddenClass,
    /// Stage 2 — per-resource safety minimum, including `none`. Terminal.
    SafetyMinimum,
    /// Stage 3 — explicit operation policy (`deny` / `requires_approval` /
    /// `pre_authorized` bounds, including out-of-bounds handling).
    OperationPolicy,
    /// Stage 4 — class ceiling, only for operations with no explicit policy.
    ClassCeiling,
    /// Stage 5 — quota, a mandatory final veto over every prior outcome.
    Quota,
}

impl DecisionStage {
    /// The stage's ordinal in `docs/effects.md` §1 (1–5).
    #[must_use]
    pub fn ordinal(self) -> u8 {
        match self {
            DecisionStage::ForbiddenClass => 1,
            DecisionStage::SafetyMinimum => 2,
            DecisionStage::OperationPolicy => 3,
            DecisionStage::ClassCeiling => 4,
            DecisionStage::Quota => 5,
        }
    }
}

/// Effect classes (`docs/effects.md` §1) as wire vocabulary.
///
/// Ordering is deliberately **not** implemented here: "which class outranks
/// which" is a policy fact and lives in the broker's policy module, together
/// with the code that acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectClass {
    /// Read; no mutation.
    R,
    /// Declarative host change.
    D,
    /// Data mutation.
    M,
    /// External effect; no rollback.
    E,
    /// Forbidden; refused, never authorized.
    F,
}

/// A structured refusal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseError {
    /// What kind of failure.
    pub code: ErrorCode,
    /// Which precedence stage decided, when a policy stage did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<DecisionStage>,
}

impl ResponseError {
    /// A refusal with no policy stage attached (parse/auth failures).
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        ResponseError { code, stage: None }
    }

    /// A refusal attributed to a precedence stage.
    #[must_use]
    pub fn at_stage(code: ErrorCode, stage: DecisionStage) -> Self {
        ResponseError {
            code,
            stage: Some(stage),
        }
    }
}

/// What the broker computed for a call, echoed so the caller can correlate with
/// the ledger. Every field here is broker-derived; none may be supplied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallBinding {
    /// Identity the broker resolved from the presented token.
    pub agent_id: String,
    /// Digest of the manifest the broker evaluated against.
    pub manifest_digest: Digest,
    /// The exact computed effect set (`docs/effects.md` §1).
    pub effect_set: Vec<EffectClass>,
    /// SHA-256 over the RFC 8785 canonical bytes of the validated operation.
    pub operation_digest: Digest,
}

/// Operation results, tagged by operation name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "result", deny_unknown_fields)]
pub enum OperationResult {
    /// Result of `system.info`.
    #[serde(rename = "system.info")]
    SystemInfo(Box<SystemInfo>),
}

/// A broker response: exactly one of `result` or `error` is present.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    /// Protocol version.
    pub v: u8,
    /// Correlation id; absent when the request was too malformed to yield one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// Present on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<OperationResult>,
    /// Present on success: what the broker bound this call to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<CallBinding>,
    /// Present on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl Response {
    /// A successful response with its binding.
    #[must_use]
    pub fn ok(id: RequestId, result: OperationResult, binding: CallBinding) -> Self {
        Response {
            v: PROTOCOL_VERSION,
            id: Some(id),
            result: Some(result),
            binding: Some(binding),
            error: None,
        }
    }

    /// A refusal, correlated where possible.
    #[must_use]
    pub fn failed(id: Option<RequestId>, error: ResponseError) -> Self {
        Response {
            v: PROTOCOL_VERSION,
            id,
            result: None,
            binding: None,
            error: Some(error),
        }
    }

    /// Whether this response carries a result.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.result.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strict;

    fn parse_request(raw: &str) -> Result<Request, String> {
        let value = strict::parse(raw.as_bytes()).map_err(|e| e.to_string())?;
        serde_json::from_value::<Request>(value).map_err(|e| e.to_string())
    }

    const VALID: &str = r#"{"v":1,"id":"01J","op":"system.info","auth":{"token":"t"},"params":{}}"#;

    #[test]
    fn operation_version_defaults_to_one_and_is_typed() {
        let req = parse_request(VALID).unwrap();
        assert_eq!(req.op_version, 1, "an absent op_version means version 1");

        let explicit = parse_request(
            r#"{"v":1,"id":"01J","op":"system.info","op_version":1,"auth":{"token":"t"},"params":{}}"#,
        )
        .unwrap();
        assert_eq!(explicit.op_version, 1);

        // A version the broker will refuse still has to *parse*, so the refusal
        // is a policy answer with a proper error code rather than a parse error.
        let future = parse_request(
            r#"{"v":1,"id":"01J","op":"system.info","op_version":2,"auth":{"token":"t"},"params":{}}"#,
        )
        .unwrap();
        assert_eq!(future.op_version, 2);

        for bad in ["\"1\"", "null", "-1", "1.5"] {
            let raw = format!(
                r#"{{"v":1,"id":"01J","op":"system.info","op_version":{bad},"auth":{{"token":"t"}},"params":{{}}}}"#
            );
            assert!(
                parse_request(&raw).is_err(),
                "op_version {bad} must not parse"
            );
        }
    }

    #[test]
    fn accepts_a_well_formed_request() {
        let req = parse_request(VALID).unwrap();
        assert!(req.version_supported());
        assert_eq!(req.op, OpName::SystemInfo);
    }

    #[test]
    fn rejects_caller_asserted_identity_and_verdicts() {
        // Each of these is a broker output. None is representable as an input.
        for injected in [
            r#""agent_id":"other-agent""#,
            r#""manifest_digest":"sha256:00""#,
            r#""effect_set":["R"]"#,
            r#""operation_digest":"sha256:00""#,
            r#""authorized":true"#,
            r#""binding":{"agent_id":"x"}"#,
        ] {
            let raw = format!(
                r#"{{"v":1,"id":"01J","op":"system.info","auth":{{"token":"t"}},{injected}}}"#
            );
            let err = parse_request(&raw).unwrap_err();
            assert!(
                err.contains("unknown field"),
                "{injected} was not rejected: {err}"
            );
        }
    }

    #[test]
    fn rejects_unknown_operations_and_nested_unknown_fields() {
        let unknown_op =
            r#"{"v":1,"id":"01J","op":"system.reboot","auth":{"token":"t"},"params":{}}"#;
        assert!(parse_request(unknown_op).is_err());

        let bad_auth =
            r#"{"v":1,"id":"01J","op":"system.info","auth":{"token":"t","uid":0},"params":{}}"#;
        assert!(parse_request(bad_auth).is_err());
    }

    #[test]
    fn token_debug_is_redacted() {
        let req = parse_request(VALID).unwrap();
        assert_eq!(format!("{:?}", req.auth.token), "Token(redacted)");
        assert!(!format!("{req:?}").contains("\"t\""));
    }

    #[test]
    fn request_id_is_constrained() {
        assert!(RequestId::new("").is_err());
        assert!(RequestId::new("has space").is_err());
        assert!(RequestId::new("a".repeat(MAX_REQUEST_ID_BYTES + 1)).is_err());
        assert!(RequestId::new("01JABCDEF").is_ok());
    }

    #[test]
    fn decision_stage_ordinals_match_the_document() {
        assert_eq!(DecisionStage::ForbiddenClass.ordinal(), 1);
        assert_eq!(DecisionStage::SafetyMinimum.ordinal(), 2);
        assert_eq!(DecisionStage::OperationPolicy.ordinal(), 3);
        assert_eq!(DecisionStage::ClassCeiling.ordinal(), 4);
        assert_eq!(DecisionStage::Quota.ordinal(), 5);
    }
}
