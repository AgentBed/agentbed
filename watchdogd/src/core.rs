//! Watchdog core: durable authority, session binding, and request handling.

use crate::durability_store::{
    ambiguous_epoch_temp_residue, durable_atomic_rename, persist_safe_mode_marker, unique_temp_path,
};
use crate::error::{DurabilityError, ExternalFloorError, RpcError, TopologyError, WatchdogError};
use crate::interfaces::{Dependencies, FenceStage, InvariantOutcome, SignalKind};
use crate::read_model::{
    append_record, armed_record, decision_record, lease_renewed_record, AuthorityRecord,
    AuthorityRecordKind, DecisionLogReader,
};
use crate::rpc::protocol::{
    AuthenticatedRequest, LocalRequest, LocalResponse, SessionBind, SessionEstablished,
};
use crate::session::{BoundSession, SessionState};
use crate::worker_group_tag::WorkerGroupTag;
use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const WATCHDOG_MOUNT_ROOT: &str = "/var/lib/agentbed/watchdog";
pub const DECISION_LOG_REL: &str = "decisions/decision.log";
pub const EPOCH_HIGH_WATER_REL: &str = "epoch/high-water.json";
pub const SAFE_MODE_REL: &str = "state/safe-mode.json";

const LEASE_DURATION: Duration = Duration::from_secs(3600);
const BASE_MANDATORY_INVARIANTS: &[&str] = &["route_present"];

#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub store_root: PathBuf,
    pub socket_path: PathBuf,
    pub broker_uid: u32,
    pub broker_gid: u32,
    pub host_id: String,
}

#[derive(Debug, Clone)]
struct ArmedState {
    host_id: String,
    tx_id: String,
    epoch: u64,
    base: String,
    lease_id: String,
    worker_group_tag: WorkerGroupTag,
    #[allow(dead_code)]
    armed_at: SystemTime,
    deadline: SystemTime,
    lease_expires_at: SystemTime,
    chosen: Option<AuthorityRecordKind>,
}

#[derive(Debug)]
pub struct WatchdogCore {
    pub(crate) config: CoreConfig,
    pub(crate) deps: Dependencies,
    safe_mode: bool,
    pub(crate) durable_binding: RefCell<Option<BoundSession>>,
    armed: Option<ArmedState>,
    log_seq: u64,
    last_clock: SystemTime,
}

impl WatchdogCore {
    pub fn open(config: CoreConfig, deps: Dependencies) -> Result<Self, WatchdogError> {
        Self::startup(config, deps, false)
    }

    pub fn reopen(config: CoreConfig, deps: Dependencies) -> Result<Self, WatchdogError> {
        Self::startup(config, deps, true)
    }

