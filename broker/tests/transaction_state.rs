//! L01-AC01: broker-owned transaction state transition table.

use agentbed_broker::transaction::state::{
    broker_may_enter, is_broker_owned, is_watchdog_owned, TransactionState,
};
use agentbed_protocol::dto::transaction::TransactionState as WireState;

#[test]
fn broker_owned_states_exclude_watchdog_terminal_authority() {
    for state in [
        WireState::Proposed,
        WireState::Testing,
        WireState::Applying,
        WireState::Probation,
        WireState::Rejected,
    ] {
        assert!(is_broker_owned(state));
        assert!(!is_watchdog_owned(state));
    }
    for state in [
        WireState::ProbationPassed,
        WireState::Committing,
        WireState::Committed,
        WireState::Reverting,
        WireState::Reverted,
    ] {
        assert!(is_watchdog_owned(state));
        assert!(!is_broker_owned(state));
    }
}

#[test]
fn transition_table_matches_effects_md_broker_segment() {
    assert!(broker_may_enter(
        TransactionState::None,
        TransactionState::Proposed
    ));
    assert!(broker_may_enter(
        TransactionState::Proposed,
        TransactionState::Testing
    ));
    assert!(broker_may_enter(
        TransactionState::Testing,
        TransactionState::Applying
    ));
    assert!(broker_may_enter(
        TransactionState::Applying,
        TransactionState::Probation
    ));
    assert!(broker_may_enter(
        TransactionState::Proposed,
        TransactionState::Rejected
    ));

    // Watchdog-owned targets are refused at the broker boundary.
    assert!(!broker_may_enter(
        TransactionState::Probation,
        TransactionState::ProbationPassed
    ));
    assert!(!broker_may_enter(
        TransactionState::Probation,
        TransactionState::Committed
    ));
    assert!(!broker_may_enter(
        TransactionState::Testing,
        TransactionState::Reverted
    ));
}

#[test]
fn idle_is_not_a_persisted_entry_state() {
    assert!(!broker_may_enter(
        TransactionState::None,
        TransactionState::Committed
    ));
}
