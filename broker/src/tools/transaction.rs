//! Gate 1 transaction RPC tools (protocol v2).
//!
//! ADR-001 §5.1 and `docs/effects.md` §3 define effect sets and semantics.
//! The transaction engine is implemented in later lanes; these modules expose
//! the contract-facing call descriptors only.

use crate::policy::CallDescriptor;
use crate::safety::Resource;
use agentbed_protocol::wire::EffectClass;

pub mod test {
    use super::{CallDescriptor, EffectClass, Resource};

    pub const OP: &str = "tx.test";
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
}

pub mod apply {
    use super::{CallDescriptor, EffectClass, Resource};

    pub const OP: &str = "tx.apply";
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
}

pub mod rollback {
    use super::{CallDescriptor, EffectClass, Resource};

    pub const OP: &str = "tx.rollback";
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
}

pub mod status {
    use super::CallDescriptor;

    pub const OP: &str = "tx.status";
    pub const VERSION: u32 = 1;

    #[must_use]
    pub fn describe_call() -> CallDescriptor {
        CallDescriptor::read_only(OP)
    }
}
