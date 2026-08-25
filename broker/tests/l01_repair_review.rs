//! L01 repair tests for native review #5010391942 (PR #20 @ daaaae0).
//! Review #5011747127 additions cover immutable WAL payload drift, conflicting
//! `config.propose` retry after idempotency-index faults, and moved-base REJECTED
//! idempotency ordering. Review #5013663187 adds crash-before-retry moved-base
//! consistency, append-only WAL history, and orphan-event divergence fail-closed.
//! `WalRecord::result_json` is transition-mutable (each
//! record carries that transition's serialized outcome) and is excluded from
//! cross-record immutability checks alongside `seq`, `state`, `idempotency_key`,
//! and `idem_fingerprint`, which are per-transition metadata rather than tx identity.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::events::{EventCursor, EventLog, EventRecord, StoredEvent};
use agentbed_broker::storage::durability::RealDurability;
use agentbed_broker::storage::wal::{WalRecord, WalStore};
use agentbed_broker::transaction::engine::{EngineError, TransactionEngine};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::{BaseRevision, TransactionState};
use agentbed_protocol::wire::{
    ConfigFileChange, ConfigProposeParams, EffectClass, IdempotencyKey, TransactionId,
    TxApplyParams, TxTestParams,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb4-repair-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn idem(s: &str) -> IdempotencyKey {
    IdempotencyKey::new(s).expect("idempotency key")
}

fn tx(s: &str) -> TransactionId {
    TransactionId::new(s).expect("tx id")
}

fn propose_params(key: &str, path: &str) -> ConfigProposeParams {
    ConfigProposeParams {
        idempotency_key: idem(key),
        changes: vec![ConfigFileChange {
            path: path.to_owned(),
            content: "{ }".to_owned(),
        }],
    }
}

fn wal_record_count(state_dir: &Path) -> usize {
    let records_dir = state_dir.join("wal/records");
    std::fs::read_dir(records_dir)
        .expect("records dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .count()
}

fn event_count(state_dir: &Path) -> usize {
    let log = state_dir.join("events/log.jsonl");
    if !log.exists() {
        return 0;
    }
    std::fs::read_to_string(log)
        .expect("log")
        .lines()
        .filter(|line| !line.is_empty())
        .count()
}

struct MovedBaseAdapter;

impl agentbed_broker::adapter::HostAdapter for MovedBaseAdapter {
    fn info(&self) -> agentbed_protocol::dto::system_info::AdapterInfo {
        UnresolvedAdapter.info()
    }

    fn safety_vector(&self) -> agentbed_protocol::dto::system_info::SafetyVector {
        UnresolvedAdapter.safety_vector()
    }

    fn safety_source(&self) -> agentbed_protocol::dto::system_info::SafetySource {
        UnresolvedAdapter.safety_source()
    }

    fn current_base_revision(&self) -> BaseRevision {
        BaseRevision {
            generation: Some("gen-moved".to_owned()),
            etc_git_commit: "deadbeef".to_owned(),
            config_digest: Digest::from_sha256_bytes([0x33; 32]),
        }
    }
}

#[test]
fn post_restart_config_propose_replay_returns_original_without_duplicate_activity() {
    let dir = scratch();
    let params = propose_params("repair-propose-1", "/etc/nixos/configuration.nix");
    let (first_tx, wal_after_first) = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        let first = engine
            .config_propose("agent:a", "sha256:abc", &params)
            .expect("propose");
        (first.tx_id, wal_record_count(&dir))
    };

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let replay = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("replay");
    assert_eq!(replay.tx_id, first_tx);
    assert_eq!(wal_record_count(&dir), wal_after_first);
    assert_eq!(event_count(&dir), wal_after_first);
}

#[test]
fn post_restart_tx_apply_replay_returns_original_without_duplicate_activity() {
    let dir = scratch();
    let propose = propose_params("repair-apply-propose", "/etc/nixos/configuration.nix");
    let apply_key = idem("repair-apply-1");
    let (tx_id, wal_after_apply) = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        let proposed = engine
            .config_propose("agent:a", "sha256:abc", &propose)
            .expect("propose");
        engine
            .tx_test(
                "agent:a",
                "sha256:abc",
                &TxTestParams {
                    tx_id: tx(&proposed.tx_id),
                },
            )
            .expect("test");
        engine
            .tx_apply(
                "agent:a",
                "sha256:abc",
                &TxApplyParams {
                    tx_id: tx(&proposed.tx_id),
                    idempotency_key: apply_key.clone(),
                },
            )
            .expect("apply");
        (proposed.tx_id, wal_record_count(&dir))
    };

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let replay = engine
        .tx_apply(
            "agent:a",
            "sha256:abc",
            &TxApplyParams {
                tx_id: tx(&tx_id),
                idempotency_key: apply_key,
            },
        )
        .expect("replay apply");
    assert_eq!(replay.tx_id, tx_id);
    assert_eq!(replay.state, TransactionState::Applying);
    assert_eq!(wal_record_count(&dir), wal_after_apply);
}

#[test]
fn conflicting_idempotency_reuse_fails_closed_after_restart() {
    let dir = scratch();
    let key = "repair-conflict";
    {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &propose_params(key, "/etc/nixos/a.nix"),
            )
            .expect("first");
    }
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let err = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params(key, "/etc/nixos/b.nix"),
        )
        .expect_err("conflict");
    assert!(matches!(err, EngineError::IdempotencyConflict));
}

#[test]
fn orphan_wal_temp_enters_safe_mode_and_refuses_dm() {
    let dir = scratch();
    {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &propose_params("orphan-temp", "/etc/nixos/configuration.nix"),
            )
            .expect("propose");
    }
    std::fs::write(
        dir.join("wal/records/2.json.tmp"),
        br#"{"seq":2,"tx_id":"01ARZ3NDEKTSV4RRFFQ0000002"}"#,
    )
    .expect("orphan temp");

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let err = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("after-orphan", "/etc/nixos/configuration.nix"),
        )
        .expect_err("safe mode");
    assert!(matches!(err, EngineError::SafeMode));
}

