//! Nix adapter probe and safety vector.

use crate::command_runner::{CommandError, CommandRunner, CommandSpec};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::system_info::{
    AdapterInfo, DataSafety, ExternalEffectsSafety, HostSafety, RecoveryRequires, SafetySource,
    SafetyVector, ServiceStateSafety,
};
use agentbed_protocol::dto::transaction::BaseRevision;

/// Probe outcome including adapter identity and safety vector.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub adapter: AdapterInfo,
    pub safety: SafetyVector,
    pub safety_source: SafetySource,
    pub base_revision: BaseRevision,
}

/// Probe refused because required observations are missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeError {
    IncompleteObservation,
}

/// Probe the host through the injected runner.
pub fn probe(runner: &dyn CommandRunner) -> Result<ProbeResult, ProbeError> {
    let generation = match runner.run(&CommandSpec::nix_current_generation()) {
        Ok(output) => output.stdout.trim().to_owned(),
        Err(CommandError::NonZeroExit { .. } | CommandError::NotRegistered) => {
            return Err(ProbeError::IncompleteObservation);
        }
    };
    if generation.is_empty() {
        return Err(ProbeError::IncompleteObservation);
    }

    let etc_git = runner
        .run(&CommandSpec::etc_git_head())
        .map_err(|_| ProbeError::IncompleteObservation)?
        .stdout
        .trim()
        .to_owned();
    let digest_hex = runner
        .run(&CommandSpec::config_digest())
        .map_err(|_| ProbeError::IncompleteObservation)?
        .stdout
        .trim()
        .to_owned();
    let digest_bytes = hex_to_digest(&digest_hex).map_err(|_| ProbeError::IncompleteObservation)?;

    let base_revision = BaseRevision {
        generation: Some(generation.clone()),
        etc_git_commit: etc_git,
        config_digest: digest_bytes,
    };

    Ok(ProbeResult {
        adapter: AdapterInfo {
            kind: "nix".to_owned(),
            resolved: true,
            available_at_gate: 1,
            generations: generation.parse().ok(),
            snapshots: None,
        },
        safety: SafetyVector {
            root_config: HostSafety::Generation,
            packages: HostSafety::Generation,
            bootloader: HostSafety::None,
            kernel: HostSafety::None,
            service_state: ServiceStateSafety::None,
            plugin_data: DataSafety::None,
            desktop_data: DataSafety::None,
            home_data: DataSafety::None,
            external_effects: ExternalEffectsSafety::None,
            recovery_requires: RecoveryRequires::RemoteReboot,
        },
        safety_source: SafetySource::AdapterProbe,
        base_revision,
    })
}

fn hex_to_digest(hex: &str) -> Result<Digest, ProbeError> {
    let hex = hex.trim();
    let hex = if hex.len() > 64 { &hex[..64] } else { hex };
    if hex.len() != 64 {
        return Err(ProbeError::IncompleteObservation);
    }
    let mut bytes = [0_u8; 32];
    for (idx, chunk) in hex.as_bytes().chunks(2).enumerate() {
        if idx >= 32 {
            return Err(ProbeError::IncompleteObservation);
        }
        let pair = std::str::from_utf8(chunk).map_err(|_| ProbeError::IncompleteObservation)?;
        bytes[idx] = u8::from_str_radix(pair, 16).map_err(|_| ProbeError::IncompleteObservation)?;
    }
    Ok(Digest::from_sha256_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_succeeds_with_registered_fixture() {
        use crate::command_runner::{CommandOutput, FakeCommandRunner};
        let runner = FakeCommandRunner::new();
        runner.register(
            CommandSpec::nix_current_generation(),
            CommandOutput::ok("42\n"),
        );
        runner.register(CommandSpec::etc_git_head(), CommandOutput::ok("abc123\n"));
        runner.register(
            CommandSpec::config_digest(),
            CommandOutput::ok("11".repeat(64)),
        );
        let result = probe(&runner).expect("probe");
        assert!(result.adapter.resolved);
    }
}
