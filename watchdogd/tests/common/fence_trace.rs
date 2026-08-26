//! Shared fence ordering sink for hermetic AC08 tests.

use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceTraceEvent {
    Term,
    BoundedWait,
    AliveAfterTerm,
    Kill,
    ConfirmedExit,
    CandidateJobsRemain,
    JobInspectionFailed,
    ZeroCandidateJobs,
}

#[derive(Debug, Default)]
pub struct FenceTrace {
    pub events: Mutex<Vec<FenceTraceEvent>>,
}

impl FenceTrace {
    pub fn push(&self, event: FenceTraceEvent) {
        self.events.lock().expect("lock").push(event);
    }

    pub fn snapshot(&self) -> Vec<FenceTraceEvent> {
        self.events.lock().expect("lock").clone()
    }
}
