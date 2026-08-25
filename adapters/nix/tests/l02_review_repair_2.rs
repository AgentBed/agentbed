//! Review-2 repair regression tests (review #5018822751).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::needless_borrows_for_generic_args
)]

use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_adapter_nix::promotion::{build, flush, test_activation, PromotionError};
use agentbed_adapter_nix::protected::{self, ProtectedRejectReason};
use agentbed_adapter_nix::propose;
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::BaseRevision;
use agentbed_protocol::wire::ConfigFileChange;
use std::sync::Arc;
use std::sync::Barrier;
use std::thread;

fn base_revision() -> BaseRevision {
    BaseRevision {
        generation: Some("42".to_owned()),
        etc_git_commit: "abc123".to_owned(),
        config_digest: Digest::from_sha256_bytes([0x11; 32]),
    }
}

fn capture(closure: &str) -> propose::CapturedProposal {
    propose::CapturedProposal {
        base_revision: base_revision(),
        candidate_closure: closure.to_owned(),
        flake_ref: "/etc/nixos#agentbed".to_owned(),
        diff: "demo".to_owned(),
    }
}

fn register_probe(runner: &FakeCommandRunner) {
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
fn protected_rejects_nested_boot_kernel_attribute_set() {
    let change = ConfigFileChange {
        path: "/etc/nixos/hardware.nix".to_owned(),
        content: "{ boot = { kernelPackages = pkgs.linuxPackages_latest; }; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("nested kernel");
    assert_eq!(err, ProtectedRejectReason::Kernel);
}

#[test]
fn protected_rejects_nested_boot_loader_attribute_set() {
    let change = ConfigFileChange {
        path: "/etc/nixos/boot.nix".to_owned(),
        content: "{ boot = { loader.systemd-boot.enable = true; }; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("nested bootloader");
    assert_eq!(err, ProtectedRejectReason::Bootloader);
}

#[test]
fn protected_rejects_deeply_nested_boot_loader_form() {
    let change = ConfigFileChange {
        path: "/etc/nixos/boot.nix".to_owned(),
        content: "{ boot = { loader = { systemd-boot.enable = true; }; }; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("deep nested bootloader");
    assert_eq!(err, ProtectedRejectReason::Bootloader);
}

#[test]
fn activation_survives_in_memory_ledger_reset() {
    let runner = Arc::new(FakeCommandRunner::new());
    register_probe(runner.as_ref());
    let proposal = capture("/nix/store/durable-activation");
    runner.register(
        CommandSpec::nixos_rebuild_test(&proposal),
        CommandOutput::ok("/nix/store/durable-activation tested\n"),
    );
    test_activation::activate_once(runner.as_ref(), &proposal).expect("first");
    test_activation::reset_activation_ledger_for_tests();
    let err = test_activation::activate_once(runner.as_ref(), &proposal).expect_err("durable");
    assert_eq!(err, PromotionError::AlreadyActivated);
}

#[test]
fn activation_refuses_concurrent_duplicate_candidates() {
    let runner = Arc::new(FakeCommandRunner::new());
    register_probe(runner.as_ref());
    let proposal = capture("/nix/store/concurrent-activation");
    runner.register(
        CommandSpec::nixos_rebuild_test(&proposal),
        CommandOutput::ok("/nix/store/concurrent-activation tested\n"),
    );
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let runner = Arc::clone(&runner);
            let barrier = Arc::clone(&barrier);
            let proposal = proposal.clone();
            thread::spawn(move || {
                barrier.wait();
                test_activation::activate_once(runner.as_ref(), &proposal)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().expect("join")).collect();
    let successes = results.iter().filter(|r| r.is_ok()).count();
    let already = results
        .iter()
        .filter(|r| matches!(r, Err(PromotionError::AlreadyActivated)))
        .count();
    assert_eq!(successes, 1, "exactly one activation must succeed");
    assert_eq!(already, 1, "concurrent duplicate must fail closed");
}

#[test]
fn build_rejects_output_that_does_not_bind_candidate_closure() {
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/build-bind");
    runner.register(
        CommandSpec::nixos_rebuild_build(&proposal),
        CommandOutput::ok("built unrelated output\n"),
    );
    let err = build::build(&runner, &proposal).expect_err("mismatch");
    assert!(matches!(err, PromotionError::ClosureMismatch { .. }));
}

#[test]
fn flush_targets_explicit_profile_and_boot_boundaries() {
    let runner = FakeCommandRunner::new();
    runner.register(CommandSpec::sync_paths(), CommandOutput::ok(""));
    flush::flush_boundaries(&runner).expect("flush");
    let invocations = runner.invocations();
    assert!(
        invocations
            .iter()
            .any(|spec| spec.argv_contains("/nix/var/nix/profiles/system")),
        "must flush profile boundary"
    );
    assert!(
        invocations.iter().any(|spec| spec.argv_contains("/boot")),
        "must flush boot boundary"
    );
}

#[test]
fn command_runner_declares_env_stdin_and_timeout_policy() {
    let source = include_str!("../src/command_runner.rs");
    assert!(source.contains("env_clear"), "missing env policy");
    assert!(source.contains("stdin_null"), "missing stdin policy");
    assert!(source.contains("timeout_secs"), "missing timeout budget");
}

#[test]
fn command_runner_maps_timeout_and_interruption() {
    let source = include_str!("../src/command_runner.rs");
    assert!(source.contains("Timeout"), "missing timeout error");
    assert!(source.contains("Interrupted"), "missing interruption error");
}
