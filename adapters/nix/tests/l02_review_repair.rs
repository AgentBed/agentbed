//! Review-repair regression tests (RED → GREEN gate for review #5014581086).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::needless_borrows_for_generic_args
)]

use agentbed_adapter_nix::capture::CaptureStore;
use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_adapter_nix::probe;
use agentbed_adapter_nix::promotion::{pin, readback, test_activation, PromotionError};
use agentbed_adapter_nix::propose;
use agentbed_adapter_nix::protected::{self, ProtectedRejectReason};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::BaseRevision;
use agentbed_protocol::wire::ConfigFileChange;
use std::sync::Arc;

fn valid_config_digest_hex() -> String {
    "11".repeat(32)
}

fn capture() -> propose::CapturedProposal {
    propose::CapturedProposal {
        base_revision: BaseRevision {
            generation: Some("42".to_owned()),
            etc_git_commit: "abc123".to_owned(),
            config_digest: Digest::from_sha256_bytes([0x11; 32]),
        },
        candidate_closure: "/nix/store/candidate-closure".to_owned(),
        flake_ref: "/etc/nixos#agentbed".to_owned(),
        diff: "demo".to_owned(),
    }
}

#[test]
fn protected_rejects_indirect_watchdog_package_content() {
    let change = ConfigFileChange {
        path: "/etc/nixos/services/custom.nix".to_owned(),
        content: "{ services.agentbed-watchdogd.package = pkgs.agentbed-watchdogd; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("watchdog package");
    assert_eq!(err, ProtectedRejectReason::Watchdog);
}

#[test]
fn protected_rejects_watchdog_unit_and_state_paths() {
    let cases = [
        (
            "/etc/systemd/system/agentbed-watchdogd.service",
            "{}",
            ProtectedRejectReason::Watchdog,
        ),
        (
            "/var/lib/agentbed/watchdog/state.json",
            "{}",
            ProtectedRejectReason::Watchdog,
        ),
    ];
    for (path, content, expected) in cases {
        let change = ConfigFileChange {
            path: path.to_owned(),
            content: content.to_owned(),
        };
        let err = protected::check_protected_changes(&[change]).expect_err(path);
        assert_eq!(err, expected);
    }
}

#[test]
fn protected_rejects_duplicate_conflicting_changes() {
    let changes = vec![
        ConfigFileChange {
            path: "/etc/nixos/demo.nix".to_owned(),
            content: "a".to_owned(),
        },
        ConfigFileChange {
            path: "/etc/nixos/demo.nix".to_owned(),
            content: "b".to_owned(),
        },
    ];
    protected::check_protected_changes(&changes).expect_err("duplicate");
}

#[test]
fn probe_rejects_overlong_digest() {
    let runner = FakeCommandRunner::new();
    runner.register(
        CommandSpec::nix_current_generation(),
        CommandOutput::ok("42\n"),
    );
    runner.register(CommandSpec::etc_git_head(), CommandOutput::ok("abc123\n"));
    runner.register(
        CommandSpec::config_digest(),
        CommandOutput::ok("11".repeat(128)),
    );
    let err = probe::probe(&runner).expect_err("overlong digest");
    assert!(matches!(err, probe::ProbeError::IncompleteObservation));
}

#[test]
fn probe_rejects_short_and_non_hex_digest() {
    let runner = FakeCommandRunner::new();
    runner.register(
        CommandSpec::nix_current_generation(),
        CommandOutput::ok("42\n"),
    );
    runner.register(CommandSpec::etc_git_head(), CommandOutput::ok("abc123\n"));
    runner.register(CommandSpec::config_digest(), CommandOutput::ok("abc"));
    assert!(matches!(
        probe::probe(&runner).expect_err("short"),
        probe::ProbeError::IncompleteObservation
    ));

    runner.register(
        CommandSpec::config_digest(),
        CommandOutput::ok("zz".repeat(32)),
    );
    assert!(matches!(
        probe::probe(&runner).expect_err("non-hex"),
        probe::ProbeError::IncompleteObservation
    ));
}

#[test]
fn pin_closure_must_match_captured_candidate() {
    let runner = FakeCommandRunner::new();
    let capture = capture();
    runner.register(
        CommandSpec::nix_store_realise(&capture.candidate_closure),
        CommandOutput::ok("/nix/store/other-closure\n"),
    );
    let err = pin::pin_closure(&runner, &capture).expect_err("mismatch rejected");
    assert!(matches!(err, PromotionError::ClosureMismatch { .. }));
}

#[test]
fn readback_must_query_closure_store_path() {
    let runner = FakeCommandRunner::new();
    let pinned = "/nix/store/candidate-closure";
    runner.register(
        CommandSpec::read_profile_target(),
        CommandOutput::ok(&format!("{pinned}\n")),
    );
    runner.register(
        CommandSpec::read_boot_default(),
        CommandOutput::ok(&format!("{pinned}\n")),
    );
    runner.register(
        CommandSpec::read_closure_hash(pinned),
        CommandOutput::ok("hash-abc\n"),
    );
    runner.register(
        CommandSpec::read_closure_store_path(pinned),
        CommandOutput::ok(&format!("{pinned}\n")),
    );
    readback::read_agreement(&runner, pinned).expect("readback");
    assert!(
        runner
            .invocations()
            .iter()
            .any(|spec| spec.argv_contains("--out-path")),
        "must query closure store path"
    );
}

#[test]
fn test_activation_allows_only_once_per_candidate() {
    test_activation::reset_activation_ledger_for_tests();
    let runner = Arc::new(FakeCommandRunner::new());
    let capture = capture();
    runner.register(
        CommandSpec::nix_current_generation(),
        CommandOutput::ok("42\n"),
    );
    runner.register(CommandSpec::etc_git_head(), CommandOutput::ok("abc123\n"));
    runner.register(
        CommandSpec::config_digest(),
        CommandOutput::ok(valid_config_digest_hex()),
    );
    runner.register(
        CommandSpec::nixos_rebuild_test(&capture),
        CommandOutput::ok("/nix/store/candidate-closure tested\n"),
    );
    test_activation::activate_once(runner.as_ref(), &capture).expect("first");
    test_activation::activate_once(runner.as_ref(), &capture).expect_err("second");
}

#[test]
fn test_activation_rejects_moved_base() {
    test_activation::reset_activation_ledger_for_tests();
    let runner = Arc::new(FakeCommandRunner::new());
    let capture = capture();
    runner.register(
        CommandSpec::nix_current_generation(),
        CommandOutput::ok("99\n"),
    );
    runner.register(CommandSpec::etc_git_head(), CommandOutput::ok("abc123\n"));
    runner.register(
        CommandSpec::config_digest(),
        CommandOutput::ok(valid_config_digest_hex()),
    );
    runner.register(
        CommandSpec::nixos_rebuild_test(&capture),
        CommandOutput::ok("/nix/store/candidate-closure tested\n"),
    );
    test_activation::activate_once(runner.as_ref(), &capture).expect_err("moved base");
}

#[test]
fn capture_store_must_be_durably_persisted() {
    let dir = std::env::temp_dir().join(format!("agb6-capture-durable-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = CaptureStore::new(dir.clone());
    let changes = vec![ConfigFileChange {
        path: "/etc/nixos/demo.nix".to_owned(),
        content: "{}".to_owned(),
    }];
    store.store_active(&capture(), &changes).expect("store");
    assert!(dir.join("active.json.synced").exists());
}
