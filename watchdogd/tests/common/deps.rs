//! Build `Dependencies` from hermetic fakes implementing production traits.

use std::sync::Arc;

use super::fakes::FakeBundle;
use agentbed_watchdogd::interfaces::Dependencies;

pub fn dependencies_from(bundle: &FakeBundle) -> Dependencies {
    Dependencies::new(
        Arc::clone(&bundle.clock),
        Arc::clone(&bundle.entropy),
        Arc::clone(&bundle.topology),
        Arc::clone(&bundle.durability),
        Arc::clone(&bundle.process_group),
        Arc::clone(&bundle.job_inspector),
        Arc::clone(&bundle.external_floor),
        Arc::clone(&bundle.invariants),
        Arc::clone(&bundle.base_revision),
        Arc::clone(&bundle.peer_cred),
        Arc::clone(&bundle.peer_cred),
    )
}
