use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;

pub fn configure_boot(runner: &dyn CommandRunner, pinned: &str) -> Result<(), PromotionError> {
    runner.run(&CommandSpec::switch_to_configuration_boot(pinned))?;
    Ok(())
}
