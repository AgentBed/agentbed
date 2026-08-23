//! Stage 5 accounting.
//!
//! In memory, per process, reset on restart. That is honest for Gate 0 and
//! wrong for production: a quota that a restart clears is a quota an agent can
//! reset by crashing the broker. Durable accounting shares the transaction WAL
//! (`docs/effects.md` §3) and lands with it at Gate 1–2.
//!
//! The counter advances only when a call is **authorized**, so a refused call
//! cannot consume an agent's budget — otherwise a hostile caller could exhaust
//! another agent's quota by sending calls that were always going to fail.

use crate::policy::QuotaState;
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-agent counters.
#[derive(Debug, Default)]
pub struct QuotaLedger {
    calls: Mutex<HashMap<String, u64>>,
}

impl QuotaLedger {
    /// Current state for an agent.
    #[must_use]
    pub fn state_for(&self, agent_id: &str) -> QuotaState {
        let calls_used = match self.calls.lock() {
            // An agent with no counter yet has used nothing.
            Ok(guard) => guard.get(agent_id).copied().unwrap_or(0),
            // A poisoned lock must not read as "quota free": report the maximum
            // so stage 5 refuses rather than over-permitting. Distinguishing
            // this from "no counter yet" matters — conflating them made every
            // first call look exhausted.
            Err(_) => u64::MAX,
        };
        QuotaState { calls_used }
    }

    /// Record one authorized call.
    pub fn charge(&self, agent_id: &str) {
        if let Ok(mut guard) = self.calls.lock() {
            let entry = guard.entry(agent_id.to_owned()).or_insert(0);
            *entry = entry.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_per_agent_and_starts_at_zero() {
        let ledger = QuotaLedger::default();
        assert_eq!(ledger.state_for("a").calls_used, 0);
        ledger.charge("a");
        ledger.charge("a");
        ledger.charge("b");
        assert_eq!(ledger.state_for("a").calls_used, 2);
        assert_eq!(ledger.state_for("b").calls_used, 1);
    }
}
