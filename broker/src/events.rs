//! Durable `agentbed://events` append log with cursor replay.

#![allow(clippy::expect_used, missing_debug_implementations)]

use crate::storage::durability::{DurabilityError, DurabilityOps, RealDurability};
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One append-only event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    pub kind: String,
    pub payload: String,
}

/// Stored event with monotonic sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub seq: u64,
    pub kind: String,
    pub payload: String,
}

/// Client-held replay cursor (opaque JSON string on the wire).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorBody {
    log_id: String,
    seq: u64,
}

/// Client-held replay cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventCursor {
    log_id: String,
    seq: u64,
}

impl EventCursor {
    #[must_use]
    pub fn after(event: &StoredEvent) -> Self {
        Self {
            log_id: String::new(),
            seq: event.seq,
        }
    }

    #[must_use]
    pub fn after_seq(log_id: impl Into<String>, seq: u64) -> Self {
        Self {
            log_id: log_id.into(),
            seq,
        }
    }

    #[must_use]
    pub fn foreign(log_id: impl Into<String>, seq: u64) -> Self {
        Self {
            log_id: log_id.into(),
            seq,
        }
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let body = CursorBody {
            log_id: self.log_id.clone(),
            seq: self.seq,
        };
        serde_json::to_string(&body).unwrap_or_default()
    }

    pub fn parse(encoded: &str) -> Result<Self, EventError> {
        let body: CursorBody =
            serde_json::from_str(encoded).map_err(|_| EventError::MalformedCursor)?;
        Ok(Self {
            log_id: body.log_id,
            seq: body.seq,
        })
    }

    #[must_use]
    pub fn with_log_id(mut self, log_id: String) -> Self {
        self.log_id = log_id;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventError {
    MalformedCursor,
    ForeignLog,
    BeyondTail,
    Io(DurabilityError),
}

/// Append-only durable event log.
pub struct EventLog {
    root: PathBuf,
    durability: Arc<dyn DurabilityOps>,
    meta: Mutex<EventMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EventMeta {
    log_id: String,
    next_seq: u64,
}

impl EventLog {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, EventError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root).map_err(|_| EventError::Io(DurabilityError::Io))?;
        let meta_path = root.join("meta.json");
        let meta = if meta_path.exists() {
            let text = std::fs::read_to_string(&meta_path)
                .map_err(|_| EventError::Io(DurabilityError::Io))?;
            serde_json::from_str(&text).map_err(|_| EventError::MalformedCursor)?
        } else {
            EventMeta {
                log_id: format!("log-{}", std::process::id()),
                next_seq: 1,
            }
        };
        Ok(Self {
            root,
            durability: Arc::new(RealDurability),
            meta: Mutex::new(meta),
        })
    }

    /// Validate log tail integrity against persisted metadata.
    pub fn validate_integrity(&self) -> Result<(), EventError> {
        let events = self.read_all()?;
        let meta = self.meta.lock().expect("meta lock");
        let expected_next = events.last().map_or(1, |event| event.seq.saturating_add(1));
        if meta.next_seq != expected_next {
            return Err(EventError::MalformedCursor);
        }
        for (idx, event) in events.iter().enumerate() {
            let expected = u64::try_from(idx).unwrap_or(u64::MAX).saturating_add(1);
            if event.seq != expected {
                return Err(EventError::MalformedCursor);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn log_id(&self) -> String {
        self.meta.lock().expect("meta lock").log_id.clone()
    }

    pub fn append(&self, record: EventRecord) -> Result<StoredEvent, EventError> {
        let mut meta = self.meta.lock().expect("meta lock");
        let stored = StoredEvent {
            seq: meta.next_seq,
            kind: record.kind,
            payload: record.payload,
        };
        meta.next_seq = meta.next_seq.saturating_add(1);
        let line = serde_json::to_string(&stored).map_err(|_| EventError::MalformedCursor)?;
        let log_path = self.root.join("log.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|_| EventError::Io(DurabilityError::Io))?;
        writeln!(file, "{line}").map_err(|_| EventError::Io(DurabilityError::Io))?;
        self.durability
            .sync_file(&log_path)
            .map_err(EventError::Io)?;
        let meta_path = self.root.join("meta.json");
        let meta_bytes = serde_json::to_vec(&*meta).map_err(|_| EventError::MalformedCursor)?;
        let temp = self.root.join("meta.json.tmp");
        self.durability
            .write_all_and_sync(&temp, &meta_bytes)
            .map_err(EventError::Io)?;
        self.durability
            .atomic_rename(&temp, &meta_path)
            .map_err(EventError::Io)?;
        Ok(stored)
    }

    pub fn replay(&self, cursor: &EventCursor) -> Result<Vec<StoredEvent>, EventError> {
        let meta = self.meta.lock().expect("meta lock");
        let effective_log_id = if cursor.log_id.is_empty() {
            meta.log_id.clone()
        } else {
            cursor.log_id.clone()
        };
        if effective_log_id != meta.log_id {
            return Err(EventError::ForeignLog);
        }
        let events = self.read_all()?;
        let max_seq = meta.next_seq.saturating_sub(1);
        if cursor.seq > max_seq && max_seq > 0 {
            return Err(EventError::BeyondTail);
        }
        Ok(events.into_iter().filter(|e| e.seq > cursor.seq).collect())
    }

    pub fn latest_cursor(&self) -> Result<EventCursor, EventError> {
        let meta = self.meta.lock().expect("meta lock");
        let seq = meta.next_seq.saturating_sub(1);
        Ok(EventCursor {
            log_id: meta.log_id.clone(),
            seq,
        })
    }

    pub fn cursor_after(&self, event: &StoredEvent) -> EventCursor {
        EventCursor::after(event).with_log_id(self.log_id())
    }

    fn read_all(&self) -> Result<Vec<StoredEvent>, EventError> {
        let log_path = self.root.join("log.jsonl");
        if !log_path.exists() {
            return Ok(Vec::new());
        }
        let text =
            std::fs::read_to_string(&log_path).map_err(|_| EventError::Io(DurabilityError::Io))?;
        let mut out = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let event: StoredEvent =
                serde_json::from_str(line).map_err(|_| EventError::MalformedCursor)?;
            out.push(event);
        }
        Ok(out)
    }
}
