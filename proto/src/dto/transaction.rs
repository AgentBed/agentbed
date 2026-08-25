//! Transaction and proposal result shapes for protocol v2 operations.
//!
//! These are wire vocabulary only; state transitions and persistence live in
//! the broker (`docs/effects.md` §3).

use crate::digest::Digest;
use crate::wire::EffectClass;
use serde::{Deserialize, Serialize};

/// ULID on the wire (Crockford base32, 26 characters).
pub type TxId = String;

/// Base revision captured at propose/apply time (`docs/effects.md` §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseRevision {
    /// Nix generation or snapshot identifier when the adapter resolves one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    /// Git commit of tracked `/etc` when present.
    pub etc_git_commit: String,
    /// Digest of the active configuration tree.
    pub config_digest: Digest,
}

/// Human-readable diff returned by `config.propose` (ADR-001 §5.2 step 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigProposeResult {
    pub tx_id: TxId,
    pub diff: String,
    pub test_plan: TestPlan,
    pub affected_resources: Vec<String>,
    pub base_revision: BaseRevision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_closure: Option<String>,
}

/// Adapter-specific steps the broker will run during `tx.test` (ADR-001 §5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestPlan {
    pub adapter: String,
    pub steps: Vec<String>,
}

/// Terminal and in-flight states (`docs/effects.md` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransactionState {
    Idle,
    Proposed,
    Testing,
    Applying,
    Probation,
    ProbationPassed,
    Committing,
    Committed,
    Rejected,
    Reverting,
    Reverted,
}

/// Read surface for `tx.status` (ADR-001 §5.1, effects.md §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TxStatusResult {
    pub tx_id: TxId,
    pub state: TransactionState,
    pub effect_set: Vec<EffectClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<BaseRevision>,
}

/// Acknowledgement for mutating transaction steps before the engine runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TxStepResult {
    pub tx_id: TxId,
    pub state: TransactionState,
}