#[test]
fn checkpoint_seq_mismatch_enters_safe_mode_and_refuses_dm() {
    let dir = scratch();
    let wal_dir = dir.join("wal");
    {
        let durability = Arc::new(RealDurability);
        let mut store = WalStore::open(&wal_dir, durability).expect("wal");
        store
            .append_transition(&WalRecord {
                record_version: 1,
                seq: 1,
                tx_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
                state: TransactionState::Proposed,
                idempotency_key: Some("k".to_owned()),
                idem_fingerprint: None,
                agent_id: "agent:a".to_owned(),
                manifest_digest: Digest::from_sha256_bytes([0; 32]),
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
            })
            .expect("append");
    }
    std::fs::write(wal_dir.join("checkpoint.json"), br#"{"seq":99}"#).expect("bad checkpoint");

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let err = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("after-mismatch", "/etc/nixos/configuration.nix"),
        )
        .expect_err("safe mode");
    assert!(matches!(err, EngineError::SafeMode));
}

#[test]
fn corrupt_event_log_enters_safe_mode_and_refuses_dm() {
    let dir = scratch();
    {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &propose_params("corrupt-events", "/etc/nixos/configuration.nix"),
            )
            .expect("propose");
    }
    let log_path = dir.join("events/log.jsonl");
    let mut bytes = std::fs::read(&log_path).expect("read log");
    bytes.truncate(bytes.len().saturating_sub(4));
    std::fs::write(&log_path, bytes).expect("truncate log");

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let err = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("after-corrupt-events", "/etc/nixos/configuration.nix"),
        )
        .expect_err("safe mode");
    assert!(matches!(err, EngineError::SafeMode));
}

#[test]
fn concurrent_same_idempotency_key_serializes_to_one_transaction() {
    let dir = scratch();
    let engine = Arc::new(TransactionEngine::open(&dir, UnresolvedAdapter).expect("open"));
    let barrier = Arc::new(Barrier::new(2));
    let params = propose_params("repair-race", "/etc/nixos/configuration.nix");
    let mut handles = Vec::new();
    for _ in 0..2 {
        let engine = Arc::clone(&engine);
        let barrier = Arc::clone(&barrier);
        let params = params.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            engine
                .config_propose("agent:a", "sha256:abc", &params)
                .expect("propose")
                .tx_id
        }));
    }
    let ids: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("join"))
        .collect();
    assert_eq!(ids[0], ids[1]);
    assert_eq!(wal_record_count(&dir), 1);
}

#[test]
fn event_append_failure_enters_safe_mode_without_exposing_transition() {
    let dir = scratch();
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
    engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("event-fail", "/etc/nixos/configuration.nix"),
        )
        .expect("first propose");
    let wal_before = wal_record_count(&dir);

    let log_path = dir.join("events/log.jsonl");
    let mut perms = std::fs::metadata(&log_path).expect("meta").permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&log_path, perms).expect("readonly log");

    let propose = propose_params("event-fail-2", "/etc/nixos/other.nix");
    let err = engine
        .config_propose("agent:a", "sha256:abc", &propose)
        .expect_err("append failure must fail closed");
    assert!(matches!(err, EngineError::SafeMode));
    assert_eq!(wal_record_count(&dir), wal_before);
}

#[test]
fn events_cursor_replay_survives_restart_without_loss_or_duplication() {
    let dir = scratch().join("events-only");
    std::fs::create_dir_all(&dir).expect("dir");
    let cursor = {
        let log = EventLog::open(&dir).expect("open");
        let first = log
            .append(EventRecord {
                kind: "boot".to_owned(),
                payload: "{}".to_owned(),
            })
            .expect("append");
        EventCursor::after(&first).with_log_id(log.log_id())
    };
    {
        let log = EventLog::open(&dir).expect("reopen");
        let _ = log
            .append(EventRecord {
                kind: "tx.state".to_owned(),
                payload: "{}".to_owned(),
            })
            .expect("append");
    }
    let log = EventLog::open(&dir).expect("final");
    let first = log.replay(&cursor).expect("first");
    let second = log.replay(&cursor).expect("second");
    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
}

#[test]
fn foreign_agent_cannot_advance_another_agents_transaction() {
    let dir = scratch();
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
    let propose = engine
        .config_propose(
            "agent:a",
            "sha256:aaa",
            &propose_params("owner-bind", "/etc/nixos/configuration.nix"),
        )
        .expect("propose");

    let err = engine
        .tx_test(
            "agent:b",
            "sha256:bbb",
            &TxTestParams {
                tx_id: tx(&propose.tx_id),
            },
        )
        .expect_err("foreign agent");
    assert!(matches!(err, EngineError::OwnershipMismatch));

    let status = engine.tx_status(&propose.tx_id).expect("status");
    assert_eq!(status.state, TransactionState::Proposed);
}

#[test]
fn moved_base_refusal_is_recorded_and_survives_restart() {
    let dir = scratch();
    let propose = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        let propose = engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &propose_params("moved-base", "/etc/nixos/configuration.nix"),
            )
            .expect("propose");
        engine
            .tx_test(
                "agent:a",
                "sha256:abc",
                &TxTestParams {
                    tx_id: tx(&propose.tx_id),
                },
            )
            .expect("test");
        propose
    };

    let wal_before = wal_record_count(&dir);
    let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("reopen");
    let err = engine
        .tx_apply(
            "agent:a",
            "sha256:abc",
            &TxApplyParams {
                tx_id: tx(&propose.tx_id),
                idempotency_key: idem("moved-apply"),
            },
        )
        .expect_err("moved base");
    assert!(matches!(err, EngineError::BaseRevisionMoved));
    assert!(wal_record_count(&dir) > wal_before);

    let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("final");
    let status = engine.tx_status(&propose.tx_id).expect("status");
    assert_eq!(status.state, TransactionState::Rejected);
}

