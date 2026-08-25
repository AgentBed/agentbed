//! Promotion primitives for build, test, pin, profile, boot, flush, readback.

pub mod boot;
pub mod build;
pub mod flush;
pub mod pin;
pub mod profile;
pub mod readback;
pub mod test_activation;

use crate::command_runner::CommandError;

/// Promotion boundary failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionError {
    CommandFailed {
        code: i32,
        stderr: String,
    },
    AgreementMismatch {
        profile: String,
        boot: String,
        expected: String,
    },
    ClosureMismatch {
        expected: String,
        actual: String,
    },
    AlreadyActivated,
    BaseMoved,
    NotRegistered,
}

impl From<CommandError> for PromotionError {
    fn from(value: CommandError) -> Self {
        match value {
            CommandError::NotRegistered => Self::NotRegistered,
            CommandError::NonZeroExit { code, stderr } => Self::CommandFailed { code, stderr },
            CommandError::Timeout => Self::CommandFailed {
                code: -1,
                stderr: "timeout".to_owned(),
            },
            CommandError::Interrupted => Self::CommandFailed {
                code: -2,
                stderr: "interrupted".to_owned(),
            },
        }
    }
}

/// Static scan proving promotion never invokes forbidden live activation commands.
pub fn assert_no_forbidden_live_activation_commands() {
    const FORBIDDEN: &[&str] = &[
        "nixos-rebuild switch",
        "switch-to-configuration switch",
        "systemctl",
        "systemd-run",
    ];
    let sources = [
        include_str!("build.rs"),
        include_str!("test_activation.rs"),
        include_str!("pin.rs"),
        include_str!("profile.rs"),
        include_str!("boot.rs"),
        include_str!("flush.rs"),
        include_str!("readback.rs"),
    ];
    for source in sources {
        for needle in FORBIDDEN {
            assert!(
                !source.contains(needle),
                "forbidden command fragment found: {needle}"
            );
        }
    }
}
