use crate::capture::CaptureStore;
use crate::command_runner::{CommandRunner, CommandSpec};
use crate::probe;
use crate::promotion::PromotionError;
use crate::propose::CapturedProposal;

pub fn activate_once(
    runner: &dyn CommandRunner,
    capture: &CapturedProposal,
    store: &CaptureStore,
) -> Result<(), PromotionError> {
    store
        .reserve_activation(&capture.candidate_closure)
        .map_err(|_| PromotionError::AlreadyActivated)?;

    let Ok(current) = probe::probe(runner) else {
        store.release_activation_reservation(&capture.candidate_closure);
        return Err(PromotionError::BaseMoved);
    };
    if current.base_revision != capture.base_revision {
        store.release_activation_reservation(&capture.candidate_closure);
        return Err(PromotionError::BaseMoved);
    }

    let test_result = runner.run(&CommandSpec::nixos_rebuild_test(capture));
    store
        .finalize_activation(&capture.candidate_closure)
        .map_err(|_| PromotionError::AlreadyActivated)?;

    match test_result {
        Ok(output) => {
            let stdout = output.stdout.trim();
            if !stdout.contains(&capture.candidate_closure) {
                return Err(PromotionError::ClosureMismatch {
                    expected: capture.candidate_closure.clone(),
                    actual: stdout.to_owned(),
                });
            }
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
