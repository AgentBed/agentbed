//! L01-AC02 / L01-AC03: WAL durability and failure injection.

#![allow(clippy::expect_used, clippy::unwrap_used, unused_variables)]

use agentbed_broker::storage::durability::{
    DurabilityError, FaultInjectedDurability, RealDurability,
};
use agentbed_broker::storage::wal::{WalRecord, WalStore};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::{BaseRevision, TransactionState};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb4-wal-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn sample_record(seq: u64, state: TransactionState) -> WalRecord {
    WalRecord {
        seq,
        tx_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
        state,
        idempotency_key: Some("idem-1".to_owned()),
        idem_fingerprint: None,
        agent_id: "agent:test".to_owned(),
        manifest_digest: Digest::from_sha256_bytes([0; 32]),
        base_revision: BaseRevision {
            generation: Some("gen-1".to_owned()),
            etc_git_commit: "abc".to_owned(),
            config_digest: Digest::from_sha256_bytes([0x11; 32]),
        },
        effect_set: vec![agentbed_protocol::wire::EffectClass::D],
        diff: "diff".to_owned(),
        affected_resources: vec!["root_config".to_owned()],
        approval_ref: None,
        result_json: None,
    }
}

#[test]
fn persist_before_transition_never_exposes_state_before_rename() {
    let dir = scratch();
    let durability = Arc::new(RealDurability);
    let store = WalStore::open(&dir, durability.clone()).expect("open");

    let fault = Arc::new(FaultInjectedDurability::new(durability));
    fault.fail_after_write_before_fsync(true);
    let mut store = WalStore::open(&dir, fault).expect("reopen");

    let err = store
        .append_transition(&sample_record(1, TransactionState::Proposed))
        .expect_err("fsync failure must propagate");
    assert!(matches!(err, DurabilityError::FsyncFailed));

    // Checkpoint must still reflect zero durable transitions.
    let recovered = WalStore::open(&dir, Arc::new(RealDurability)).expect("recover");
    assert_eq!(recovered.checkpoint_seq(), 0);
}

#[test]
fn atomic_rename_makes_record_visible_only_after_parent_fsync() {
    let dir = scratch();
    let durability = Arc::new(RealDurability);
    let mut store = WalStore::open(&dir, durability).expect("open");

    store
        .append_transition(&sample_record(1, TransactionState::Proposed))
        .expect("append");
    let recovered = WalStore::open(&dir, Arc::new(RealDurability)).expect("recover");
    assert_eq!(recovered.checkpoint_seq(), 1);
    let records = recovered.load_records().expect("load");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, TransactionState::Proposed);
}

#[test]
fn truncated_checkpoint_enters_safe_mode_on_recovery() {
    let dir = scratch();
    let durability = Arc::new(RealDurability);
    let mut store = WalStore::open(&dir, durability).expect("open");
    store
        .append_transition(&sample_record(1, TransactionState::Proposed))
        .expect("append");

    let checkpoint = dir.join("checkpoint.json");
    let mut bytes = std::fs::read(&checkpoint).expect("read");
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&checkpoint, bytes).expect("truncate");

    let recovered = WalStore::open(&dir, Arc::new(RealDurability)).expect("open");
    assert!(recovered.safe_mode());
}
