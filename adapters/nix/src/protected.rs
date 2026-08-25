//! Semantic class-F protected-path rejection before staging.

use agentbed_protocol::wire::ConfigFileChange;
use std::path::{Component, Path, PathBuf};

/// Why a proposed change was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedRejectReason {
    Watchdog,
    BrokerWal,
    RollbackPath,
    OobStore,
    SelfProtection,
    Kernel,
    Bootloader,
    Firewall,
    StorageLayout,
}

/// Reject any change that would touch a protected resource.
pub fn check_protected_changes(changes: &[ConfigFileChange]) -> Result<(), ProtectedRejectReason> {
    for change in changes {
        check_one(change)?;
    }
    Ok(())
}

fn check_one(change: &ConfigFileChange) -> Result<(), ProtectedRejectReason> {
    let normalized = normalize_path(&change.path);
    let lower = normalized.to_lowercase();

    if lower.contains("watchdogd") {
        return Err(ProtectedRejectReason::Watchdog);
    }
    if lower.contains("/var/lib/agentbed/wal/") {
        return Err(ProtectedRejectReason::BrokerWal);
    }
    if lower.contains("/var/lib/agentbed/rollback/") {
        return Err(ProtectedRejectReason::RollbackPath);
    }
    if lower.contains("/var/lib/agentbed/oob/") {
        return Err(ProtectedRejectReason::OobStore);
    }
    if lower.contains("/etc/nixos/agentbed/") {
        return Err(ProtectedRejectReason::SelfProtection);
    }

    let content = change.content.to_lowercase();
    if content.contains("boot.kernelpackages") {
        return Err(ProtectedRejectReason::Kernel);
    }
    if content.contains("boot.loader") {
        return Err(ProtectedRejectReason::Bootloader);
    }
    if content.contains("networking.firewall") {
        return Err(ProtectedRejectReason::Firewall);
    }
    if content.contains("filesystems") && content.contains("file") {
        return Err(ProtectedRejectReason::StorageLayout);
    }

    let _ = normalized;
    Ok(())
}

fn normalize_path(path: &str) -> String {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => {
                out.push(other.as_os_str());
            }
        }
    }
    out.to_string_lossy().into_owned()
}