#[test]
fn wal_recovery_with_ten_plus_records_stays_operational() {
    let dir = scratch();
    let durability = Arc::new(RealDurability);
    let wal_dir = dir.join("wal");
    {
        let mut store = WalStore::open(&wal_dir, durability.clone()).expect("open");
        for seq in 1..=12 {
            store
                .append_transition(&WalRecord {
                    record_version: 1,
                    seq,
                    tx_id: format!("01ARZ3NDEKTSV4RRFFQ{seq:07X}"),
                    state: TransactionState::Proposed,
                    idempotency_key: Some(format!("k-{seq}")),
                    idem_fingerprint: None,
                    agent_id: "agent:a".to_owned(),
                    manifest_digest: Digest::from_sha256_bytes([0; 32]),
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
                })
                .expect("append");
        }
    }

    let recovered = WalStore::open(&wal_dir, durability).expect("recover");
    assert!(!recovered.recover().safe_mode);
    let records = recovered.load_records().expect("load");
    assert_eq!(records.len(), 12);
    assert_eq!(records.last().map(|r| r.seq), Some(12));

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("engine");
    engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("after-12", "/etc/nixos/configuration.nix"),
        )
        .expect("dm after long wal");
}

#[test]
fn event_cursor_encodes_as_opaque_base64url() {
    let dir = scratch();
    let log = EventLog::open(&dir).expect("open");
    let stored = log
        .append(EventRecord {
            kind: "boot".to_owned(),
            payload: "{}".to_owned(),
        })
        .expect("append");
    let cursor = log.cursor_after(&stored);
    let encoded = cursor.encode();
    assert!(!encoded.starts_with('{'));
    let parsed = EventCursor::parse(&encoded).expect("parse opaque cursor");
    assert_eq!(parsed, cursor.with_log_id(log.log_id()));
}

#[test]
fn transaction_ids_are_not_static_process_counter_values() {
    let dir = scratch();
    let tx_id = TransactionEngine::open(&dir, UnresolvedAdapter)
        .expect("open")
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("ulid-not-counter", "/etc/nixos/configuration.nix"),
        )
        .expect("propose")
        .tx_id;
    assert!(
        !tx_id.starts_with("01ARZ3NDEKTSV4RRFFQ"),
        "transaction ids must not reuse a fixed restart-reset counter prefix"
    );
}

#[test]
fn post_restart_recovered_transaction_is_not_overwritten_by_new_propose() {
    let dir = scratch();
    let first_tx = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &propose_params("restart-tx-1", "/etc/nixos/a.nix"),
            )
            .expect("first")
            .tx_id
    };

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let second_tx = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("restart-tx-2", "/etc/nixos/b.nix"),
        )
        .expect("second")
        .tx_id;

    assert_ne!(first_tx, second_tx);
    assert!(engine.tx_status(&first_tx).is_ok());
    assert!(engine.tx_status(&second_tx).is_ok());
}

#[test]
fn duplicate_tx_id_in_wal_enters_safe_mode() {
    let dir = scratch();
    let wal_dir = dir.join("wal");
    let durability = Arc::new(RealDurability);
    let shared_tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    {
        let mut store = WalStore::open(&wal_dir, durability.clone()).expect("open");
        for seq in 1..=2 {
            store
                .append_transition(&WalRecord {
                    record_version: 1,
                    seq,
                    tx_id: shared_tx.to_owned(),
                    state: TransactionState::Proposed,
                    idempotency_key: Some(format!("dup-{seq}")),
                    idem_fingerprint: None,
                    agent_id: "agent:a".to_owned(),
                    manifest_digest: Digest::from_sha256_bytes([0; 32]),
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
                })
                .expect("append");
        }
    }

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let err = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("after-dup", "/etc/nixos/configuration.nix"),
        )
        .expect_err("safe mode");
    assert!(matches!(err, EngineError::SafeMode));
}

#[test]
fn stale_event_cursor_is_rejected_after_log_recreation() {
    let dir = scratch();
    let stale_cursor = {
        let log = EventLog::open(&dir).expect("open");
        assert!(uuid::Uuid::parse_str(&log.log_id()).is_ok());
        let event = log
            .append(EventRecord {
                kind: "boot".to_owned(),
                payload: "{}".to_owned(),
            })
            .expect("append");
        log.cursor_after(&event).encode()
    };

    std::fs::remove_file(dir.join("meta.json")).expect("remove meta");
    std::fs::remove_file(dir.join("log.jsonl")).expect("remove log");

    let log = EventLog::open(&dir).expect("recreated");
    let parsed = EventCursor::parse(&stale_cursor).expect("parse stale");
    assert!(log.replay(&parsed).is_err());
}

#[test]
fn cursor_without_log_id_is_rejected_on_replay() {
    let dir = scratch();
    let log = EventLog::open(&dir).expect("open");
    let event = log
        .append(EventRecord {
            kind: "boot".to_owned(),
            payload: "{}".to_owned(),
        })
        .expect("append");
    let cursor = EventCursor::after(&event);
    assert!(log.replay(&cursor).is_err());
}

#[test]
fn moved_base_apply_replay_returns_original_refusal_after_restart() {
    let dir = scratch();
    let apply_key = idem("moved-replay");
    let tx_id = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        let propose = engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &propose_params("moved-replay-propose", "/etc/nixos/configuration.nix"),
            )
            .expect("propose");
        engine
            .tx_test(
                "agent:a",
                "sha256:abc",
                &TxTestParams {
                    tx_id: tx(&propose.tx_id),
                },
            )
            .expect("test");
        propose.tx_id
    };

    let apply_params = TxApplyParams {
        tx_id: tx(&tx_id),
        idempotency_key: apply_key.clone(),
    };

    {
        let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("moved");
        let err = engine
            .tx_apply("agent:a", "sha256:abc", &apply_params)
            .expect_err("moved base");
        assert!(matches!(err, EngineError::BaseRevisionMoved));
        let err = engine
            .tx_apply("agent:a", "sha256:abc", &apply_params)
            .expect_err("idempotent replay");
        assert!(matches!(err, EngineError::BaseRevisionMoved));
    }

    let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("reopen");
    let err = engine
        .tx_apply("agent:a", "sha256:abc", &apply_params)
        .expect_err("restart replay");
    assert!(matches!(err, EngineError::BaseRevisionMoved));
    let status = engine.tx_status(&tx_id).expect("status");
    assert_eq!(status.state, TransactionState::Rejected);
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

