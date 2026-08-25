//! L02 hermetic adapter integration tests (RED → GREEN gate).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use agentbed_adapter_nix::adapter::NixAdapter;
use agentbed_adapter_nix::capture::CaptureStore;
use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_adapter_nix::promotion::{
    boot, build, flush, pin, profile, readback, test_activation, PromotionError,
};
use agentbed_adapter_nix::protected::{self, ProtectedRejectReason};
use agentbed_adapter_nix::probe;
use agentbed_adapter_nix::propose;
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::system_info::{
    HostSafety, RecoveryRequires, SafetySource, ServiceStateSafety,
};
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

fn benign_change() -> ConfigFileChange {
    ConfigFileChange {
        path: "/etc/nixos/services/demo.nix".to_owned(),
        content: "{ services.demo.enable = true; }".to_owned(),
    }
}

fn register_probe_commands(runner: &FakeCommandRunner) {
    runner.register(
        CommandSpec::nix_current_generation(),
        CommandOutput::ok("42\n"),
    );
    runner.register(
        CommandSpec::etc_git_head(),
        CommandOutput::ok("abc123\n"),
    );
    runner.register(
        CommandSpec::config_digest(),
        CommandOutput::ok(&"11".repeat(64)),
    );
}

#[test]
fn probe_reports_generation_only_when_verified() {
    let runner = Arc::new(FakeCommandRunner::new());
    register_probe_commands(runner.as_ref());
    let result = probe::probe(runner.as_ref()).expect("probe");
    assert!(result.adapter.resolved);
    assert_eq!(result.adapter.kind, "nix");
    assert_eq!(result.safety.root_config, HostSafety::Generation);
    assert_eq!(result.safety.packages, HostSafety::Generation);
    assert_eq!(result.safety.bootloader, HostSafety::None);
    assert_eq!(result.safety.kernel, HostSafety::None);
    assert_eq!(result.safety_source, SafetySource::AdapterProbe);
}

#[test]
fn probe_refuses_when_generation_missing() {
    let runner = Arc::new(FakeCommandRunner::new());
    runner.register(
        CommandSpec::nix_current_generation(),
        CommandOutput::err(1, ""),
    );
    let err = probe::probe(runner.as_ref()).expect_err("must refuse");
    assert!(matches!(err, probe::ProbeError::IncompleteObservation));
}

