//! Production process-group fencing — unavailable in library-only L03.

use crate::error::FenceError;
use crate::interfaces::{FenceStage, ProcessGroupFence, SignalKind};
use std::time::Duration;

/// Production fencer: refuses all signaling; ambiguity resolves toward still-alive.
#[derive(Debug, Default)]
pub struct UnavailableProcessGroupFencer;

impl UnavailableProcessGroupFencer {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ProcessGroupFence for UnavailableProcessGroupFencer {
    fn signal(&self, _kind: SignalKind) -> Result<(), FenceError> {
        Err(FenceError::Unavailable)
    }

    fn group_alive(&self, _stage: FenceStage) -> bool {
        true
    }

    fn bounded_wait(&self, _timeout: Duration) -> Result<(), FenceError> {
        Err(FenceError::Unavailable)
    }
}
