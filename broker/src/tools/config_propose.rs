//! `config.propose` — stage a declarative change set (protocol v2).
//!
//! ADR-001 §5.1: minimum effect set `{D}`; returns a diff and test plan.
//! Execution is implemented in later Gate 1 lanes; this module defines the
//! contract-facing descriptor only.

use crate::policy::CallDescriptor;
use crate::safety::Resource;
use agentbed_protocol::wire::EffectClass;

pub const OP: &str = "config.propose";
pub const VERSION: u32 = 1;

#[must_use]
pub fn describe_call() -> CallDescriptor {
    CallDescriptor {
        op: OP,
        effect_set: vec![EffectClass::D],
        footprint: vec![(Resource::RootConfig, EffectClass::D)],
        globally_forbidden: false,
        within_bounds: None,
    }
}