#[test]
fn protected_path_matrix_rejects_class_f_before_staging() {
    let cases: [(&str, &str, ProtectedRejectReason); 9] = [
        (
            "/etc/nixos/watchdogd/config.nix",
            "{}",
            ProtectedRejectReason::Watchdog,
        ),
        (
            "/var/lib/agentbed/wal/records/1.json",
            "{}",
            ProtectedRejectReason::BrokerWal,
        ),
        (
            "/var/lib/agentbed/rollback/precommit.json",
            "{}",
            ProtectedRejectReason::RollbackPath,
        ),
        (
            "/var/lib/agentbed/oob/state.json",
            "{}",
            ProtectedRejectReason::OobStore,
        ),
        (
            "/etc/nixos/agentbed/self.nix",
            "{}",
            ProtectedRejectReason::SelfProtection,
        ),
        (
            "/etc/nixos/configuration.nix",
            "{ boot.kernelPackages = pkgs.linuxPackages_latest; }",
            ProtectedRejectReason::Kernel,
        ),
        (
            "/etc/nixos/configuration.nix",
            "{ boot.loader.systemd-boot.enable = true; }",
            ProtectedRejectReason::Bootloader,
        ),
        (
            "/etc/nixos/configuration.nix",
            "{ networking.firewall.enable = true; }",
            ProtectedRejectReason::Firewall,
        ),
        (
            "/etc/nixos/../nixos/watchdogd/unit.nix",
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
        assert_eq!(err, expected, "path={path}");
    }
}

#[test]
fn propose_captures_immutable_candidate_and_replays_identically() {
    let runner = Arc::new(FakeCommandRunner::new());
    register_probe_commands(runner.as_ref());
    runner.register(
        CommandSpec::nix_eval_candidate(&[benign_change()]),
        CommandOutput::ok("/nix/store/candidate-closure\n"),
    );
    let base = base_revision();
    let first = propose::propose(runner.as_ref(), &[benign_change()], &base).expect("propose");
    let second = propose::propose(runner.as_ref(), &[benign_change()], &base).expect("replay");
    assert_eq!(first.capture.candidate_closure, second.capture.candidate_closure);
    assert_eq!(first.capture.base_revision, base);
    assert!(first.diff.contains("demo.nix"));
    assert_eq!(first.test_plan.adapter, "nix");
    assert_eq!(first.test_plan.steps, vec!["nixos-rebuild test"]);
}

#[test]
fn promotion_build_and_test_bind_to_capture() {
    let runner = Arc::new(FakeCommandRunner::new());
    let capture = propose::CapturedProposal {
        base_revision: base_revision(),
        candidate_closure: "/nix/store/candidate-closure".to_owned(),
        flake_ref: "/etc/nixos#agentbed".to_owned(),
        diff: "demo".to_owned(),
    };
    runner.register(
        CommandSpec::nixos_rebuild_build(&capture),
        CommandOutput::ok("built\n"),
    );
    runner.register(
        CommandSpec::nixos_rebuild_test(&capture),
        CommandOutput::ok("tested\n"),
    );
    build::build(runner.as_ref(), &capture).expect("build");
    test_activation::activate_once(runner.as_ref(), &capture).expect("test");
    let invocations = runner.invocations();
    assert!(invocations.iter().any(|c| c.argv_contains("build")));
    assert_eq!(
        invocations
            .iter()
            .filter(|c| c.argv_contains("test"))
            .count(),
        1
    );
}

#[test]
fn promotion_pin_profile_boot_flush_readback_happy_path() {
    let runner = Arc::new(FakeCommandRunner::new());
    let capture = propose::CapturedProposal {
        base_revision: base_revision(),
        candidate_closure: "/nix/store/candidate-closure".to_owned(),
        flake_ref: "/etc/nixos#agentbed".to_owned(),
        diff: "demo".to_owned(),
    };
    let pinned = "/nix/store/candidate-closure";
    runner.register(
        CommandSpec::nix_store_realise(pinned),
        CommandOutput::ok(&format!("{pinned}\n")),
    );
    runner.register(
        CommandSpec::nix_env_profile_set(pinned),
        CommandOutput::ok(""),
    );
    runner.register(
        CommandSpec::switch_to_configuration_boot(pinned),
        CommandOutput::ok(""),
    );
    runner.register(CommandSpec::sync_paths(), CommandOutput::ok(""));
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
    pin::pin_closure(runner.as_ref(), &capture).expect("pin");
    profile::advance_profile(runner.as_ref(), pinned).expect("profile");
    boot::configure_boot(runner.as_ref(), pinned).expect("boot");
    flush::flush_boundaries(runner.as_ref()).expect("flush");
    let agreement = readback::read_agreement(runner.as_ref(), pinned).expect("readback");
    assert!(agreement.profile_matches);
    assert!(agreement.boot_matches);
    assert!(agreement.closure_matches);
}

#[test]
fn promotion_failures_are_explicit_at_each_boundary() {
    let runner = Arc::new(FakeCommandRunner::new());
    let capture = propose::CapturedProposal {
        base_revision: base_revision(),
        candidate_closure: "/nix/store/candidate-closure".to_owned(),
        flake_ref: "/etc/nixos#agentbed".to_owned(),
        diff: "demo".to_owned(),
    };
    runner.register(
        CommandSpec::nixos_rebuild_build(&capture),
        CommandOutput::err(1, "build failed"),
    );
    let err = build::build(runner.as_ref(), &capture).expect_err("build");
    assert!(matches!(err, PromotionError::CommandFailed { .. }));

    runner.clear();
    runner.register(
        CommandSpec::nixos_rebuild_build(&capture),
        CommandOutput::ok("built\n"),
    );
    runner.register(
        CommandSpec::nixos_rebuild_test(&capture),
        CommandOutput::err(1, "test failed"),
    );
    build::build(runner.as_ref(), &capture).expect("build ok");
    let err = test_activation::activate_once(runner.as_ref(), &capture).expect_err("test");
    assert!(matches!(err, PromotionError::CommandFailed { .. }));
}

#[test]
fn readback_detects_profile_boot_mismatch() {
    let runner = Arc::new(FakeCommandRunner::new());
    let pinned = "/nix/store/candidate-closure";
    runner.register(
        CommandSpec::read_profile_target(),
        CommandOutput::ok("/nix/store/other\n"),
    );
    runner.register(
        CommandSpec::read_boot_default(),
        CommandOutput::ok(&format!("{pinned}\n")),
    );
    runner.register(
        CommandSpec::read_closure_hash(pinned),
        CommandOutput::ok("hash-abc\n"),
    );
    let err = readback::read_agreement(runner.as_ref(), pinned).expect_err("mismatch");
    assert!(matches!(err, PromotionError::AgreementMismatch { .. }));
}

#[test]
fn fake_runner_never_invokes_live_nixos_rebuild() {
    let runner = Arc::new(FakeCommandRunner::new());
    assert!(runner.forbids_live_commands());
}

#[test]
fn nix_adapter_host_surface_matches_probe() {
    let runner = Arc::new(FakeCommandRunner::new());
    register_probe_commands(runner.as_ref());
    let store = CaptureStore::new(PathBuf::from("/tmp/agb6-capture"));
    let adapter = NixAdapter::new(runner, store);
    let info = adapter.info();
    assert!(info.resolved);
    assert_eq!(info.kind, "nix");
    let safety = adapter.safety_vector();
    assert_eq!(safety.root_config, HostSafety::Generation);
    assert_eq!(safety.recovery_requires, RecoveryRequires::RemoteReboot);
}

#[test]
fn promotion_module_has_no_forbidden_switch_commands() {
    agentbed_adapter_nix::promotion::assert_no_forbidden_live_activation_commands();
}
