use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;

pub fn flush_boundaries(runner: &dyn CommandRunner) -> Result<(), PromotionError> {
    runner.run(&CommandSpec::sync_profile_boot_boundaries())?;
    Ok(())
}
