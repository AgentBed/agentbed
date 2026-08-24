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
//! 4. **Project the operation through its schema**, then build the digest over
//!    that projection using the construction frozen in `docs/protocol.md` §4/§7:
//!    `SHA-256(domain(protocol) || JCS({operation, operation_version, arguments}))`.
//!    The bytes an approval or ledger record binds are produced here, from the
//!    validated typed operation — never from the caller's serialization, never
//!    accepted from the gateway, and never re-serialized later
//!    (`docs/effects.md` §1).
//! 5. **Compute the effect set and evaluate the ladder** (`policy`).
//! 6. Execute only if the ladder allowed. Stage 5 admits atomically inside
//!    the ladder, so there is no window between "may proceed" and "counted".

use crate::adapter::HostAdapter;
use crate::digest::OperationDigest;
use crate::identity::{AgentContext, TokenStore};
use crate::manifest::ManifestStore;
use crate::observability::{CallObservation, ObservationSink};
use crate::peercred::PeerCredentials;
use crate::policy::{evaluate, CallAdmission, CallDescriptor, Decision};
use crate::quota::QuotaLedger;
use crate::tools::{config_propose, events, system_info, transaction};
use crate::transaction::engine::{EngineError, TransactionEngine};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::ConfigProposeResult;
use agentbed_protocol::strict;
use agentbed_protocol::wire::{
    CallBinding, ConfigProposeParams, DecisionStage, EffectClass, ErrorCode, EventsReplayParams,
    OpName, OperationResult, Request, RequestId, Response, ResponseError, SystemInfoParams,
    TxApplyParams, TxRollbackParams, TxStatusParams, TxTestParams,
};
use agentbed_protocol::PROTOCOL_VERSION_V1;
use agentbed_schemas::{validate, SchemaKind};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

/// Everything the request path needs, assembled once at startup.
pub struct Dispatcher {
    tokens: TokenStore,
    manifests: ManifestStore,
    adapter: Arc<dyn HostAdapter>,
    quotas: QuotaLedger,
    transactions: Arc<TransactionEngine>,
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
    #[allow(clippy::expect_used)]
    pub fn new(
        tokens: TokenStore,
        manifests: ManifestStore,
        adapter: Box<dyn HostAdapter>,
        state_dir: impl AsRef<std::path::Path>,
    ) -> Self {
        let state_dir = state_dir.as_ref().to_path_buf();
        let adapter: Arc<dyn HostAdapter> = Arc::from(adapter);
        let transactions = Arc::new(
            TransactionEngine::open_owned(&state_dir, Arc::clone(&adapter))
                .expect("transaction engine initializes"),
        );
        Dispatcher {
            tokens,
            manifests,
            adapter,
            quotas: QuotaLedger::default(),
            transactions,
        }
    }

