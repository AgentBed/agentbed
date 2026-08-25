use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;
use crate::propose::CapturedProposal;

pub fn pin_closure(
    runner: &dyn CommandRunner,
    capture: &CapturedProposal,
) -> Result<String, PromotionError> {
    let output = runner.run(&CommandSpec::nix_store_realise(&capture.candidate_closure))?;
    let realised = output.stdout.trim().to_owned();
    if realised != capture.candidate_closure {
        return Err(PromotionError::ClosureMismatch {
            expected: capture.candidate_closure.clone(),
            actual: realised,
        });
    }
    Ok(realised)
}
