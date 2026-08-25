use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;
use crate::propose::CapturedProposal;

pub fn build(runner: &dyn CommandRunner, capture: &CapturedProposal) -> Result<(), PromotionError> {
    runner.run(&CommandSpec::nixos_rebuild_build(capture))?;
    Ok(())
}
