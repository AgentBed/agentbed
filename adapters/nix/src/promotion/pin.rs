use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;
use crate::propose::CapturedProposal;

pub fn pin_closure(
    runner: &dyn CommandRunner,
    capture: &CapturedProposal,
) -> Result<String, PromotionError> {
    let output = runner.run(&CommandSpec::nix_store_realise(&capture.candidate_closure))?;
    Ok(output.stdout.trim().to_owned())
}
