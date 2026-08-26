//! Watchdog error types for open/reopen and RPC handling.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyError {
    MissingMount,
    SameDeviceAlias,
    SymlinkComponent,
    NonRegularComponent,
    WrongOwnershipOrMode,
    WrongLinkCount,
    HardLinkAmbiguity,
    OrdinaryDirectoryFallback,
    Unwritable,
    UnavailableStore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurabilityError {
    InjectedFailure,
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalFloorError {
    Ambiguous,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerCredError {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceStage {
    AfterTerm,
    AfterKill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceError {
    SignalFailed,
    Incomplete,
    WaitFailed,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvariantError {
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobInspectError {
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogError {
    Topology(TopologyError),
    SafeModeActive,
    SafeModePersistUnavailable,
    EpochLogMismatch,
    Durability(DurabilityError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    Durability(DurabilityError),
    SafeModeActive,
    OversizeFrame,
    MalformedFrame,
    CrcMismatch,
    UnknownVersion,
    DenyUnknown,
    ReplayCounter,
    StaleReconnect,
    ConflictingRequest,
    WrongCapability,
    WrongPeer,
    ResponseBindingMismatch,
    MovedBase,
    WeakenedMandatoryInvariant,
    ExpiredDeadline,
    WrongEpoch,
    UnknownTransaction,
    StaleEpoch,
    WrongBinding,
    ClockRegression,
    FenceIncomplete,
    MalformedRequest,
    Transport(String),
}

impl From<DurabilityError> for RpcError {
    fn from(error: DurabilityError) -> Self {
        Self::Durability(error)
    }
}

impl fmt::Display for WatchdogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for WatchdogError {}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for RpcError {}
