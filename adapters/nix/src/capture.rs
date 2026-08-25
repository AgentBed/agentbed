//! Immutable candidate/base capture store.

use crate::propose::CapturedProposal;
use agentbed_protocol::wire::ConfigFileChange;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Errors from capture persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    Conflict,
    Io,
}

/// Durable filesystem sync boundary for crash-safety tests and production.
pub trait PathSync: Send + Sync + std::fmt::Debug {
    fn sync_path(&self, path: &Path) -> Result<(), CaptureError>;
    fn sync_parent(&self, path: &Path) -> Result<(), CaptureError>;
    fn sync_dir(&self, path: &Path) -> Result<(), CaptureError>;
}

/// Production fsync implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdPathSync;

impl PathSync for StdPathSync {
    fn sync_path(&self, path: &Path) -> Result<(), CaptureError> {
        sync_path_impl(path)
    }

    fn sync_parent(&self, path: &Path) -> Result<(), CaptureError> {
        sync_parent_impl(path)
    }

    fn sync_dir(&self, path: &Path) -> Result<(), CaptureError> {
        let dir = File::open(path).map_err(|_| CaptureError::Io)?;
        dir.sync_all().map_err(|_| CaptureError::Io)
    }
}

/// Test hook that fails sync on selected paths.
#[derive(Debug)]
pub struct FailSyncOn {
    fail_paths: Vec<PathBuf>,
    inner: StdPathSync,
}

impl FailSyncOn {
    pub fn new(fail_paths: Vec<PathBuf>, inner: StdPathSync) -> Self {
        Self { fail_paths, inner }
    }

    fn should_fail(&self, path: &Path) -> bool {
        self.fail_paths
            .iter()
            .any(|candidate| candidate == path || path.starts_with(candidate))
    }
}

impl PathSync for FailSyncOn {
    fn sync_path(&self, path: &Path) -> Result<(), CaptureError> {
        if self.should_fail(path) {
            return Err(CaptureError::Io);
        }
        self.inner.sync_path(path)
    }

    fn sync_parent(&self, path: &Path) -> Result<(), CaptureError> {
        if let Some(parent) = path.parent() {
            if self.should_fail(parent) {
                return Err(CaptureError::Io);
            }
        }
        self.inner.sync_parent(path)
    }

    fn sync_dir(&self, path: &Path) -> Result<(), CaptureError> {
        if self.should_fail(path) {
            return Err(CaptureError::Io);
        }
        self.inner.sync_dir(path)
    }
}

/// Durable capture store for crash recovery.
#[derive(Debug)]
pub struct CaptureStore {
    root: PathBuf,
    lock: Mutex<()>,
    syncer: Arc<dyn PathSync>,
}

impl CaptureStore {
    pub fn new(root: PathBuf) -> Self {
        Self::with_syncer(root, Arc::new(StdPathSync))
    }

