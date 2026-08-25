use crate::command_runner::{CommandRunner, CommandSpec};
use crate::promotion::PromotionError;

/// Agreement between profile target, boot default, and pinned closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agreement {
    pub profile_matches: bool,
    pub boot_matches: bool,
    pub closure_matches: bool,
}

pub fn read_agreement(
    runner: &dyn CommandRunner,
    pinned: &str,
) -> Result<Agreement, PromotionError> {
    let profile = runner
        .run(&CommandSpec::read_profile_target())?
        .stdout
        .trim()
        .to_owned();
    let boot = runner
        .run(&CommandSpec::read_boot_default())?
        .stdout
        .trim()
        .to_owned();
    let store_path = runner
        .run(&CommandSpec::read_closure_store_path(pinned))?
        .stdout
        .trim()
        .to_owned();
    let _hash = runner.run(&CommandSpec::read_closure_hash(pinned))?;
    let profile_matches = profile == pinned;
    let boot_matches = boot == pinned;
    let closure_matches = store_path == pinned;
    if !profile_matches || !boot_matches || !closure_matches {
        return Err(PromotionError::AgreementMismatch {
            profile,
            boot,
            expected: pinned.to_owned(),
        });
    }
    Ok(Agreement {
        profile_matches,
        boot_matches,
        closure_matches,
    })
}