    /// Handle one complete frame body. Always yields exactly one response.
    #[allow(clippy::too_many_lines)]
    pub fn handle_frame(
        &self,
        body: &[u8],
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
    ) -> Response {
        let Some(request) = Self::parse(body, peer, observer) else {
            return Response::failed(
                None,
                PROTOCOL_VERSION_V1,
                ResponseError::new(ErrorCode::InvalidRequest),
            );
        };
        let protocol = request.v;

        if !request.operation_allowed() {
            observer.record(CallObservation::rejected(
                peer,
                ErrorCode::InvalidRequest,
                "operation_not_allowed_for_protocol_version",
            ));
            return Response::failed(
                Some(request.id),
                protocol,
                ResponseError::new(ErrorCode::InvalidRequest),
            );
        }

        let Ok(agent) = self.tokens.resolve(request.auth.token.expose()) else {
            observer.record(CallObservation {
                request_id: Some(request.id.to_string()),
                op: Some(request.op.as_str()),
                ..CallObservation::rejected(peer, ErrorCode::Unauthenticated, "token_not_resolved")
            });
            return Response::failed(
                Some(request.id),
                protocol,
                ResponseError::new(ErrorCode::Unauthenticated),
            );
        };

        match request.op {
            OpName::SystemInfo => {
                if request.op_version != system_info::VERSION {
                    return Self::unsupported_operation_version(
                        &request,
                        &agent,
                        peer,
                        observer,
                        system_info::OP,
                    );
                }
                self.system_info(&request, &agent, peer, observer, protocol)
            }
            OpName::ConfigPropose => {
                if request.op_version != config_propose::VERSION {
                    return Self::unsupported_operation_version(
                        &request,
                        &agent,
                        peer,
                        observer,
                        config_propose::OP,
                    );
                }
                self.config_propose(&request, &agent, peer, observer, protocol)
            }
            OpName::TxTest => {
                if request.op_version != transaction::test::VERSION {
                    return Self::unsupported_operation_version(
                        &request,
                        &agent,
                        peer,
                        observer,
                        transaction::test::OP,
                    );
                }
                self.tx_test(&request, &agent, peer, observer, protocol)
            }
            OpName::TxApply => {
                if request.op_version != transaction::apply::VERSION {
                    return Self::unsupported_operation_version(
                        &request,
                        &agent,
                        peer,
                        observer,
                        transaction::apply::OP,
                    );
                }
                self.tx_apply(&request, &agent, peer, observer, protocol)
            }
            OpName::TxRollback => {
                if request.op_version != transaction::rollback::VERSION {
                    return Self::unsupported_operation_version(
                        &request,
                        &agent,
                        peer,
                        observer,
                        transaction::rollback::OP,
                    );
                }
                self.contract_only(
                    &request,
                    &agent,
                    peer,
                    observer,
                    protocol,
                    transaction::rollback::describe_call(),
                    transaction::rollback::OP,
                    SchemaKind::TxRollbackRequest,
                    |params| {
                        serde_json::from_value::<TxRollbackParams>(params.clone())
                            .map_err(|_| "params_rejected")
                    },
                )
            }
            OpName::TxStatus => {
                if request.op_version != transaction::status::VERSION {
                    return Self::unsupported_operation_version(
                        &request,
                        &agent,
                        peer,
                        observer,
                        transaction::status::OP,
                    );
                }
                self.tx_status(&request, &agent, peer, observer, protocol)
            }
            OpName::EventsReplay => {
                if request.op_version != events::VERSION {
                    return Self::unsupported_operation_version(
                        &request,
                        &agent,
                        peer,
                        observer,
                        events::OP,
                    );
                }
                self.events_replay(&request, &agent, peer, observer, protocol)
            }
        }
    }

    fn config_propose(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
    ) -> Response {
        self.execute_v2(
            request,
            agent,
            peer,
            observer,
            protocol,
            &config_propose::describe_call(),
            config_propose::OP,
            SchemaKind::ConfigProposeRequest,
            |params| {
                serde_json::from_value::<ConfigProposeParams>(params.clone())
                    .map_err(|_| "params_rejected")
            },
            |manifest_digest, params| {
                let outcome = self.transactions.config_propose(
                    agent.agent_id(),
                    &manifest_digest.to_string(),
                    params,
                )?;
                Ok(OperationResult::ConfigPropose(Box::new(
                    ConfigProposeResult {
                        tx_id: outcome.tx_id,
                        diff: outcome.diff,
                        test_plan: outcome.test_plan,
                        affected_resources: outcome.affected_resources,
                        base_revision: outcome.base_revision,
                    },
                )))
            },
        )
    }

    fn events_replay(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
    ) -> Response {
        self.execute_v2(
            request,
            agent,
            peer,
            observer,
            protocol,
            &events::describe_call(),
            events::OP,
            SchemaKind::EventsReplayRequest,
            |params| {
                serde_json::from_value::<EventsReplayParams>(params.clone())
                    .map_err(|_| "params_rejected")
            },
            |_manifest_digest, params| {
                let replay = self.transactions.events_replay(params.cursor.as_deref())?;
                Ok(OperationResult::EventsReplay(Box::new(replay)))
            },
        )
    }

