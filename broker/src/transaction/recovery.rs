//! WAL semantic validation during broker recovery.

use crate::storage::wal::WalRecord;
use crate::transaction::state::{broker_may_enter, is_watchdog_owned, TransactionState, WireState};
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