fn wal_record_with_base_revision(
    seq: u64,
    tx_id: &str,
    state: TransactionState,
    agent_id: &str,
    manifest_byte: u8,
    base_revision: BaseRevision,
) -> WalRecord {
    let mut record = wal_record(seq, tx_id, state, agent_id, manifest_byte);
    record.base_revision = base_revision;
    record
}

fn wal_record_to_json(record: &WalRecord) -> serde_json::Value {
    let value = serde_json::to_value(record).expect("serialize wal record");
    assert!(serde_json::from_value::<WalRecord>(value.clone()).is_ok());
    value
}

fn append_wal_records(dir: &Path, records: &[WalRecord]) {
    let durability = Arc::new(RealDurability);
    let mut store = WalStore::open(dir.join("wal"), durability).expect("wal");
    for record in records {
        store.append_transition(record).expect("append");
    }
}

fn write_wal_json_record(dir: &Path, seq: u64, value: &serde_json::Value) {
    let wal_dir = dir.join("wal");
    std::fs::create_dir_all(wal_dir.join("records")).expect("records");
    std::fs::write(
        wal_dir.join("records").join(format!("{seq}.json")),
        serde_json::to_string(&value).expect("json"),
    )
    .expect("write record");
    std::fs::write(
        wal_dir.join("checkpoint.json"),
        format!(r#"{{"seq":{seq}}}"#),
    )
    .expect("write checkpoint");
}

fn write_mutated_wal_record(
    dir: &Path,
    seq: u64,
    record: &WalRecord,
    mutate: impl FnOnce(&mut serde_json::Value),
) {
    let mut value = wal_record_to_json(record);
    mutate(&mut value);
    write_wal_json_record(dir, seq, &value);
}

fn assert_dm_refused_in_safe_mode(dir: &Path) {
    let engine = TransactionEngine::open(dir, UnresolvedAdapter).expect("open");
    let err = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params("after-invalid-wal", "/etc/nixos/configuration.nix"),
        )
        .expect_err("safe mode");
    assert!(matches!(err, EngineError::SafeMode));
}

fn set_path_readonly(path: &Path, readonly: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if readonly { 0o555 } else { 0o755 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn idempotency_binding_path(dir: &Path, agent_id: &str, op: &str, key: &str) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let binding = format!("{agent_id}:{op}:{key}");
    let mut hasher = DefaultHasher::new();
    binding.hash(&mut hasher);
    dir.join("idempotency")
        .join(format!("{:016x}.json", hasher.finish()))
}

// --- Review #5011209070: WAL semantic validation (RED) ---

#[test]
fn watchdog_committed_record_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    append_wal_records(
        &dir,
        &[
            wal_record(1, tx, TransactionState::Proposed, "agent:a", 0),
            wal_record(2, tx, TransactionState::Committed, "agent:a", 0),
        ],
    );
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn watchdog_committing_record_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    append_wal_records(
        &dir,
        &[
            wal_record(1, tx, TransactionState::Proposed, "agent:a", 0),
            wal_record(2, tx, TransactionState::Committing, "agent:a", 0),
        ],
    );
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn impossible_testing_to_proposed_chain_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    append_wal_records(
        &dir,
        &[
            wal_record(1, tx, TransactionState::Proposed, "agent:a", 0),
            wal_record(2, tx, TransactionState::Testing, "agent:a", 0),
            wal_record(3, tx, TransactionState::Proposed, "agent:a", 0),
        ],
    );
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn immutable_agent_identity_change_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    append_wal_records(
        &dir,
        &[
            wal_record(1, tx, TransactionState::Proposed, "agent:a", 0),
            wal_record(2, tx, TransactionState::Testing, "agent:b", 0),
        ],
    );
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn immutable_manifest_digest_change_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    append_wal_records(
        &dir,
        &[
            wal_record(1, tx, TransactionState::Proposed, "agent:a", 0),
            wal_record(2, tx, TransactionState::Testing, "agent:a", 1),
        ],
    );
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn immutable_base_revision_change_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let moved_base = BaseRevision {
        generation: Some("gen-moved".to_owned()),
        etc_git_commit: "deadbeef".to_owned(),
        config_digest: Digest::from_sha256_bytes([0x99; 32]),
    };
    append_wal_records(
        &dir,
        &[
            wal_record(1, tx, TransactionState::Proposed, "agent:a", 0),
            wal_record_with_base_revision(
                2,
                tx,
                TransactionState::Testing,
                "agent:a",
                0,
                moved_base,
            ),
        ],
    );
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn unsupported_wal_record_version_enters_safe_mode() {
    let dir = scratch();
    let record = wal_record(
        1,
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        TransactionState::Proposed,
        "agent:a",
        0,
    );
    write_mutated_wal_record(&dir, 1, &record, |value| {
        value
            .as_object_mut()
            .expect("object")
            .insert("record_version".to_owned(), serde_json::json!(2));
    });
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn wal_record_missing_required_field_enters_safe_mode() {
    let dir = scratch();
    let record = wal_record(
        1,
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        TransactionState::Proposed,
        "agent:a",
        0,
    );
    write_mutated_wal_record(&dir, 1, &record, |value| {
        value.as_object_mut().expect("object").remove("agent_id");
    });
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn wal_record_invalid_required_field_enters_safe_mode() {
    let dir = scratch();
    let record = wal_record(
        1,
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        TransactionState::Proposed,
        "agent:a",
        0,
    );
    write_mutated_wal_record(&dir, 1, &record, |value| {
        value["state"] = serde_json::json!("not_a_real_state");
    });
    assert_dm_refused_in_safe_mode(&dir);
}

// --- Review #5011209070: idempotency durability ordering (RED) ---

#[test]
fn idempotency_index_write_failure_immediate_retry_does_not_duplicate() {
    let dir = scratch();
    let params = propose_params("idem-write-fail", "/etc/nixos/configuration.nix");
    let idem_dir = dir.join("idempotency");
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
    set_path_readonly(&idem_dir, true);
    let err = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect_err("idem write failure");
    assert!(matches!(err, EngineError::Storage(_)));
    set_path_readonly(&idem_dir, false);

    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 0);

    let durability = Arc::new(RealDurability);
    let store = WalStore::open(dir.join("wal"), durability).expect("wal");
    let original_tx = store.load_records().expect("load")[0].tx_id.clone();
    let replay = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("same-key replay");
    assert_eq!(replay.tx_id, original_tx);
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 1);
}

#[test]
fn idempotency_index_write_failure_restart_replays_authoritatively() {
    let dir = scratch();
    let params = propose_params("idem-write-restart", "/etc/nixos/configuration.nix");
    let idem_dir = dir.join("idempotency");
    let failed_tx = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        set_path_readonly(&idem_dir, true);
        let _ = engine
            .config_propose("agent:a", "sha256:abc", &params)
            .expect_err("idem write failure");
        set_path_readonly(&idem_dir, false);
        let durability = Arc::new(RealDurability);
        let store = WalStore::open(dir.join("wal"), durability).expect("wal");
        store.load_records().expect("load")[0].tx_id.clone()
    };

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let replay = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("restart replay");
    assert_eq!(replay.tx_id, failed_tx);
    assert_eq!(wal_record_count(&dir), 1);
}

