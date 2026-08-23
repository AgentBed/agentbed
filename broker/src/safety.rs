//! Per-resource safety comparison (`docs/effects.md` §2).
//!
//! The *values* are protocol vocabulary and live in
//! `agentbed_protocol::dto::system_info`. The **order** lives here, in the
//! broker, next to the code that refuses — deliberately, so that "which
//! coverage outranks which" cannot be changed by anything outside the
//! privileged process.
//!
//! The refusal rule is uniform for D and M: a step targeting a resource whose
//! reported safety is below the manifest's minimum — or at `none` — is refused.
//! There is **no manifest opt-in to mutate at `none`**.

use agentbed_protocol::dto::system_info::{
    DataSafety, HostSafety, SafetyVector, ServiceStateSafety,
};
use serde::Deserialize;

/// Resources that carry rollback coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    /// Root configuration.
    RootConfig,
    /// Installed packages.
    Packages,
    /// Bootloader (class F in v0; listed so a footprint can name it).
    Bootloader,
    /// Kernel (class F in v0).
    Kernel,
    /// Systemd unit desired state.
    ServiceState,
    /// Plugin data volumes.
    PluginData,
    /// Desktop profiles and home volumes.
    DesktopData,
    /// Home directories and agent workspaces.
    HomeData,
    /// External effects; definitionally `none`.
    ExternalEffects,
}

/// Rank within a resource kind's total order. Comparable only against ranks of
/// the same kind, which is why this is a private detail of each accessor.
type Rank = u8;

fn host_rank(value: HostSafety) -> Rank {
    match value {
        HostSafety::None => 0,
        HostSafety::SnapshotReboot => 1,
        HostSafety::SnapshotLive => 2,
        HostSafety::Generation => 3,
    }
}

fn service_rank(value: ServiceStateSafety) -> Rank {
    match value {
        ServiceStateSafety::None => 0,
        ServiceStateSafety::DesiredState => 1,
    }
}

fn data_rank(value: DataSafety) -> Rank {
    match value {
        DataSafety::None => 0,
        DataSafety::DedicatedSnapshot => 1,
    }
}

/// A manifest's per-resource minimums (`risk.min_safety`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinSafety {
    /// Minimum coverage for root configuration.
    pub root_config: Option<HostSafety>,
    /// Minimum coverage for packages.
    pub packages: Option<HostSafety>,
    /// Minimum coverage for the bootloader.
    pub bootloader: Option<HostSafety>,
    /// Minimum coverage for the kernel.
    pub kernel: Option<HostSafety>,
    /// Minimum coverage for unit desired state.
    pub service_state: Option<ServiceStateSafety>,
    /// Minimum coverage for plugin data.
    pub plugin_data: Option<DataSafety>,
    /// Minimum coverage for desktop data.
    pub desktop_data: Option<DataSafety>,
    /// Minimum coverage for home data and workspaces.
    pub home_data: Option<DataSafety>,
}

/// Whether the host's reported coverage satisfies the manifest for a resource.
///
/// A resource at `none` fails regardless of what the manifest asks for: `none`
/// is never a floor a manifest can lower itself to. A resource the manifest
/// does not mention still fails at `none` for the same reason — silence is not
/// consent to mutate something unrecoverable.
#[must_use]
pub fn meets_minimum(resource: Resource, host: &SafetyVector, min: &MinSafety) -> bool {
    match resource {
        Resource::RootConfig => host_meets(host.root_config, min.root_config),
        Resource::Packages => host_meets(host.packages, min.packages),
        Resource::Bootloader => host_meets(host.bootloader, min.bootloader),
        Resource::Kernel => host_meets(host.kernel, min.kernel),
        Resource::ServiceState => {
            let actual = service_rank(host.service_state);
            actual > 0 && actual >= min.service_state.map_or(0, service_rank)
        }
        Resource::PluginData => data_meets(host.plugin_data, min.plugin_data),
        Resource::DesktopData => data_meets(host.desktop_data, min.desktop_data),
        Resource::HomeData => data_meets(host.home_data, min.home_data),
        // external_effects is always none and no D/M step targets it; an
        // attempt to treat it as mutable is refused here rather than compared.
        Resource::ExternalEffects => false,
    }
}

fn host_meets(actual: HostSafety, minimum: Option<HostSafety>) -> bool {
    let actual = host_rank(actual);
    actual > 0 && actual >= minimum.map_or(0, host_rank)
}

fn data_meets(actual: DataSafety, minimum: Option<DataSafety>) -> bool {
    let actual = data_rank(actual);
    actual > 0 && actual >= minimum.map_or(0, data_rank)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentbed_protocol::dto::system_info::{ExternalEffectsSafety, RecoveryRequires};

    fn vector(root: HostSafety, home: DataSafety) -> SafetyVector {
        SafetyVector {
            root_config: root,
            packages: root,
            bootloader: HostSafety::None,
            kernel: HostSafety::None,
            service_state: ServiceStateSafety::DesiredState,
            plugin_data: DataSafety::DedicatedSnapshot,
            desktop_data: DataSafety::DedicatedSnapshot,
            home_data: home,
            external_effects: ExternalEffectsSafety::None,
            recovery_requires: RecoveryRequires::OobConsole,
        }
    }

    #[test]
    fn none_is_refused_even_when_the_manifest_asks_for_nothing() {
        let host = vector(HostSafety::None, DataSafety::None);
        let empty = MinSafety::default();
        assert!(!meets_minimum(Resource::RootConfig, &host, &empty));
        assert!(!meets_minimum(Resource::HomeData, &host, &empty));
    }

    #[test]
    fn coverage_must_reach_the_declared_minimum() {
        let host = vector(HostSafety::SnapshotReboot, DataSafety::DedicatedSnapshot);
        let demanding = MinSafety {
            root_config: Some(HostSafety::Generation),
            ..MinSafety::default()
        };
        assert!(!meets_minimum(Resource::RootConfig, &host, &demanding));

        let satisfied = MinSafety {
            root_config: Some(HostSafety::SnapshotReboot),
            ..MinSafety::default()
        };
        assert!(meets_minimum(Resource::RootConfig, &host, &satisfied));
    }

    #[test]
    fn service_state_uses_its_own_two_value_order() {
        let mut host = vector(HostSafety::Generation, DataSafety::DedicatedSnapshot);
        assert!(meets_minimum(
            Resource::ServiceState,
            &host,
            &MinSafety::default()
        ));
        host.service_state = ServiceStateSafety::None;
        assert!(!meets_minimum(
            Resource::ServiceState,
            &host,
            &MinSafety::default()
        ));
    }

    #[test]
    fn external_effects_is_never_a_mutable_target() {
        let host = vector(HostSafety::Generation, DataSafety::DedicatedSnapshot);
        assert!(!meets_minimum(
            Resource::ExternalEffects,
            &host,
            &MinSafety::default()
        ));
    }
}
