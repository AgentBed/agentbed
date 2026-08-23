//! Request dispatch: parse, authenticate, authorize, then execute.
//!
//! The dispatcher is a `match` over a closed enum, not a table of registered
//! handlers. That is deliberate (ADR §5.0: "fixed, narrow RPC"): with no
//! registry there is no way to reach code by naming it, and an unknown
//! operation fails during deserialization rather than at a lookup miss.
//!
//! # Order of operations, and why it is this order
//!
//! 1. **Parse strictly.** Unknown fields — including any attempt to assert an
//!    identity, an effect set, a digest or a verdict — are refused here, before
//!    anything is authenticated. A forged frame gets no further than the parser.
//! 2. **Resolve identity from the token**, in this process. Nothing else about
//!    the caller is consulted: not the peer credential (it authenticated the
//!    channel), not the frame (it has no field to consult).
//! 3. **Load the manifest here** and compute its digest, because the gateway is
//!    untrusted by the broker (`docs/threat-model.md`, boundary 2).
//! 4. **Project the operation through its schema**, then canonicalize *that*
//!    projection (RFC 8785) and digest it. The bytes the ledger and approvals
//!    bind are produced here, from the validated typed operation — never from
//!    the caller's serialization and never re-serialized later
//!    (`docs/effects.md` §1).
//! 5. **Compute the effect set and evaluate the ladder** (`policy`).
//! 6. Execute only if the ladder allowed, charging the quota first.

use crate::adapter::HostAdapter;
use crate::audit::{AuditRecord, AuditSink};
use crate::identity::{AgentContext, TokenStore};
use crate::manifest::ManifestStore;
use crate::peercred::PeerCredentials;
use crate::policy::{evaluate, Decision};
use crate::quota::QuotaLedger;
use crate::tools::system_info;
use agentbed_protocol::digest::{CanonicalDigest, Digest};
use agentbed_protocol::strict;
use agentbed_protocol::wire::{
    CallBinding, EffectClass, ErrorCode, OpName, OperationResult, Request, RequestId, Response,
    ResponseError, SystemInfoParams,
};
use agentbed_schemas::{validate, SchemaKind};
use serde_json::json;

/// Everything the request path needs, assembled once at startup.
pub struct Dispatcher {
    tokens: TokenStore,
    manifests: ManifestStore,
    adapter: Box<dyn HostAdapter>,
    quotas: QuotaLedger,
}

// Hand-written rather than derived: `Debug` on the dispatcher must never be
// able to reach token material, so it reports a count and nothing else.
impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("enrolled_tokens", &self.tokens.len())
            .finish_non_exhaustive()
    }
}

impl Dispatcher {
    /// Assemble a dispatcher.
    #[must_use]
    pub fn new(
        tokens: TokenStore,
        manifests: ManifestStore,
        adapter: Box<dyn HostAdapter>,
    ) -> Self {
        Dispatcher {
            tokens,
            manifests,
            adapter,
            quotas: QuotaLedger::default(),
        }
    }

    /// Handle one complete frame body. Always yields exactly one response.
    pub fn handle_frame(
        &self,
        body: &[u8],
        peer: PeerCredentials,
        audit: &dyn AuditSink,
    ) -> Response {
        let Some(request) = Self::parse(body, peer, audit) else {
            return Response::failed(None, ResponseError::new(ErrorCode::InvalidRequest));
        };

        let Ok(agent) = self.tokens.resolve(request.auth.token.expose()) else {
            audit.record(AuditRecord {
                request_id: Some(request.id.to_string()),
                op: Some(request.op.as_str()),
                ..AuditRecord::rejected(peer, ErrorCode::Unauthenticated, "token_not_resolved")
            });
            return Response::failed(
                Some(request.id),
                ResponseError::new(ErrorCode::Unauthenticated),
            );
        };

        match request.op {
            OpName::SystemInfo => self.system_info(&request, &agent, peer, audit),
        }
    }