#[test]
fn idempotency_index_rename_failure_immediate_retry_does_not_duplicate() {
    let dir = scratch();
    let key = "idem-rename-immediate";
    let params = propose_params(key, "/etc/nixos/configuration.nix");
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
    let blocker = idempotency_binding_path(&dir, "agent:a", "config.propose", key);
    std::fs::create_dir_all(&blocker).expect("rename blocker");

    let err = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect_err("rename failure");
    assert!(matches!(err, EngineError::Storage(_)));
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 0);

    std::fs::remove_dir_all(&blocker).expect("clear blocker");
    let durability = Arc::new(RealDurability);
    let store = WalStore::open(dir.join("wal"), durability).expect("wal");
    let original_tx = store.load_records().expect("load")[0].tx_id.clone();
    let replay = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("same-key replay");
    assert_eq!(replay.tx_id, original_tx);
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 1);
}

#[test]
fn idempotency_index_rename_failure_restart_replays_authoritatively() {
    let dir = scratch();
    let key = "idem-rename-restart";
    let params = propose_params(key, "/etc/nixos/configuration.nix");
    let failed_tx = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        let blocker = idempotency_binding_path(&dir, "agent:a", "config.propose", key);
        std::fs::create_dir_all(&blocker).expect("rename blocker");
        let _ = engine
            .config_propose("agent:a", "sha256:abc", &params)
            .expect_err("rename failure");
        std::fs::remove_dir_all(&blocker).expect("clear blocker");
        let durability = Arc::new(RealDurability);
        let store = WalStore::open(dir.join("wal"), durability).expect("wal");
        store.load_records().expect("load")[0].tx_id.clone()
    };

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let replay = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("restart replay");
    assert_eq!(replay.tx_id, failed_tx);
    assert_eq!(wal_record_count(&dir), 1);
}

// --- Review #5011747127: immutable WAL payload drift (RED) ---

#[test]
fn immutable_effect_set_change_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let proposed = wal_record(1, tx, TransactionState::Proposed, "agent:a", 0);
    let mut testing = wal_record(2, tx, TransactionState::Testing, "agent:a", 0);
    testing.effect_set = vec![EffectClass::R];
    append_wal_records(&dir, &[proposed, testing]);
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn immutable_diff_change_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let proposed = wal_record(1, tx, TransactionState::Proposed, "agent:a", 0);
    let mut testing = wal_record(2, tx, TransactionState::Testing, "agent:a", 0);
    testing.diff = "mutated diff payload".to_owned();
    append_wal_records(&dir, &[proposed, testing]);
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn immutable_affected_resources_change_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let proposed = wal_record(1, tx, TransactionState::Proposed, "agent:a", 0);
    let mut testing = wal_record(2, tx, TransactionState::Testing, "agent:a", 0);
    testing.affected_resources = vec!["other_resource".to_owned()];
    append_wal_records(&dir, &[proposed, testing]);
    assert_dm_refused_in_safe_mode(&dir);
}

#[test]
fn immutable_approval_ref_change_in_wal_enters_safe_mode() {
    let dir = scratch();
    let tx = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let proposed = wal_record(1, tx, TransactionState::Proposed, "agent:a", 0);
    let mut testing = wal_record(2, tx, TransactionState::Testing, "agent:a", 0);
    testing.approval_ref = Some("approval-xyz".to_owned());
    append_wal_records(&dir, &[proposed, testing]);
    assert_dm_refused_in_safe_mode(&dir);
}

// --- Review #5011747127: conflicting config.propose after idempotency fault (RED) ---

fn original_propose_tx_id(dir: &Path) -> String {
    let durability = Arc::new(RealDurability);
    let store = WalStore::open(dir.join("wal"), durability).expect("wal");
    store
        .load_records()
        .expect("load")
        .first()
        .expect("load")
        .tx_id
        .clone()
}

