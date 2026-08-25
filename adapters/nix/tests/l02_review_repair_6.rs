//! Review-6 repair regression tests (review #5019682481).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::needless_borrows_for_generic_args
)]

use agentbed_adapter_nix::capture::CaptureStore;
use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_adapter_nix::promotion::{boot, pin, profile, PromotionError};
use agentbed_adapter_nix::propose;
use std::path::PathBuf;

fn base_revision() -> agentbed_protocol::dto::transaction::BaseRevision {
    agentbed_protocol::dto::transaction::BaseRevision {
        generation: Some("42".to_owned()),
        etc_git_commit: "abc123".to_owned(),
        config_digest: agentbed_protocol::digest::Digest::from_sha256_bytes([0x11; 32]),
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
        "agb6-review6-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn pin_candidate(
    runner: &FakeCommandRunner,
    store: &CaptureStore,
    proposal: &propose::CapturedProposal,
) {
    runner.register(
        CommandSpec::nix_store_realise(&proposal.candidate_closure),
        CommandOutput::ok(format!("{}\n", proposal.candidate_closure)),
    );
    pin::pin_closure(runner, proposal, store).expect("pin");
}

fn is_boot_invocation(spec: &CommandSpec) -> bool {
    spec.argv
        .get(1)
        .is_some_and(|arg| arg == "boot")
        && spec.executable.contains("switch-to-configuration")
}

fn profile_read_index(invocations: &[CommandSpec]) -> Option<usize> {
    invocations
        .iter()
        .position(|spec| spec == &CommandSpec::read_profile_target())
}

fn boot_read_index(invocations: &[CommandSpec]) -> Option<usize> {
    invocations.iter().position(is_boot_invocation)
}

#[test]
fn boot_rejects_when_profile_not_advanced_to_pin() {
    let store = CaptureStore::new(scratch("boot-no-profile-advance"));
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/profile-before-boot");
    pin_candidate(&runner, &store, &proposal);
    runner.register(
        CommandSpec::read_profile_target(),
        CommandOutput::ok("/nix/store/stale-profile\n"),
    );
    runner.register(
        CommandSpec::switch_to_configuration_boot(&proposal.candidate_closure),
        CommandOutput::ok(""),
    );
    let err = boot::configure_boot(&runner, &proposal, &store).expect_err("stale profile");
    assert!(matches!(err, PromotionError::ProfileMismatch { .. }));
    assert!(
        runner
            .invocations()
            .iter()
            .all(|spec| !is_boot_invocation(spec)),
        "must not invoke switch-to-configuration boot before verified profile"
    );
}

#[test]
fn boot_rejects_when_profile_target_missing() {
    let store = CaptureStore::new(scratch("boot-missing-profile"));
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/missing-profile-target");
    pin_candidate(&runner, &store, &proposal);
    runner.register(
        CommandSpec::switch_to_configuration_boot(&proposal.candidate_closure),
        CommandOutput::ok(""),
    );
    let err = boot::configure_boot(&runner, &proposal, &store).expect_err("missing profile");
    assert_eq!(err, PromotionError::NotRegistered);
    assert!(
        runner
            .invocations()
            .iter()
            .all(|spec| !is_boot_invocation(spec)),
        "must not invoke boot when profile readback is unavailable"
    );
}

#[test]
fn boot_rejects_when_profile_readback_malformed() {
    let store = CaptureStore::new(scratch("boot-malformed-profile"));
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/malformed-profile");
    pin_candidate(&runner, &store, &proposal);
    runner.register(
        CommandSpec::read_profile_target(),
        CommandOutput::ok("   \n"),
    );
    runner.register(
        CommandSpec::switch_to_configuration_boot(&proposal.candidate_closure),
        CommandOutput::ok(""),
    );
    let err = boot::configure_boot(&runner, &proposal, &store).expect_err("malformed profile");
    assert!(matches!(err, PromotionError::ProfileMismatch { .. }));
    assert!(
        runner
            .invocations()
            .iter()
            .all(|spec| !is_boot_invocation(spec)),
        "must not invoke boot on malformed profile readback"
    );
}

#[test]
fn profile_rejects_post_set_readback_mismatch() {
    let store = CaptureStore::new(scratch("profile-post-set-mismatch"));
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/post-set-mismatch");
    pin_candidate(&runner, &store, &proposal);
    runner.register(
        CommandSpec::nix_env_profile_set(&proposal.candidate_closure),
        CommandOutput::ok(""),
    );
    runner.register(
        CommandSpec::read_profile_target(),
        CommandOutput::ok("/nix/store/still-stale\n"),
    );
    let err = profile::advance_profile(&runner, &proposal, &store).expect_err("post-set mismatch");
    assert!(matches!(err, PromotionError::ProfileMismatch { .. }));
}

#[test]
fn profile_advances_when_post_set_readback_matches() {
    let store = CaptureStore::new(scratch("profile-post-set-ok"));
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/post-set-ok");
    pin_candidate(&runner, &store, &proposal);
    runner.register(
        CommandSpec::nix_env_profile_set(&proposal.candidate_closure),
        CommandOutput::ok(""),
    );
    runner.register(
        CommandSpec::read_profile_target(),
        CommandOutput::ok(format!("{}\n", proposal.candidate_closure)),
    );
    profile::advance_profile(&runner, &proposal, &store).expect("verified profile advance");
    let invocations = runner.invocations();
    assert!(
        invocations
            .iter()
            .any(|spec| spec == &CommandSpec::nix_env_profile_set(&proposal.candidate_closure)),
        "must invoke nix-env --set"
    );
    assert!(
        profile_read_index(&invocations).is_some(),
        "must verify profile readback after nix-env --set"
    );
}

#[test]
fn boot_configures_when_profile_matches_pin() {
    let store = CaptureStore::new(scratch("boot-profile-verified"));
    let runner = FakeCommandRunner::new();
    let proposal = capture("/nix/store/boot-profile-verified");
    pin_candidate(&runner, &store, &proposal);
    runner.register(
        CommandSpec::read_profile_target(),
        CommandOutput::ok(format!("{}\n", proposal.candidate_closure)),
    );
    runner.register(
        CommandSpec::switch_to_configuration_boot(&proposal.candidate_closure),
        CommandOutput::ok(""),
    );
    boot::configure_boot(&runner, &proposal, &store).expect("verified boot");
    let invocations = runner.invocations();
    let profile_idx = profile_read_index(&invocations).expect("profile readback required");
    let boot_idx = boot_read_index(&invocations).expect("boot invocation required");
    assert!(
        profile_idx < boot_idx,
        "profile must be verified before switch-to-configuration boot"
    );
}