    fn startup(
        config: CoreConfig,
        deps: Dependencies,
        is_reopen: bool,
    ) -> Result<Self, WatchdogError> {
        if config.store_root.join(SAFE_MODE_REL).exists() {
            return Err(WatchdogError::SafeModeActive);
        }
        match deps.topology.verify_startup(&config.store_root) {
            Err(TopologyError::UnavailableStore) => {
                return Err(WatchdogError::Topology(TopologyError::UnavailableStore));
            }
            Err(e) => {
                if e == TopologyError::Unwritable {
                    let tmp = config.store_root.join(".tmp-safe");
                    let marker = config.store_root.join(SAFE_MODE_REL);
                    if let Some(parent) = marker.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if deps.durability.atomic_rename(&tmp, &marker).is_err() {
                        return Err(WatchdogError::SafeModePersistUnavailable);
                    }
                }
                return Err(WatchdogError::Topology(e));
            }
            Ok(()) => {}
        }

        let epoch_path = config.store_root.join(EPOCH_HIGH_WATER_REL);
        let log_path = config.store_root.join(DECISION_LOG_REL);
        let log_reader = if log_path.exists() {
            Some(DecisionLogReader::open(&log_path).map_err(|_| WatchdogError::SafeModeActive)?)
        } else {
            None
        };
        let log_records = log_reader
            .as_ref()
            .map_or(0, DecisionLogReader::record_count);
        let log_epoch = log_reader.as_ref().map_or(0, DecisionLogReader::max_epoch);

        match deps.external_floor.read_floor_epoch() {
            Err(ExternalFloorError::Ambiguous | ExternalFloorError::Unavailable) => {
                return Err(WatchdogError::SafeModeActive);
            }
            Ok(floor) => {
                if is_reopen {
                    let file_epoch = read_epoch_file(&epoch_path)?;
                    if file_epoch < floor {
                        return Err(WatchdogError::SafeModeActive);
                    }
                    if log_records > 0 && file_epoch != log_epoch {
                        return Err(WatchdogError::EpochLogMismatch);
                    }
                    if file_epoch > 0 && log_records == 0 {
                        return Err(WatchdogError::EpochLogMismatch);
                    }
                } else if epoch_path.exists() {
                    let file_epoch = read_epoch_file(&epoch_path)?;
                    if file_epoch < floor {
                        return Err(WatchdogError::SafeModeActive);
                    }
                } else {
                    if let Some(parent) = epoch_path.parent() {
                        std::fs::create_dir_all(parent).map_err(|_| {
                            WatchdogError::Durability(DurabilityError::Io("io".into()))
                        })?;
                    }
                    write_epoch_file(&epoch_path, 0, &deps)?;
                }
            }
        }

        let (durable_binding, armed, last_clock) =
            Self::reopen_state(is_reopen, log_reader.as_ref())?;
        let log_seq = log_records as u64;

        Ok(Self {
            config,
            deps,
            safe_mode: false,
            durable_binding: RefCell::new(durable_binding),
            armed,
            log_seq,
            last_clock,
        })
    }

    fn reopen_state(
        is_reopen: bool,
        log_reader: Option<&DecisionLogReader>,
    ) -> Result<(Option<BoundSession>, Option<ArmedState>, SystemTime), WatchdogError> {
        let mut durable_binding = None;
        let mut armed = None;
        let mut last_clock = SystemTime::UNIX_EPOCH;
        if is_reopen {
            if let Some(reader) = log_reader {
                if let Some(reconstructed) = reader
                    .reconstruct_active_authority()
                    .map_err(|_| WatchdogError::SafeModeActive)?
                {
                    let binding = reconstructed.binding.clone();
                    last_clock = reconstructed.last_activity;
                    armed = Some(ArmedState {
                        host_id: binding.host_id.clone(),
                        tx_id: binding.tx_id.clone(),
                        epoch: binding.epoch,
                        base: reconstructed.base,
                        lease_id: binding.lease_id.clone(),
                        worker_group_tag: binding.worker_group_tag,
                        armed_at: reconstructed.armed_at,
                        deadline: reconstructed.deadline,
                        lease_expires_at: reconstructed.lease_expires_at,
                        chosen: reconstructed.chosen,
                    });
                    durable_binding = Some(binding);
                }
            }
        }
        Ok((durable_binding, armed, last_clock))
    }

    pub fn read_decision_log_sequence(&self) -> u64 {
        self.log_seq
    }

    pub(crate) fn ensure_not_safe_mode(&self) -> Result<(), RpcError> {
        if self.safe_mode {
            Err(RpcError::SafeModeActive)
        } else {
            Ok(())
        }
    }

    pub fn handle_request(
        &mut self,
        verified: AuthenticatedRequest,
        session: &mut SessionState,
    ) -> Result<LocalResponse, RpcError> {
        self.ensure_not_safe_mode()?;
        let counter = verified.counter();
        let req = verified.into_request();
        let resp = self.dispatch_request(&req)?;
        session.advance_counter(counter);
        Ok(resp)
    }

