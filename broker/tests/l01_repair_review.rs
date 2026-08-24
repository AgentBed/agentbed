//! L01 repair tests for native review #5010391942 (PR #20 @ daaaae0).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::events::{EventCursor, EventLog, EventRecord};
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
