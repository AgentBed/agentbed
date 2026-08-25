use crate::capture::CaptureStore;
use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;
use crate::propose::CapturedProposal;

pub fn advance_profile(
    runner: &dyn CommandRunner,
    capture: &CapturedProposal,
    store: &CaptureStore,
) -> Result<(), PromotionError> {
    let pinned = verified_pin(store, capture)?;
    runner.run(&CommandSpec::nix_env_profile_set(&pinned))?;
    Ok(())
}

pub(crate) fn verified_pin(
    store: &CaptureStore,
    capture: &CapturedProposal,
) -> Result<String, PromotionError> {
    match store.load_verified_pin(capture) {
        Ok(closure) => Ok(closure),
        Err(crate::capture::CaptureError::Conflict) => {
            if store.root().join("pin.json").exists() {
                Err(PromotionError::PinMismatch {
                    expected: capture.candidate_closure.clone(),
                    actual: "corrupt or mismatched pin record".to_owned(),
                })
            } else {
                Err(PromotionError::PinRequired)
            }
        }
        Err(crate::capture::CaptureError::Io) => Err(PromotionError::PinMismatch {
            expected: capture.candidate_closure.clone(),
            actual: "pin readback failed".to_owned(),
        }),
    }
}
