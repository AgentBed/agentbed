//! Review-3 repair regression tests (review #5019192819).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::needless_borrows_for_generic_args
)]

use agentbed_adapter_nix::capture::{CaptureStore, FailSyncOn, StdPathSync};
use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_adapter_nix::promotion::{boot, pin, profile, test_activation, PromotionError};
use agentbed_adapter_nix::propose;
use agentbed_adapter_nix::protected::{self, ProtectedRejectReason};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::BaseRevision;
use agentbed_protocol::wire::ConfigFileChange;
use std::path::PathBuf;
use std::sync::Arc;

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

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agb6-review3-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
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
fn protected_rejects_quoted_boot_kernel_packages() {
    let change = ConfigFileChange {
        path: "/etc/nixos/kernel.nix".to_owned(),
        content: "{ boot.\"kernelPackages\" = pkgs.linuxPackages_latest; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("quoted kernel");
    assert_eq!(err, ProtectedRejectReason::Kernel);
}

#[test]
fn protected_rejects_quoted_boot_loader_grub() {
    let change = ConfigFileChange {
        path: "/etc/nixos/boot.nix".to_owned(),
        content: "{ boot.\"loader\".grub.enable = true; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("quoted loader");
    assert_eq!(err, ProtectedRejectReason::Bootloader);
}

#[test]
fn protected_rejects_quoted_networking_firewall() {
    let change = ConfigFileChange {
        path: "/etc/nixos/fw.nix".to_owned(),
        content: "{ networking.\"firewall\".enable = true; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("quoted firewall");
    assert_eq!(err, ProtectedRejectReason::Firewall);
}

#[test]
fn protected_rejects_dynamic_kernel_attribute_expression() {
    let change = ConfigFileChange {
        path: "/etc/nixos/dynamic.nix".to_owned(),
        content: "{ boot.${pkg} = pkgs.linuxPackages_latest; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("dynamic");
    assert_eq!(err, ProtectedRejectReason::DynamicExpression);
}

#[test]
fn protected_allows_kernel_string_decoy_in_comment() {
    let change = ConfigFileChange {
        path: "/etc/nixos/safe.nix".to_owned(),
        content:
            "# boot.kernelPackages = pkgs.linuxPackages_latest;\n{ services.nginx.enable = true; }"
                .to_owned(),
    };
    protected::check_protected_changes(&[change]).expect("comment decoy");
}

#[test]
fn protected_allows_kernel_string_decoy_as_value() {
    let change = ConfigFileChange {
        path: "/etc/nixos/safe.nix".to_owned(),
        content: "{ description = \"boot.kernelPackages decoy\"; }".to_owned(),
    };
    protected::check_protected_changes(&[change]).expect("value decoy");
}

#[test]
fn activation_skips_command_when_reservation_fsync_fails() {
    let dir = scratch("activation-fsync");
    let activations = dir.join("activations");
    let store = CaptureStore::with_syncer(
        dir,
        Arc::new(FailSyncOn::new(vec![activations.clone()], StdPathSync)),
    );
    let runner = Arc::new(FakeCommandRunner::new());
    register_probe(runner.as_ref());
    let proposal = capture("/nix/store/fsync-reservation");
    runner.register(
        CommandSpec::nixos_rebuild_test(&proposal),
        CommandOutput::ok("/nix/store/fsync-reservation tested\n"),
    );
    let err =
        test_activation::activate_once(runner.as_ref(), &proposal, &store).expect_err("fsync fail");
    assert_eq!(err, PromotionError::ReservationNotDurable);
    assert!(
        runner
            .invocations()
            .iter()
            .all(|spec| !spec.argv_contains("test")),
        "must not invoke nixos-rebuild test before durable reservation"
    );
}

#[test]
fn profile_rejects_without_durable_pin() {
    let store = CaptureStore::new(scratch("profile-no-pin"));
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/profile-no-pin");
    let err = profile::advance_profile(&runner, &proposal, &store).expect_err("no pin");
    assert_eq!(err, PromotionError::PinRequired);
    assert!(runner.invocations().is_empty(), "must not advance profile");
}

#[test]
fn profile_rejects_capture_without_matching_pin() {
    let store = CaptureStore::new(scratch("profile-bypass"));
    let runner = FakeCommandRunner::new();
    let pinned_capture = capture("/nix/store/profile-bypass");
    let other_capture = capture("/nix/store/other-closure");
    runner.register(
        CommandSpec::nix_store_realise(&pinned_capture.candidate_closure),
        CommandOutput::ok("/nix/store/profile-bypass\n"),
    );
    pin::pin_closure(&runner, &pinned_capture, &store).expect("pin");
    let err = profile::advance_profile(&runner, &other_capture, &store).expect_err("bypass");
    assert!(matches!(err, PromotionError::PinMismatch { .. }));
}

#[test]
fn boot_rejects_without_durable_pin() {
    let store = CaptureStore::new(scratch("boot-no-pin"));
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/boot-no-pin");
    let err = boot::configure_boot(&runner, &proposal, &store).expect_err("no pin");
    assert_eq!(err, PromotionError::PinRequired);
    assert!(runner.invocations().is_empty(), "must not configure boot");
}

#[test]
fn boot_rejects_corrupted_pin_record() {
    let dir = scratch("boot-corrupt-pin");
    let store = CaptureStore::new(dir.clone());
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/boot-corrupt");
    runner.register(
        CommandSpec::nix_store_realise(&proposal.candidate_closure),
        CommandOutput::ok("/nix/store/boot-corrupt\n"),
    );
    pin::pin_closure(&runner, &proposal, &store).expect("pin");
    std::fs::write(
        dir.join("pin.json"),
        r#"{"candidate_closure":"/nix/store/other","base_generation":"42","base_git_commit":"abc123","base_config_digest":"11111111111111111111111111111111","realised_closure":"/nix/store/other"}"#,
    )
    .expect("corrupt");
    let err = boot::configure_boot(&runner, &proposal, &store).expect_err("corrupt pin");
    assert!(matches!(err, PromotionError::PinMismatch { .. }));
}
