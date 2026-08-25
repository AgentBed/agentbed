use crate::command_runner::{CommandRunner, CommandSpec};
use crate::probe;
use crate::promotion::PromotionError;
use crate::propose::CapturedProposal;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

fn activated() -> &'static Mutex<HashSet<String>> {
    static LEDGER: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Reset activation ledger between hermetic tests.
pub fn reset_activation_ledger_for_tests() {
    activated().lock().expect("ledger").clear();
}

pub fn activate_once(
    runner: &dyn CommandRunner,
    capture: &CapturedProposal,
) -> Result<(), PromotionError> {
    let key = capture.candidate_closure.clone();
    {
        let ledger = activated().lock().expect("ledger");
        if ledger.contains(&key) {
            return Err(PromotionError::AlreadyActivated);
        }
    }

    let current = probe::probe(runner).map_err(|_| PromotionError::BaseMoved)?;
    if current.base_revision != capture.base_revision {
        return Err(PromotionError::BaseMoved);
    }

    let output = runner.run(&CommandSpec::nixos_rebuild_test(capture))?;
    let stdout = output.stdout.trim();
    if !stdout.contains(&capture.candidate_closure) {
        return Err(PromotionError::ClosureMismatch {
            expected: capture.candidate_closure.clone(),
            actual: stdout.to_owned(),
        });
    }

    activated().lock().expect("ledger").insert(key);
    Ok(())
}
