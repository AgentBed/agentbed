//! Strict reader for the watchdog-owned append-only authority log.

use crate::error::DurabilityError;
use crate::interfaces::Durability;
use crate::session::BoundSession;
use crate::worker_group_tag::WorkerGroupTag;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HEADER_BYTES: usize = 8;
const MAX_RECORD_BYTES: usize = 64 * 1024;
pub(crate) const LEASE_WINDOW: Duration = Duration::from_secs(3600);

/// Frozen lease policy: `min(observed_at + 3600s, hard_deadline)`.
pub(crate) fn lease_expiry_at(
    observed_at: SystemTime,
    hard_deadline: SystemTime,
) -> Result<SystemTime, Error> {
    if hard_deadline <= observed_at {
        return Err(invalid_log("deadline before observed_at"));
    }
    let candidate = observed_at
        .checked_add(LEASE_WINDOW)
        .ok_or_else(|| invalid_log("lease window overflow"))?;
    Ok(if candidate <= hard_deadline {
        candidate
    } else {
        hard_deadline
    })
}

/// Watchdog-owned durable authority record kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorityRecordKind {
    Armed,
    ProbationPassed,
    LeaseRenewed,
    BeginCommit,
    BeginRevert,
    Committed,
    Reverted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthorityRecord {
    pub(crate) sequence: u64,
    epoch: u64,
    kind: AuthorityRecordKind,
    host_id: Option<String>,
    tx_id: Option<String>,
    base: Option<String>,
    lease_id: Option<String>,
    worker_group_tag: Option<WorkerGroupTag>,
    armed_at_secs: Option<u64>,
    armed_at_nanos: Option<u32>,
    deadline_secs: Option<u64>,
    deadline_nanos: Option<u32>,
    lease_expires_at_secs: Option<u64>,
    lease_expires_at_nanos: Option<u32>,
    renewed_at_secs: Option<u64>,
    renewed_at_nanos: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReconstructedAuthority {
    pub binding: BoundSession,
    pub base: String,
    pub armed_at: SystemTime,
    pub deadline: SystemTime,
    pub lease_expires_at: SystemTime,
    pub chosen: Option<AuthorityRecordKind>,
    pub log_seq: u64,
    pub last_activity: SystemTime,
}

/// Immutable in-memory projection of a validated decision log.
#[derive(Debug)]
pub struct DecisionLogReader {
    records: Vec<AuthorityRecord>,
}

impl AuthorityRecord {
    fn binding_fields(&self) -> Result<(&str, &str, &str, WorkerGroupTag), Error> {
        let host_id = self
            .host_id
            .as_deref()
            .ok_or_else(|| invalid_log("missing host_id"))?;
        let tx_id = self
            .tx_id
            .as_deref()
            .ok_or_else(|| invalid_log("missing tx_id"))?;
        let lease_id = self
            .lease_id
            .as_deref()
            .ok_or_else(|| invalid_log("missing lease_id"))?;
        let worker_group_tag = self
            .worker_group_tag
            .ok_or_else(|| invalid_log("missing worker_group_tag"))?;
        if host_id.is_empty() {
            return Err(invalid_log("empty host_id"));
        }
        if tx_id.is_empty() {
            return Err(invalid_log("empty tx_id"));
        }
        if lease_id.is_empty() {
            return Err(invalid_log("empty lease_id"));
        }
        Ok((host_id, tx_id, lease_id, worker_group_tag))
    }

    fn time_field(secs: Option<u64>, nanos: Option<u32>, label: &str) -> Result<SystemTime, Error> {
        let secs = secs.ok_or_else(|| invalid_log(&format!("missing {label}_secs")))?;
        let nanos = nanos.ok_or_else(|| invalid_log(&format!("missing {label}_nanos")))?;
        if nanos >= 1_000_000_000 {
            return Err(invalid_log(&format!("invalid {label}_nanos")));
        }
        UNIX_EPOCH
            .checked_add(Duration::new(secs, nanos))
            .ok_or_else(|| invalid_log(&format!("invalid {label}")))
    }

    fn has_renewed_at_fields(&self) -> bool {
        self.renewed_at_secs.is_some() || self.renewed_at_nanos.is_some()
    }

    fn has_armed_time_fields(&self) -> bool {
        self.armed_at_secs.is_some() || self.armed_at_nanos.is_some()
    }

    fn has_deadline_fields(&self) -> bool {
        self.deadline_secs.is_some() || self.deadline_nanos.is_some()
    }

    fn has_lease_expiry_fields(&self) -> bool {
        self.lease_expires_at_secs.is_some() || self.lease_expires_at_nanos.is_some()
    }

    fn reject_forbidden_fields(&self, forbidden: &[&str]) -> Result<(), Error> {
        for label in forbidden {
            let present = match *label {
                "base" => self.base.is_some(),
                "armed_at" => self.has_armed_time_fields(),
                "deadline" => self.has_deadline_fields(),
                "lease_expires_at" => self.has_lease_expiry_fields(),
                "renewed_at" => self.has_renewed_at_fields(),
                _ => false,
            };
            if present {
                return Err(invalid_log(&format!("forbidden field {label}")));
            }
        }
        Ok(())
    }

    fn validate_schema(&self) -> Result<(), Error> {
        match self.kind {
            AuthorityRecordKind::Armed => {
                let (_, _, _, _) = self.binding_fields()?;
                if self.base.as_deref().is_none_or(str::is_empty) {
                    return Err(invalid_log("missing base"));
                }
                self.reject_forbidden_fields(&["renewed_at"])?;
                let armed_at =
                    Self::time_field(self.armed_at_secs, self.armed_at_nanos, "armed_at")?;
                let deadline =
                    Self::time_field(self.deadline_secs, self.deadline_nanos, "deadline")?;
                let lease_expires_at = Self::time_field(
                    self.lease_expires_at_secs,
                    self.lease_expires_at_nanos,
                    "lease_expires_at",
                )?;
                if armed_at >= deadline {
                    return Err(invalid_log("armed_at not before deadline"));
                }
                let expected = lease_expiry_at(armed_at, deadline)?;
                if lease_expires_at != expected {
                    return Err(invalid_log("lease_expires_at policy mismatch"));
                }
            }
            AuthorityRecordKind::LeaseRenewed => {
                let (_, _, _, _) = self.binding_fields()?;
                self.reject_forbidden_fields(&["base", "armed_at", "deadline"])?;
                Self::time_field(self.renewed_at_secs, self.renewed_at_nanos, "renewed_at")?;
                Self::time_field(
                    self.lease_expires_at_secs,
                    self.lease_expires_at_nanos,
                    "lease_expires_at",
                )?;
            }
            AuthorityRecordKind::BeginCommit | AuthorityRecordKind::BeginRevert => {
                let (_, _, _, _) = self.binding_fields()?;
                self.reject_forbidden_fields(&[
                    "base",
                    "armed_at",
                    "deadline",
                    "lease_expires_at",
                    "renewed_at",
                ])?;
            }
            AuthorityRecordKind::ProbationPassed
            | AuthorityRecordKind::Committed
            | AuthorityRecordKind::Reverted => {
                let (_, tx_id, _, _) = self.binding_fields()?;
                if tx_id.is_empty() {
                    return Err(invalid_log("missing tx_id"));
                }
                self.reject_forbidden_fields(&[
                    "base",
                    "armed_at",
                    "deadline",
                    "lease_expires_at",
                    "renewed_at",
                ])?;
            }
        }
        Ok(())
    }

    fn to_bound_session(&self) -> Result<BoundSession, Error> {
        let (host_id, tx_id, lease_id, worker_group_tag) = self.binding_fields()?;
        Ok(BoundSession {
            host_id: host_id.to_owned(),
            tx_id: tx_id.to_owned(),
            epoch: self.epoch,
            lease_id: lease_id.to_owned(),
            worker_group_tag,
        })
    }
}

impl DecisionLogReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self {
                records: Vec::new(),
            });
        }
        let mut bytes = Vec::new();
        File::open(path)?.read_to_end(&mut bytes)?;
        let mut records = Vec::new();
        let mut last_epoch = 0_u64;
        let mut offset = 0usize;
        while offset < bytes.len() {
            let remaining = bytes
                .get(offset..)
                .ok_or_else(|| invalid_log("invalid record offset"))?;
            if remaining.len() < HEADER_BYTES {
                return Err(invalid_log("truncated record header"));
            }
            let length = read_u32(remaining, 0)? as usize;
            if length > MAX_RECORD_BYTES {
                return Err(invalid_log("oversized decision record"));
            }
            let crc = read_u32(remaining, 4)?;
            let record_end = HEADER_BYTES
                .checked_add(length)
                .ok_or_else(|| invalid_log("record length overflow"))?;
            let payload = remaining
                .get(HEADER_BYTES..record_end)
                .ok_or_else(|| invalid_log("truncated decision record"))?;
            if crc32(payload) != crc {
                return Err(invalid_log("decision record CRC mismatch"));
            }
            let record: AuthorityRecord = serde_json::from_slice(payload)
                .map_err(|_| invalid_log("invalid decision record JSON"))?;
            record.validate_schema()?;
            let expected = u64::try_from(records.len())
                .ok()
                .and_then(|count| count.checked_add(1))
                .ok_or_else(|| invalid_log("decision sequence overflow"))?;
            if record.sequence != expected {
                return Err(invalid_log("non-monotonic decision sequence"));
            }
            if record.epoch < last_epoch {
                return Err(invalid_log("decreasing epoch"));
            }
            last_epoch = record.epoch;
            records.push(record);
            offset = offset
                .checked_add(record_end)
                .ok_or_else(|| invalid_log("record offset overflow"))?;
        }
        Ok(Self { records })
    }

    #[must_use]
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn last_kind(&self) -> Option<AuthorityRecordKind> {
        self.records.last().map(|record| record.kind)
    }

    #[must_use]
    pub fn contains_kind(&self, kind: AuthorityRecordKind) -> bool {
        self.records.iter().any(|record| record.kind == kind)
    }

    pub(crate) fn max_epoch(&self) -> u64 {
        self.records
            .iter()
            .map(|record| record.epoch)
            .max()
            .unwrap_or(0)
    }

    pub(crate) fn reconstruct_active_authority(
        &self,
    ) -> Result<Option<ReconstructedAuthority>, Error> {
        let mut active: Option<ReconstructedAuthority> = None;
        for record in &self.records {
            match record.kind {
                AuthorityRecordKind::Armed => {
                    if active.is_some() {
                        return Err(invalid_log("duplicate armed authority"));
                    }
                    let binding = record.to_bound_session()?;
                    let base = record
                        .base
                        .clone()
                        .ok_or_else(|| invalid_log("missing base"))?;
                    let armed_at = AuthorityRecord::time_field(
                        record.armed_at_secs,
                        record.armed_at_nanos,
                        "armed_at",
                    )?;
                    let deadline = AuthorityRecord::time_field(
                        record.deadline_secs,
                        record.deadline_nanos,
                        "deadline",
                    )?;
                    let lease_expires_at = AuthorityRecord::time_field(
                        record.lease_expires_at_secs,
                        record.lease_expires_at_nanos,
                        "lease_expires_at",
                    )?;
                    active = Some(ReconstructedAuthority {
                        binding,
                        base,
                        armed_at,
                        deadline,
                        lease_expires_at,
                        chosen: None,
                        log_seq: record.sequence,
                        last_activity: armed_at,
                    });
                }
                AuthorityRecordKind::LeaseRenewed => {
                    let state = active
                        .as_mut()
                        .ok_or_else(|| invalid_log("lease renewal without armed"))?;
                    if state.chosen.is_some() {
                        return Err(invalid_log("lease renewal after decision"));
                    }
                    let binding = record.to_bound_session()?;
                    if binding != state.binding {
                        return Err(invalid_log("lease renewal binding mismatch"));
                    }
                    let renewed_at = AuthorityRecord::time_field(
                        record.renewed_at_secs,
                        record.renewed_at_nanos,
                        "renewed_at",
                    )?;
                    let lease_expires_at = AuthorityRecord::time_field(
                        record.lease_expires_at_secs,
                        record.lease_expires_at_nanos,
                        "lease_expires_at",
                    )?;
                    if renewed_at < state.last_activity {
                        return Err(invalid_log("renewal before last activity"));
                    }
                    if renewed_at >= state.lease_expires_at {
                        return Err(invalid_log("renewal after prior expiry"));
                    }
                    if renewed_at >= state.deadline {
                        return Err(invalid_log("renewal at or past hard deadline"));
                    }
                    let expected = lease_expiry_at(renewed_at, state.deadline)?;
                    if lease_expires_at != expected {
                        return Err(invalid_log("lease renewal policy mismatch"));
                    }
                    if lease_expires_at <= state.lease_expires_at {
                        return Err(invalid_log("lease renewal did not extend expiry"));
                    }
                    state.lease_expires_at = lease_expires_at;
                    state.last_activity = renewed_at;
                    state.log_seq = record.sequence;
                }
                AuthorityRecordKind::BeginCommit | AuthorityRecordKind::BeginRevert => {
                    let state = active
                        .as_mut()
                        .ok_or_else(|| invalid_log("decision without armed"))?;
                    if state.chosen.is_some() {
                        return Err(invalid_log("duplicate decision authority"));
                    }
                    let binding = record.to_bound_session()?;
                    if binding != state.binding {
                        return Err(invalid_log("decision binding mismatch"));
                    }
                    state.chosen = Some(record.kind);
                    state.log_seq = record.sequence;
                }
                AuthorityRecordKind::ProbationPassed
                | AuthorityRecordKind::Committed
                | AuthorityRecordKind::Reverted => {}
            }
        }
        Ok(active)
    }
}