    fn system_info(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        audit: &dyn AuditSink,
    ) -> Response {
        let id = request.id.clone();

        // (4) Project through the typed shape, validate the projection
        // against the published schema, canonicalize it and digest those bytes.
        let operation_digest = match Self::project_and_digest(request) {
            Ok(digest) => digest,
            Err((error, reason)) => {
                return Self::refuse(&id, agent, peer, audit, error, reason, None)
            }
        };

        // (3) Load and digest the manifest here, not upstream.
        // No manifest, no authorization. A missing or unloadable manifest is a
        // refusal, never a default-allow.
        let Ok(manifest) = self.manifests.load(agent.manifest_ref()) else {
            return Self::refuse(
                &id,
                agent,
                peer,
                audit,
                ResponseError::new(ErrorCode::Denied),
                "manifest_unavailable",
                Some(operation_digest),
            );
        };

        // (5) Effect set and the five-stage ladder.
        let call = system_info::describe_call();
        let decision = evaluate(
            &call,
            &manifest,
            &self.adapter.safety_vector(),
            self.quotas.state_for(agent.agent_id()),
        );

        match decision {
            Decision::Refuse {
                code,
                stage,
                reason,
            } => {
                audit.record(AuditRecord {
                    request_id: Some(id.to_string()),
                    agent_id: Some(agent.agent_id().to_owned()),
                    peer,
                    op: Some(system_info::OP),
                    effect_set: call.effect_set.clone(),
                    manifest_digest: Some(manifest.digest().clone()),
                    operation_digest: Some(operation_digest),
                    allowed: false,
                    stage: Some(stage),
                    error: Some(code),
                    reason,
                });
                Response::failed(Some(id), ResponseError::at_stage(code, stage))
            }
            Decision::Allow => {
                // Charge before executing: a call that ran must have been paid
                // for even if the response never reaches the caller.
                self.quotas.charge(agent.agent_id());
                let info = system_info::execute(self.adapter.as_ref());
                let binding = CallBinding {
                    agent_id: agent.agent_id().to_owned(),
                    manifest_digest: manifest.digest().clone(),
                    effect_set: call.effect_set.clone(),
                    operation_digest: operation_digest.clone(),
                };
                audit.record(AuditRecord {
                    request_id: Some(id.to_string()),
                    agent_id: Some(agent.agent_id().to_owned()),
                    peer,
                    op: Some(system_info::OP),
                    effect_set: call.effect_set.clone(),
                    manifest_digest: Some(manifest.digest().clone()),
                    operation_digest: Some(operation_digest),
                    allowed: true,
                    stage: None,
                    error: None,
                    reason: "authorized",
                });
                Response::ok(id, OperationResult::SystemInfo(Box::new(info)), binding)
            }
        }
    }

    /// Project the request's parameters through their typed shape, validate
    /// that projection against the published schema, and digest its RFC 8785
    /// canonical bytes.
    ///
    /// The order matters: the digest is taken over what the broker *validated*,
    /// not over what the caller sent, so the bytes an approval or ledger record
    /// binds describe the operation as it will actually be executed.
    fn project_and_digest(request: &Request) -> Result<Digest, (ResponseError, &'static str)> {
        // Unknown parameters are refused rather than ignored: an argument the
        // broker does not understand could be one that raises the effect set.
        let params =
            serde_json::from_value::<SystemInfoParams>(request.params.clone()).map_err(|_| {
                (
                    ResponseError::new(ErrorCode::InvalidRequest),
                    "params_rejected",
                )
            })?;
        let projected = serde_json::to_value(&params).map_err(|_| {
            (
                ResponseError::new(ErrorCode::Internal),
                "params_not_serializable",
            )
        })?;

        // The gateway validates too; neither result is trusted by the other,
        // and only this one gates execution.
        validate(SchemaKind::SystemInfoRequest, &projected).map_err(|_| {
            (
                ResponseError::new(ErrorCode::InvalidRequest),
                "params_failed_schema",
            )
        })?;

        let canonical_operation = json!({ "op": system_info::OP, "params": projected });
        let canonical = CanonicalDigest::of(&canonical_operation).map_err(|_| {
            (
                ResponseError::new(ErrorCode::Internal),
                "operation_not_canonicalizable",
            )
        })?;
        Ok(canonical.digest().clone())
    }

    /// Record and return a refusal that happened before or instead of the
    /// ladder (bad parameters, unusable manifest, internal failure).
    #[allow(clippy::too_many_arguments)]
    fn refuse(
        id: &RequestId,
        agent: &AgentContext,
        peer: PeerCredentials,
        audit: &dyn AuditSink,
        error: ResponseError,
        reason: &'static str,
        operation_digest: Option<Digest>,
    ) -> Response {
        audit.record(AuditRecord {
            request_id: Some(id.to_string()),
            agent_id: Some(agent.agent_id().to_owned()),
            peer,
            op: Some(system_info::OP),
            effect_set: vec![EffectClass::R],
            manifest_digest: None,
            operation_digest,
            allowed: false,
            stage: error.stage,
            error: Some(error.code),
            reason,
        });
        Response::failed(Some(id.clone()), error)
    }

    /// Strict parse plus version check. Returns `None` when the frame is not a
    /// well-formed request, having already recorded why.
    fn parse(body: &[u8], peer: PeerCredentials, audit: &dyn AuditSink) -> Option<Request> {
        let Ok(value) = strict::parse(body) else {
            audit.record(AuditRecord::rejected(
                peer,
                ErrorCode::InvalidRequest,
                "malformed_json",
            ));
            return None;
        };
        // Covers unknown fields (an asserted agent_id lands here), unknown
        // operations, and malformed envelopes alike.
        let Ok(request) = serde_json::from_value::<Request>(value) else {
            audit.record(AuditRecord::rejected(
                peer,
                ErrorCode::InvalidRequest,
                "envelope_rejected",
            ));
            return None;
        };
        if !request.version_supported() {
            audit.record(AuditRecord::rejected(
                peer,
                ErrorCode::InvalidRequest,
                "unsupported_protocol_version",
            ));
            return None;
        }
        Some(request)
    }
}
