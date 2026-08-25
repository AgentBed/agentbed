use crate::capture::CaptureStore;
use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::{profile, PromotionError};
use crate::propose::CapturedProposal;

pub fn configure_boot(
    runner: &dyn CommandRunner,
    capture: &CapturedProposal,
    store: &CaptureStore,
) -> Result<(), PromotionError> {
    let pinned = profile::verified_pin(store, capture)?;
    runner.run(&CommandSpec::switch_to_configuration_boot(&pinned))?;
    Ok(())
}
