//! Deterministic Nix proposal and immutable capture.

use crate::capture::{CaptureError, CaptureStore};
use crate::command_runner::{CommandError, CommandRunner, CommandSpec};
use agentbed_protocol::dto::transaction::{BaseRevision, TestPlan};
use agentbed_protocol::wire::ConfigFileChange;

/// Immutable capture of a proposed candidate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CapturedProposal {
    pub base_revision: BaseRevision,
    pub candidate_closure: String,
    pub flake_ref: String,
    pub diff: String,
}

/// Successful proposal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalResult {
    pub capture: CapturedProposal,
    pub diff: String,
    pub test_plan: TestPlan,
    pub affected_resources: Vec<String>,
}

/// Proposal failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposeError {
    Command(CommandError),
    Capture(CaptureError),
}

/// Produce a deterministic proposal bound to the active base revision.
pub fn propose(
    runner: &dyn CommandRunner,
    changes: &[ConfigFileChange],
    base: &BaseRevision,
) -> Result<ProposalResult, ProposeError> {
    let closure = runner
        .run(&CommandSpec::nix_eval_candidate(changes))
        .map_err(ProposeError::Command)?
        .stdout
        .trim()
        .to_owned();
    let diff = changes
        .iter()
        .map(|c| format!("{}: {}", c.path, c.content))
        .collect::<Vec<_>>()
        .join("\n");
    let capture = CapturedProposal {
        base_revision: base.clone(),
        candidate_closure: closure,
        flake_ref: "/etc/nixos#agentbed".to_owned(),
        diff: diff.clone(),
    };
    Ok(ProposalResult {
        capture,
        diff,
        test_plan: TestPlan {
            adapter: "nix".to_owned(),
            steps: vec!["nixos-rebuild test".to_owned()],
        },
        affected_resources: vec!["root_config".to_owned()],
    })
}

/// Propose and persist capture, refusing conflicting reuse.
pub fn propose_and_capture(
    runner: &dyn CommandRunner,
    store: &CaptureStore,
    changes: &[ConfigFileChange],
    base: &BaseRevision,
) -> Result<ProposalResult, ProposeError> {
    if store
        .load_active()
        .map_err(ProposeError::Capture)?
        .is_some()
    {
        return Err(ProposeError::Capture(CaptureError::Conflict));
    }
    let result = propose(runner, changes, base)?;
    store
        .store_active(&result.capture, changes)
        .map_err(ProposeError::Capture)?;
    Ok(result)
}
