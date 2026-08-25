//! Review-2 broker regression tests (review #5018822751).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::needless_borrows_for_generic_args,
    clippy::ptr_arg,
    unused_imports
)]

use agentbed_adapter_nix::adapter::NixAdapter;
use agentbed_adapter_nix::capture::CaptureStore;
use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_broker::transaction::engine::TransactionEngine;
use agentbed_protocol::dto::transaction::ConfigProposeResult;
use agentbed_protocol::wire::{ConfigFileChange, ConfigProposeParams, IdempotencyKey};
use agentbed_schemas::{validate, SchemaKind};
use std::path::PathBuf;
use std::sync::Arc;

fn scratch() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb6-review2-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn idem(s: &str) -> IdempotencyKey {
    IdempotencyKey::new(s).expect("idempotency key")
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

#[test]
fn nix_propose_public_response_matches_v2_schema_without_candidate_closure() {
    let dir = scratch();
    let runner = Arc::new(FakeCommandRunner::new());
    register_nix_probe(runner.as_ref());
    let change = ConfigFileChange {
        path: "/etc/nixos/services/demo.nix".to_owned(),
        content: "{ services.demo.enable = true; }".to_owned(),
    };
    runner.register(
        CommandSpec::nix_eval_candidate(std::slice::from_ref(&change)),
        CommandOutput::ok("/nix/store/candidate-closure\n"),
    );
    let adapter = NixAdapter::new(runner, CaptureStore::new(dir.join("capture")));
    let engine = TransactionEngine::open(&dir, adapter).expect("open");
    let params = ConfigProposeParams {
        idempotency_key: idem("schema-compat-1"),
        changes: vec![change],
    };
    let outcome = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("propose");
    assert!(
        outcome.candidate_closure.is_none(),
        "public engine outcome must not expose candidate_closure"
    );

    let public = ConfigProposeResult {
        tx_id: outcome.tx_id,
        diff: outcome.diff,
        test_plan: outcome.test_plan,
        affected_resources: outcome.affected_resources,
        base_revision: outcome.base_revision,
        candidate_closure: None,
    };
    let json = serde_json::to_value(&public).expect("json");
    assert!(
        json.get("candidate_closure").is_none(),
        "public response must not expose candidate_closure"
    );
    validate(SchemaKind::ConfigProposeResponse, &json).expect("schema");
}

#[test]
fn nix_propose_wal_still_records_internal_candidate_closure() {
    let dir = scratch();
    let runner = Arc::new(FakeCommandRunner::new());
    register_nix_probe(runner.as_ref());
    let change = ConfigFileChange {
        path: "/etc/nixos/services/demo.nix".to_owned(),
        content: "{ services.demo.enable = true; }".to_owned(),
    };
    runner.register(
        CommandSpec::nix_eval_candidate(std::slice::from_ref(&change)),
        CommandOutput::ok("/nix/store/candidate-closure\n"),
    );
    let adapter = NixAdapter::new(runner, CaptureStore::new(dir.join("capture")));
    let engine = TransactionEngine::open(&dir, adapter).expect("open");
    let params = ConfigProposeParams {
        idempotency_key: idem("wal-internal-closure-1"),
        changes: vec![change],
    };
    engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("propose");

    let wal_raw = std::fs::read_dir(dir.join("wal/records"))
        .expect("records")
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).expect("read"))
        .collect::<String>();
    assert!(wal_raw.contains("/nix/store/candidate-closure"));
}

#[test]
fn nix_propose_rejects_nested_kernel_without_wal_side_effects() {
    let dir = scratch();
    let adapter = {
        let runner = Arc::new(FakeCommandRunner::new());
        register_nix_probe(runner.as_ref());
        let change = ConfigFileChange {
            path: "/etc/nixos/hardware.nix".to_owned(),
            content: "{ boot = { kernelPackages = pkgs.linuxPackages_latest; }; }".to_owned(),
        };
        runner.register(
            CommandSpec::nix_eval_candidate(std::slice::from_ref(&change)),
            CommandOutput::ok("/nix/store/candidate-closure\n"),
        );
        NixAdapter::new(runner, CaptureStore::new(dir.join("capture")))
    };
    let engine = TransactionEngine::open(&dir, adapter).expect("open");
    let params = ConfigProposeParams {
        idempotency_key: idem("nested-kernel-1"),
        changes: vec![ConfigFileChange {
            path: "/etc/nixos/hardware.nix".to_owned(),
            content: "{ boot = { kernelPackages = pkgs.linuxPackages_latest; }; }".to_owned(),
        }],
    };
    let err = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect_err("reject");
    assert!(matches!(
        err,
        agentbed_broker::transaction::engine::EngineError::ProposeRejected { .. }
    ));
    let wal_records = dir.join("wal/records");
    let count = if wal_records.exists() {
        std::fs::read_dir(wal_records)
            .expect("records")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .count()
    } else {
        0
    };
    assert_eq!(count, 0);
}
