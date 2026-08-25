use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;
use crate::propose::CapturedProposal;

pub fn build(runner: &dyn CommandRunner, capture: &CapturedProposal) -> Result<(), PromotionError> {
    let output = runner.run(&CommandSpec::nixos_rebuild_build(capture))?;
    let stdout = output.stdout.trim();
    if !stdout.contains(&capture.candidate_closure) {
        return Err(PromotionError::ClosureMismatch {
            expected: capture.candidate_closure.clone(),
            actual: stdout.to_owned(),
        });
    }
    Ok(())
}