    fn dispatch_request(&mut self, req: &LocalRequest) -> Result<LocalResponse, RpcError> {
        match req {
            LocalRequest::Arm {
                request_id,
                host_id,
                tx_id,
                epoch,
                base,
                deadline_secs,
                deadline_nanos,
                mandatory_invariants,
                additive_manifest_checks,
            } => {
                let deadline = SystemTime::UNIX_EPOCH
                    .checked_add(Duration::new(*deadline_secs, *deadline_nanos))
                    .ok_or(RpcError::ExpiredDeadline)?;
                self.handle_arm(
                    request_id,
                    host_id,
                    tx_id,
                    *epoch,
                    base,
                    deadline,
                    mandatory_invariants,
                    additive_manifest_checks,
                )
            }
            LocalRequest::ReportHealth {
                request_id, tx_id, ..
            } => {
                self.require_armed_tx(tx_id)?;
                Ok(LocalResponse::HealthAck {
                    request_id: request_id.clone(),
                })
            }
            LocalRequest::RequestLeaseRenewal {
                request_id,
                tx_id,
                epoch,
                lease_id,
                worker_group_tag,
                ..
            } => {
                self.handle_lease_renewal(tx_id, *epoch, lease_id, *worker_group_tag)?;
                Ok(LocalResponse::LeaseRenewed {
                    request_id: request_id.clone(),
                })
            }
            LocalRequest::Heartbeat {
                request_id,
                tx_id,
                epoch,
                lease_id,
                worker_group_tag,
                ..
            } => {
                self.handle_lease_renewal(tx_id, *epoch, lease_id, *worker_group_tag)?;
                Ok(LocalResponse::HeartbeatAck {
                    request_id: request_id.clone(),
                })
            }
            LocalRequest::RequestDecision {
                request_id,
                tx_id,
                epoch,
                ..
            } => self.handle_decision(request_id, tx_id, *epoch),
        }
    }

    fn require_armed_tx(&self, tx_id: &str) -> Result<(), RpcError> {
        let armed = self.armed.as_ref().ok_or(RpcError::UnknownTransaction)?;
        if armed.tx_id != tx_id {
            return Err(RpcError::UnknownTransaction);
        }
        Ok(())
    }

    fn validate_manifest(
        mandatory_invariants: &[String],
        additive_manifest_checks: &[String],
    ) -> Result<(), RpcError> {
        if mandatory_invariants.is_empty() {
            return Err(RpcError::WeakenedMandatoryInvariant);
        }
        for required in BASE_MANDATORY_INVARIANTS {
            if !mandatory_invariants.iter().any(|name| name == required) {
                return Err(RpcError::WeakenedMandatoryInvariant);
            }
        }
        for additive in additive_manifest_checks {
            if !mandatory_invariants.iter().any(|name| name == additive) {
                return Err(RpcError::WeakenedMandatoryInvariant);
            }
        }
        Ok(())
    }

