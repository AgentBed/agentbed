//! Host adapters and the honest default.
//! *per resource*, and D/M steps against a resource below the manifest minimum
//! — or at `none` — are refused.
//!
//! Gate 0 ships no adapter. That leaves a choice about what to report, and the
//! only defensible answer is `none` everywhere: nothing has been probed, so
//! nothing can be claimed. Reporting a plausible `generation` because the host
//! happens to look like NixOS would be a guess in exactly the place the
//! documents forbid guessing, and it would license mutations whose rollback
//! nobody has verified.
//!
//! Because "all `none`" is also what a genuinely unrecoverable host reports,
//! the result carries [`SafetySource`] so the two are distinguishable: one is a
//! measurement, the other is the absence of one. Both refuse D/M steps.

use agentbed_protocol::digest::Digest;
use agentbed_protocol::dto::system_info::{
    AdapterInfo, DataSafety, ExternalEffectsSafety, HostSafety, RecoveryRequires, SafetySource,
    SafetyVector, ServiceStateSafety,
};
use agentbed_protocol::dto::transaction::{BaseRevision, TestPlan};
use agentbed_protocol::wire::ConfigFileChange;

/// Outcome of adapter-specific `config.propose` staging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterProposeOutcome {
    pub diff: String,
    pub test_plan: TestPlan,
    pub affected_resources: Vec<String>,
    pub candidate_closure: Option<String>,
}

/// Adapter-level propose failures before WAL side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterProposeError {
    Rejected(String),
}

/// What the broker needs from a host adapter.
pub trait HostAdapter: Send + Sync {
    /// Adapter identity, as reported to callers.
    fn info(&self) -> AdapterInfo;

    /// Rollback coverage per resource.
    fn safety_vector(&self) -> SafetyVector;

    /// Where those values came from.
    fn safety_source(&self) -> SafetySource;

    /// Active base revision for transaction base-movement checks (`effects.md` §3).
    fn current_base_revision(&self) -> BaseRevision;

    /// Stage a proposal through the adapter. Unresolved adapters keep L01 synthetic behavior.
    fn propose_config(
        &self,
        changes: &[ConfigFileChange],
    ) -> Result<AdapterProposeOutcome, AdapterProposeError> {
        Ok(unresolved_propose(changes))
    }
}

fn unresolved_propose(changes: &[ConfigFileChange]) -> AdapterProposeOutcome {
    AdapterProposeOutcome {
        diff: changes
            .iter()
            .map(|c| format!("{} => staged", c.path))
            .collect::<Vec<_>>()
            .join("\n"),
        test_plan: TestPlan {
            adapter: "unresolved".to_owned(),
            steps: vec!["noop-test".to_owned()],
        },
        affected_resources: vec!["root_config".to_owned()],
        candidate_closure: None,
    }
}

/// The Gate 0 adapter: resolves nothing, claims nothing.
#[derive(Debug, Default)]
pub struct UnresolvedAdapter;

impl HostAdapter for UnresolvedAdapter {
    fn info(&self) -> AdapterInfo {
        AdapterInfo {
            kind: "unresolved".to_owned(),
            resolved: false,
            // The Nix adapter (config.propose, tx.*, nixos-rebuild test) is
            // Gate 1; apt + Btrfs is Gate 5.
            available_at_gate: 1,
            // Not "0 generations" — that would be a measurement. No adapter
            // looked, so there is no count to report.
            generations: None,
            snapshots: None,
        }
    }

    fn safety_vector(&self) -> SafetyVector {
        SafetyVector {
            root_config: HostSafety::None,
            packages: HostSafety::None,
            bootloader: HostSafety::None,
            kernel: HostSafety::None,
            service_state: ServiceStateSafety::None,
            plugin_data: DataSafety::None,
            desktop_data: DataSafety::None,
            home_data: DataSafety::None,
            external_effects: ExternalEffectsSafety::None,
            // Without an adapter there is no verified recovery path short of
            // the out-of-band console.
            recovery_requires: RecoveryRequires::OobConsole,
        }
    }

    fn safety_source(&self) -> SafetySource {
        SafetySource::UnresolvedAdapter
    }

    fn current_base_revision(&self) -> BaseRevision {
        BaseRevision {
            generation: Some("gen-1".to_owned()),
            etc_git_commit: "deadbeef".to_owned(),
            config_digest: Digest::from_sha256_bytes([0x22; 32]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unresolved_adapter_claims_no_coverage_anywhere() {
        let adapter = UnresolvedAdapter;
        let vector = adapter.safety_vector();
        assert_eq!(vector.root_config, HostSafety::None);
        assert_eq!(vector.packages, HostSafety::None);
        assert_eq!(vector.service_state, ServiceStateSafety::None);
        assert_eq!(vector.home_data, DataSafety::None);
        assert_eq!(adapter.safety_source(), SafetySource::UnresolvedAdapter);
        assert!(!adapter.info().resolved);
        assert!(
            adapter.info().generations.is_none(),
            "no adapter looked, so there is no count"
        );
    }
}
