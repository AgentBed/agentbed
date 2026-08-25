//! Injected command runner boundary for hermetic Nix adapter tests.

use agentbed_protocol::wire::ConfigFileChange;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Fixed executable paths — no ambient PATH guessing.
const NIXOS_REBUILD: &str = "/run/current-system/sw/bin/nixos-rebuild";
const NIX: &str = "/run/current-system/sw/bin/nix";
const NIX_STORE: &str = "/run/current-system/sw/bin/nix-store";
const NIX_ENV: &str = "/run/current-system/sw/bin/nix-env";
const SYNC: &str = "/run/current-system/sw/bin/sync";
const READLINK: &str = "/run/current-system/sw/bin/readlink";
const CAT: &str = "/run/current-system/sw/bin/cat";

/// Specification of a command the adapter may invoke.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandSpec {
    pub executable: String,
    pub argv: Vec<String>,
    pub working_dir: Option<PathBuf>,
}

impl CommandSpec {
    pub fn nix_current_generation() -> Self {
        Self {
            executable: READLINK.to_owned(),
            argv: vec![
                READLINK.to_owned(),
                "-f".to_owned(),
                "/nix/var/nix/profiles/system".to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn etc_git_head() -> Self {
        Self {
            executable: CAT.to_owned(),
            argv: vec![
                CAT.to_owned(),
                "/etc/.git/refs/heads/agentbed-base".to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn config_digest() -> Self {
        Self {
            executable: NIX.to_owned(),
            argv: vec![
                NIX.to_owned(),
                "hash".to_owned(),
                "file".to_owned(),
                "/etc/nixos".to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn nix_eval_candidate(changes: &[ConfigFileChange]) -> Self {
        let mut argv = vec![
            NIX.to_owned(),
            "eval".to_owned(),
            "--raw".to_owned(),
            "/etc/nixos#agentbed.candidate".to_owned(),
        ];
        for change in changes {
            argv.push("--argstr".to_owned());
            argv.push(change.path.clone());
            argv.push(change.content.clone());
        }
        Self {
            executable: NIX.to_owned(),
            argv,
            working_dir: Some(PathBuf::from("/etc/nixos")),
        }
    }

    pub fn nixos_rebuild_build(capture: &crate::propose::CapturedProposal) -> Self {
        Self {
            executable: NIXOS_REBUILD.to_owned(),
            argv: vec![
                NIXOS_REBUILD.to_owned(),
                "build".to_owned(),
                "--flake".to_owned(),
                capture.flake_ref.clone(),
            ],
            working_dir: Some(PathBuf::from("/etc/nixos")),
        }
    }

    pub fn nixos_rebuild_test(capture: &crate::propose::CapturedProposal) -> Self {
        Self {
            executable: NIXOS_REBUILD.to_owned(),
            argv: vec![
                NIXOS_REBUILD.to_owned(),
                "test".to_owned(),
                "--flake".to_owned(),
                capture.flake_ref.clone(),
                "--option".to_owned(),
                "boot.loader.grub.default".to_owned(),
                capture.base_revision.generation.clone().unwrap_or_default(),
            ],
            working_dir: Some(PathBuf::from("/etc/nixos")),
        }
    }

    pub fn nix_store_realise(closure: &str) -> Self {
        Self {
            executable: NIX_STORE.to_owned(),
            argv: vec![
                NIX_STORE.to_owned(),
                "--realise".to_owned(),
                closure.to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn nix_env_profile_set(closure: &str) -> Self {
        Self {
            executable: NIX_ENV.to_owned(),
            argv: vec![
                NIX_ENV.to_owned(),
                "-p".to_owned(),
                "/nix/var/nix/profiles/system".to_owned(),
                "--set".to_owned(),
                closure.to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn switch_to_configuration_boot(closure: &str) -> Self {
        Self {
            executable: format!("{closure}/bin/switch-to-configuration"),
            argv: vec![
                format!("{closure}/bin/switch-to-configuration"),
                "boot".to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn sync_paths() -> Self {
        Self {
            executable: SYNC.to_owned(),
            argv: vec![SYNC.to_owned()],
            working_dir: None,
        }
    }

    pub fn read_profile_target() -> Self {
        Self {
            executable: READLINK.to_owned(),
            argv: vec![
                READLINK.to_owned(),
                "-f".to_owned(),
                "/nix/var/nix/profiles/system".to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn read_boot_default() -> Self {
        Self {
            executable: CAT.to_owned(),
            argv: vec![
                CAT.to_owned(),
                "/boot/loader/entries/agentbed-default".to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn read_closure_hash(closure: &str) -> Self {
        Self {
            executable: NIX_STORE.to_owned(),
            argv: vec![
                NIX_STORE.to_owned(),
                "--query".to_owned(),
                "--hash".to_owned(),
                closure.to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn read_closure_store_path(closure: &str) -> Self {
        Self {
            executable: NIX_STORE.to_owned(),
            argv: vec![
                NIX_STORE.to_owned(),
                "--query".to_owned(),
                "--out-path".to_owned(),
                closure.to_owned(),
            ],
            working_dir: None,
        }
    }

    pub fn argv_contains(&self, needle: &str) -> bool {
        self.argv.iter().any(|arg| arg.contains(needle))
    }
}

/// Captured stdout/stderr and exit status from a fake command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            exit_code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn err(code: i32, stderr: impl Into<String>) -> Self {
        Self {
            exit_code: code,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

/// Errors from command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    NotRegistered,
    NonZeroExit { code: i32, stderr: String },
}

/// Narrow command runner abstraction.
pub trait CommandRunner: Send + Sync {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError>;
}

/// Hermetic fake runner for tests.
#[derive(Debug, Default)]
pub struct FakeCommandRunner {
    registry: Mutex<HashMap<CommandSpec, CommandOutput>>,
    invocations: Mutex<Vec<CommandSpec>>,
}

impl FakeCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, spec: CommandSpec, output: CommandOutput) {
        self.registry.lock().expect("registry").insert(spec, output);
    }

    pub fn clear(&self) {
        self.registry.lock().expect("registry").clear();
        self.invocations.lock().expect("invocations").clear();
    }

    pub fn invocations(&self) -> Vec<CommandSpec> {
        self.invocations.lock().expect("invocations").clone()
    }

    pub fn forbids_live_commands(&self) -> bool {
        true
    }
}

impl CommandRunner for FakeCommandRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
        self.invocations
            .lock()
            .expect("invocations")
            .push(spec.clone());
        let output = self
            .registry
            .lock()
            .expect("registry")
            .get(spec)
            .cloned()
            .ok_or(CommandError::NotRegistered)?;
        if output.exit_code != 0 {
            return Err(CommandError::NonZeroExit {
                code: output.exit_code,
                stderr: output.stderr.clone(),
            });
        }
        Ok(output)
    }
}

/// Shared runner handle used by the adapter.
pub type SharedRunner = Arc<dyn CommandRunner>;
