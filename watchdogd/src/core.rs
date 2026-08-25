//! Watchdog core: durable authority, session binding, and request handling.

use crate::durability_store::{
    durable_atomic_rename, persist_safe_mode_marker, unique_temp_path, LEGACY_EPOCH_TEMP,
};
use crate::error::{DurabilityError, ExternalFloorError, RpcError, TopologyError, WatchdogError};
use crate::interfaces::{Dependencies, FenceStage, InvariantOutcome, SignalKind};
use crate::read_model::{append_record, AuthorityRecordKind, DecisionLogReader};
use crate::rpc::protocol::{
    AuthenticatedRequest, LocalRequest, LocalResponse, SessionBind, SessionEstablished,
};
use crate::session::{BoundSession, SessionState};
use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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

#[derive(Debug)]
struct ArmedState {
    tx_id: String,
    epoch: u64,
    base: String,
    lease_id: String,
    process_group: i32,
    armed_at: SystemTime,
    deadline: SystemTime,
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

        Ok(Self {
            config,
            deps,
            safe_mode: false,
            durable_binding: RefCell::new(None),
            armed: None,
            log_seq: log_records as u64,
            last_clock: SystemTime::UNIX_EPOCH,
        })
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
                process_group,
                ..
            } => {
                self.handle_lease_renewal(tx_id, *epoch, lease_id, *process_group)?;
                Ok(LocalResponse::LeaseRenewed {
                    request_id: request_id.clone(),
                })
            }
            LocalRequest::Heartbeat {
                request_id,
                tx_id,
                epoch,
                lease_id,
                process_group,
                ..
            } => {
                self.handle_lease_renewal(tx_id, *epoch, lease_id, *process_group)?;
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
        let durable_epoch =
            read_epoch_file(&self.config.store_root.join(EPOCH_HIGH_WATER_REL)).unwrap_or(0);
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
        let (lease_id, process_group) = {
            let bound = self.durable_binding.borrow();
            let bound = bound.as_ref().ok_or(RpcError::WrongBinding)?;
            (bound.lease_id.clone(), bound.process_group)
        };
        self.advance_epoch(epoch)?;
        self.append_authority(AuthorityRecordKind::Armed, epoch)?;
        self.armed = Some(ArmedState {
            tx_id: tx_id.to_owned(),
            epoch,
            base: base.to_owned(),
            lease_id,
            process_group,
            armed_at: now,
            deadline,
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
        process_group: i32,
    ) -> Result<(), RpcError> {
        let (armed_tx, armed_epoch, armed_lease, armed_pg, armed_deadline) = {
            let armed = self.armed.as_ref().ok_or(RpcError::UnknownTransaction)?;
            (
                armed.tx_id.clone(),
                armed.epoch,
                armed.lease_id.clone(),
                armed.process_group,
                armed.deadline,
            )
        };
        if armed_tx != tx_id {
            return Err(RpcError::UnknownTransaction);
        }
        if armed_epoch != epoch {
            return Err(RpcError::StaleEpoch);
        }
        if armed_lease != lease_id || armed_pg != process_group {
            return Err(RpcError::WrongBinding);
        }
        let now = self.deps.clock.now();
        if now < self.last_clock {
            return Err(RpcError::ClockRegression);
        }
        if now > armed_deadline {
            return Err(RpcError::ExpiredDeadline);
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
        let (armed_tx, armed_epoch, armed_base, armed_deadline, armed_at, process_group) = {
            let armed = self.armed.as_ref().ok_or(RpcError::UnknownTransaction)?;
            (
                armed.tx_id.clone(),
                armed.epoch,
                armed.base.clone(),
                armed.deadline,
                armed.armed_at,
                armed.process_group,
            )
        };
        if armed_tx != tx_id {
            return Err(RpcError::UnknownTransaction);
        }
        if armed_epoch != epoch {
            return Err(RpcError::StaleEpoch);
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
        let lease_expired = armed_at
            .checked_add(LEASE_DURATION)
            .is_some_and(|expiry| now > expiry);
        if lease_expired {
            self.run_fence(process_group)?;
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
        self.append_authority(kind, epoch)?;
        Ok(LocalResponse::AuthorityChosen {
            request_id: request_id.to_owned(),
            kind,
        })
    }

    fn run_fence(&mut self, pgid: i32) -> Result<(), RpcError> {
        if let Err(error) = self.do_run_fence(pgid) {
            let _ = self.enter_safe_mode();
            return Err(error);
        }
        Ok(())
    }

    fn do_run_fence(&self, pgid: i32) -> Result<(), RpcError> {
        self.deps
            .process_group
            .signal(SignalKind::Term, pgid)
            .map_err(|_| RpcError::FenceIncomplete)?;
        self.deps
            .process_group
            .bounded_wait(Duration::from_secs(1))
            .map_err(|_| RpcError::FenceIncomplete)?;
        let _ = self.deps.process_group.group_alive(FenceStage::AfterTerm);
        self.deps
            .process_group
            .signal(SignalKind::Kill, pgid)
            .map_err(|_| RpcError::FenceIncomplete)?;
        if self.deps.process_group.group_alive(FenceStage::AfterKill) {
            return Err(RpcError::FenceIncomplete);
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

    fn append_authority(&mut self, kind: AuthorityRecordKind, epoch: u64) -> Result<(), RpcError> {
        let path = self.config.store_root.join(DECISION_LOG_REL);
        let sequence = self
            .log_seq
            .checked_add(1)
            .ok_or(RpcError::SafeModeActive)?;
        if let Err(error) = append_record(&path, sequence, epoch, kind, &*self.deps.durability) {
            return Err(RpcError::Durability(error));
        }
        self.log_seq = sequence;
        Ok(())
    }

    fn advance_epoch(&mut self, epoch: u64) -> Result<(), RpcError> {
        let path = self.config.store_root.join(EPOCH_HIGH_WATER_REL);
        if let Some(parent) = path.parent() {
            let legacy_tmp = parent.join(LEGACY_EPOCH_TEMP);
            if legacy_tmp.exists() {
                self.enter_safe_mode()?;
                return Err(RpcError::SafeModeActive);
            }
            if fs::create_dir_all(parent).is_err() {
                self.enter_safe_mode()?;
                return Err(RpcError::SafeModeActive);
            }
        }
        let current = read_epoch_file(&path).unwrap_or(0);
        if epoch < current {
            return Err(RpcError::StaleEpoch);
        }
        let bytes = epoch_bytes(epoch);
        let tmp = unique_temp_path(&self.config.store_root, "epoch");
        if write_epoch_temp_exclusive(&tmp, &bytes).is_err() {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        if let Err(error) = self.deps.durability.file_fsync(&tmp) {
            return Err(RpcError::Durability(error));
        }
        if durable_atomic_rename(&*self.deps.durability, &tmp, &path).is_err() {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        if self.deps.durability.readback_verify(&path, &bytes).is_err() {
            self.enter_safe_mode()?;
            return Err(RpcError::SafeModeActive);
        }
        let parent = path.parent().ok_or(RpcError::SafeModeActive)?;
        if let Err(error) = self.deps.durability.dir_fsync(parent) {
            return Err(RpcError::Durability(error));
        }
        Ok(())
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

fn epoch_bytes(epoch: u64) -> Vec<u8> {
    epoch.to_be_bytes().to_vec()
}

fn read_epoch_file(path: &Path) -> Result<u64, WatchdogError> {
    let data = fs::read(path).map_err(|_| WatchdogError::EpochLogMismatch)?;
    if data.len() < 8 {
        return Err(WatchdogError::EpochLogMismatch);
    }
    let bytes: [u8; 8] = data
        .get(..8)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(WatchdogError::EpochLogMismatch)?;
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