    pub fn with_syncer(root: PathBuf, syncer: Arc<dyn PathSync>) -> Self {
        Self {
            root,
            lock: Mutex::new(()),
            syncer,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_durably_persisted(&self) -> bool {
        self.active_path().with_extension("json.synced").exists()
    }

    pub fn activation_record_path(&self, candidate_closure: &str) -> PathBuf {
        self.activations_dir()
            .join(activation_filename(candidate_closure))
    }

    pub fn is_activation_recorded(&self, candidate_closure: &str) -> bool {
        self.activation_record_path(candidate_closure).exists()
    }

    pub fn reserve_activation(&self, candidate_closure: &str) -> Result<(), CaptureError> {
        let _guard = self.lock.lock().expect("capture");
        let activations = self.activations_dir();
        fs::create_dir_all(&activations).map_err(|_| CaptureError::Io)?;
        self.syncer.sync_dir(&self.root)?;
        self.syncer.sync_dir(&activations)?;
        if self.activation_record_path(candidate_closure).exists() {
            return Err(CaptureError::Conflict);
        }
        let lock = self.activation_lock_path(candidate_closure);
        if lock.exists() {
            return Err(CaptureError::Conflict);
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|_| CaptureError::Conflict)?;
        file.write_all(b"activating\n")
            .map_err(|_| CaptureError::Io)?;
        self.syncer.sync_path(&lock)?;
        self.syncer.sync_parent(&lock)?;
        Ok(())
    }

    pub fn release_activation_reservation(&self, candidate_closure: &str) {
        let _guard = self.lock.lock().expect("capture");
        let lock = self.activation_lock_path(candidate_closure);
        let _ = fs::remove_file(lock);
    }

    pub fn finalize_activation(&self, candidate_closure: &str) -> Result<(), CaptureError> {
        let _guard = self.lock.lock().expect("capture");
        let activated = self.activation_record_path(candidate_closure);
        if activated.exists() {
            return Ok(());
        }
        let lock = self.activation_lock_path(candidate_closure);
        if lock.exists() {
            fs::rename(&lock, &activated).map_err(|_| CaptureError::Io)?;
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&activated)
                .map_err(|_| CaptureError::Conflict)?;
        }
        self.syncer.sync_path(&activated)?;
        self.syncer.sync_parent(&activated)?;
        Ok(())
    }

    pub fn store_pin(
        &self,
        capture: &CapturedProposal,
        realised: &str,
    ) -> Result<(), CaptureError> {
        let _guard = self.lock.lock().expect("capture");
        fs::create_dir_all(&self.root).map_err(|_| CaptureError::Io)?;
        self.syncer.sync_dir(&self.root)?;
        let record = PinRecord {
            candidate_closure: capture.candidate_closure.clone(),
            base_generation: capture.base_revision.generation.clone(),
            base_git_commit: capture.base_revision.etc_git_commit.clone(),
            base_config_digest: digest_hex(&capture.base_revision.config_digest),
            realised_closure: realised.to_owned(),
        };
        let raw = serde_json::to_string(&record).map_err(|_| CaptureError::Io)?;
        let tmp = self.pin_path().with_extension("json.tmp");
        fs::write(&tmp, raw).map_err(|_| CaptureError::Io)?;
        self.syncer.sync_path(&tmp)?;
        fs::rename(&tmp, self.pin_path()).map_err(|_| CaptureError::Io)?;
        let pin_path = self.pin_path();
        self.syncer.sync_path(&pin_path)?;
        self.syncer.sync_parent(&pin_path)?;
        let marker = self.pin_path().with_extension("json.synced");
        let mut marker_file = File::create(&marker).map_err(|_| CaptureError::Io)?;
        marker_file
            .write_all(b"synced\n")
            .map_err(|_| CaptureError::Io)?;
        self.syncer.sync_path(&marker)?;
        self.syncer.sync_parent(&marker)?;
        Ok(())
    }

    pub fn load_verified_pin(&self, capture: &CapturedProposal) -> Result<String, CaptureError> {
        let path = self.pin_path();
        if !path.exists() {
            return Err(CaptureError::Conflict);
        }
        let raw = fs::read_to_string(path).map_err(|_| CaptureError::Io)?;
        let record: PinRecord = serde_json::from_str(&raw).map_err(|_| CaptureError::Io)?;
        if !record.matches_capture(capture) {
            return Err(CaptureError::Conflict);
        }
        if !self.pin_path().with_extension("json.synced").exists() {
            return Err(CaptureError::Conflict);
        }
        Ok(record.realised_closure)
    }

    fn activations_dir(&self) -> PathBuf {
        self.root.join("activations")
    }

    fn activation_lock_path(&self, candidate_closure: &str) -> PathBuf {
        self.activations_dir()
            .join(format!("{}.lock", activation_filename(candidate_closure)))
    }

    fn pin_path(&self) -> PathBuf {
        self.root.join("pin.json")
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
        self.syncer.sync_dir(&self.root)?;
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
        self.syncer.sync_path(&tmp)?;
        fs::rename(&tmp, &final_path).map_err(|_| CaptureError::Io)?;
        self.syncer.sync_path(&final_path)?;
        self.syncer.sync_parent(&final_path)?;
        let marker = final_path.with_extension("json.synced");
        let mut marker_file = File::create(&marker).map_err(|_| CaptureError::Io)?;
        marker_file
            .write_all(b"synced\n")
            .map_err(|_| CaptureError::Io)?;
        self.syncer.sync_path(&marker)?;
        self.syncer.sync_parent(&marker)?;
        Ok(())
    }

    fn active_path(&self) -> PathBuf {
        self.root.join("active.json")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinRecord {
    pub candidate_closure: String,
    pub base_generation: Option<String>,
    pub base_git_commit: String,
    pub base_config_digest: String,
    pub realised_closure: String,
}

impl PinRecord {
    fn matches_capture(&self, capture: &CapturedProposal) -> bool {
        self.candidate_closure == capture.candidate_closure
            && self.base_generation == capture.base_revision.generation
            && self.base_git_commit == capture.base_revision.etc_git_commit
            && self.base_config_digest == digest_hex(&capture.base_revision.config_digest)
            && self.realised_closure == capture.candidate_closure
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

fn activation_filename(candidate_closure: &str) -> String {
    candidate_closure
        .trim_start_matches("/nix/store/")
        .replace('/', "_")
        + ".activated"
}

fn digest_hex(digest: &agentbed_protocol::digest::Digest) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(64);
    for byte in digest.as_bytes() {
        write!(out, "{byte:02x}").expect("hex");
    }
    out
}

fn sync_path_impl(path: &Path) -> Result<(), CaptureError> {
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|_| CaptureError::Io)?;
    file.sync_all().map_err(|_| CaptureError::Io)
}

fn sync_parent_impl(path: &Path) -> Result<(), CaptureError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let dir = File::open(parent).map_err(|_| CaptureError::Io)?;
    dir.sync_all().map_err(|_| CaptureError::Io)
}
