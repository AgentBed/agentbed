//! Durable safe-mode helpers.

use crate::error::{DurabilityError, RpcError};
use crate::interfaces::Durability;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SAFE_MODE_PAYLOAD: &[u8] = br#"{"safe_mode":true}"#;
pub const LEGACY_EPOCH_TEMP: &str = ".tmp-epoch";
pub const EPOCH_TEMP_PREFIX: &str = ".tmp-epoch";
pub const SAFE_MODE_TEMP_PREFIX: &str = ".tmp-safe-mode";

pub fn persist_safe_mode_marker(
    store_root: &Path,
    durability: &dyn Durability,
) -> Result<(), RpcError> {
    let marker = store_root.join(crate::core::SAFE_MODE_REL);
    let parent = marker.parent().ok_or(RpcError::SafeModeActive)?;
    fs::create_dir_all(parent).map_err(|_| RpcError::SafeModeActive)?;
    if ambiguous_temp_residue(parent, SAFE_MODE_TEMP_PREFIX) {
        return Err(RpcError::SafeModeActive);
    }
    let tmp = unique_temp_path(parent, "safe-mode");
    write_exclusive(&tmp, SAFE_MODE_PAYLOAD).map_err(|_| RpcError::SafeModeActive)?;
    durability.file_fsync(&tmp).map_err(RpcError::Durability)?;
    durable_atomic_rename(durability, &tmp, &marker).map_err(|_| RpcError::SafeModeActive)?;
    durability.dir_fsync(parent).map_err(RpcError::Durability)?;
    durability
        .readback_verify(&marker, SAFE_MODE_PAYLOAD)
        .map_err(RpcError::Durability)?;
    Ok(())
}

pub fn durable_atomic_rename(
    durability: &dyn Durability,
    from: &Path,
    to: &Path,
) -> Result<(), DurabilityError> {
    durability.atomic_rename(from, to)?;
    if from.exists() {
        fs::rename(from, to).map_err(|error| DurabilityError::Io(error.to_string()))?;
    }
    Ok(())
}

#[must_use]
pub fn unique_temp_path(parent_dir: &Path, label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    parent_dir.join(format!(".tmp-{label}-{nanos}"))
}

pub fn ambiguous_epoch_temp_residue(parent: &Path) -> bool {
    ambiguous_temp_residue(parent, EPOCH_TEMP_PREFIX)
}

pub fn ambiguous_safe_mode_temp_residue(parent: &Path) -> bool {
    ambiguous_temp_residue(parent, SAFE_MODE_TEMP_PREFIX)
}

fn ambiguous_temp_residue(parent: &Path, prefix: &str) -> bool {
    let Ok(entries) = fs::read_dir(parent) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(prefix))
    })
}

#[cfg(unix)]
fn write_exclusive(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_exclusive(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(path, bytes)
}
