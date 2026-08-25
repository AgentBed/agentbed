use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;

pub fn advance_profile(runner: &dyn CommandRunner, pinned: &str) -> Result<(), PromotionError> {
    runner.run(&CommandSpec::nix_env_profile_set(pinned))?;
    Ok(())
}
