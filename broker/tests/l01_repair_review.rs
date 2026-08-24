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
                &TxTestParams {
                    tx_id: tx(&proposed.tx_id),
                },
            )
            .expect("test");
        engine
            .tx_apply(
                "agent:a",
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
