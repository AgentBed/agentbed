//! Broker-owned transaction state vocabulary and transition table.

#![allow(clippy::match_like_matches_macro)]

pub use agentbed_protocol::dto::transaction::TransactionState as WireState;

/// Internal state including the non-persisted idle sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    None,
    Proposed,
    Testing,
    Applying,
    Probation,
    Rejected,
    ProbationPassed,
    Committing,
    Committed,
    Reverting,
    Reverted,
}

impl From<WireState> for TransactionState {
    fn from(value: WireState) -> Self {
        match value {
            WireState::Idle => Self::None,
            WireState::Proposed => Self::Proposed,
            WireState::Testing => Self::Testing,
            WireState::Applying => Self::Applying,
            WireState::Probation => Self::Probation,
            WireState::Rejected => Self::Rejected,
            WireState::ProbationPassed => Self::ProbationPassed,
            WireState::Committing => Self::Committing,
            WireState::Committed => Self::Committed,
            WireState::Reverting => Self::Reverting,
            WireState::Reverted => Self::Reverted,
        }
    }
}

impl From<TransactionState> for WireState {
    fn from(value: TransactionState) -> Self {
        match value {
            TransactionState::None => Self::Idle,
            TransactionState::Proposed => Self::Proposed,
            TransactionState::Testing => Self::Testing,
            TransactionState::Applying => Self::Applying,
            TransactionState::Probation => Self::Probation,
            TransactionState::Rejected => Self::Rejected,
            TransactionState::ProbationPassed => Self::ProbationPassed,
            TransactionState::Committing => Self::Committing,
            TransactionState::Committed => Self::Committed,
            TransactionState::Reverting => Self::Reverting,
            TransactionState::Reverted => Self::Reverted,
        }
    }
}

#[must_use]
pub fn is_broker_owned(state: WireState) -> bool {
    matches!(
        state,
        WireState::Proposed
            | WireState::Testing
            | WireState::Applying
            | WireState::Probation
            | WireState::Rejected
    )
}

#[must_use]
pub fn is_watchdog_owned(state: WireState) -> bool {
    matches!(
        state,
        WireState::ProbationPassed
            | WireState::Committing
            | WireState::Committed
            | WireState::Reverting
            | WireState::Reverted
    )
}

#[must_use]
pub fn broker_may_enter(from: TransactionState, to: TransactionState) -> bool {
    use TransactionState::{Applying, None, Probation, Proposed, Rejected, Testing};
    match (from, to) {
        (None, Proposed) => true,
        (Proposed, Testing | Rejected) => true,
        (Testing, Applying | Rejected) => true,
        (Applying, Probation | Rejected) => true,
        _ => false,
    }
}
