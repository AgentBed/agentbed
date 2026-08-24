//! Durable idempotency bindings for D/M operations.

#![allow(clippy::expect_used, missing_debug_implementations)]

use crate::storage::durability::{DurabilityError, DurabilityOps, RealDurability};
use crate::storage::wal::WalRecord;
use agentbed_protocol::dto::transaction::TransactionState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One durable idempotency binding with the original serialized result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdempotencyRecord {
    pub key: String,
    pub tx_id: String,
    pub fingerprint: String,
    pub result_json: String,
}

/// On-disk idempotency index (`{state_dir}/idempotency/`).
pub struct IdempotencyStore {
    root: PathBuf,
    durability: Arc<dyn DurabilityOps>,
    entries: Mutex<HashMap<String, IdempotencyRecord>>,
}

impl IdempotencyStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, DurabilityError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let durability: Arc<dyn DurabilityOps> = Arc::new(RealDurability);
        let mut entries = HashMap::new();
        if let Ok(read_dir) = std::fs::read_dir(&root) {
            for entry in read_dir.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    let text = std::fs::read_to_string(&path).map_err(|_| DurabilityError::Io)?;
                    let record: IdempotencyRecord =
                        serde_json::from_str(&text).map_err(|_| DurabilityError::FsyncFailed)?;
                    entries.insert(record.key.clone(), record);
                }
            }
        }
        Ok(Self {
            root,
            durability,
            entries: Mutex::new(entries),
        })
    }

    pub fn get(&self, key: &str) -> Option<IdempotencyRecord> {
        self.entries.lock().expect("idem").get(key).cloned()
    }

    pub fn insert(&self, record: IdempotencyRecord) -> Result<(), DurabilityError> {
        let filename = format!("{:016x}.json", hash_key(&record.key));
        let temp = self.root.join(format!("{filename}.tmp"));
        let final_path = self.root.join(filename);
        let bytes = serde_json::to_vec(&record).map_err(|_| DurabilityError::FsyncFailed)?;
        self.durability.write_all_and_sync(&temp, &bytes)?;
        self.durability.atomic_rename(&temp, &final_path)?;
        self.entries
            .lock()
            .expect("idem")
            .insert(record.key.clone(), record);
        Ok(())
    }

    pub fn merge_from_wal(&self, records: &[WalRecord]) {
        let mut map = self.entries.lock().expect("idem");
        for record in records {
            let Some(key) = record.idempotency_key.as_ref() else {
                continue;
            };
            let Some(result_json) = record.result_json.as_ref() else {
                continue;
            };
            let Some(op) = op_for_state(record.state) else {
                continue;
            };
            let binding_key = format!("{}:{op}:{key}", record.agent_id);
            let fingerprint = record
                .idem_fingerprint
                .clone()
                .unwrap_or_else(|| record.tx_id.clone());
            map.entry(binding_key.clone())
                .or_insert_with(|| IdempotencyRecord {
                    key: binding_key,
                    tx_id: record.tx_id.clone(),
                    fingerprint,
                    result_json: result_json.clone(),
                });
        }
    }
}

fn op_for_state(state: TransactionState) -> Option<&'static str> {
    match state {
        TransactionState::Proposed => Some("config.propose"),
        TransactionState::Applying => Some("tx.apply"),
        _ => None,
    }
}

fn hash_key(key: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}
