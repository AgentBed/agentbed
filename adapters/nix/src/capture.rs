//! Immutable candidate/base capture store.

use crate::propose::CapturedProposal;
use agentbed_protocol::wire::ConfigFileChange;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Errors from capture persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    Conflict,
    Io,
}

/// Durable capture store for crash recovery.
#[derive(Debug)]
pub struct CaptureStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl CaptureStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            lock: Mutex::new(()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_durably_persisted(&self) -> bool {
        self.active_path().with_extension("json.synced").exists()
    }

    pub fn load_active(&self) -> Result<Option<StoredCapture>, CaptureError> {
        let path = self.active_path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(path).map_err(|_| CaptureError::Io)?;
        serde_json::from_str(&raw)
            .map_err(|_| CaptureError::Io)
            .map(Some)
    }

    pub fn store_active(
        &self,
        capture: &CapturedProposal,
        changes: &[ConfigFileChange],
    ) -> Result<(), CaptureError> {
        let _guard = self.lock.lock().expect("capture");
        fs::create_dir_all(&self.root).map_err(|_| CaptureError::Io)?;
        if self.active_path().exists() {
            return Err(CaptureError::Conflict);
        }
        let stored = StoredCapture {
            capture: capture.clone(),
            fingerprint: changes_fingerprint(changes),
        };
        let raw = serde_json::to_string(&stored).map_err(|_| CaptureError::Io)?;
        let tmp = self.root.join("active.json.tmp");
        let final_path = self.active_path();
        fs::write(&tmp, raw).map_err(|_| CaptureError::Io)?;
        sync_path(&tmp)?;
        fs::rename(&tmp, &final_path).map_err(|_| CaptureError::Io)?;
        sync_path(&final_path)?;
        sync_parent(&final_path)?;
        let marker = final_path.with_extension("json.synced");
        let mut marker_file = File::create(&marker).map_err(|_| CaptureError::Io)?;
        marker_file
            .write_all(b"synced\n")
            .map_err(|_| CaptureError::Io)?;
        sync_path(&marker)?;
        sync_parent(&marker)?;
        Ok(())
    }

    fn active_path(&self) -> PathBuf {
        self.root.join("active.json")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCapture {
    pub capture: CapturedProposal,
    pub fingerprint: String,
}

pub(crate) fn changes_fingerprint(changes: &[ConfigFileChange]) -> String {
    serde_json::to_string(changes).unwrap_or_default()
}

fn sync_path(path: &Path) -> Result<(), CaptureError> {
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|_| CaptureError::Io)?;
    file.sync_all().map_err(|_| CaptureError::Io)
}

fn sync_parent(path: &Path) -> Result<(), CaptureError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let dir = File::open(parent).map_err(|_| CaptureError::Io)?;
    dir.sync_all().map_err(|_| CaptureError::Io)
}
