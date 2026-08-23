//! Structured, redacted observability.
//!
//! # This is not the audit ledger
//!
//! Naming matters here, so the module says plainly what it is: Gate 0 emits
//! structured local observations of what the broker decided. It is **not** the
//! anchored audit ledger of ADR §5.2 — there is no hash chain, no sequence
//! number, no off-host WORM anchor, no tamper-evidence, and no durability
//! guarantee at all. A process restart loses everything written here.
//!
//! The ledger arrives at Gate 2, and the record shape it will use already
//! exists as `schemas/ledger-record.schema.json`. Calling this an "audit trail"
//! before then would claim a property nobody has built, which is exactly the
//! kind of claim the threat model asks these documents not to make.
//!
//! # Redaction
//!
//! Observations carry the *resolved* identity, never the credential that
//! produced it, and never caller-supplied strings beyond a correlation id
//! constrained to graphic ASCII. `Token`'s `Debug` is redacted at the protocol
//! layer, so a token cannot reach an observation even through a formatting
//! mistake — `no_credential_material_reaches_an_observation` asserts it.
//!
//! # Why it exists at Gate 0 at all
//!
//! Beyond debugging: this is where the **resolved** identity is written down. A
//! test proving the broker ignored a caller's asserted identity reads it here,
//! from the same value the handler acted on — not from the response, which a
//! later refactor could decouple from the decision.

use crate::peercred::PeerCredentials;
use agentbed_protocol::digest::Digest;
use agentbed_protocol::wire::{DecisionStage, EffectClass, ErrorCode};
use std::sync::{Arc, Mutex};

/// What the broker decided about one request.
#[derive(Debug, Clone)]
pub struct CallObservation {
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

impl CallObservation {
    /// A record for a request that never reached identity resolution.
    #[must_use]
    pub fn rejected(peer: PeerCredentials, error: ErrorCode, reason: &'static str) -> Self {
        CallObservation {
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

/// Where observations go.
pub trait ObservationSink: Send + Sync {
    /// Record one decision. Implementations must not panic and must not block
    /// indefinitely: this runs on the connection's thread, so an observability
    /// failure must never become a request-path failure.
    fn record(&self, record: CallObservation);
}

/// Writes one line per observation to stderr, for the spike.
#[derive(Debug, Default)]
pub struct StderrObserver;

impl ObservationSink for StderrObserver {
    fn record(&self, record: CallObservation) {
        // Never interpolate caller-supplied strings beyond the correlation id
        // (which is constrained to graphic ASCII) and the resolved agent id
        // (which comes from the token store, not the wire).
        eprintln!(
            "observation agent={} peer_uid={} op={} allowed={} stage={} error={} reason={} req={}",
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

/// Collects observations in memory so tests can assert on what the broker decided.
#[derive(Debug, Clone, Default)]
pub struct CollectingObserver {
    records: Arc<Mutex<Vec<CallObservation>>>,
}

impl CollectingObserver {
    /// A snapshot of everything recorded so far.
    ///
    /// Returns an empty vec if a previous panic poisoned the lock — the broker
    /// must not turn an audit-side failure into a request-path failure.
    #[must_use]
    pub fn records(&self) -> Vec<CallObservation> {
        self.records.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl ObservationSink for CollectingObserver {
    fn record(&self, record: CallObservation) {
        if let Ok(mut guard) = self.records.lock() {
            guard.push(record);
        }
    }
}
