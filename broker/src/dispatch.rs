//! Request dispatch: parse, authenticate, then hand to the one handler.
//!
//! The dispatcher is a `match` over a closed enum, not a table of registered
//! handlers. That is deliberate (ADR §5.0: "fixed, narrow RPC"): with no
//! registry there is no way to reach code by naming it, and an unknown
//! operation fails during deserialization rather than at a lookup miss.
//!
//! Order of operations, and why it is this order:
//!
//! 1. **Parse strictly.** Unknown fields — including any attempt to assert an
//!    identity — are refused here, before anything is authenticated. A forged
//!    frame gets no further than the parser.
//! 2. **Resolve identity from the token**, in the broker, ignoring everything
//!    else about the caller.
//! 3. Evaluate policy and run the handler (Gate 0: those arrive in the next
//!    commits).

use crate::audit::{AuditRecord, AuditSink};
use crate::identity::TokenStore;
use crate::peercred::PeerCredentials;
use agentbed_protocol::strict;
use agentbed_protocol::wire::{ErrorCode, OpName, Request, Response, ResponseError};

/// Everything the request path needs, assembled once at startup.
#[derive(Debug)]
pub struct Dispatcher {
    tokens: TokenStore,
}

impl Dispatcher {
    /// Build a dispatcher over a token store.
    #[must_use]
    pub fn new(tokens: TokenStore) -> Self {
        Dispatcher { tokens }
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

        // Identity is derived here, from the presented token alone. Note what
        // is *not* consulted: the peer credentials (they authenticated the
        // channel), and any field of the request (there is none to consult).
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
            OpName::SystemInfo => {
                // The policy engine and the system.info handler land in the
                // next two commits; until then an authenticated call is
                // refused rather than served, so no path can answer without
                // having been authorized.
                audit.record(AuditRecord {
                    request_id: Some(request.id.to_string()),
                    agent_id: Some(agent.agent_id().to_owned()),
                    op: Some(request.op.as_str()),
                    ..AuditRecord::rejected(peer, ErrorCode::Internal, "handler_not_wired")
                });
                Response::failed(Some(request.id), ResponseError::new(ErrorCode::Internal))
            }
        }
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
