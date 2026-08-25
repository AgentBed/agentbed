//! Review-4 repair regression tests (review #5019424546).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::needless_borrows_for_generic_args
)]

use agentbed_adapter_nix::capture::{CaptureError, CaptureStore, PathSync, StdPathSync};
use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_adapter_nix::promotion::{test_activation, PromotionError};
use agentbed_adapter_nix::propose;
use agentbed_adapter_nix::protected::{self, ProtectedRejectReason};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::transaction::BaseRevision;
use agentbed_protocol::wire::ConfigFileChange;
use std::path::Path;
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
        "agb6-review4-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
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
fn protected_rejects_fully_quoted_boot_kernel_attrpath() {
    let change = ConfigFileChange {
        path: "/etc/nixos/kernel.nix".to_owned(),
        content: "{ \"boot\".\"kernelPackages\" = pkgs.linuxPackages_latest; }".to_owned(),
    };
    let err =
        protected::check_protected_changes(&[change]).expect_err("fully quoted kernel");
    assert_eq!(err, ProtectedRejectReason::Kernel);
}

#[test]
fn protected_rejects_fully_quoted_boot_loader_grub_attrpath() {
    let change = ConfigFileChange {
        path: "/etc/nixos/boot.nix".to_owned(),
        content: "{ \"boot\".\"loader\".grub.enable = true; }".to_owned(),
    };
    let err =
        protected::check_protected_changes(&[change]).expect_err("fully quoted loader");
    assert_eq!(err, ProtectedRejectReason::Bootloader);
}

#[test]
fn protected_rejects_fully_quoted_networking_firewall_attrpath() {
    let change = ConfigFileChange {
        path: "/etc/nixos/fw.nix".to_owned(),
        content: "{ \"networking\".\"firewall\".enable = true; }".to_owned(),
    };
    let err =
        protected::check_protected_changes(&[change]).expect_err("fully quoted firewall");
    assert_eq!(err, ProtectedRejectReason::Firewall);
}

#[test]
fn protected_rejects_mixed_quoted_unquoted_kernel_attrpath() {
    let change = ConfigFileChange {
        path: "/etc/nixos/kernel.nix".to_owned(),
        content: "{ \"boot\".kernelPackages = pkgs.linuxPackages_latest; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("mixed quoted kernel");
    assert_eq!(err, ProtectedRejectReason::Kernel);
}

#[test]
fn protected_rejects_dynamic_fully_quoted_attrpath() {
    let change = ConfigFileChange {
        path: "/etc/nixos/dynamic.nix".to_owned(),
        content: "{ \"boot\".${pkg} = pkgs.linuxPackages_latest; }".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("dynamic quoted");
    assert_eq!(err, ProtectedRejectReason::DynamicExpression);
}

#[test]
fn protected_allows_fully_quoted_kernel_decoy_in_comment() {
    let change = ConfigFileChange {
        path: "/etc/nixos/safe.nix".to_owned(),
        content: "# \"boot\".\"kernelPackages\" = pkgs.linuxPackages_latest;\n{ services.nginx.enable = true; }"
            .to_owned(),
    };
    protected::check_protected_changes(&[change]).expect("comment decoy");
}

#[test]
fn protected_allows_fully_quoted_kernel_decoy_as_value() {
    let change = ConfigFileChange {
        path: "/etc/nixos/safe.nix".to_owned(),
        content: "{ description = \"boot.kernelPackages decoy\"; }".to_owned(),
    };
    protected::check_protected_changes(&[change]).expect("value decoy");
}

#[derive(Debug)]
struct FailRootDirSync(PathBuf);

impl PathSync for FailRootDirSync {
    fn sync_path(&self, path: &Path) -> Result<(), CaptureError> {
        StdPathSync.sync_path(path)
    }

    fn sync_parent(&self, path: &Path) -> Result<(), CaptureError> {
        StdPathSync.sync_parent(path)
    }

    fn sync_dir(&self, path: &Path) -> Result<(), CaptureError> {
        if path == self.0.as_path() {
            return Err(CaptureError::Io);
        }
        StdPathSync.sync_dir(path)
    }
}

#[test]
fn activation_skips_probe_when_root_parent_fsync_fails() {
    let dir = scratch("activation-root-fsync");
    let store = CaptureStore::with_syncer(dir.clone(), Arc::new(FailRootDirSync(dir.clone())));
    let runner = Arc::new(FakeCommandRunner::new());
    register_probe(runner.as_ref());
    let proposal = capture("/nix/store/root-parent-fsync");
    runner.register(
        CommandSpec::nixos_rebuild_test(&proposal),
        CommandOutput::ok("/nix/store/root-parent-fsync tested\n"),
    );
    let err =
        test_activation::activate_once(runner.as_ref(), &proposal, &store).expect_err("root fsync");
    assert_eq!(err, PromotionError::ReservationNotDurable);
    assert!(
        runner.invocations().is_empty(),
        "must not invoke probe or test before durable activations parent entry"
    );
}