#[test]
fn conflicting_propose_after_idempotency_write_failure_immediate() {
    let dir = scratch();
    let key = "conflict-write-immediate";
    let first = propose_params(key, "/etc/nixos/a.nix");
    let conflicting = propose_params(key, "/etc/nixos/b.nix");
    let idem_dir = dir.join("idempotency");
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
    set_path_readonly(&idem_dir, true);
    let err = engine
        .config_propose("agent:a", "sha256:abc", &first)
        .expect_err("idem write failure");
    assert!(matches!(err, EngineError::Storage(_)));
    set_path_readonly(&idem_dir, false);

    let original_tx = original_propose_tx_id(&dir);
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 0);

    let err = engine
        .config_propose("agent:a", "sha256:abc", &conflicting)
        .expect_err("conflicting retry");
    assert!(matches!(err, EngineError::IdempotencyConflict));
    assert_eq!(original_propose_tx_id(&dir), original_tx);
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 0);
}

#[test]
fn conflicting_propose_after_idempotency_write_failure_restart() {
    let dir = scratch();
    let key = "conflict-write-restart";
    let first = propose_params(key, "/etc/nixos/a.nix");
    let conflicting = propose_params(key, "/etc/nixos/b.nix");
    let idem_dir = dir.join("idempotency");
    let original_tx = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        set_path_readonly(&idem_dir, true);
        let _ = engine
            .config_propose("agent:a", "sha256:abc", &first)
            .expect_err("idem write failure");
        set_path_readonly(&idem_dir, false);
        original_propose_tx_id(&dir)
    };

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let err = engine
        .config_propose("agent:a", "sha256:abc", &conflicting)
        .expect_err("conflicting retry after restart");
    assert!(matches!(err, EngineError::IdempotencyConflict));
    assert_eq!(original_propose_tx_id(&dir), original_tx);
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 0);
}

#[test]
fn conflicting_propose_after_idempotency_rename_failure_immediate() {
    let dir = scratch();
    let key = "conflict-rename-immediate";
    let first = propose_params(key, "/etc/nixos/a.nix");
    let conflicting = propose_params(key, "/etc/nixos/b.nix");
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
    let blocker = idempotency_binding_path(&dir, "agent:a", "config.propose", key);
    std::fs::create_dir_all(&blocker).expect("rename blocker");

    let err = engine
        .config_propose("agent:a", "sha256:abc", &first)
        .expect_err("rename failure");
    assert!(matches!(err, EngineError::Storage(_)));
    std::fs::remove_dir_all(&blocker).expect("clear blocker");

    let original_tx = original_propose_tx_id(&dir);
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 0);

    let err = engine
        .config_propose("agent:a", "sha256:abc", &conflicting)
        .expect_err("conflicting retry");
    assert!(matches!(err, EngineError::IdempotencyConflict));
    assert_eq!(original_propose_tx_id(&dir), original_tx);
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 0);
}

#[test]
fn conflicting_propose_after_idempotency_rename_failure_restart() {
    let dir = scratch();
    let key = "conflict-rename-restart";
    let first = propose_params(key, "/etc/nixos/a.nix");
    let conflicting = propose_params(key, "/etc/nixos/b.nix");
    let original_tx = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        let blocker = idempotency_binding_path(&dir, "agent:a", "config.propose", key);
        std::fs::create_dir_all(&blocker).expect("rename blocker");
        let _ = engine
            .config_propose("agent:a", "sha256:abc", &first)
            .expect_err("rename failure");
        std::fs::remove_dir_all(&blocker).expect("clear blocker");
        original_propose_tx_id(&dir)
    };

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let err = engine
        .config_propose("agent:a", "sha256:abc", &conflicting)
        .expect_err("conflicting retry after restart");
    assert!(matches!(err, EngineError::IdempotencyConflict));
    assert_eq!(original_propose_tx_id(&dir), original_tx);
    assert_eq!(wal_record_count(&dir), 1);
    assert_eq!(event_count(&dir), 0);
}

// --- Review #5011747127: moved-base REJECTED idempotency ordering (RED) ---

fn setup_moved_base_apply(dir: &Path, apply_key: &str) -> (String, TxApplyParams) {
    let tx_id = {
        let engine = TransactionEngine::open(dir, UnresolvedAdapter).expect("open");
        let propose = engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &propose_params(
                    &format!("{apply_key}-propose"),
                    "/etc/nixos/configuration.nix",
                ),
            )
            .expect("propose");
        engine
            .tx_test(
                "agent:a",
                "sha256:abc",
                &TxTestParams {
                    tx_id: tx(&propose.tx_id),
                },
            )
            .expect("test");
        propose.tx_id
    };
    let apply_params = TxApplyParams {
        tx_id: tx(&tx_id),
        idempotency_key: idem(apply_key),
    };
    (tx_id, apply_params)
}

#[test]
fn moved_base_rejection_after_idempotency_write_failure_immediate_replay() {
    let dir = scratch();
    let apply_key = "moved-idem-write-immediate";
    let (tx_id, apply_params) = setup_moved_base_apply(&dir, apply_key);
    let idem_dir = dir.join("idempotency");
    let wal_before;
    let events_before;
    {
        let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("moved");
        set_path_readonly(&idem_dir, true);
        wal_before = wal_record_count(&dir);
        let err = engine
            .tx_apply("agent:a", "sha256:abc", &apply_params)
            .expect_err("idem write failure");
        assert!(matches!(err, EngineError::Storage(_)));
        events_before = event_count(&dir);
        set_path_readonly(&idem_dir, false);

        let err = engine
            .tx_apply("agent:a", "sha256:abc", &apply_params)
            .expect_err("replay refusal");
        assert!(matches!(err, EngineError::BaseRevisionMoved));
        assert_eq!(
            wal_record_count(&dir),
            wal_before
                .checked_add(1)
                .expect("append-only rejection adds one WAL record")
        );
        assert_eq!(count_wal_rejected_for_tx(&dir, &tx_id), 1);
        assert_eq!(event_count(&dir), events_before);
    }

    let status = TransactionEngine::open(&dir, MovedBaseAdapter)
        .expect("status engine")
        .tx_status(&tx_id)
        .expect("status");
    assert_eq!(status.state, TransactionState::Rejected);
}

