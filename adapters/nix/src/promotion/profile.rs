use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;
use crate::propose::CapturedProposal;

pub fn advance_profile(
    runner: &dyn CommandRunner,
    capture: &CapturedProposal,
    pinned: &str,
) -> Result<(), PromotionError> {
    if pinned != capture.candidate_closure {
        return Err(PromotionError::ClosureMismatch {
            expected: capture.candidate_closure.clone(),
            actual: pinned.to_owned(),
        });
    }
    runner.run(&CommandSpec::nix_env_profile_set(pinned))?;
    Ok(())
}