pub(crate) fn append_record(
    path: &Path,
    record: &AuthorityRecord,
    durability: &dyn Durability,
) -> Result<(), DurabilityError> {
    record
        .validate_schema()
        .map_err(|error| DurabilityError::Io(error.to_string()))?;
    let payload =
        serde_json::to_vec(record).map_err(|error| DurabilityError::Io(error.to_string()))?;
    let length = u32::try_from(payload.len())
        .map_err(|_| DurabilityError::Io("decision record too large".to_owned()))?;
    let mut frame = Vec::with_capacity(HEADER_BYTES.saturating_add(payload.len()));
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&crc32(&payload).to_be_bytes());
    frame.extend_from_slice(&payload);

    let parent = path
        .parent()
        .ok_or_else(|| DurabilityError::Io("decision log has no parent".to_owned()))?;
    std::fs::create_dir_all(parent).map_err(io_durability)?;

    let mut file = open_append_no_follow(path).map_err(io_durability)?;
    file.write_all(&frame).map_err(io_durability)?;
    durability.file_fsync(path)?;
    durability.dir_fsync(parent)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn armed_record(
    sequence: u64,
    epoch: u64,
    host_id: &str,
    tx_id: &str,
    base: &str,
    lease_id: &str,
    worker_group_tag: WorkerGroupTag,
    armed_at: SystemTime,
    deadline: SystemTime,
) -> Result<AuthorityRecord, Error> {
    let lease_expires_at = lease_expiry_at(armed_at, deadline)?;
    let (armed_at_secs, armed_at_nanos) = time_parts(armed_at)?;
    let (deadline_secs, deadline_nanos) = time_parts(deadline)?;
    let (lease_expires_at_secs, lease_expires_at_nanos) = time_parts(lease_expires_at)?;
    Ok(AuthorityRecord {
        sequence,
        epoch,
        kind: AuthorityRecordKind::Armed,
        host_id: Some(host_id.to_owned()),
        tx_id: Some(tx_id.to_owned()),
        base: Some(base.to_owned()),
        lease_id: Some(lease_id.to_owned()),
        worker_group_tag: Some(worker_group_tag),
        armed_at_secs: Some(armed_at_secs),
        armed_at_nanos: Some(armed_at_nanos),
        deadline_secs: Some(deadline_secs),
        deadline_nanos: Some(deadline_nanos),
        lease_expires_at_secs: Some(lease_expires_at_secs),
        lease_expires_at_nanos: Some(lease_expires_at_nanos),
        renewed_at_secs: None,
        renewed_at_nanos: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lease_renewed_record(
    sequence: u64,
    epoch: u64,
    host_id: &str,
    tx_id: &str,
    lease_id: &str,
    worker_group_tag: WorkerGroupTag,
    renewed_at: SystemTime,
    hard_deadline: SystemTime,
) -> Result<AuthorityRecord, Error> {
    let lease_expires_at = lease_expiry_at(renewed_at, hard_deadline)?;
    let (renewed_at_secs, renewed_at_nanos) = time_parts(renewed_at)?;
    let (lease_expires_at_secs, lease_expires_at_nanos) = time_parts(lease_expires_at)?;
    Ok(AuthorityRecord {
        sequence,
        epoch,
        kind: AuthorityRecordKind::LeaseRenewed,
        host_id: Some(host_id.to_owned()),
        tx_id: Some(tx_id.to_owned()),
        base: None,
        lease_id: Some(lease_id.to_owned()),
        worker_group_tag: Some(worker_group_tag),
        armed_at_secs: None,
        armed_at_nanos: None,
        deadline_secs: None,
        deadline_nanos: None,
        lease_expires_at_secs: Some(lease_expires_at_secs),
        lease_expires_at_nanos: Some(lease_expires_at_nanos),
        renewed_at_secs: Some(renewed_at_secs),
        renewed_at_nanos: Some(renewed_at_nanos),
    })
}

pub(crate) fn decision_record(
    sequence: u64,
    epoch: u64,
    kind: AuthorityRecordKind,
    host_id: &str,
    tx_id: &str,
    lease_id: &str,
    worker_group_tag: WorkerGroupTag,
) -> AuthorityRecord {
    AuthorityRecord {
        sequence,
        epoch,
        kind,
        host_id: Some(host_id.to_owned()),
        tx_id: Some(tx_id.to_owned()),
        base: None,
        lease_id: Some(lease_id.to_owned()),
        worker_group_tag: Some(worker_group_tag),
        armed_at_secs: None,
        armed_at_nanos: None,
        deadline_secs: None,
        deadline_nanos: None,
        lease_expires_at_secs: None,
        lease_expires_at_nanos: None,
        renewed_at_secs: None,
        renewed_at_nanos: None,
    }
}

pub(crate) fn time_parts(time: SystemTime) -> Result<(u64, u32), Error> {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid_log("pre-unix time"))?;
    Ok((duration.as_secs(), duration.subsec_nanos()))
}

#[cfg(unix)]
fn open_append_no_follow(path: &Path) -> Result<File, Error> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_append_no_follow(path: &Path) -> Result<File, Error> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| invalid_log("header offset overflow"))?;
    let array: [u8; 4] = bytes
        .get(offset..end)
        .ok_or_else(|| invalid_log("truncated record header"))?
        .try_into()
        .map_err(|_| invalid_log("invalid record header"))?;
    Ok(u32::from_be_bytes(array))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & mask);
        }
    }
    !crc
}

fn invalid_log(message: &str) -> Error {
    Error::new(ErrorKind::InvalidData, message)
}

#[allow(clippy::needless_pass_by_value)]
fn io_durability(error: Error) -> DurabilityError {
    DurabilityError::Io(error.to_string())
}