#[test]
fn moved_base_rejection_after_idempotency_write_failure_restart_replay() {
    let dir = scratch();
    let apply_key = "moved-idem-write-restart";
    let (tx_id, apply_params) = setup_moved_base_apply(&dir, apply_key);
    let idem_dir = dir.join("idempotency");
    let wal_before;
    let events_before;
    {
        let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("moved");
        set_path_readonly(&idem_dir, true);
        wal_before = wal_record_count(&dir);
        let err = engine
            .tx_apply("agent:a", "sha256:abc", &apply_params)
            .expect_err("idem write failure");
        assert!(matches!(err, EngineError::Storage(_)));
        events_before = event_count(&dir);
        set_path_readonly(&idem_dir, false);
    }

    let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("reopen");
    let err = engine
        .tx_apply("agent:a", "sha256:abc", &apply_params)
        .expect_err("replay refusal after restart");
    assert!(matches!(err, EngineError::BaseRevisionMoved));
    assert_eq!(
        wal_record_count(&dir),
        wal_before
            .checked_add(1)
            .expect("append-only rejection adds one WAL record")
    );
    assert_eq!(count_wal_rejected_for_tx(&dir, &tx_id), 1);
    assert_eq!(event_count(&dir), events_before);

    let status = engine.tx_status(&tx_id).expect("status");
    assert_eq!(status.state, TransactionState::Rejected);
}

#[test]
fn moved_base_rejection_after_idempotency_rename_failure_immediate_replay() {
    let dir = scratch();
    let apply_key = "moved-idem-rename-immediate";
    let (tx_id, apply_params) = setup_moved_base_apply(&dir, apply_key);
    let blocker = idempotency_binding_path(&dir, "agent:a", "tx.apply", apply_key);
    let wal_before;
    let events_before;
    {
        let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("moved");
        std::fs::create_dir_all(&blocker).expect("rename blocker");
        wal_before = wal_record_count(&dir);
        let err = engine
            .tx_apply("agent:a", "sha256:abc", &apply_params)
            .expect_err("rename failure");
        assert!(matches!(err, EngineError::Storage(_)));
        events_before = event_count(&dir);
        std::fs::remove_dir_all(&blocker).expect("clear blocker");

        let err = engine
            .tx_apply("agent:a", "sha256:abc", &apply_params)
            .expect_err("replay refusal");
        assert!(matches!(err, EngineError::BaseRevisionMoved));
        assert_eq!(
            wal_record_count(&dir),
            wal_before
                .checked_add(1)
                .expect("append-only rejection adds one WAL record")
        );
        assert_eq!(count_wal_rejected_for_tx(&dir, &tx_id), 1);
        assert_eq!(event_count(&dir), events_before);
    }

    let status = TransactionEngine::open(&dir, MovedBaseAdapter)
        .expect("status engine")
        .tx_status(&tx_id)
        .expect("status");
    assert_eq!(status.state, TransactionState::Rejected);
}

#[test]
fn moved_base_rejection_after_idempotency_rename_failure_restart_replay() {
    let dir = scratch();
    let apply_key = "moved-idem-rename-restart";
    let (tx_id, apply_params) = setup_moved_base_apply(&dir, apply_key);
    let blocker = idempotency_binding_path(&dir, "agent:a", "tx.apply", apply_key);
    let wal_before;
    let events_before;
    {
        let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("moved");
        std::fs::create_dir_all(&blocker).expect("rename blocker");
        wal_before = wal_record_count(&dir);
        let _ = engine
            .tx_apply("agent:a", "sha256:abc", &apply_params)
            .expect_err("rename failure");
        events_before = event_count(&dir);
        std::fs::remove_dir_all(&blocker).expect("clear blocker");
    }

    let engine = TransactionEngine::open(&dir, MovedBaseAdapter).expect("reopen");
    let err = engine
        .tx_apply("agent:a", "sha256:abc", &apply_params)
        .expect_err("replay refusal after restart");
    assert!(matches!(err, EngineError::BaseRevisionMoved));
    assert_eq!(
        wal_record_count(&dir),
        wal_before
            .checked_add(1)
            .expect("append-only rejection adds one WAL record")
    );
    assert_eq!(count_wal_rejected_for_tx(&dir, &tx_id), 1);
    assert_eq!(event_count(&dir), events_before);

    let status = engine.tx_status(&tx_id).expect("status");
    assert_eq!(status.state, TransactionState::Rejected);
}

// --- Review #5013663187: moved-base crash consistency and divergence fail-closed (RED) ---

fn tx_state_event_payload(tx_id: &str, state: &str) -> String {
    format!(r#"{{"tx_id":"{tx_id}","state":"{state}"}}"#)
}

fn read_stored_events(state_dir: &Path) -> Vec<StoredEvent> {
    let log_path = state_dir.join("events/log.jsonl");
    if !log_path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&log_path)
        .expect("event log")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("stored event"))
        .collect()
}

fn count_tx_state_events(state_dir: &Path, tx_id: &str, state: &str) -> usize {
    let expected_payload = tx_state_event_payload(tx_id, state);
    read_stored_events(state_dir)
        .into_iter()
        .filter(|event| event.kind == "tx.state" && event.payload == expected_payload)
        .count()
}

fn load_wal_records(state_dir: &Path) -> Vec<WalRecord> {
    let durability = Arc::new(RealDurability);
    let store = WalStore::open(state_dir.join("wal"), durability).expect("wal");
    store.load_records().expect("load")
}

fn count_wal_rejected_for_tx(state_dir: &Path, tx_id: &str) -> usize {
    load_wal_records(state_dir)
        .into_iter()
        .filter(|record| record.tx_id == tx_id && record.state == TransactionState::Rejected)
        .count()
}

fn wal_record_bytes_at_seq(state_dir: &Path, seq: u64) -> Vec<u8> {
    std::fs::read(state_dir.join("wal/records").join(format!("{seq}.json")))
        .expect("wal record bytes")
}