    fn tx_test(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
    ) -> Response {
        self.execute_v2(
            request,
            agent,
            peer,
            observer,
            protocol,
            &transaction::test::describe_call(),
            transaction::test::OP,
            SchemaKind::TxTestRequest,
            |params| {
                serde_json::from_value::<TxTestParams>(params.clone())
                    .map_err(|_| "params_rejected")
            },
            |_manifest_digest, params| {
                let step = self.transactions.tx_test(agent.agent_id(), params)?;
                Ok(OperationResult::TxTest(Box::new(step)))
            },
        )
    }

    fn tx_apply(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
    ) -> Response {
        self.execute_v2(
            request,
            agent,
            peer,
            observer,
            protocol,
            &transaction::apply::describe_call(),
            transaction::apply::OP,
            SchemaKind::TxApplyRequest,
            |params| {
                serde_json::from_value::<TxApplyParams>(params.clone())
                    .map_err(|_| "params_rejected")
            },
            |_manifest_digest, params| {
                let step = self.transactions.tx_apply(agent.agent_id(), params)?;
                Ok(OperationResult::TxApply(Box::new(step)))
            },
        )
    }

    fn tx_status(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
    ) -> Response {
        self.execute_v2(
            request,
            agent,
            peer,
            observer,
            protocol,
            &transaction::status::describe_call(),
            transaction::status::OP,
            SchemaKind::TxStatusRequest,
            |params| {
                serde_json::from_value::<TxStatusParams>(params.clone())
                    .map_err(|_| "params_rejected")
            },
            |_manifest_digest, params| {
                let status = self.transactions.tx_status_params(params)?;
                Ok(OperationResult::TxStatus(Box::new(status)))
            },
        )
    }

