//! Serialized D/M transaction engine with WAL-backed state.

#![allow(
    clippy::expect_used,
    missing_debug_implementations,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

use crate::adapter::HostAdapter;
use crate::events::{EventLog, EventRecord};
use crate::storage::durability::{DurabilityError, RealDurability};
use crate::storage::wal::{WalRecord, WalStore};
use crate::transaction::state::{broker_may_enter, TransactionState, WireState};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::{
    BaseRevision, TestPlan, TxId, TxStatusResult, TxStepResult,
};
use agentbed_protocol::wire::{
    ConfigProposeParams, EffectClass, TxApplyParams, TxStatusParams, TxTestParams,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Engine-level failures (no sensitive prose on the wire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    SafeMode,
    NotFound,
    InvalidTransition,
    IdempotencyConflict,
    BaseRevisionMoved,
    WatchdogAuthorityRequired,
    Storage(DurabilityError),
}

/// Outcome of `config.propose` including durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProposeOutcome {
    pub tx_id: TxId,
    pub diff: String,
    pub test_plan: TestPlan,
    pub affected_resources: Vec<String>,
    pub base_revision: BaseRevision,
    pub state: WireState,
}

#[derive(Debug, Clone)]
struct TxSnapshot {
    tx_id: TxId,
    state: WireState,
    #[allow(dead_code)]
    agent_id: String,
    manifest_digest: Digest,
    base_revision: BaseRevision,
    effect_set: Vec<EffectClass>,
    diff: String,
    affected_resources: Vec<String>,
}

#[derive(Debug, Clone)]
struct IdempotencyEntry {
    tx_id: TxId,
    fingerprint: String,
}

/// Serialized transaction engine — one lock for all D/M work.
pub struct TransactionEngine {
    adapter: Arc<dyn HostAdapter>,
    wal: Mutex<WalStore>,
    events: EventLog,
    txs: Mutex<HashMap<TxId, TxSnapshot>>,
    idempotency: Mutex<HashMap<String, IdempotencyEntry>>,
    next_seq: Mutex<u64>,
    safe_mode: Mutex<bool>,
}

impl TransactionEngine {
    pub fn open(
        root: impl AsRef<Path>,
        adapter: impl HostAdapter + 'static,
    ) -> Result<Self, EngineError> {
        Self::open_owned(root, Arc::new(adapter))
    }

