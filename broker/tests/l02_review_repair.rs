//! Review-repair broker regression tests (review #5014581086).

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
use agentbed_broker::transaction::engine::{EngineError, TransactionEngine};
use agentbed_protocol::wire::{ConfigFileChange, ConfigProposeParams, IdempotencyKey};
use std::path::PathBuf;
use std::sync::Arc;

fn scratch() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb6-review-{}-{}",
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
fn nix_propose_wal_records_candidate_closure() {
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
        idempotency_key: idem("wal-closure-1"),
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
fn nix_propose_rejects_indirect_watchdog_content_without_wal() {
    let dir = scratch();
    let adapter = {
        let runner = Arc::new(FakeCommandRunner::new());
        register_nix_probe(runner.as_ref());
        NixAdapter::new(runner, CaptureStore::new(dir.join("capture")))
    };
    let engine = TransactionEngine::open(&dir, adapter).expect("open");
    let params = ConfigProposeParams {
        idempotency_key: idem("watchdog-indirect-1"),
        changes: vec![ConfigFileChange {
            path: "/etc/nixos/custom.nix".to_owned(),
            content: "{ services.agentbed-watchdogd.package = pkgs.agentbed-watchdogd; }"
                .to_owned(),
        }],
    };
    let err = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect_err("reject");
    assert!(matches!(err, EngineError::ProposeRejected { .. }));
    assert!(!dir.join("wal/records").exists());
}
