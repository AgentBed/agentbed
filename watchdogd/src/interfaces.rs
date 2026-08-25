//! Injected external interfaces for hermetic testing.

pub use crate::error::FenceStage;
use crate::error::{
    DurabilityError, ExternalFloorError, FenceError, InvariantError, JobInspectError,
    PeerCredError, TopologyError,
};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCred {
    pub uid: u32,
    pub gid: u32,
    pub pid: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvariantOutcome {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Term,
    Kill,
}

pub trait Clock: std::fmt::Debug + Send + Sync {
    fn now(&self) -> SystemTime;
}

pub trait Entropy: std::fmt::Debug + Send + Sync {
    fn fill(&self, out: &mut [u8]);
}

pub trait TopologyProbe: std::fmt::Debug + Send + Sync {
    fn verify_startup(&self, store_root: &Path) -> Result<(), TopologyError>;
}

pub trait Durability: std::fmt::Debug + Send + Sync {
    fn file_fsync(&self, path: &Path) -> Result<(), DurabilityError>;
    fn dir_fsync(&self, path: &Path) -> Result<(), DurabilityError>;
    fn atomic_rename(&self, from: &Path, to: &Path) -> Result<(), DurabilityError>;
    fn readback_verify(&self, path: &Path, expected: &[u8]) -> Result<(), DurabilityError>;
}

pub trait ProcessGroupFence: std::fmt::Debug + Send + Sync {
    fn signal(&self, kind: SignalKind, pgid: i32) -> Result<(), FenceError>;
    fn group_alive(&self, stage: FenceStage) -> bool;
    fn bounded_wait(&self, timeout: Duration) -> Result<(), FenceError>;
}

pub trait JobInspector: std::fmt::Debug + Send + Sync {
    fn candidate_job_count(&self) -> Result<u32, JobInspectError>;
}

pub trait ExternalFloorReader: std::fmt::Debug + Send + Sync {
    fn read_floor_epoch(&self) -> Result<u64, ExternalFloorError>;
}

pub trait InvariantObserver: std::fmt::Debug + Send + Sync {
    fn evaluate_mandatory(&self) -> Result<InvariantOutcome, InvariantError>;
}

pub trait BaseRevisionObserver: std::fmt::Debug + Send + Sync {
    fn observed_base_revision(&self) -> String;
}

pub trait PeerCredSource: std::fmt::Debug + Send + Sync {
    fn peer_credentials(&self) -> Result<PeerCred, PeerCredError>;
}

pub trait StreamPeerAuth: std::fmt::Debug + Send + Sync {
    fn peer_credentials_for_stream(
        &self,
        stream: &std::os::unix::net::UnixStream,
    ) -> Result<PeerCred, PeerCredError>;
}

impl<T> Entropy for Arc<T>
where
    T: Entropy + ?Sized,
{
    fn fill(&self, out: &mut [u8]) {
        self.as_ref().fill(out);
    }
}

impl<T> PeerCredSource for Arc<T>
where
    T: PeerCredSource + ?Sized,
{
    fn peer_credentials(&self) -> Result<PeerCred, PeerCredError> {
        self.as_ref().peer_credentials()
    }
}

#[derive(Debug)]
pub struct Dependencies {
    pub clock: Arc<dyn Clock>,
    pub entropy: Arc<dyn Entropy>,
    pub topology: Arc<dyn TopologyProbe>,
    pub durability: Arc<dyn Durability>,
    pub process_group: Arc<dyn ProcessGroupFence>,
    pub job_inspector: Arc<dyn JobInspector>,
    pub external_floor: Arc<dyn ExternalFloorReader>,
    pub invariants: Arc<dyn InvariantObserver>,
    pub base_revision: Arc<dyn BaseRevisionObserver>,
    pub peer_cred: Arc<dyn PeerCredSource>,
    pub stream_peer: Arc<dyn StreamPeerAuth>,
}

impl Dependencies {
    #[allow(clippy::too_many_arguments)]
    pub fn new<C, E, T, D, P, J, F, I, B, R, S>(
        clock: Arc<C>,
        entropy: Arc<E>,
        topology: Arc<T>,
        durability: Arc<D>,
        process_group: Arc<P>,
        job_inspector: Arc<J>,
        external_floor: Arc<F>,
        invariants: Arc<I>,
        base_revision: Arc<B>,
        peer_cred: Arc<R>,
        stream_peer: Arc<S>,
    ) -> Self
    where
        C: Clock + 'static,
        E: Entropy + 'static,
        T: TopologyProbe + 'static,
        D: Durability + 'static,
        P: ProcessGroupFence + 'static,
        J: JobInspector + 'static,
        F: ExternalFloorReader + 'static,
        I: InvariantObserver + 'static,
        B: BaseRevisionObserver + 'static,
        R: PeerCredSource + 'static,
        S: StreamPeerAuth + 'static,
    {
        Self {
            clock,
            entropy,
            topology,
            durability,
            process_group,
            job_inspector,
            external_floor,
            invariants,
            base_revision,
            peer_cred,
            stream_peer,
        }
    }
}