    /// Open with an owned adapter (preferred for long-lived brokers).
    pub fn open_owned(
        root: impl AsRef<Path>,
        adapter: Arc<dyn HostAdapter>,
    ) -> Result<Self, EngineError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root).map_err(|_| EngineError::Storage(DurabilityError::Io))?;
        let wal_dir = root.join("wal");
        let events_dir = root.join("events");
        let wal =
            WalStore::open(&wal_dir, Arc::new(RealDurability)).map_err(EngineError::Storage)?;
        let safe_mode = wal.safe_mode();
        let events = EventLog::open(&events_dir).map_err(|_| EngineError::SafeMode)?;

        let mut txs = HashMap::new();
        let mut next_seq = 0_u64;
        if !safe_mode {
            for record in wal.load_records().map_err(EngineError::Storage)? {
                next_seq = next_seq.max(record.seq);
                txs.insert(
                    record.tx_id.clone(),
                    TxSnapshot {
                        tx_id: record.tx_id,
                        state: record.state,
                        agent_id: record.agent_id,
                        manifest_digest: record.manifest_digest,
                        base_revision: record.base_revision,
                        effect_set: record.effect_set,
                        diff: record.diff,
                        affected_resources: record.affected_resources,
                    },
                );
            }
        }

        Ok(Self {
            adapter,
            wal: Mutex::new(wal),
            events,
            txs: Mutex::new(txs),
            idempotency: Mutex::new(HashMap::new()),
            next_seq: Mutex::new(next_seq),
            safe_mode: Mutex::new(safe_mode),
        })
    }

    pub fn config_propose(
        &self,
        agent_id: &str,
        manifest_digest: &str,
        params: &ConfigProposeParams,
    ) -> Result<ConfigProposeOutcome, EngineError> {
        self.ensure_dm_allowed()?;
        let key = idem_key(agent_id, "config.propose", params.idempotency_key.as_str());
        let fingerprint = propose_fingerprint(params);
        if let Some(entry) = self.idempotency.lock().expect("idem").get(&key) {
            if entry.fingerprint != fingerprint {
                return Err(EngineError::IdempotencyConflict);
            }
            let snap = self
                .txs
                .lock()
                .expect("txs")
                .get(&entry.tx_id)
                .cloned()
                .ok_or(EngineError::IdempotencyConflict)?;
            return Ok(snapshot_to_propose_outcome(snap));
        }

        let digest: Digest = serde_json::from_str(&format!("\"{manifest_digest}\""))
            .unwrap_or_else(|_| Digest::from_sha256_bytes([0; 32]));
        let base_revision = self.adapter.current_base_revision();
        let tx_id = new_tx_id();
        let diff = params
            .changes
            .iter()
            .map(|c| format!("{} => staged", c.path))
            .collect::<Vec<_>>()
            .join("\n");
        let affected_resources = vec!["root_config".to_owned()];
        self.persist_transition(
            &tx_id,
            WireState::Proposed,
            Some(params.idempotency_key.as_str().to_owned()),
            agent_id,
            digest.clone(),
            base_revision.clone(),
            vec![EffectClass::D],
            diff.clone(),
            affected_resources.clone(),
        )?;
        self.idempotency.lock().expect("idem").insert(
            key,
            IdempotencyEntry {
                tx_id: tx_id.clone(),
                fingerprint,
            },
        );
        let _ = self.events.append(EventRecord {
            kind: "tx.state".to_owned(),
            payload: format!("{{\"tx_id\":\"{tx_id}\",\"state\":\"PROPOSED\"}}"),
        });
        Ok(ConfigProposeOutcome {
            tx_id,
            diff,
            test_plan: TestPlan {
                adapter: "unresolved".to_owned(),
                steps: vec!["noop-test".to_owned()],
            },
            affected_resources,
            base_revision,
            state: WireState::Proposed,
        })
    }

    pub fn tx_test(
        &self,
        agent_id: &str,
        params: &TxTestParams,
    ) -> Result<TxStepResult, EngineError> {
        self.ensure_dm_allowed()?;
        self.transition(agent_id, params.tx_id.as_str(), TransactionState::Testing)
    }

    pub fn tx_apply(
        &self,
        agent_id: &str,
        params: &TxApplyParams,
    ) -> Result<TxStepResult, EngineError> {
        self.ensure_dm_allowed()?;
        let snap = self
            .snapshot(params.tx_id.as_str())
            .ok_or(EngineError::NotFound)?;
        let current = self.adapter.current_base_revision();
        if snap.base_revision != current {
            return Err(EngineError::BaseRevisionMoved);
        }
        self.transition(agent_id, params.tx_id.as_str(), TransactionState::Applying)
    }

    pub fn advance_to_probation(
        &self,
        agent_id: &str,
        tx_id: &str,
    ) -> Result<TxStepResult, EngineError> {
        self.ensure_dm_allowed()?;
        let snap = self.snapshot(tx_id).ok_or(EngineError::NotFound)?;
        if snap.state == WireState::Probation {
            return Err(EngineError::WatchdogAuthorityRequired);
        }
        self.transition(agent_id, tx_id, TransactionState::Probation)
    }

    pub fn tx_status(&self, tx_id: &str) -> Result<TxStatusResult, EngineError> {
        if *self.safe_mode.lock().expect("safe") {
            return Err(EngineError::SafeMode);
        }
        let snap = self.snapshot(tx_id).ok_or(EngineError::NotFound)?;
        Ok(TxStatusResult {
            tx_id: snap.tx_id,
            state: snap.state,
            effect_set: snap.effect_set,
            base_revision: Some(snap.base_revision),
        })
    }

    pub fn tx_status_params(&self, params: &TxStatusParams) -> Result<TxStatusResult, EngineError> {
        self.tx_status(params.tx_id.as_str())
    }

    fn transition(
        &self,
        agent_id: &str,
        tx_id: &str,
        target: TransactionState,
    ) -> Result<TxStepResult, EngineError> {
        let snap = self.snapshot(tx_id).ok_or(EngineError::NotFound)?;
        let from = TransactionState::from(snap.state);
        if !broker_may_enter(from, target) {
            return Err(EngineError::InvalidTransition);
        }
        let wire_target: WireState = target.into();
        self.persist_transition(
            tx_id,
            wire_target,
            None,
            agent_id,
            snap.manifest_digest,
            snap.base_revision,
            snap.effect_set,
            snap.diff,
            snap.affected_resources,
        )?;
        let _ = self.events.append(EventRecord {
            kind: "tx.state".to_owned(),
            payload: format!("{{\"tx_id\":\"{tx_id}\",\"state\":\"{wire_target:?}\"}}"),
        });
        Ok(TxStepResult {
            tx_id: tx_id.to_owned(),
            state: wire_target,
        })
    }

    fn persist_transition(
        &self,
        tx_id: &str,
        state: WireState,
        idempotency_key: Option<String>,
        agent_id: &str,
        manifest_digest: Digest,
        base_revision: BaseRevision,
        effect_set: Vec<EffectClass>,
        diff: String,
        affected_resources: Vec<String>,
    ) -> Result<(), EngineError> {
        if *self.safe_mode.lock().expect("safe") {
            return Err(EngineError::SafeMode);
        }
        let seq = {
            let mut next = self.next_seq.lock().expect("seq");
            *next = next.saturating_add(1);
            *next
        };
        let record = WalRecord {
            seq,
            tx_id: tx_id.to_owned(),
            state,
            idempotency_key,
            agent_id: agent_id.to_owned(),
            manifest_digest: manifest_digest.clone(),
            base_revision: base_revision.clone(),
            effect_set: effect_set.clone(),
            diff: diff.clone(),
            affected_resources: affected_resources.clone(),
            approval_ref: None,
            result_json: None,
        };
        self.wal
            .lock()
            .expect("wal")
            .append_transition(&record)
            .map_err(EngineError::Storage)?;
        self.txs.lock().expect("txs").insert(
            tx_id.to_owned(),
            TxSnapshot {
                tx_id: tx_id.to_owned(),
                state,
                agent_id: agent_id.to_owned(),
                manifest_digest,
                base_revision,
                effect_set,
                diff,
                affected_resources,
            },
        );
        Ok(())
    }

    fn snapshot(&self, tx_id: &str) -> Option<TxSnapshot> {
        self.txs.lock().expect("txs").get(tx_id).cloned()
    }

    fn ensure_dm_allowed(&self) -> Result<(), EngineError> {
        if *self.safe_mode.lock().expect("safe") {
            return Err(EngineError::SafeMode);
        }
        Ok(())
    }
}

fn snapshot_to_propose_outcome(snap: TxSnapshot) -> ConfigProposeOutcome {
    ConfigProposeOutcome {
        tx_id: snap.tx_id,
        diff: snap.diff,
        test_plan: TestPlan {
            adapter: "unresolved".to_owned(),
            steps: vec!["noop-test".to_owned()],
        },
        affected_resources: snap.affected_resources,
        base_revision: snap.base_revision,
        state: snap.state,
    }
}

fn idem_key(agent_id: &str, op: &str, key: &str) -> String {
    format!("{agent_id}:{op}:{key}")
}

/// 19-char fixed prefix for hermetic ULID-shaped transaction ids in tests.
const TX_ID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ";

fn propose_fingerprint(params: &ConfigProposeParams) -> String {
    serde_json::to_string(params).unwrap_or_default()
}

fn new_tx_id() -> TxId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    // Prefix + 7 Crockford-safe hex digits = 26-byte ULID-shaped id.
    let suffix = format!("{:07X}", n % 0x0FFF_FFFF);
    format!("{TX_ID_PREFIX}{suffix}")
}
