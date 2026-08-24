//! Injectable durability operations for WAL and event persistence.

#![allow(missing_debug_implementations)]

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Durability failure surfaced to callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurabilityError {
    Io,
    FsyncFailed,
    RenameFailed,
}

impl From<io::Error> for DurabilityError {
    fn from(_value: io::Error) -> Self {
        Self::Io
    }
}

/// Filesystem durability contract used by WAL/event stores.
pub trait DurabilityOps: Send + Sync {
    fn write_all_and_sync(&self, path: &Path, bytes: &[u8]) -> Result<(), DurabilityError>;
    fn sync_file(&self, path: &Path) -> Result<(), DurabilityError>;
    fn sync_parent(&self, path: &Path) -> Result<(), DurabilityError>;
    fn atomic_rename(&self, from: &Path, to: &Path) -> Result<(), DurabilityError>;
}

/// Production durability using the host filesystem.
#[derive(Debug, Default)]
pub struct RealDurability;

impl DurabilityOps for RealDurability {
    fn write_all_and_sync(&self, path: &Path, bytes: &[u8]) -> Result<(), DurabilityError> {
        std::fs::write(path, bytes).map_err(|_| DurabilityError::Io)?;
        self.sync_file(path)
    }

    fn sync_file(&self, path: &Path) -> Result<(), DurabilityError> {
        use std::fs::OpenOptions;
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|_| DurabilityError::Io)?;
        file.sync_all().map_err(|_| DurabilityError::Io)?;
        Ok(())
    }

    fn sync_parent(&self, path: &Path) -> Result<(), DurabilityError> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        let file = std::fs::File::open(parent).map_err(|_| DurabilityError::Io)?;
        file.sync_all().map_err(|_| DurabilityError::Io)?;
        Ok(())
    }

    fn atomic_rename(&self, from: &Path, to: &Path) -> Result<(), DurabilityError> {
        std::fs::rename(from, to).map_err(|_| DurabilityError::Io)?;
        self.sync_parent(to)
    }
}

/// Test double that can fail at fsync boundaries.
pub struct FaultInjectedDurability {
    inner: Arc<dyn DurabilityOps>,
    fail_after_write_before_fsync: AtomicBool,
}

impl FaultInjectedDurability {
    #[must_use]
    pub fn new(inner: Arc<dyn DurabilityOps>) -> Self {
        Self {
            inner,
            fail_after_write_before_fsync: AtomicBool::new(false),
        }
    }

    pub fn fail_after_write_before_fsync(&self, enabled: bool) {
        self.fail_after_write_before_fsync
            .store(enabled, Ordering::SeqCst);
    }
}

impl DurabilityOps for FaultInjectedDurability {
    fn write_all_and_sync(&self, path: &Path, bytes: &[u8]) -> Result<(), DurabilityError> {
        std::fs::write(path, bytes).map_err(|_| DurabilityError::Io)?;
        if self.fail_after_write_before_fsync.load(Ordering::SeqCst) {
            return Err(DurabilityError::FsyncFailed);
        }
        self.inner.sync_file(path)
    }

    fn sync_file(&self, path: &Path) -> Result<(), DurabilityError> {
        if self.fail_after_write_before_fsync.load(Ordering::SeqCst) {
            return Err(DurabilityError::FsyncFailed);
        }
        self.inner.sync_file(path)
    }

    fn sync_parent(&self, path: &Path) -> Result<(), DurabilityError> {
        self.inner.sync_parent(path)
    }

    fn atomic_rename(&self, from: &Path, to: &Path) -> Result<(), DurabilityError> {
        self.inner.atomic_rename(from, to)
    }
}