    fn read_durable_epoch_strict(&mut self) -> Result<u64, RpcError> {
        let path = self.config.store_root.join(EPOCH_HIGH_WATER_REL);
        if let Ok(epoch) = read_epoch_file(&path) {
            Ok(epoch)
        } else {
            self.enter_safe_mode()?;
            Err(RpcError::SafeModeActive)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_arm(
        &mut self,
        request_id: &str,
        host_id: &str,
        tx_id: &str,
        epoch: u64,
        base: &str,
        deadline: SystemTime,
        mandatory_invariants: &[String],
        additive_manifest_checks: &[String],
    ) -> Result<LocalResponse, RpcError> {
        if host_id != self.config.host_id {
            return Err(RpcError::WrongBinding);
        }
        let observed = self.deps.base_revision.observed_base_revision();
        if !observed.is_empty() && observed != base {
            return Err(RpcError::MovedBase);
        }
        Self::validate_manifest(mandatory_invariants, additive_manifest_checks)?;
        let durable_epoch = self.read_durable_epoch_strict()?;
        if epoch < durable_epoch {
            return Err(RpcError::StaleEpoch);
        }
        if epoch != self.bound_epoch()? {
            return Err(RpcError::WrongEpoch);
        }
        let now = self.deps.clock.now();
        if deadline <= now {
            return Err(RpcError::ExpiredDeadline);
        }
        if self.armed.is_some() {
            return Err(RpcError::ConflictingRequest);
        }
        let (lease_id, worker_group_tag) = {
            let bound = self.durable_binding.borrow();
            let bound = bound.as_ref().ok_or(RpcError::WrongBinding)?;
            (bound.lease_id.clone(), bound.worker_group_tag)
        };
        self.advance_epoch(epoch)?;
        let lease_expires_at = min_time(now + LEASE_DURATION, deadline);
        let record = armed_record(
            self.next_sequence()?,
            epoch,
            host_id,
            tx_id,
            base,
            &lease_id,
            worker_group_tag,
            now,
            deadline,
            lease_expires_at,
        );
        self.append_record(record)?;
        self.armed = Some(ArmedState {
            host_id: host_id.to_owned(),
            tx_id: tx_id.to_owned(),
            epoch,
            base: base.to_owned(),
            lease_id,
            worker_group_tag,
            armed_at: now,
            deadline,
            lease_expires_at,
            chosen: None,
        });
        self.last_clock = now;
        Ok(LocalResponse::Armed {
            request_id: request_id.to_owned(),
        })
    }

    fn handle_lease_renewal(
        &mut self,
        tx_id: &str,
        epoch: u64,
        lease_id: &str,
        worker_group_tag: WorkerGroupTag,
    ) -> Result<(), RpcError> {
        let (
            host_id,
            armed_tx,
            armed_epoch,
            armed_lease,
            armed_tag,
            armed_deadline,
            lease_expires_at,
        ) = {
            let armed = self.armed.as_ref().ok_or(RpcError::UnknownTransaction)?;
            (
                armed.host_id.clone(),
                armed.tx_id.clone(),
                armed.epoch,
                armed.lease_id.clone(),
                armed.worker_group_tag,
                armed.deadline,
                armed.lease_expires_at,
            )
        };
        if armed_tx != tx_id {
            return Err(RpcError::UnknownTransaction);
        }
        if armed_epoch != epoch {
            return Err(RpcError::StaleEpoch);
        }
        if armed_lease != lease_id || armed_tag != worker_group_tag {
            return Err(RpcError::WrongBinding);
        }
        if self
            .armed
            .as_ref()
            .is_some_and(|armed| armed.chosen.is_some())
        {
            return Err(RpcError::ConflictingRequest);
        }
        let now = self.deps.clock.now();
        if now < self.last_clock {
            return Err(RpcError::ClockRegression);
        }
        if now >= lease_expires_at || now > armed_deadline {
            return Err(RpcError::ExpiredDeadline);
        }
        let new_expiry = min_time(now + LEASE_DURATION, armed_deadline);
        if new_expiry <= lease_expires_at {
            self.last_clock = now;
            return Ok(());
        }
        let record = lease_renewed_record(
            self.next_sequence()?,
            epoch,
            &host_id,
            tx_id,
            lease_id,
            worker_group_tag,
            new_expiry,
        );
        self.append_record(record)?;
        if let Some(armed) = self.armed.as_mut() {
            armed.lease_expires_at = new_expiry;
        }
        self.last_clock = now;
        Ok(())
    }

    fn handle_decision(
        &mut self,
        request_id: &str,
        tx_id: &str,
        epoch: u64,
    ) -> Result<LocalResponse, RpcError> {
        let (
            host_id,
            armed_tx,
            armed_epoch,
            armed_base,
            armed_deadline,
            lease_expires_at,
            armed_lease,
            armed_tag,
            chosen,
        ) = {
            let armed = self.armed.as_ref().ok_or(RpcError::UnknownTransaction)?;
            (
                armed.host_id.clone(),
                armed.tx_id.clone(),
                armed.epoch,
                armed.base.clone(),
                armed.deadline,
                armed.lease_expires_at,
                armed.lease_id.clone(),
                armed.worker_group_tag,
                armed.chosen,
            )
        };
        if armed_tx != tx_id {
            return Err(RpcError::UnknownTransaction);
        }
        if armed_epoch != epoch {
            return Err(RpcError::StaleEpoch);
        }
        if chosen.is_some() {
            return Err(RpcError::ConflictingRequest);
        }
        let log_path = self.config.store_root.join(DECISION_LOG_REL);
        if log_path.exists() && DecisionLogReader::open(&log_path).is_err() {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        let now = self.deps.clock.now();
        let observed = self.deps.base_revision.observed_base_revision();
        if !observed.is_empty() && observed != armed_base {
            return Err(RpcError::MovedBase);
        }
        if now >= lease_expires_at {
            self.run_fence()?;
        }
        if now > armed_deadline {
            return Err(RpcError::ExpiredDeadline);
        }
        let outcome = self
            .deps
            .invariants
            .evaluate_mandatory()
            .map_err(|_| RpcError::SafeModeActive)?;
        let kind = match outcome {
            InvariantOutcome::Pass => AuthorityRecordKind::BeginCommit,
            InvariantOutcome::Fail => AuthorityRecordKind::BeginRevert,
        };
        let record = decision_record(
            self.next_sequence()?,
            epoch,
            kind,
            &host_id,
            tx_id,
            &armed_lease,
            armed_tag,
        );
        self.append_record(record)?;
        if let Some(armed) = self.armed.as_mut() {
            armed.chosen = Some(kind);
        }
        Ok(LocalResponse::AuthorityChosen {
            request_id: request_id.to_owned(),
            kind,
        })
    }

    fn run_fence(&mut self) -> Result<(), RpcError> {
        if let Err(error) = self.do_run_fence() {
            let _ = self.enter_safe_mode();
            return Err(error);
        }
        Ok(())
    }

    fn do_run_fence(&self) -> Result<(), RpcError> {
        self.deps
            .process_group
            .signal(SignalKind::Term)
            .map_err(|_| RpcError::FenceIncomplete)?;
        self.deps
            .process_group
            .bounded_wait(Duration::from_secs(1))
            .map_err(|_| RpcError::FenceIncomplete)?;
        let alive_after_term = self.deps.process_group.group_alive(FenceStage::AfterTerm);
        if alive_after_term {
            self.deps
                .process_group
                .signal(SignalKind::Kill)
                .map_err(|_| RpcError::FenceIncomplete)?;
            self.deps
                .process_group
                .bounded_wait(Duration::from_secs(1))
                .map_err(|_| RpcError::FenceIncomplete)?;
            if self.deps.process_group.group_alive(FenceStage::AfterKill) {
                return Err(RpcError::FenceIncomplete);
            }
        }
        let jobs = self
            .deps
            .job_inspector
            .candidate_job_count()
            .map_err(|_| RpcError::FenceIncomplete)?;
        if jobs != 0 {
            return Err(RpcError::FenceIncomplete);
        }
        Ok(())
    }

    fn next_sequence(&self) -> Result<u64, RpcError> {
        self.log_seq.checked_add(1).ok_or(RpcError::SafeModeActive)
    }

    fn append_record(&mut self, record: AuthorityRecord) -> Result<(), RpcError> {
        let path = self.config.store_root.join(DECISION_LOG_REL);
        let sequence = record.sequence;
        if let Err(error) = append_record(&path, &record, &*self.deps.durability) {
            self.latch_safe_mode_best_effort();
            return Err(RpcError::Durability(error));
        }
        self.log_seq = sequence;
        Ok(())
    }

    fn advance_epoch(&mut self, epoch: u64) -> Result<(), RpcError> {
        let path = self.config.store_root.join(EPOCH_HIGH_WATER_REL);
        let parent = path.parent().ok_or(RpcError::SafeModeActive)?;
        if ambiguous_epoch_temp_residue(parent) {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        if fs::create_dir_all(parent).is_err() {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        let current = self.read_durable_epoch_strict()?;
        if epoch < current {
            return Err(RpcError::StaleEpoch);
        }
        let bytes = epoch_bytes(epoch);
        let tmp = unique_temp_path(parent, "epoch");
        if write_epoch_temp_exclusive(&tmp, &bytes).is_err() {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        if let Err(error) = self.deps.durability.file_fsync(&tmp) {
            self.latch_safe_mode_best_effort();
            return Err(RpcError::Durability(error));
        }
        if durable_atomic_rename(&*self.deps.durability, &tmp, &path).is_err() {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        if let Err(error) = self.deps.durability.dir_fsync(parent) {
            self.latch_safe_mode_best_effort();
            return Err(RpcError::Durability(error));
        }
        if self.deps.durability.readback_verify(&path, &bytes).is_err() {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        Ok(())
    }

    fn latch_safe_mode_best_effort(&mut self) {
        self.safe_mode = true;
        let _ = persist_safe_mode_marker(&self.config.store_root, &*self.deps.durability);
    }

    fn enter_safe_mode(&mut self) -> Result<(), RpcError> {
        self.safe_mode = true;
        persist_safe_mode_marker(&self.config.store_root, &*self.deps.durability)?;
        Ok(())
    }

    fn bound_epoch(&self) -> Result<u64, RpcError> {
        self.durable_binding
            .borrow()
            .as_ref()
            .map(|b| b.epoch)
            .ok_or(RpcError::WrongBinding)
    }
}

impl SessionState {
    pub fn bind(
        core: &WatchdogCore,
        peer_cred: &dyn crate::interfaces::PeerCredSource,
        entropy: &dyn crate::interfaces::Entropy,
        bind: SessionBind,
    ) -> Result<(Self, SessionEstablished), RpcError> {
        let durable = core.durable_binding.borrow().clone();
        let (state, established, binding) = SessionState::try_bind(
            core.config.broker_uid,
            core.config.broker_gid,
            core.safe_mode,
            &durable,
            peer_cred,
            entropy,
            bind,
        )?;
        if let Some(b) = binding {
            *core.durable_binding.borrow_mut() = Some(b);
        }
        Ok((state, established))
    }

    pub fn bind_with_stream_cred(
        core: &WatchdogCore,
        cred: &crate::interfaces::PeerCred,
        entropy: &dyn crate::interfaces::Entropy,
        bind: SessionBind,
    ) -> Result<(Self, SessionEstablished), RpcError> {
        let durable = core.durable_binding.borrow().clone();
        let (state, established, binding) = SessionState::try_bind_with_cred(
            core.config.broker_uid,
            core.config.broker_gid,
            core.safe_mode,
            &durable,
            cred,
            entropy,
            bind,
        )?;
        if let Some(b) = binding {
            *core.durable_binding.borrow_mut() = Some(b);
        }
        Ok((state, established))
    }
}

fn min_time(a: SystemTime, b: SystemTime) -> SystemTime {
    if a <= b {
        a
    } else {
        b
    }
}

fn epoch_bytes(epoch: u64) -> Vec<u8> {
    epoch.to_be_bytes().to_vec()
}

fn read_epoch_file(path: &Path) -> Result<u64, WatchdogError> {
    let data = fs::read(path).map_err(|_| WatchdogError::EpochLogMismatch)?;
    if data.len() != 8 {
        return Err(WatchdogError::EpochLogMismatch);
    }
    let bytes: [u8; 8] = data
        .try_into()
        .map_err(|_| WatchdogError::EpochLogMismatch)?;
    Ok(u64::from_be_bytes(bytes))
}

fn write_epoch_file(path: &Path, epoch: u64, deps: &Dependencies) -> Result<(), WatchdogError> {
    let bytes = epoch_bytes(epoch);
    fs::write(path, &bytes)
        .map_err(|_| WatchdogError::Durability(DurabilityError::Io("io".into())))?;
    deps.durability
        .file_fsync(path)
        .map_err(WatchdogError::Durability)?;
    if let Some(parent) = path.parent() {
        deps.durability
            .dir_fsync(parent)
            .map_err(WatchdogError::Durability)?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_epoch_temp_exclusive(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_EXCL)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_epoch_temp_exclusive(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(path, bytes)
}