    /// Shared v2 path: validate, digest, policy, then execute against the engine.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn execute_v2<P, F, X>(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
        call: &CallDescriptor,
        op: &'static str,
        schema: SchemaKind,
        project: F,
        execute: X,
    ) -> Response
    where
        P: DeserializeOwned + Serialize + Clone,
        F: FnOnce(&Value) -> Result<P, &'static str>,
        X: FnOnce(&Digest, &P) -> Result<OperationResult, EngineError>,
    {
        let id = request.id.clone();

        let Ok(typed) = project(&request.params) else {
            return Self::refuse(
                &id,
                agent,
                peer,
                observer,
                protocol,
                ResponseError::new(ErrorCode::InvalidRequest),
                "params_rejected",
                None,
                op,
                call.effect_set.clone(),
            );
        };

        let operation_digest = match Self::project_and_digest_with(
            protocol,
            op,
            request.op_version,
            &request.params,
            schema,
            |_| Ok(typed.clone()),
        ) {
            Ok(digest) => digest,
            Err((error, reason)) => {
                return Self::refuse(
                    &id,
                    agent,
                    peer,
                    observer,
                    protocol,
                    error,
                    reason,
                    None,
                    op,
                    call.effect_set.clone(),
                )
            }
        };

        let Ok(manifest) = self.manifests.load(agent.manifest_ref()) else {
            return Self::refuse(
                &id,
                agent,
                peer,
                observer,
                protocol,
                ResponseError::new(ErrorCode::Denied),
                "manifest_unavailable",
                Some(operation_digest),
                op,
                call.effect_set.clone(),
            );
        };

        let admission = AgentAdmission {
            ledger: &self.quotas,
            agent_id: agent.agent_id(),
        };
        let decision = evaluate(call, &manifest, &self.adapter.safety_vector(), &admission);

        match decision {
            Decision::Refuse {
                code,
                stage,
                reason,
            } => {
                observer.record(CallObservation {
                    request_id: Some(id.to_string()),
                    agent_id: Some(agent.agent_id().to_owned()),
                    peer,
                    op: Some(op),
                    effect_set: call.effect_set.clone(),
                    manifest_digest: Some(manifest.digest().clone()),
                    operation_digest: Some(operation_digest),
                    allowed: false,
                    stage: Some(stage),
                    error: Some(code),
                    reason,
                });
                Response::failed(Some(id), protocol, ResponseError::at_stage(code, stage))
            }
            Decision::Allow => match execute(manifest.digest(), &typed) {
                Ok(result) => {
                    let binding = CallBinding {
                        agent_id: agent.agent_id().to_owned(),
                        manifest_digest: manifest.digest().clone(),
                        effect_set: call.effect_set.clone(),
                        operation_digest: operation_digest.clone(),
                    };
                    observer.record(CallObservation {
                        request_id: Some(id.to_string()),
                        agent_id: Some(agent.agent_id().to_owned()),
                        peer,
                        op: Some(op),
                        effect_set: call.effect_set.clone(),
                        manifest_digest: Some(manifest.digest().clone()),
                        operation_digest: Some(operation_digest),
                        allowed: true,
                        stage: None,
                        error: None,
                        reason: "authorized",
                    });
                    Response::ok(id, protocol, result, binding)
                }
                Err(engine_err) => Self::engine_failure(
                    &id,
                    agent,
                    peer,
                    observer,
                    protocol,
                    op,
                    call.effect_set.clone(),
                    manifest.digest(),
                    operation_digest,
                    &engine_err,
                ),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn engine_failure(
        id: &RequestId,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
        op: &'static str,
        effect_set: Vec<EffectClass>,
        manifest_digest: &Digest,
        operation_digest: Digest,
        err: &EngineError,
    ) -> Response {
        let (code, stage, reason) = match err {
            EngineError::NotFound
            | EngineError::BaseRevisionMoved
            | EngineError::IdempotencyConflict => (
                ErrorCode::Denied,
                DecisionStage::OperationPolicy,
                "transaction_refused",
            ),
            EngineError::SafeMode
            | EngineError::InvalidTransition
            | EngineError::WatchdogAuthorityRequired => (
                ErrorCode::Denied,
                DecisionStage::ForbiddenClass,
                "transaction_refused",
            ),
            EngineError::Storage(_) => (
                ErrorCode::Internal,
                DecisionStage::ForbiddenClass,
                "storage_failure",
            ),
        };
        observer.record(CallObservation {
            request_id: Some(id.to_string()),
            agent_id: Some(agent.agent_id().to_owned()),
            peer,
            op: Some(op),
            effect_set,
            manifest_digest: Some(manifest_digest.clone()),
            operation_digest: Some(operation_digest),
            allowed: false,
            stage: Some(stage),
            error: Some(code),
            reason,
        });
        let error = if matches!(code, ErrorCode::Internal) {
            ResponseError::new(code)
        } else {
            ResponseError::at_stage(code, stage)
        };
        Response::failed(Some(id.clone()), protocol, error)
    }

    fn system_info(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
    ) -> Response {
        let id = request.id.clone();

        let operation_digest = match Self::project_and_digest::<SystemInfoParams>(
            protocol,
            system_info::OP,
            system_info::VERSION,
            &request.params,
            SchemaKind::SystemInfoRequest,
        ) {
            Ok(digest) => digest,
            Err((error, reason)) => {
                return Self::refuse(
                    &id,
                    agent,
                    peer,
                    observer,
                    protocol,
                    error,
                    reason,
                    None,
                    system_info::OP,
                    vec![EffectClass::R],
                )
            }
        };

        let Ok(manifest) = self.manifests.load(agent.manifest_ref()) else {
            return Self::refuse(
                &id,
                agent,
                peer,
                observer,
                protocol,
                ResponseError::new(ErrorCode::Denied),
                "manifest_unavailable",
                Some(operation_digest),
                system_info::OP,
                vec![EffectClass::R],
            );
        };

        let call = system_info::describe_call();
        let admission = AgentAdmission {
            ledger: &self.quotas,
            agent_id: agent.agent_id(),
        };
        let decision = evaluate(&call, &manifest, &self.adapter.safety_vector(), &admission);

        match decision {
            Decision::Refuse {
                code,
                stage,
                reason,
            } => {
                observer.record(CallObservation {
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
                Response::failed(Some(id), protocol, ResponseError::at_stage(code, stage))
            }
            Decision::Allow => {
                let info = system_info::execute(self.adapter.as_ref());
                let binding = CallBinding {
                    agent_id: agent.agent_id().to_owned(),
                    manifest_digest: manifest.digest().clone(),
                    effect_set: call.effect_set.clone(),
                    operation_digest: operation_digest.clone(),
                };
                observer.record(CallObservation {
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
                Response::ok(
                    id,
                    protocol,
                    OperationResult::SystemInfo(Box::new(info)),
                    binding,
                )
            }
        }
    }

    /// Validate, digest, and evaluate policy for a v2 contract operation whose
    /// execution lands in a later Gate 1 lane.
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    fn contract_only<P, F>(
        &self,
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        protocol: u8,
        call: CallDescriptor,
        op: &'static str,
        schema: SchemaKind,
        project: F,
    ) -> Response
    where
        P: DeserializeOwned + Serialize,
        F: FnOnce(&Value) -> Result<P, &'static str>,
    {
        let id = request.id.clone();

        let operation_digest = match Self::project_and_digest_with(
            protocol,
            op,
            request.op_version,
            &request.params,
            schema,
            project,
        ) {
            Ok(digest) => digest,
            Err((error, reason)) => {
                return Self::refuse(
                    &id,
                    agent,
                    peer,
                    observer,
                    protocol,
                    error,
                    reason,
                    None,
                    op,
                    call.effect_set.clone(),
                )
            }
        };

        let Ok(manifest) = self.manifests.load(agent.manifest_ref()) else {
            return Self::refuse(
                &id,
                agent,
                peer,
                observer,
                protocol,
                ResponseError::new(ErrorCode::Denied),
                "manifest_unavailable",
                Some(operation_digest),
                op,
                call.effect_set.clone(),
            );
        };

        let admission = AgentAdmission {
            ledger: &self.quotas,
            agent_id: agent.agent_id(),
        };
        let decision = evaluate(&call, &manifest, &self.adapter.safety_vector(), &admission);

        match decision {
            Decision::Refuse {
                code,
                stage,
                reason,
            } => {
                observer.record(CallObservation {
                    request_id: Some(id.to_string()),
                    agent_id: Some(agent.agent_id().to_owned()),
                    peer,
                    op: Some(op),
                    effect_set: call.effect_set.clone(),
                    manifest_digest: Some(manifest.digest().clone()),
                    operation_digest: Some(operation_digest),
                    allowed: false,
                    stage: Some(stage),
                    error: Some(code),
                    reason,
                });
                Response::failed(Some(id), protocol, ResponseError::at_stage(code, stage))
            }
            Decision::Allow => {
                observer.record(CallObservation {
                    request_id: Some(id.to_string()),
                    agent_id: Some(agent.agent_id().to_owned()),
                    peer,
                    op: Some(op),
                    effect_set: call.effect_set.clone(),
                    manifest_digest: Some(manifest.digest().clone()),
                    operation_digest: Some(operation_digest),
                    allowed: false,
                    stage: None,
                    error: Some(ErrorCode::Internal),
                    reason: "execution_not_implemented_at_l00",
                });
                Response::failed(Some(id), protocol, ResponseError::new(ErrorCode::Internal))
            }
        }
    }

    fn unsupported_operation_version(
        request: &Request,
        agent: &AgentContext,
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
        op: &'static str,
    ) -> Response {
        observer.record(CallObservation {
            request_id: Some(request.id.to_string()),
            agent_id: Some(agent.agent_id().to_owned()),
            op: Some(op),
            ..CallObservation::rejected(
                peer,
                ErrorCode::UnsupportedOperation,
                "unsupported_operation_version",
            )
        });
        Response::failed(
            Some(request.id.clone()),
            request.v,
            ResponseError::new(ErrorCode::UnsupportedOperation),
        )
    }

    fn project_and_digest<P>(
        protocol: u8,
        operation: &'static str,
        operation_version: u32,
        params: &Value,
        schema: SchemaKind,
    ) -> Result<Digest, (ResponseError, &'static str)>
    where
        P: DeserializeOwned + Serialize,
    {
        Self::project_and_digest_with(
            protocol,
            operation,
            operation_version,
            params,
            schema,
            |value| serde_json::from_value::<P>(value.clone()).map_err(|_| "params_rejected"),
        )
    }

    fn project_and_digest_with<P, F>(
        protocol: u8,
        operation: &'static str,
        operation_version: u32,
        params: &Value,
        schema: SchemaKind,
        project: F,
    ) -> Result<Digest, (ResponseError, &'static str)>
    where
        P: DeserializeOwned + Serialize,
        F: FnOnce(&Value) -> Result<P, &'static str>,
    {
        let typed = project(params)
            .map_err(|reason| (ResponseError::new(ErrorCode::InvalidRequest), reason))?;
        let projected = serde_json::to_value(&typed).map_err(|_| {
            (
                ResponseError::new(ErrorCode::Internal),
                "params_not_serializable",
            )
        })?;

        validate(schema, &projected).map_err(|_| {
            (
                ResponseError::new(ErrorCode::InvalidRequest),
                "params_failed_schema",
            )
        })?;

        let canonical = OperationDigest::of(protocol, operation, operation_version, &projected)
            .map_err(|_| {
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
        observer: &dyn ObservationSink,
        protocol: u8,
        error: ResponseError,
        reason: &'static str,
        operation_digest: Option<Digest>,
        op: &'static str,
        effect_set: Vec<EffectClass>,
    ) -> Response {
        observer.record(CallObservation {
            request_id: Some(id.to_string()),
            agent_id: Some(agent.agent_id().to_owned()),
            peer,
            op: Some(op),
            effect_set,
            manifest_digest: None,
            operation_digest,
            allowed: false,
            stage: error.stage,
            error: Some(error.code),
            reason,
        });
        Response::failed(Some(id.clone()), protocol, error)
    }

    /// Strict parse plus version check. Returns `None` when the frame is not a
    /// well-formed request, having already recorded why.
    fn parse(
        body: &[u8],
        peer: PeerCredentials,
        observer: &dyn ObservationSink,
    ) -> Option<Request> {
        let Ok(value) = strict::parse(body) else {
            observer.record(CallObservation::rejected(
                peer,
                ErrorCode::InvalidRequest,
                "malformed_json",
            ));
            return None;
        };
        let Ok(request) = serde_json::from_value::<Request>(value) else {
            observer.record(CallObservation::rejected(
                peer,
                ErrorCode::InvalidRequest,
                "envelope_rejected",
            ));
            return None;
        };
        if !request.version_supported() {
            observer.record(CallObservation::rejected(
                peer,
                ErrorCode::InvalidRequest,
                "unsupported_protocol_version",
            ));
            return None;
        }
        Some(request)
    }
}

/// Binds the quota ledger to one agent for the duration of a call.
///
/// The ladder never learns the agent id or the counter — it gets the ability to
/// admit one call and nothing else.
struct AgentAdmission<'a> {
    ledger: &'a QuotaLedger,
    agent_id: &'a str,
}

impl CallAdmission for AgentAdmission<'_> {
    fn try_admit(&self, limit: Option<u64>) -> bool {
        self.ledger.try_admit(self.agent_id, limit)
    }
}
