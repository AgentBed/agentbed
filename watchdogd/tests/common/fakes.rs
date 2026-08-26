//! Hermetic fakes — observe and pre-program outcomes only; never perform production transitions.

use super::fence_trace::{FenceTrace, FenceTraceEvent};
use agentbed_watchdogd::error::{
    DurabilityError, ExternalFloorError, FenceError, FenceStage, InvariantError, JobInspectError,
    TopologyError,
};
use agentbed_watchdogd::interfaces::{
    BaseRevisionObserver, Clock, Durability, Entropy, ExternalFloorReader, InvariantObserver,
    InvariantOutcome, JobInspector, PeerCred, PeerCredSource, ProcessGroupFence, SignalKind,
    StreamPeerAuth, TopologyProbe,
};
use agentbed_watchdogd::read_model::{AuthorityRecordKind, DecisionLogReader};
use std::collections::VecDeque;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Debug)]
pub struct FakeClock {
    now: Mutex<SystemTime>,
}

impl Default for FakeClock {
    fn default() -> Self {
        Self {
            now: Mutex::new(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
        }
    }
}

impl FakeClock {
    pub fn set(&self, t: SystemTime) {
        *self.now.lock().expect("lock") = t;
    }

    pub fn advance(&self, d: Duration) {
        let mut now = self.now.lock().expect("lock");
        *now += d;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().expect("lock")
    }
}

#[derive(Debug, Default)]
pub struct FakeEntropy {
    bytes: Mutex<Vec<u8>>,
}

impl FakeEntropy {
    pub fn set_bytes(&self, bytes: Vec<u8>) {
        *self.bytes.lock().expect("lock") = bytes;
    }
}

impl Entropy for FakeEntropy {
    fn fill(&self, out: &mut [u8]) {
        let src = self.bytes.lock().expect("lock");
        for (i, b) in out.iter_mut().enumerate() {
            *b = src.get(i).copied().unwrap_or(0x42);
        }
    }
}

#[derive(Debug, Default)]
pub struct FakeTopology {
    pub verify_calls: Mutex<Vec<PathBuf>>,
    outcomes: Mutex<VecDeque<Result<(), TopologyError>>>,
}

impl FakeTopology {
    pub fn push_outcome(&self, outcome: Result<(), TopologyError>) {
        self.outcomes.lock().expect("lock").push_back(outcome);
    }
}

impl TopologyProbe for FakeTopology {
    fn verify_startup(&self, store_root: &Path) -> Result<(), TopologyError> {
        self.verify_calls
            .lock()
            .expect("lock")
            .push(store_root.to_path_buf());
        self.outcomes
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(Ok(()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityOp {
    FileFsync,
    DirFsync,
    AtomicRename,
    Readback,
}

#[derive(Debug, Default)]
pub struct FakeDurability {
    pub ops: Mutex<Vec<DurabilityOp>>,
    pub file_fsync_paths: Mutex<Vec<PathBuf>>,
    pub dir_fsync_paths: Mutex<Vec<PathBuf>>,
    pub rename_ops: Mutex<Vec<(PathBuf, PathBuf)>>,
    fail_matching: Mutex<VecDeque<DurabilityOp>>,
    fail_file_fsync_at: Mutex<Option<usize>>,
    fail_dir_fsync_at: Mutex<Option<usize>>,
    file_fsync_invocations: Mutex<usize>,
    dir_fsync_invocations: Mutex<usize>,
}

impl FakeDurability {
    pub fn fail_on(&self, op: DurabilityOp) {
        self.fail_matching.lock().expect("lock").push_back(op);
    }

    /// Fail on the Nth `file_fsync` call (1-based).
    pub fn fail_on_file_fsync_invocation(&self, n: usize) {
        *self.fail_file_fsync_at.lock().expect("lock") = Some(n);
    }

    /// Fail on the Nth `dir_fsync` call (1-based).
    pub fn fail_on_dir_fsync_invocation(&self, n: usize) {
        *self.fail_dir_fsync_at.lock().expect("lock") = Some(n);
    }

    fn maybe_fail(&self, op: DurabilityOp) -> Result<(), DurabilityError> {
        let mut fails = self.fail_matching.lock().expect("lock");
        if fails.front().copied() == Some(op) {
            fails.pop_front();
            return Err(DurabilityError::InjectedFailure);
        }
        Ok(())
    }
}

impl Durability for FakeDurability {
    fn file_fsync(&self, path: &Path) -> Result<(), DurabilityError> {
        self.ops.lock().expect("lock").push(DurabilityOp::FileFsync);
        self.file_fsync_paths
            .lock()
            .expect("lock")
            .push(path.to_path_buf());
        let mut count = self.file_fsync_invocations.lock().expect("lock");
        *count += 1;
        if *self.fail_file_fsync_at.lock().expect("lock") == Some(*count) {
            return Err(DurabilityError::InjectedFailure);
        }
        self.maybe_fail(DurabilityOp::FileFsync)
    }

    fn dir_fsync(&self, path: &Path) -> Result<(), DurabilityError> {
        self.ops.lock().expect("lock").push(DurabilityOp::DirFsync);
        self.dir_fsync_paths
            .lock()
            .expect("lock")
            .push(path.to_path_buf());
        let mut count = self.dir_fsync_invocations.lock().expect("lock");
        *count += 1;
        if *self.fail_dir_fsync_at.lock().expect("lock") == Some(*count) {
            return Err(DurabilityError::InjectedFailure);
        }
        self.maybe_fail(DurabilityOp::DirFsync)
    }

    fn atomic_rename(&self, from: &Path, to: &Path) -> Result<(), DurabilityError> {
        self.ops
            .lock()
            .expect("lock")
            .push(DurabilityOp::AtomicRename);
        self.rename_ops
            .lock()
            .expect("lock")
            .push((from.to_path_buf(), to.to_path_buf()));
        self.maybe_fail(DurabilityOp::AtomicRename)
    }

    fn readback_verify(&self, _path: &Path, _expected: &[u8]) -> Result<(), DurabilityError> {
        self.ops.lock().expect("lock").push(DurabilityOp::Readback);
        self.maybe_fail(DurabilityOp::Readback)
    }
}

#[derive(Debug)]
pub struct FakeProcessGroup {
    fence_trace: Arc<FenceTrace>,
    alive_after_term: Mutex<VecDeque<bool>>,
    alive_after_kill: Mutex<VecDeque<bool>>,
    fail_bounded_wait: Mutex<VecDeque<bool>>,
}

impl FakeProcessGroup {
    pub fn new(fence_trace: Arc<FenceTrace>) -> Self {
        Self {
            fence_trace,
            alive_after_term: Mutex::new(VecDeque::new()),
            alive_after_kill: Mutex::new(VecDeque::new()),
            fail_bounded_wait: Mutex::new(VecDeque::new()),
        }
    }

    pub fn fail_next_bounded_wait(&self) {
        self.fail_bounded_wait.lock().expect("lock").push_back(true);
    }

    pub fn alive_after_term(&self, alive: bool) {
        self.alive_after_term.lock().expect("lock").push_back(alive);
    }

    pub fn alive_after_kill(&self, alive: bool) {
        self.alive_after_kill.lock().expect("lock").push_back(alive);
    }
}

impl ProcessGroupFence for FakeProcessGroup {
    fn signal(&self, kind: SignalKind) -> Result<(), FenceError> {
        let event = match kind {
            SignalKind::Term => FenceTraceEvent::Term,
            SignalKind::Kill => FenceTraceEvent::Kill,
        };
        self.fence_trace.push(event);
        Ok(())
    }

    fn group_alive(&self, stage: FenceStage) -> bool {
        match stage {
            FenceStage::AfterTerm => {
                self.fence_trace.push(FenceTraceEvent::AliveAfterTerm);
                self.alive_after_term
                    .lock()
                    .expect("lock")
                    .pop_front()
                    .unwrap_or(true)
            }
            FenceStage::AfterKill => {
                let alive = self
                    .alive_after_kill
                    .lock()
                    .expect("lock")
                    .pop_front()
                    .unwrap_or(false);
                if !alive {
                    self.fence_trace.push(FenceTraceEvent::ConfirmedExit);
                }
                alive
            }
        }
    }

    fn bounded_wait(&self, _timeout: Duration) -> Result<(), FenceError> {
        self.fence_trace.push(FenceTraceEvent::BoundedWait);
        if self
            .fail_bounded_wait
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(false)
        {
            return Err(FenceError::WaitFailed);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct FakeJobInspector {
    fence_trace: Arc<FenceTrace>,
    log_path: Mutex<Option<PathBuf>>,
    counts: Mutex<VecDeque<Result<u32, JobInspectError>>>,
}

impl FakeJobInspector {
    pub fn new(fence_trace: Arc<FenceTrace>) -> Self {
        Self {
            fence_trace,
            log_path: Mutex::new(None),
            counts: Mutex::new(VecDeque::new()),
        }
    }

    pub fn push_count(&self, count: Result<u32, JobInspectError>) {
        self.counts.lock().expect("lock").push_back(count);
    }

    pub fn observe_log_at_inspection(&self, path: PathBuf) {
        *self.log_path.lock().expect("lock") = Some(path);
    }
}

impl JobInspector for FakeJobInspector {
    fn candidate_job_count(&self) -> Result<u32, JobInspectError> {
        if let Some(path) = self.log_path.lock().expect("lock").clone() {
            if path.exists() {
                let reader = DecisionLogReader::open(&path).expect("reader");
                assert!(
                    !reader.contains_kind(AuthorityRecordKind::BeginRevert),
                    "BEGIN_REVERT must be absent at job inspection"
                );
            }
        }
        match self
            .counts
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(Ok(0))
        {
            Ok(0) => {
                self.fence_trace.push(FenceTraceEvent::ZeroCandidateJobs);
                Ok(0)
            }
            Ok(n) => {
                self.fence_trace.push(FenceTraceEvent::CandidateJobsRemain);
                Ok(n)
            }
            Err(err) => {
                self.fence_trace.push(FenceTraceEvent::JobInspectionFailed);
                Err(err)
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct FakeExternalFloor {
    states: Mutex<VecDeque<Result<u64, ExternalFloorError>>>,
}

impl FakeExternalFloor {
    pub fn push_floor(&self, state: Result<u64, ExternalFloorError>) {
        self.states.lock().expect("lock").push_back(state);
    }
}

impl ExternalFloorReader for FakeExternalFloor {
    fn read_floor_epoch(&self) -> Result<u64, ExternalFloorError> {
        self.states
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(Ok(0))
    }
}

#[derive(Debug, Default)]
pub struct FakeInvariants {
    outcomes: Mutex<VecDeque<Result<InvariantOutcome, InvariantError>>>,
}

impl FakeInvariants {
    pub fn push_outcome(&self, outcome: Result<InvariantOutcome, InvariantError>) {
        self.outcomes.lock().expect("lock").push_back(outcome);
    }
}

impl InvariantObserver for FakeInvariants {
    fn evaluate_mandatory(&self) -> Result<InvariantOutcome, InvariantError> {
        self.outcomes
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(Ok(InvariantOutcome::Pass))
    }
}

#[derive(Debug, Default)]
pub struct FakeBaseRevision {
    observed: Mutex<String>,
}

impl FakeBaseRevision {
    pub fn set_observed(&self, base: &str) {
        *self.observed.lock().expect("lock") = base.to_owned();
    }
}

impl BaseRevisionObserver for FakeBaseRevision {
    fn observed_base_revision(&self) -> String {
        self.observed.lock().expect("lock").clone()
    }
}

#[derive(Debug, Default)]
pub struct FakePeerCred {
    creds: Mutex<VecDeque<PeerCred>>,
}

impl FakePeerCred {
    pub fn enqueue_cred(&self, cred: PeerCred) {
        self.creds.lock().expect("lock").push_back(cred);
    }

    pub fn broker_cred(uid: u32, gid: u32, pid: i32) -> PeerCred {
        PeerCred { uid, gid, pid }
    }
}

impl PeerCredSource for FakePeerCred {
    fn peer_credentials(&self) -> Result<PeerCred, agentbed_watchdogd::error::PeerCredError> {
        self.creds
            .lock()
            .expect("lock")
            .pop_front()
            .ok_or(agentbed_watchdogd::error::PeerCredError::Unavailable)
    }
}

impl StreamPeerAuth for FakePeerCred {
    fn peer_credentials_for_stream(
        &self,
        _stream: &UnixStream,
    ) -> Result<PeerCred, agentbed_watchdogd::error::PeerCredError> {
        PeerCredSource::peer_credentials(self)
    }
}

#[derive(Debug)]
pub struct FakeBundle {
    pub clock: Arc<FakeClock>,
    pub entropy: Arc<FakeEntropy>,
    pub topology: Arc<FakeTopology>,
    pub durability: Arc<FakeDurability>,
    pub process_group: Arc<FakeProcessGroup>,
    pub job_inspector: Arc<FakeJobInspector>,
    pub external_floor: Arc<FakeExternalFloor>,
    pub invariants: Arc<FakeInvariants>,
    pub base_revision: Arc<FakeBaseRevision>,
    pub peer_cred: Arc<FakePeerCred>,
    pub fence_trace: Arc<FenceTrace>,
}

impl FakeBundle {
    pub fn new() -> Self {
        let clock = Arc::new(FakeClock::default());
        let entropy = Arc::new(FakeEntropy::default());
        entropy.set_bytes((0u8..32).collect());
        let fence_trace = Arc::new(FenceTrace::default());
        Self {
            clock,
            entropy,
            topology: Arc::new(FakeTopology::default()),
            durability: Arc::new(FakeDurability::default()),
            process_group: Arc::new(FakeProcessGroup::new(Arc::clone(&fence_trace))),
            job_inspector: Arc::new(FakeJobInspector::new(Arc::clone(&fence_trace))),
            external_floor: Arc::new(FakeExternalFloor::default()),
            invariants: Arc::new(FakeInvariants::default()),
            base_revision: Arc::new(FakeBaseRevision::default()),
            peer_cred: Arc::new(FakePeerCred::default()),
            fence_trace,
        }
    }
}
