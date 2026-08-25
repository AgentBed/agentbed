//! Review-5 repair regression tests (review #5019548471).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::needless_borrows_for_generic_args
)]

use agentbed_adapter_nix::capture::{CaptureError, CaptureStore, PathSync, StdPathSync};
use agentbed_adapter_nix::command_runner::{CommandOutput, CommandSpec, FakeCommandRunner};
use agentbed_adapter_nix::promotion::{test_activation, PromotionError};
use agentbed_adapter_nix::propose;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        "agb6-review5-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch");
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

fn result_md_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plans/AGB-6/RESULT.md")
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
fn activation_skips_probe_when_interrupted_creator_left_activations_without_root_sync() {
    let dir = scratch("interrupted-creator");
    fs::create_dir_all(dir.join("activations")).expect("pre-create activations");
    let store = CaptureStore::with_syncer(dir.clone(), Arc::new(FailRootDirSync(dir.clone())));
    let runner = Arc::new(FakeCommandRunner::new());
    register_probe(runner.as_ref());
    let proposal = capture("/nix/store/interrupted-creator");
    runner.register(
        CommandSpec::nixos_rebuild_test(&proposal),
        CommandOutput::ok("/nix/store/interrupted-creator tested\n"),
    );
    let err =
        test_activation::activate_once(runner.as_ref(), &proposal, &store).expect_err("root fsync");
    assert_eq!(err, PromotionError::ReservationNotDurable);
    assert!(
        runner.invocations().is_empty(),
        "must not invoke probe or test when root durability cannot be proven"
    );
}

#[test]
fn result_md_maps_all_l02_acceptance_ids_in_traceability_table() {
    let content = fs::read_to_string(result_md_path()).expect("RESULT.md");
    assert!(
        content.contains("## Acceptance traceability"),
        "RESULT.md must include acceptance traceability section"
    );
    for id in 1..=10 {
        let needle = format!("**L02-AC{id:02}**");
        assert!(
            content.contains(&needle),
            "RESULT.md traceability must include row for {needle}"
        );
    }
    assert!(
        content.contains("## Residual gaps"),
        "RESULT.md must document residual gaps"
    );
    assert!(
        content.contains("L02-AC10"),
        "RESULT.md must document hard non-goals"
    );
}
