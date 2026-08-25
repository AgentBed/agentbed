//! Strict reader for the watchdog-owned append-only authority log.

use crate::error::DurabilityError;
use crate::interfaces::Durability;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Error, ErrorKind, Read, Write};
use std::path::Path;

const HEADER_BYTES: usize = 8;
const MAX_RECORD_BYTES: usize = 64 * 1024;

/// Watchdog-owned durable authority record kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorityRecordKind {
    Armed,
    ProbationPassed,
    BeginCommit,
    BeginRevert,
    Committed,
    Reverted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityRecord {
    sequence: u64,
    epoch: u64,
    kind: AuthorityRecordKind,
}

/// Immutable in-memory projection of a validated decision log.
#[derive(Debug)]
pub struct DecisionLogReader {
    records: Vec<AuthorityRecord>,
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
}

pub(crate) fn append_record(
    path: &Path,
    sequence: u64,
    epoch: u64,
    kind: AuthorityRecordKind,
    durability: &dyn Durability,
) -> Result<(), DurabilityError> {
    let payload = serde_json::to_vec(&AuthorityRecord {
        sequence,
        epoch,
        kind,
    })
    .map_err(|error| DurabilityError::Io(error.to_string()))?;
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
