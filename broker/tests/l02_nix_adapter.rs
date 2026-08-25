//! L02 broker integration: protected-path rejection and Nix propose path.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::needless_borrows_for_generic_args,
    clippy::ptr_arg,
    clippy::useless_format,
    clippy::cloned_ref_to_slice_refs,
    unused_imports
)]

use agentbed_adapter_nix::adapter::NixAdapter;
use agentbed_adapter_nix::capture::CaptureStore;
use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_broker::adapter::{HostAdapter, UnresolvedAdapter};
use agentbed_broker::events::EventLog;
use agentbed_broker::storage::wal::WalStore;
use agentbed_broker::transaction::engine::{EngineError, TransactionEngine};
use agentbed_protocol::wire::{
    ConfigFileChange, ConfigProposeParams, IdempotencyKey, TransactionId, TxTestParams,
};
use std::path::PathBuf;
use std::sync::Arc;

fn scratch() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb6-l02-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn idem(s: &str) -> IdempotencyKey {
    IdempotencyKey::new(s).expect("idempotency key")
}

fn wal_count(dir: &PathBuf) -> usize {
    let records = dir.join("wal/records");
    if !records.exists() {
        return 0;
    }
    std::fs::read_dir(records)
        .expect("records")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .count()
}

fn event_count(dir: &PathBuf) -> usize {
    let log = dir.join("events/log.jsonl");
    if !log.exists() {
        return 0;
    }
    std::fs::read_to_string(log)
        .expect("log")
        .lines()
        .filter(|line| !line.is_empty())
        .count()
}

fn register_nix_probe(runner: &FakeCommandRunner) {
    runner.register(
        CommandSpec::nix_current_generation(),
        CommandOutput::ok("42\n"),
    );
    runner.register(CommandSpec::etc_git_head(), CommandOutput::ok("abc123\n"));
    runner.register(
        CommandSpec::config_digest(),
        CommandOutput::ok("11".repeat(32)),
    );
}

fn nix_adapter() -> NixAdapter {
    let runner = Arc::new(FakeCommandRunner::new());
    register_nix_probe(runner.as_ref());
    NixAdapter::new(runner, CaptureStore::new(scratch().join("capture")))
}

#[test]
fn unresolved_adapter_keeps_l01_synthetic_propose() {
    let dir = scratch();
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
    let params = ConfigProposeParams {
        idempotency_key: idem("unresolved-1"),
        changes: vec![ConfigFileChange {
            path: "/etc/nixos/demo.nix".to_owned(),
            content: "{}".to_owned(),
        }],
    };
    let out = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("propose");
    assert_eq!(out.test_plan.adapter, "unresolved");
    assert_eq!(out.test_plan.steps, vec!["noop-test".to_owned()]);
}

#[test]
fn nix_propose_rejects_protected_paths_without_wal_side_effects() {
    let dir = scratch();
    let adapter = nix_adapter();
    let engine = TransactionEngine::open(&dir, adapter).expect("open");
    let params = ConfigProposeParams {
        idempotency_key: idem("protected-1"),
        changes: vec![ConfigFileChange {
            path: "/etc/nixos/watchdogd/config.nix".to_owned(),
            content: "{}".to_owned(),
        }],
    };
    let err = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect_err("reject");
    assert!(matches!(err, EngineError::ProposeRejected { .. }));
    assert_eq!(wal_count(&dir), 0);
    assert_eq!(event_count(&dir), 0);
}

#[test]
fn nix_propose_stages_candidate_with_nix_test_plan() {
    let dir = scratch();
    let runner = Arc::new(FakeCommandRunner::new());
    register_nix_probe(runner.as_ref());
    runner.register(
        CommandSpec::nix_eval_candidate(&[ConfigFileChange {
            path: "/etc/nixos/services/demo.nix".to_owned(),
            content: "{ services.demo.enable = true; }".to_owned(),
        }]),
        CommandOutput::ok("/nix/store/candidate-closure\n"),
    );
    let adapter = NixAdapter::new(runner, CaptureStore::new(dir.join("capture")));
    let engine = TransactionEngine::open(&dir, adapter).expect("open");
    let params = ConfigProposeParams {
        idempotency_key: idem("nix-propose-1"),
        changes: vec![ConfigFileChange {
            path: "/etc/nixos/services/demo.nix".to_owned(),
            content: "{ services.demo.enable = true; }".to_owned(),
        }],
    };
    let out = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("propose");
    assert_eq!(out.test_plan.adapter, "nix");
    assert_eq!(out.test_plan.steps, vec!["nixos-rebuild test".to_owned()]);
    assert!(out.diff.contains("demo.nix"));
    assert_eq!(wal_count(&dir), 1);
    assert_eq!(event_count(&dir), 1);
}

