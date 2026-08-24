//! Broker WAL persistence and recovery.

#![allow(clippy::expect_used, missing_debug_implementations)]

use crate::storage::durability::{DurabilityError, DurabilityOps};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::{BaseRevision, TransactionState};
use agentbed_protocol::wire::EffectClass;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One durable WAL record written before a visible state entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalRecord {
    pub seq: u64,
    pub tx_id: String,
    pub state: TransactionState,
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub idem_fingerprint: Option<String>,
    pub agent_id: String,
    pub manifest_digest: Digest,
    pub base_revision: BaseRevision,
    pub effect_set: Vec<EffectClass>,
    pub diff: String,
    pub affected_resources: Vec<String>,
    pub approval_ref: Option<String>,
    pub result_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Checkpoint {
    seq: u64,
}

/// Recovery outcome from WAL open.
#[derive(Debug)]
pub struct WalRecovery {
    pub safe_mode: bool,
    pub records: Vec<WalRecord>,
}

/// On-disk WAL store with safe-mode recovery.
pub struct WalStore {
    root: PathBuf,
    durability: Arc<dyn DurabilityOps>,
    safe_mode: bool,
    checkpoint_seq: u64,
}

impl WalStore {
    /// Open or recover a WAL directory.
    pub fn open(
        root: impl AsRef<Path>,
        durability: Arc<dyn DurabilityOps>,
    ) -> Result<Self, DurabilityError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(root.join("records"))?;

        let checkpoint_path = root.join("checkpoint.json");
        let (safe_mode, checkpoint_seq) = if checkpoint_path.exists() {
            match std::fs::read_to_string(&checkpoint_path) {
                Ok(text) => match serde_json::from_str::<Checkpoint>(&text) {
                    Ok(cp) => (false, cp.seq),
                    Err(_) => (true, 0),
                },
                Err(_) => (true, 0),
            }
        } else {
            (false, 0)
        };

        Ok(Self {
            root,
            durability,
            safe_mode,
            checkpoint_seq,
        })
    }

    #[must_use]
    pub fn safe_mode(&self) -> bool {
        self.safe_mode
    }

    #[must_use]
    pub fn checkpoint_seq(&self) -> u64 {
        self.checkpoint_seq
    }

    /// Recover WAL records, entering safe mode on any ambiguous or corrupt state.
    #[must_use]
    pub fn recover(&self) -> WalRecovery {
        if self.safe_mode {
            return WalRecovery {
                safe_mode: true,
                records: Vec::new(),
            };
        }
        let records_dir = self.root.join("records");
        if has_ambiguous_temp_files(&records_dir) {
            return WalRecovery {
                safe_mode: true,
                records: Vec::new(),
            };
        }
        let Ok(records) = self.load_records() else {
            return WalRecovery {
                safe_mode: true,
                records: Vec::new(),
            };
        };
        if !records_consistent_with_checkpoint(&records, self.checkpoint_seq) {
            return WalRecovery {
                safe_mode: true,
                records: Vec::new(),
            };
        }
        WalRecovery {
            safe_mode: false,
            records,
        }
    }

    pub fn load_records(&self) -> Result<Vec<WalRecord>, DurabilityError> {
        if self.safe_mode {
            return Err(DurabilityError::FsyncFailed);
        }
        let mut records = Vec::new();
        let records_dir = self.root.join("records");
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&records_dir)
            .map_err(|_| DurabilityError::Io)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort_by_key(|path| wal_record_seq(path).unwrap_or(u64::MAX));
        for path in paths {
            let text = std::fs::read_to_string(&path).map_err(|_| DurabilityError::Io)?;
            let record: WalRecord =
                serde_json::from_str(&text).map_err(|_| DurabilityError::FsyncFailed)?;
            records.push(record);
        }
        Ok(records)
    }

    pub fn append_transition(&mut self, record: &WalRecord) -> Result<(), DurabilityError> {
        if self.safe_mode {
            return Err(DurabilityError::FsyncFailed);
        }
        let seq = record.seq;
        let temp = self.root.join("records").join(format!("{seq}.json.tmp"));
        let final_path = self.root.join("records").join(format!("{seq}.json"));
        let bytes = serde_json::to_vec(record).map_err(|_| DurabilityError::FsyncFailed)?;
        self.durability.write_all_and_sync(&temp, &bytes)?;
        self.durability.atomic_rename(&temp, &final_path)?;

        let checkpoint = Checkpoint { seq };
        let checkpoint_path = self.root.join("checkpoint.json");
        let checkpoint_temp = self.root.join("checkpoint.json.tmp");
        let cp_bytes = serde_json::to_vec(&checkpoint).map_err(|_| DurabilityError::FsyncFailed)?;
        self.durability
            .write_all_and_sync(&checkpoint_temp, &cp_bytes)?;
        self.durability
            .atomic_rename(&checkpoint_temp, &checkpoint_path)?;
        self.checkpoint_seq = seq;
        Ok(())
    }

    pub fn revert_last_transition(&mut self, seq: u64) -> Result<(), DurabilityError> {
        let record_path = self.root.join("records").join(format!("{seq}.json"));
        if record_path.exists() {
            std::fs::remove_file(&record_path).map_err(|_| DurabilityError::Io)?;
        }
        let prev = seq.saturating_sub(1);
        let checkpoint_path = self.root.join("checkpoint.json");
        if prev == 0 {
            let _ = std::fs::remove_file(&checkpoint_path);
            self.checkpoint_seq = 0;
            return Ok(());
        }
        let checkpoint = Checkpoint { seq: prev };
        let checkpoint_temp = self.root.join("checkpoint.json.tmp");
        let cp_bytes = serde_json::to_vec(&checkpoint).map_err(|_| DurabilityError::FsyncFailed)?;
        self.durability
            .write_all_and_sync(&checkpoint_temp, &cp_bytes)?;
        self.durability
            .atomic_rename(&checkpoint_temp, &checkpoint_path)?;
        self.checkpoint_seq = prev;
        Ok(())
    }
}

fn wal_record_seq(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.parse().ok()
}

fn has_ambiguous_temp_files(records_dir: &Path) -> bool {
    let Ok(read_dir) = std::fs::read_dir(records_dir) else {
        return true;
    };
    for entry in read_dir.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "tmp") {
            return true;
        }
    }
    false
}

fn records_consistent_with_checkpoint(records: &[WalRecord], checkpoint_seq: u64) -> bool {
    let max_seq = records.iter().map(|r| r.seq).max().unwrap_or(0);
    if max_seq != checkpoint_seq {
        return false;
    }
    for (idx, record) in records.iter().enumerate() {
        let expected = u64::try_from(idx).unwrap_or(u64::MAX).saturating_add(1);
        if record.seq != expected {
            return false;
        }
    }
    true
}
