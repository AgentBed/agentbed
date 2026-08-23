//! `system.info` result: host facts, adapter state, per-resource safety vector.
//!
//! The enums here are exactly the total orders of `docs/effects.md` §2. They
//! carry no `Ord` implementation: comparing them against a manifest minimum is
//! a policy decision, made in the broker, next to the code that refuses.

use serde::{Deserialize, Serialize};

/// Rollback coverage for host resources: `none < snapshot_reboot <
/// snapshot_live < generation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSafety {
    /// No rollback path.
    None,
    /// Rollback requires a reboot into a snapshot.
    SnapshotReboot,
    /// Snapshot restorable on the live system.
    SnapshotLive,
    /// Boot-selectable generation.
    Generation,
}

/// Rollback coverage for runtime state: `none < desired_state`.
///
/// `desired_state` restores only the unit's desired active/inactive state.
/// Consequences of a start/stop/restart are never rolled back and must be
/// declared as M/E in `added_effects` (`docs/effects.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStateSafety {
    /// No rollback path.
    None,
    /// Desired active/inactive state restorable.
    DesiredState,
}

/// Rollback coverage for data resources: `none < dedicated_snapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSafety {
    /// Not on a dedicated, separately-mounted, restore-tested volume.
    None,
    /// Dedicated subvolume/dataset with its own snapshot schedule and an
    /// exercised restore procedure.
    DedicatedSnapshot,
}

/// External effects are definitionally unrollbackable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalEffectsSafety {
    /// The only permitted value.
    None,
}

/// What recovering this host takes. Informational, not ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRequires {
    /// Recoverable without operator intervention.
    None,
    /// A remote reboot suffices.
    RemoteReboot,
    /// Out-of-band console access needed.
    OobConsole,
}

/// The per-resource safety vector of `docs/effects.md` §2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafetyVector {
    /// Root configuration (`/etc`, system config).
    pub root_config: HostSafety,
    /// Installed packages.
    pub packages: HostSafety,
    /// Bootloader.
    pub bootloader: HostSafety,
    /// Kernel.
    pub kernel: HostSafety,
    /// Systemd unit desired state.
    pub service_state: ServiceStateSafety,
    /// Plugin data volumes.
    pub plugin_data: DataSafety,
    /// Desktop profiles and home volumes.
    pub desktop_data: DataSafety,
    /// Home directories and agent workspaces.
    pub home_data: DataSafety,
    /// Always `none`.
    pub external_effects: ExternalEffectsSafety,
    /// Recovery expectation.
    pub recovery_requires: RecoveryRequires,
}

/// Why the vector reads the way it does.
///
/// Present so an all-`none` vector is legible as "nothing has been resolved"
/// rather than mistaken for "this host cannot roll back anything, as measured".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetySource {
    /// No adapter resolved this host; values are the refusing defaults.
    UnresolvedAdapter,
    /// A host adapter probed the system and reported these values.
    AdapterProbe,
}

/// Host facts. Deliberately thin: `system.info` is class R and must not become
/// a reconnaissance surface richer than an agent's manifest justifies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostInfo {
    /// Kernel hostname.
    pub hostname: String,
    /// `ID` from `/etc/os-release`, or `unknown`.
    pub os_id: String,
    /// `VERSION_ID` from `/etc/os-release`, or `unknown`.
    pub os_version_id: String,
    /// Kernel release string.
    pub kernel_release: String,
    /// Machine architecture.
    pub architecture: String,
}

/// State of the host adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterInfo {
    /// Adapter identifier (`unresolved` at Gate 0).
    pub kind: String,
    /// Whether an adapter actually probed this host.
    pub resolved: bool,
    /// Gate at which this adapter is scheduled to land.
    pub available_at_gate: u8,
    /// Generations known to the adapter; `None` when unresolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generations: Option<u32>,
    /// Snapshots known to the adapter; `None` when unresolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshots: Option<u32>,
}

/// Result of probing Landlock support (ADR §5.1: absent features degrade to
/// deny, never to silent allow).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandlockInfo {
    /// Whether the kernel supports Landlock at all.
    pub supported: bool,
    /// Highest ABI version reported by the kernel, when supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abi_version: Option<i32>,
}

/// The `system.info` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemInfo {
    /// Host facts.
    pub host: HostInfo,
    /// Adapter state.
    pub adapter: AdapterInfo,
    /// Per-resource rollback coverage.
    pub safety: SafetyVector,
    /// Where the safety values came from.
    pub safety_source: SafetySource,
    /// Probed Landlock ABI.
    pub landlock: LandlockInfo,
}