fn assert_moved_base_crash_before_retry_consistency(
    dir: &Path,
    tx_id: &str,
    apply_params: &TxApplyParams,
    inject_fault: impl FnOnce(&Path),
    clear_fault: impl FnOnce(&Path),
) {
    let wal_before = wal_record_count(dir);
    let events_before = event_count(dir);
    let records_before = load_wal_records(dir);
    let testing_record = records_before
        .iter()
        .find(|record| record.tx_id == tx_id && record.state == TransactionState::Testing)
        .expect("testing wal record");
    let testing_seq = testing_record.seq;
    let testing_bytes = wal_record_bytes_at_seq(dir, testing_seq);
    let testing_json = wal_record_to_json(testing_record);

    {
        let engine = TransactionEngine::open(dir, MovedBaseAdapter).expect("moved");
        inject_fault(dir);
        let err = engine
            .tx_apply("agent:a", "sha256:abc", apply_params)
            .expect_err("idempotency fault must surface as storage");
        assert!(matches!(err, EngineError::Storage(_)));
    }

    clear_fault(dir);

    {
        let engine = TransactionEngine::open(dir, MovedBaseAdapter).expect("reopen before retry");
        let status = engine
            .tx_status(tx_id)
            .expect("status readable after crash");
        assert_eq!(
            status.state,
            TransactionState::Rejected,
            "tx_status must reflect durable rejection after crash-before-retry"
        );
    }

    assert_eq!(
        count_wal_rejected_for_tx(dir, tx_id),
        1,
        "WAL must retain exactly one append-only Rejected transition"
    );
    assert_eq!(
        wal_record_count(dir),
        wal_before
            .checked_add(1)
            .expect("append-only rejection adds one WAL record"),
        "WAL must gain one transition without rewriting Testing"
    );
    assert_eq!(
        wal_record_bytes_at_seq(dir, testing_seq),
        testing_bytes,
        "Testing WAL record bytes must remain unchanged after failure and restart"
    );
    let testing_after = load_wal_records(dir)
        .into_iter()
        .find(|record| record.seq == testing_seq)
        .expect("testing seq");
    assert_eq!(testing_after.state, TransactionState::Testing);
    assert_eq!(wal_record_to_json(&testing_after), testing_json);

    assert_eq!(
        event_count(dir),
        events_before
            .checked_add(1)
            .expect("append-only rejection adds one event")
    );
    assert_eq!(count_tx_state_events(dir, tx_id, "Rejected"), 1);

    let wal_after_crash = wal_record_count(dir);
    let events_after_crash = event_count(dir);
    {
        let engine = TransactionEngine::open(dir, MovedBaseAdapter).expect("retry");
        let err = engine
            .tx_apply("agent:a", "sha256:abc", apply_params)
            .expect_err("replay refusal");
        assert!(matches!(err, EngineError::BaseRevisionMoved));
    }

    assert_eq!(
        wal_record_count(dir),
        wal_after_crash,
        "retry must not add or replace WAL records"
    );
    assert_eq!(
        wal_record_bytes_at_seq(dir, testing_seq),
        testing_bytes,
        "retry must not rewrite Testing WAL in place"
    );
    assert_eq!(
        event_count(dir),
        events_after_crash,
        "retry must not append duplicate rejection events"
    );

    {
        let engine = TransactionEngine::open(dir, MovedBaseAdapter).expect("final reopen");
        let status = engine.tx_status(tx_id).expect("status after final reopen");
        assert_eq!(status.state, TransactionState::Rejected);
        let err = engine
            .tx_apply("agent:a", "sha256:abc", apply_params)
            .expect_err("persistent refusal after final reopen");
        assert!(matches!(err, EngineError::BaseRevisionMoved));
    }
    assert_eq!(count_wal_rejected_for_tx(dir, tx_id), 1);
    assert_eq!(wal_record_bytes_at_seq(dir, testing_seq), testing_bytes);
}

#[test]
fn moved_base_apply_idempotency_write_failure_crash_before_retry_consistency() {
    let dir = scratch();
    let apply_key = "crash-write-consistency";
    let (tx_id, apply_params) = setup_moved_base_apply(&dir, apply_key);
    let idem_dir = dir.join("idempotency");

    assert_moved_base_crash_before_retry_consistency(
        &dir,
        &tx_id,
        &apply_params,
        |dir| set_path_readonly(&dir.join("idempotency"), true),
        |_| set_path_readonly(&idem_dir, false),
    );
}

#[test]
fn moved_base_apply_idempotency_rename_failure_crash_before_retry_consistency() {
    let dir = scratch();
    let apply_key = "crash-rename-consistency";
    let (tx_id, apply_params) = setup_moved_base_apply(&dir, apply_key);
    let blocker = idempotency_binding_path(&dir, "agent:a", "tx.apply", apply_key);

    assert_moved_base_crash_before_retry_consistency(
        &dir,
        &tx_id,
        &apply_params,
        |_| std::fs::create_dir_all(&blocker).expect("rename blocker"),
        |_| std::fs::remove_dir_all(&blocker).expect("clear blocker"),
    );
}

#[test]
fn orphan_rejected_event_without_wal_transition_enters_safe_mode() {
    let dir = scratch();
    let tx_id = {
        let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
        let propose = engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &propose_params("orphan-event-propose", "/etc/nixos/configuration.nix"),
            )
            .expect("propose");
        engine
            .tx_test(
                "agent:a",
                "sha256:abc",
                &TxTestParams {
                    tx_id: tx(&propose.tx_id),
                },
            )
            .expect("test");
        propose.tx_id
    };

    assert_eq!(count_wal_rejected_for_tx(&dir, &tx_id), 0);

    let log = EventLog::open(dir.join("events")).expect("events");
    log.append(EventRecord {
        kind: "tx.state".to_owned(),
        payload: tx_state_event_payload(&tx_id, "Rejected"),
    })
    .expect("orphan rejected event");

    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("reopen");
    let err = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &propose_params(
                "after-orphan-rejected-event",
                "/etc/nixos/configuration.nix",
            ),
        )
        .expect_err("event/WAL divergence must enter safe mode");
    assert!(matches!(err, EngineError::SafeMode));
}
