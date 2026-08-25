//! `events.replay` — read durable `agentbed://events` tail from a cursor.

use crate::policy::CallDescriptor;
use agentbed_protocol::wire::EffectClass;

pub const OP: &str = "events.replay";
pub const VERSION: u32 = 1;

#[must_use]
pub fn describe_call() -> CallDescriptor {
    CallDescriptor {
        op: OP,
        effect_set: vec![EffectClass::R],
        footprint: vec![],
        globally_forbidden: false,
        within_bounds: None,
    }
}
