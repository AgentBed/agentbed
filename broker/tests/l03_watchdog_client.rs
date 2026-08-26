//! L03 RED — broker watchdog client boundary and WAL semantic regression.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::storage::wal::WalRecord;
use agentbed_broker::transaction::engine::{EngineError, TransactionEngine};
use agentbed_broker::transaction::recovery::validate_wal_semantics;
use agentbed_broker::watchdog::client::{WatchdogClient, WatchdogClientError};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::{BaseRevision, TransactionState};
use agentbed_protocol::wire::{ConfigFileChange, ConfigProposeParams, EffectClass, IdempotencyKey};
use agentbed_watchdogd::rpc::protocol::{LocalRequest, SessionBind, SessionEstablished};
use agentbed_watchdogd::WorkerGroupTag;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const CLIENT_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/watchdog/client.rs"
));

const TX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn fixture_worker_group_tag() -> WorkerGroupTag {
    WorkerGroupTag::try_from_raw(100).expect("valid broker test fixture tag")
}

fn scratch() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb8-l03-broker-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn idem(s: &str) -> IdempotencyKey {
    IdempotencyKey::new(s).expect("idempotency key")
}

fn wal_record(
    seq: u64,
    tx_id: &str,
    state: TransactionState,
    agent_id: &str,
    manifest_byte: u8,
) -> WalRecord {
    WalRecord {
        record_version: 1,
        seq,
        tx_id: tx_id.to_owned(),
        state,
        idempotency_key: Some(format!("k-{seq}")),
        idem_fingerprint: None,
        agent_id: agent_id.to_owned(),
        manifest_digest: Digest::from_sha256_bytes([manifest_byte; 32]),
        base_revision: BaseRevision {
            generation: Some("gen-1".to_owned()),
            etc_git_commit: "abc".to_owned(),
            config_digest: Digest::from_sha256_bytes([0x11; 32]),
        },
        effect_set: vec![EffectClass::D],
        diff: "diff".to_owned(),
        affected_resources: vec!["root_config".to_owned()],
        approval_ref: None,
        result_json: None,
    }
}

fn append_wal_records(dir: &Path, records: &[WalRecord]) {
    use agentbed_broker::storage::durability::RealDurability;
    use agentbed_broker::storage::wal::WalStore;
    use std::sync::Arc;
    let durability = Arc::new(RealDurability);
    let mut store = WalStore::open(dir.join("wal"), durability).expect("wal");
    for record in records {
        store.append_transition(record).expect("append");
    }
}

fn assert_dm_refused_in_safe_mode(dir: &Path) {
    let engine = TransactionEngine::open(dir, UnresolvedAdapter).expect("open");
    let err = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &ConfigProposeParams {
                idempotency_key: idem("l03-safe-mode"),
                changes: vec![ConfigFileChange {
                    path: "/etc/nixos/demo.nix".to_owned(),
                    content: "{}".to_owned(),
                }],
            },
        )
        .expect_err("safe mode");
    assert!(matches!(err, EngineError::SafeMode));
}

fn arm_request() -> LocalRequest {
    LocalRequest::arm(
        "req-arm-1",
        "host-test",
        TX,
        1,
        "base-a",
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        vec!["route_present".to_owned()],
        vec![],
    )
}

// --- L03-AC04 broker client boundary ---

#[test]
fn l03_ac04_client_source_excludes_forbidden_request_kinds() {
    assert!(!CLIENT_SRC.contains("Disarm"));
    assert!(!CLIENT_SRC.contains("OobHandshake"));
}

#[test]
fn l03_ac04_client_bootstrap_before_authority_requests() {
    let client = WatchdogClient::new("/tmp/agentbed-watchdog-test.sock");
    let bind = SessionBind::new(
        "host-test",
        TX,
        1,
        "lease1",
        fixture_worker_group_tag(),
        "client-nonce",
    );
    let err = client.bootstrap_session(&bind).expect_err("no server");
    assert!(matches!(
        err,
        WatchdogClientError::Transport(_) | WatchdogClientError::Bootstrap(_)
    ));
}

#[test]
fn l03_ac04_client_encodes_authenticated_arm_via_established_session() {
    let client = WatchdogClient::new("/tmp/agentbed-watchdog-test.sock");
    let established = SessionEstablished {
        capability: vec![0x11; 32],
        server_nonce: "server-nonce-red".to_owned(),
        host_id: "host-test".to_owned(),
        tx_id: TX.to_owned(),
        epoch: 1,
        counter: 0,
    };
    let request = arm_request();
    let counter = 1u64;
    let frame = client
        .encode_authenticated_request(&request, &established, counter)
        .expect("encode");
    assert!(!frame.is_empty());
}

#[test]
fn l03_ac04_client_propagates_fail_closed_transport_errors() {
    let client = WatchdogClient::new("/tmp/agentbed-watchdog-missing.sock");
    let bind = SessionBind::new(
        "host-test",
        TX,
        1,
        "lease1",
        fixture_worker_group_tag(),
        "client-nonce",
    );
    let err = client.bootstrap_session(&bind).expect_err("missing socket");
    assert!(matches!(
        err,
        WatchdogClientError::Transport(_) | WatchdogClientError::Bootstrap(_)
    ));
}

// --- L03-AC06 WAL semantic validation (non-regression) ---

#[test]
fn l03_ac06_validate_wal_semantics_rejects_watchdog_committed() {
    let records = vec![
        wal_record(1, TX, TransactionState::Proposed, "agent:a", 0),
        wal_record(2, TX, TransactionState::Committed, "agent:a", 0),
    ];
    assert!(!validate_wal_semantics(&records));
}

#[test]
fn l03_ac06_validate_wal_semantics_rejects_watchdog_committing() {
    let records = vec![
        wal_record(1, TX, TransactionState::Proposed, "agent:a", 0),
        wal_record(2, TX, TransactionState::Committing, "agent:a", 0),
    ];
    assert!(!validate_wal_semantics(&records));
}

#[test]
fn l03_ac06_validate_wal_semantics_rejects_watchdog_probation_passed() {
    let records = vec![
        wal_record(1, TX, TransactionState::Proposed, "agent:a", 0),
        wal_record(2, TX, TransactionState::ProbationPassed, "agent:a", 0),
    ];
    assert!(!validate_wal_semantics(&records));
}

#[test]
fn l03_ac06_committed_wal_engine_open_refuses_dm() {
    let dir = scratch();
    append_wal_records(
        &dir,
        &[
            wal_record(1, TX, TransactionState::Proposed, "agent:a", 0),
            wal_record(2, TX, TransactionState::Committed, "agent:a", 0),
        ],
    );
    assert_dm_refused_in_safe_mode(&dir);
}

// --- L03-AC02 source boundary ---

#[test]
fn l03_ac02_broker_client_source_has_no_append_or_choice_symbols() {
    assert!(!CLIENT_SRC.contains("append_decision"));
    assert!(!CLIENT_SRC.contains("append_authority"));
    assert!(!CLIENT_SRC.contains("choose_begin"));
    assert!(!CLIENT_SRC.contains("append_record"));
    assert!(!CLIENT_SRC.contains("AuthorityRecordKind"));
    assert!(!CLIENT_SRC.contains("Disarm"));
}
