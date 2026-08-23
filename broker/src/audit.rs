//! Audit records.
//!
//! Gate 0 writes structured lines; the hash chain and the WORM anchor are
//! Gate 2 (`docs/roadmap.md`), and the schema the chain will use already exists
//! in `schemas/ledger-record.schema.json`.
//!
//! The record exists at Gate 0 for one reason beyond debugging: it is where the
//! **resolved** identity is written down. A test that wants to prove the broker
//! ignored a caller's asserted identity reads it here, from the same value the
//! handler acted on — not from the response, which a future refactor could
//! decouple from the decision.

use crate::peercred::PeerCredentials;
use agentbed_protocol::digest::Digest;
use agentbed_protocol::wire::{DecisionStage, EffectClass, ErrorCode};
use std::sync::{Arc, Mutex};

/// What happened to one request.
#[derive(Debug, Clone)]
pub struct AuditRecord {
    /// Correlation id, when the request was well-formed enough to have one.
    pub request_id: Option<String>,
    /// Identity the broker resolved. `None` means no identity was established,
    /// which is itself the interesting case for a forged-gateway attempt.
    pub agent_id: Option<String>,
    /// Kernel-reported peer, for attribution of the *channel*.
    pub peer: PeerCredentials,
    /// Operation name, when it parsed.
    pub op: Option<&'static str>,
    /// The exact computed effect set.
    pub effect_set: Vec<EffectClass>,
    /// Digest of the manifest evaluated against.
    pub manifest_digest: Option<Digest>,
    /// SHA-256 over the RFC 8785 canonical bytes of the validated operation.
    pub operation_digest: Option<Digest>,
    /// Whether the call was allowed.
    pub allowed: bool,
    /// Which precedence stage decided, when a policy stage did.
    pub stage: Option<DecisionStage>,
    /// The wire error returned, when the call failed.
    pub error: Option<ErrorCode>,
    /// Short machine-readable reason, for the ledger's `reason_code`.
    pub reason: &'static str,
}

impl AuditRecord {
    /// A record for a request that never reached identity resolution.
    #[must_use]
    pub fn rejected(peer: PeerCredentials, error: ErrorCode, reason: &'static str) -> Self {
        AuditRecord {
            request_id: None,
            agent_id: None,
            peer,
            op: None,
            effect_set: Vec::new(),
            manifest_digest: None,
            operation_digest: None,
            allowed: false,
            stage: None,
            error: Some(error),
            reason,
        }
    }
}

/// Where audit records go.
pub trait AuditSink: Send + Sync {
    /// Record one decision. Implementations must not panic and must not block
    /// indefinitely: this runs on the connection's thread.
    fn record(&self, record: AuditRecord);
}

/// Writes one line per record to stderr, for the spike.
#[derive(Debug, Default)]
pub struct StderrAudit;

impl AuditSink for StderrAudit {
    fn record(&self, record: AuditRecord) {
        // Never interpolate caller-supplied strings beyond the correlation id
        // (which is constrained to graphic ASCII) and the resolved agent id
        // (which comes from the token store, not the wire).
        eprintln!(
            "audit agent={} peer_uid={} op={} allowed={} stage={} error={} reason={} req={}",
            record.agent_id.as_deref().unwrap_or("-"),
            record.peer.uid,
            record.op.unwrap_or("-"),
            record.allowed,
            record
                .stage
                .map_or("-".to_owned(), |s| s.ordinal().to_string()),
            record.error.map_or("-".to_owned(), |e| format!("{e:?}")),
            record.reason,
            record.request_id.as_deref().unwrap_or("-"),
        );
    }
}

/// Collects records in memory so tests can assert on what the broker decided.
#[derive(Debug, Clone, Default)]
pub struct CollectingAudit {
    records: Arc<Mutex<Vec<AuditRecord>>>,
}

impl CollectingAudit {
    /// A snapshot of everything recorded so far.
    ///
    /// Returns an empty vec if a previous panic poisoned the lock — the broker
    /// must not turn an audit-side failure into a request-path failure.
    #[must_use]
    pub fn records(&self) -> Vec<AuditRecord> {
        self.records.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl AuditSink for CollectingAudit {
    fn record(&self, record: AuditRecord) {
        if let Ok(mut guard) = self.records.lock() {
            guard.push(record);
        }
    }
}
