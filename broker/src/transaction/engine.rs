//! Serialized D/M transaction engine with WAL-backed state.

#![allow(
    clippy::expect_used,
    missing_debug_implementations,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

use crate::adapter::HostAdapter;
use crate::events::{EventCursor, EventLog, EventRecord};
use crate::storage::durability::{DurabilityError, RealDurability};
use crate::storage::idempotency::{IdempotencyRecord, IdempotencyStore};
use crate::storage::wal::{WalRecord, WalStore};
use crate::transaction::state::{broker_may_enter, TransactionState, WireState};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::{
    BaseRevision, ConfigProposeResult, TestPlan, TxId, TxStatusResult, TxStepResult,
};
use agentbed_protocol::wire::{
    ConfigProposeParams, EffectClass, EventsReplayResult, StoredEventWire, TxApplyParams,
    TxStatusParams, TxTestParams,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Engine-level failures (no sensitive prose on the wire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    SafeMode,
    NotFound,
    InvalidTransition,
    IdempotencyConflict,
    BaseRevisionMoved,
    OwnershipMismatch,
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

/// Serialized transaction engine — one lock for all D/M work.
pub struct TransactionEngine {
    adapter: Arc<dyn HostAdapter>,
    state_root: PathBuf,
    dm_lock: Mutex<()>,
    wal: Mutex<WalStore>,
    events: EventLog,
    txs: Mutex<HashMap<TxId, TxSnapshot>>,
    idempotency: IdempotencyStore,
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
        let state_root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&state_root)
            .map_err(|_| EngineError::Storage(DurabilityError::Io))?;
        let wal_dir = state_root.join("wal");
        let events_dir = state_root.join("events");
        let idem_dir = state_root.join("idempotency");

        let wal =
            WalStore::open(&wal_dir, Arc::new(RealDurability)).map_err(EngineError::Storage)?;
        let recovery = wal.recover();
        let mut safe_mode = recovery.safe_mode;
        let events = EventLog::open(&events_dir).map_err(|_| EngineError::SafeMode)?;
        if events.validate_integrity().is_err() {
            safe_mode = true;
        }

        let idempotency = IdempotencyStore::open(&idem_dir).map_err(EngineError::Storage)?;
        idempotency.merge_from_wal(&recovery.records);

        let mut txs = HashMap::new();
        let mut next_seq = 0_u64;
        if !safe_mode {
            for record in &recovery.records {
                next_seq = next_seq.max(record.seq);
                txs.insert(
                    record.tx_id.clone(),
                    TxSnapshot {
                        tx_id: record.tx_id.clone(),
                        state: record.state,
                        agent_id: record.agent_id.clone(),
                        manifest_digest: record.manifest_digest.clone(),
                        base_revision: record.base_revision.clone(),
                        effect_set: record.effect_set.clone(),
                        diff: record.diff.clone(),
                        affected_resources: record.affected_resources.clone(),
                    },
                );
            }
        }

        Ok(Self {
            adapter,
            state_root,
            dm_lock: Mutex::new(()),
            wal: Mutex::new(wal),
            events,
            txs: Mutex::new(txs),
            idempotency,
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
        let _guard = self.dm_lock.lock().expect("dm");
        self.ensure_dm_allowed()?;
        let key = idem_key(agent_id, "config.propose", params.idempotency_key.as_str());
        let fingerprint = propose_fingerprint(params);
        if let Some(entry) = self.idempotency.get(&key) {
            if entry.fingerprint != fingerprint {
                return Err(EngineError::IdempotencyConflict);
            }
            return replay_propose(&entry);
        }

        let digest: Digest = parse_manifest_digest(manifest_digest);
        let base_revision = self.adapter.current_base_revision();
        let tx_id = new_tx_id();
        let diff = params
            .changes
            .iter()
            .map(|c| format!("{} => staged", c.path))
            .collect::<Vec<_>>()
            .join("\n");
        let affected_resources = vec!["root_config".to_owned()];
        let wire_result = ConfigProposeResult {
            tx_id: tx_id.clone(),
            diff: diff.clone(),
            test_plan: TestPlan {
                adapter: "unresolved".to_owned(),
                steps: vec!["noop-test".to_owned()],
            },
            affected_resources: affected_resources.clone(),
            base_revision: base_revision.clone(),
        };
        let result_json = serde_json::to_string(&wire_result).map_err(|_| EngineError::SafeMode)?;
        let seq = self.persist_transition(
            &tx_id,
            WireState::Proposed,
            Some(params.idempotency_key.as_str().to_owned()),
            Some(fingerprint.clone()),
            agent_id,
            digest,
            base_revision.clone(),
            vec![EffectClass::D],
            diff.clone(),
            affected_resources.clone(),
            Some(result_json.clone()),
        )?;
        if let Err(err) = self.append_state_event(&tx_id, WireState::Proposed) {
            self.rollback_transition(&tx_id, seq)?;
            return Err(err);
        }
        self.record_idempotency(IdempotencyRecord {
            key,
            tx_id: tx_id.clone(),
            fingerprint,
            result_json,
        })?;
        Ok(ConfigProposeOutcome {
            tx_id,
            diff,
            test_plan: wire_result.test_plan,
            affected_resources,
            base_revision,
            state: WireState::Proposed,
        })
    }

    pub fn tx_test(
        &self,
        agent_id: &str,
        manifest_digest: &str,
        params: &TxTestParams,
    ) -> Result<TxStepResult, EngineError> {
        let _guard = self.dm_lock.lock().expect("dm");
        self.ensure_dm_allowed()?;
        let digest = parse_manifest_digest(manifest_digest);
        self.transition(
            agent_id,
            &digest,
            params.tx_id.as_str(),
            TransactionState::Testing,
            None,
            None,
            None,
            None,
        )
    }

    pub fn tx_apply(
        &self,
        agent_id: &str,
        manifest_digest: &str,
        params: &TxApplyParams,
    ) -> Result<TxStepResult, EngineError> {
        let _guard = self.dm_lock.lock().expect("dm");
        self.ensure_dm_allowed()?;
        let digest = parse_manifest_digest(manifest_digest);
        let key = idem_key(agent_id, "tx.apply", params.idempotency_key.as_str());
        let fingerprint = apply_fingerprint(params);
        if let Some(entry) = self.idempotency.get(&key) {
            if entry.fingerprint != fingerprint {
                return Err(EngineError::IdempotencyConflict);
            }
            return replay_step(&entry);
        }

        let snap = self
            .snapshot(params.tx_id.as_str())
            .ok_or(EngineError::NotFound)?;
        ensure_owner(&snap, agent_id, &digest)?;
        let current = self.adapter.current_base_revision();
        if snap.base_revision != current {
            self.transition(
                &snap.agent_id,
                &snap.manifest_digest,
                params.tx_id.as_str(),
                TransactionState::Rejected,
                None,
                None,
                None,
                None,
            )?;
            return Err(EngineError::BaseRevisionMoved);
        }
        self.transition(
            agent_id,
            &digest,
            params.tx_id.as_str(),
            TransactionState::Applying,
            Some(params.idempotency_key.as_str().to_owned()),
            Some(fingerprint.clone()),
            Some(key),
            Some(fingerprint),
        )
    }

    pub fn advance_to_probation(
        &self,
        agent_id: &str,
        manifest_digest: &str,
        tx_id: &str,
    ) -> Result<TxStepResult, EngineError> {
        let _guard = self.dm_lock.lock().expect("dm");
        self.ensure_dm_allowed()?;
        let digest = parse_manifest_digest(manifest_digest);
        let snap = self.snapshot(tx_id).ok_or(EngineError::NotFound)?;
        if snap.state == WireState::Probation {
            return Err(EngineError::WatchdogAuthorityRequired);
        }
        self.transition(
            agent_id,
            &digest,
            tx_id,
            TransactionState::Probation,
            None,
            None,
            None,
            None,
        )
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

    pub fn events_replay(&self, cursor: Option<&str>) -> Result<EventsReplayResult, EngineError> {
        if *self.safe_mode.lock().expect("safe") {
            return Err(EngineError::SafeMode);
        }
        let parsed = match cursor {
            Some(raw) if !raw.is_empty() => {
                EventCursor::parse(raw).map_err(|_| EngineError::NotFound)?
            }
            _ => EventCursor::after_seq(self.events.log_id(), 0),
        };
        let events = self
            .events
            .replay(&parsed)
            .map_err(|_| EngineError::NotFound)?;
        let next = self
            .events
            .latest_cursor()
            .map_err(|_| EngineError::NotFound)?;
        Ok(EventsReplayResult {
            log_id: self.events.log_id(),
            events: events
                .into_iter()
                .map(|event| StoredEventWire {
                    seq: event.seq,
                    kind: event.kind,
                    payload: event.payload,
                })
                .collect(),
            cursor: next.encode(),
        })
    }

    fn transition(
        &self,
        agent_id: &str,
        manifest_digest: &Digest,
        tx_id: &str,
        target: TransactionState,
        idempotency_key: Option<String>,
        idem_fingerprint: Option<String>,
        idem_store_key: Option<String>,
        idem_store_fingerprint: Option<String>,
    ) -> Result<TxStepResult, EngineError> {
        let snap = self.snapshot(tx_id).ok_or(EngineError::NotFound)?;
        ensure_owner(&snap, agent_id, manifest_digest)?;
        let from = TransactionState::from(snap.state);
        if !broker_may_enter(from, target) {
            return Err(EngineError::InvalidTransition);
        }
        let wire_target: WireState = target.into();
        let result = TxStepResult {
            tx_id: tx_id.to_owned(),
            state: wire_target,
        };
        let result_json = serde_json::to_string(&result).map_err(|_| EngineError::SafeMode)?;
        let seq = self.persist_transition(
            tx_id,
            wire_target,
            idempotency_key,
            idem_fingerprint,
            &snap.agent_id,
            snap.manifest_digest.clone(),
            snap.base_revision,
            snap.effect_set,
            snap.diff,
            snap.affected_resources,
            Some(result_json.clone()),
        )?;
        if let Err(err) = self.append_state_event(tx_id, wire_target) {
            self.rollback_transition(tx_id, seq)?;
            return Err(err);
        }
        if let (Some(key), Some(fingerprint)) = (idem_store_key, idem_store_fingerprint) {
            self.record_idempotency(IdempotencyRecord {
                key,
                tx_id: tx_id.to_owned(),
                fingerprint,
                result_json,
            })?;
        }
        Ok(result)
    }

    fn persist_transition(
        &self,
        tx_id: &str,
        state: WireState,
        idempotency_key: Option<String>,
        idem_fingerprint: Option<String>,
        agent_id: &str,
        manifest_digest: Digest,
        base_revision: BaseRevision,
        effect_set: Vec<EffectClass>,
        diff: String,
        affected_resources: Vec<String>,
        result_json: Option<String>,
    ) -> Result<u64, EngineError> {
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
            idem_fingerprint,
            agent_id: agent_id.to_owned(),
            manifest_digest: manifest_digest.clone(),
            base_revision: base_revision.clone(),
            effect_set: effect_set.clone(),
            diff: diff.clone(),
            affected_resources: affected_resources.clone(),
            approval_ref: None,
            result_json,
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
        Ok(seq)
    }

    fn rollback_transition(&self, tx_id: &str, seq: u64) -> Result<(), EngineError> {
        self.wal
            .lock()
            .expect("wal")
            .revert_last_transition(seq)
            .map_err(EngineError::Storage)?;
        self.txs.lock().expect("txs").remove(tx_id);
        *self.next_seq.lock().expect("seq") = seq.saturating_sub(1);
        self.enter_safe_mode();
        Ok(())
    }

    fn append_state_event(&self, tx_id: &str, state: WireState) -> Result<(), EngineError> {
        self.events
            .append(EventRecord {
                kind: "tx.state".to_owned(),
                payload: format!("{{\"tx_id\":\"{tx_id}\",\"state\":\"{state:?}\"}}"),
            })
            .map_err(|_| {
                self.enter_safe_mode();
                EngineError::SafeMode
            })?;
        Ok(())
    }

    fn record_idempotency(&self, record: IdempotencyRecord) -> Result<(), EngineError> {
        self.idempotency
            .insert(record)
            .map_err(EngineError::Storage)
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

    fn enter_safe_mode(&self) {
        *self.safe_mode.lock().expect("safe") = true;
        let mode_path = self.state_root.join("broker_mode.json");
        let _ = std::fs::write(
            mode_path,
            br#"{"mode":"safe_mode","reason":"durable_divergence"}"#,
        );
    }
}

fn ensure_owner(
    snap: &TxSnapshot,
    agent_id: &str,
    manifest_digest: &Digest,
) -> Result<(), EngineError> {
    if snap.agent_id != agent_id || snap.manifest_digest != *manifest_digest {
        return Err(EngineError::OwnershipMismatch);
    }
    Ok(())
}

fn parse_manifest_digest(manifest_digest: &str) -> Digest {
    serde_json::from_str(&format!("\"{manifest_digest}\""))
        .unwrap_or_else(|_| Digest::from_sha256_bytes([0; 32]))
}

fn replay_propose(entry: &IdempotencyRecord) -> Result<ConfigProposeOutcome, EngineError> {
    let result: ConfigProposeResult =
        serde_json::from_str(&entry.result_json).map_err(|_| EngineError::IdempotencyConflict)?;
    Ok(ConfigProposeOutcome {
        tx_id: result.tx_id,
        diff: result.diff,
        test_plan: result.test_plan,
        affected_resources: result.affected_resources,
        base_revision: result.base_revision,
        state: WireState::Proposed,
    })
}

fn replay_step(entry: &IdempotencyRecord) -> Result<TxStepResult, EngineError> {
    serde_json::from_str(&entry.result_json).map_err(|_| EngineError::IdempotencyConflict)
}

fn idem_key(agent_id: &str, op: &str, key: &str) -> String {
    format!("{agent_id}:{op}:{key}")
}

/// 19-char fixed prefix for hermetic ULID-shaped transaction ids in tests.
const TX_ID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ";

fn propose_fingerprint(params: &ConfigProposeParams) -> String {
    serde_json::to_string(params).unwrap_or_default()
}

fn apply_fingerprint(params: &TxApplyParams) -> String {
    serde_json::to_string(params).unwrap_or_default()
}

fn new_tx_id() -> TxId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let suffix = format!("{:07X}", n % 0x0FFF_FFFF);
    format!("{TX_ID_PREFIX}{suffix}")
}