#[test]
fn nix_propose_idempotent_replay_survives_restart() {
    let dir = scratch();
    let runner = Arc::new(FakeCommandRunner::new());
    register_nix_probe(runner.as_ref());
    let change = ConfigFileChange {
        path: "/etc/nixos/services/demo.nix".to_owned(),
        content: "{ services.demo.enable = true; }".to_owned(),
    };
    runner.register(
        CommandSpec::nix_eval_candidate(&[change.clone()]),
        CommandOutput::ok("/nix/store/candidate-closure\n"),
    );
    let adapter = NixAdapter::new(runner, CaptureStore::new(dir.join("capture")));
    let params = ConfigProposeParams {
        idempotency_key: idem("nix-replay-1"),
        changes: vec![change],
    };
    let first_tx = {
        let engine = TransactionEngine::open(&dir, adapter).expect("open");
        engine
            .config_propose("agent:a", "sha256:abc", &params)
            .expect("first")
            .tx_id
    };
    let wal_after_first = wal_count(&dir);
    let runner2 = Arc::new(FakeCommandRunner::new());
    register_nix_probe(runner2.as_ref());
    runner2.register(
        CommandSpec::nix_current_generation(),
        CommandOutput::ok("99\n"),
    );
    let adapter2 = NixAdapter::new(runner2, CaptureStore::new(dir.join("capture")));
    let engine2 = TransactionEngine::open(&dir, adapter2).expect("reopen");
    let replay = engine2
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("replay");
    assert_eq!(replay.tx_id, first_tx);
    assert_eq!(wal_count(&dir), wal_after_first);
}

#[test]
fn conflicting_nix_capture_fails_closed() {
    let dir = scratch();
    let runner = Arc::new(FakeCommandRunner::new());
    register_nix_probe(runner.as_ref());
    let change = ConfigFileChange {
        path: "/etc/nixos/services/demo.nix".to_owned(),
        content: "{ services.demo.enable = true; }".to_owned(),
    };
    runner.register(
        CommandSpec::nix_eval_candidate(&[change.clone()]),
        CommandOutput::ok("/nix/store/candidate-closure\n"),
    );
    let store = CaptureStore::new(dir.join("capture"));
    let adapter = NixAdapter::new(runner, store);
    let engine = TransactionEngine::open(&dir, adapter).expect("open");
    let first = ConfigProposeParams {
        idempotency_key: idem("nix-conflict-1"),
        changes: vec![change.clone()],
    };
    engine
        .config_propose("agent:a", "sha256:abc", &first)
        .expect("first");
    let second = ConfigProposeParams {
        idempotency_key: idem("nix-conflict-2"),
        changes: vec![change],
    };
    let err = engine
        .config_propose("agent:a", "sha256:abc", &second)
        .expect_err("conflict");
    assert!(matches!(err, EngineError::ProposeRejected { .. }));
}

#[test]
fn tx_test_still_transitions_for_nix_proposal() {
    let dir = scratch();
    let runner = Arc::new(FakeCommandRunner::new());
    register_nix_probe(runner.as_ref());
    let change = ConfigFileChange {
        path: "/etc/nixos/services/demo.nix".to_owned(),
        content: "{}".to_owned(),
    };
    runner.register(
        CommandSpec::nix_eval_candidate(&[change.clone()]),
        CommandOutput::ok("/nix/store/candidate-closure\n"),
    );
    let adapter = NixAdapter::new(runner, CaptureStore::new(dir.join("capture")));
    let engine = TransactionEngine::open(&dir, adapter).expect("open");
    let params = ConfigProposeParams {
        idempotency_key: idem("nix-test-1"),
        changes: vec![change],
    };
    let proposed = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("propose");
    let tested = engine
        .tx_test(
            "agent:a",
            "sha256:abc",
            &TxTestParams {
                tx_id: TransactionId::new(&proposed.tx_id).expect("tx"),
            },
        )
        .expect("test");
    assert_eq!(
        tested.state,
        agentbed_protocol::dto::transaction::TransactionState::Testing
    );
}
