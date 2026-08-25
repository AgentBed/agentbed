//! `HostAdapter` bridge for the Nix adapter crate.

use crate::adapter::HostAdapter;
use agentbed_adapter_nix::adapter::NixAdapter;
use agentbed_protocol::dto::system_info::{AdapterInfo, SafetySource, SafetyVector};
use agentbed_protocol::dto::transaction::BaseRevision;
use agentbed_protocol::wire::ConfigFileChange;

impl HostAdapter for NixAdapter {
    fn info(&self) -> AdapterInfo {
        NixAdapter::info(self)
    }

    fn safety_vector(&self) -> SafetyVector {
        NixAdapter::safety_vector(self)
    }

    fn safety_source(&self) -> SafetySource {
        NixAdapter::safety_source(self)
    }

    fn current_base_revision(&self) -> BaseRevision {
        self.probe_cached().map_or_else(
            |_| crate::adapter::UnresolvedAdapter.current_base_revision(),
            |probe| probe.base_revision,
        )
    }

    fn propose_config(
        &self,
        changes: &[ConfigFileChange],
    ) -> Result<crate::adapter::AdapterProposeOutcome, crate::adapter::AdapterProposeError> {
        if let Err(reason) = agentbed_adapter_nix::protected::check_protected_changes(changes) {
            return Err(crate::adapter::AdapterProposeError::Rejected(format!(
                "{reason:?}"
            )));
        }
        let base = self.current_base_revision();
        let proposal = agentbed_adapter_nix::propose::propose_and_capture(
            self.runner().as_ref(),
            self.capture_store(),
            changes,
            &base,
        )
        .map_err(|err| crate::adapter::AdapterProposeError::Rejected(format!("{err:?}")))?;
        Ok(crate::adapter::AdapterProposeOutcome {
            diff: proposal.diff,
            test_plan: proposal.test_plan,
            affected_resources: proposal.affected_resources,
        })
    }
}
