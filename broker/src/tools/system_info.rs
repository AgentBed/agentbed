//! `system.info` — the Gate 0 end-to-end read tool.
//!
//! ADR §5.1 gives it effect set `{R}` and requires it to report host facts, the
//! adapter, the per-resource safety vector, generations/snapshots, and the
//! probed Landlock ABI.

use crate::adapter::HostAdapter;
use crate::host::{host_info, landlock_info};
use crate::policy::CallDescriptor;
use agentbed_protocol::dto::system_info::SystemInfo;

/// The operation's wire name.
pub const OP: &str = "system.info";

/// The effect set for a `system.info` call.
///
/// A function rather than a constant because `docs/effects.md` §1 computes the
/// set from *tool + arguments + manifest*: the static table is a minimum that
/// arguments can only raise. `system.info` takes no arguments, so the set is
/// `{R}` — but the shape of the computation stays where a later argument would
/// have to be considered.
#[must_use]
pub fn describe_call() -> CallDescriptor {
    CallDescriptor::read_only(OP)
}

/// Gather the report.
#[must_use]
pub fn execute(adapter: &dyn HostAdapter) -> SystemInfo {
    SystemInfo {
        host: host_info(),
        adapter: adapter.info(),
        safety: adapter.safety_vector(),
        safety_source: adapter.safety_source(),
        landlock: landlock_info(),
    }
}
