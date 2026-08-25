//! Nix adapter surface (`HostAdapter` impl lives in the broker).

use crate::capture::CaptureStore;
use crate::command_runner::SharedRunner;
use crate::probe::{ProbeError, ProbeResult};
use agentbed_protocol::dto::system_info::{AdapterInfo, SafetySource, SafetyVector};
use std::sync::Mutex;

/// Resolved Nix host adapter.
pub struct NixAdapter {
    runner: SharedRunner,
    store: CaptureStore,
    probe_cache: Mutex<Option<ProbeResult>>,
}

impl NixAdapter {
    pub fn new(runner: SharedRunner, store: CaptureStore) -> Self {
        Self {
            runner,
            store,
            probe_cache: Mutex::new(None),
        }
    }

    pub fn runner(&self) -> &SharedRunner {
        &self.runner
    }

    pub fn capture_store(&self) -> &CaptureStore {
        &self.store
    }

    pub fn info(&self) -> AdapterInfo {
        self.probe_cached().map_or_else(
            |_| AdapterInfo {
                kind: "nix".to_owned(),
                resolved: false,
                available_at_gate: 1,
                generations: None,
                snapshots: None,
            },
            |probe| probe.adapter,
        )
    }

    pub fn safety_vector(&self) -> SafetyVector {
        self.probe_cached()
            .map_or_else(|_| unresolved_safety_vector(), |probe| probe.safety)
    }

    pub fn safety_source(&self) -> SafetySource {
        self.probe_cached()
            .map_or(SafetySource::UnresolvedAdapter, |probe| probe.safety_source)
    }

    pub fn probe_cached(&self) -> Result<ProbeResult, ProbeError> {
        let mut cache = self.probe_cache.lock().expect("probe");
        if let Some(result) = cache.as_ref() {
            return Ok(result.clone());
        }
        let result = crate::probe::probe(self.runner.as_ref())?;
        *cache = Some(result.clone());
        Ok(result)
    }
}

impl std::fmt::Debug for NixAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NixAdapter").finish_non_exhaustive()
    }
}

fn unresolved_safety_vector() -> SafetyVector {
    use agentbed_protocol::dto::system_info::{
        DataSafety, ExternalEffectsSafety, HostSafety, RecoveryRequires, ServiceStateSafety,
    };
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
        recovery_requires: RecoveryRequires::OobConsole,
    }
}
