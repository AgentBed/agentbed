//! WAL semantic validation during broker recovery.

use crate::events::StoredEvent;
use crate::storage::wal::WalRecord;
use crate::transaction::state::{broker_may_enter, is_watchdog_owned, TransactionState, WireState};
use agentbed_protocol::dto::transaction::TransactionState as ProtoState;
use std::collections::HashMap;

const SUPPORTED_RECORD_VERSION: u32 = 1;

/// Validate broker-owned WAL semantics for every transaction chain.
#[must_use]
pub fn validate_wal_semantics(records: &[WalRecord]) -> bool {
    let mut chains: HashMap<String, Vec<&WalRecord>> = HashMap::new();
    for record in records {
        if record.record_version != SUPPORTED_RECORD_VERSION {
            return false;
        }
        if is_watchdog_owned(record.state) {
            return false;
        }
        chains.entry(record.tx_id.clone()).or_default().push(record);
    }

    for chain in chains.values_mut() {
        chain.sort_by_key(|record| record.seq);
        let Some(first) = chain.first() else {
            continue;
        };
        if first.state != WireState::Proposed {
            return false;
        }
        for window in chain.windows(2) {
            let Some(prev) = window.first() else {
                return false;
            };
            let Some(next) = window.get(1) else {
                return false;
            };
            if prev.agent_id != next.agent_id
                || prev.manifest_digest != next.manifest_digest
                || prev.base_revision != next.base_revision
                || prev.effect_set != next.effect_set
                || prev.diff != next.diff
                || prev.affected_resources != next.affected_resources
                || prev.approval_ref != next.approval_ref
            {
                return false;
            }
            let from = TransactionState::from(prev.state);
            let to = TransactionState::from(next.state);
            if !broker_may_enter(from, to) {
                return false;
            }
        }
    }

    true
}

/// Cross-validate `tx.state` events against authoritative WAL transition history.
///
/// WAL is authoritative: events may lag WAL when secondary idempotency persistence
/// fails after WAL (+ optional event) durability. Extra event states without a
/// matching WAL transition are divergence and fail closed.
#[must_use]
pub fn validate_tx_state_events_against_wal(records: &[WalRecord], events: &[StoredEvent]) -> bool {
    let wal_states = wal_state_sequences(records);
    let Some(event_states) = tx_state_event_sequences(events) else {
        return false;
    };
    for (tx_id, ev_seq) in &event_states {
        let wal_seq = wal_states
            .get(tx_id)
            .map_or(&[] as &[ProtoState], |v| v.as_slice());
        if ev_seq.len() > wal_seq.len() {
            return false;
        }
        let Some(wal_prefix) = wal_seq.get(..ev_seq.len()) else {
            return false;
        };
        if ev_seq != wal_prefix {
            return false;
        }
    }
    true
}

fn wal_state_sequences(records: &[WalRecord]) -> HashMap<String, Vec<ProtoState>> {
    let mut chains: HashMap<String, Vec<&WalRecord>> = HashMap::new();
    for record in records {
        chains.entry(record.tx_id.clone()).or_default().push(record);
    }
    chains
        .into_iter()
        .map(|(tx_id, chain)| {
            let mut sorted = chain;
            sorted.sort_by_key(|record| record.seq);
            let states = sorted.iter().map(|record| record.state).collect();
            (tx_id, states)
        })
        .collect()
}

fn tx_state_event_sequences(events: &[StoredEvent]) -> Option<HashMap<String, Vec<ProtoState>>> {
    let mut out: HashMap<String, Vec<ProtoState>> = HashMap::new();
    for event in events {
        if event.kind != "tx.state" {
            continue;
        }
        let parsed = parse_tx_state_event_payload(&event.payload)?;
        out.entry(parsed.tx_id).or_default().push(parsed.state);
    }
    Some(out)
}

fn parse_tx_state_event_payload(payload: &str) -> Option<TxStateEventPayload> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let obj = value.as_object()?;
    if obj.len() != 2 {
        return None;
    }
    let tx_id = obj.get("tx_id").and_then(|field| field.as_str())?;
    if tx_id.is_empty() {
        return None;
    }
    let state_str = obj.get("state").and_then(|field| field.as_str())?;
    let state = proto_state_from_debug_name(state_str)?;
    Some(TxStateEventPayload {
        tx_id: tx_id.to_owned(),
        state,
    })
}

fn proto_state_from_debug_name(name: &str) -> Option<ProtoState> {
    match name {
        "Idle" => Some(ProtoState::Idle),
        "Proposed" => Some(ProtoState::Proposed),
        "Testing" => Some(ProtoState::Testing),
        "Applying" => Some(ProtoState::Applying),
        "Probation" => Some(ProtoState::Probation),
        "ProbationPassed" => Some(ProtoState::ProbationPassed),
        "Committing" => Some(ProtoState::Committing),
        "Committed" => Some(ProtoState::Committed),
        "Rejected" => Some(ProtoState::Rejected),
        "Reverting" => Some(ProtoState::Reverting),
        "Reverted" => Some(ProtoState::Reverted),
        _ => None,
    }
}

struct TxStateEventPayload {
    tx_id: String,
    state: ProtoState,
}
