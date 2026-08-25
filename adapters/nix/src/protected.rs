//! Semantic class-F protected-path rejection before staging.

use agentbed_protocol::wire::ConfigFileChange;
use std::collections::HashMap;
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
    ConflictingChange,
}

/// Reject any change that would touch a protected resource.
pub fn check_protected_changes(changes: &[ConfigFileChange]) -> Result<(), ProtectedRejectReason> {
    let mut seen: HashMap<String, String> = HashMap::new();
    for change in changes {
        let normalized = normalize_path(&change.path);
        if let Some(existing) = seen.get(&normalized) {
            if existing != &change.content {
                return Err(ProtectedRejectReason::ConflictingChange);
            }
            continue;
        }
        seen.insert(normalized.clone(), change.content.clone());
        check_one(change, &normalized)?;
    }
    Ok(())
}

fn check_one(change: &ConfigFileChange, normalized: &str) -> Result<(), ProtectedRejectReason> {
    let lower = normalized.to_lowercase();

    if lower.contains("watchdogd")
        || lower.contains("agentbed-watchdogd")
        || lower.contains("/var/lib/agentbed/watchdog")
    {
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
    if content.contains("agentbed-watchdogd")
        || content.contains("watchdogd.package")
        || content.contains("watchdogd.enable")
        || content.contains("services.agentbed-watchdogd")
        || content.contains("systemd.services.agentbed-watchdogd")
    {
        return Err(ProtectedRejectReason::Watchdog);
    }
    if content.contains("boot.kernelpackages") {
        return Err(ProtectedRejectReason::Kernel);
    }
    if content.contains("boot.loader") {
        return Err(ProtectedRejectReason::Bootloader);
    }
    if content.contains("networking.firewall") {
        return Err(ProtectedRejectReason::Firewall);
    }
    if content.contains("filesystems.") || content.contains("filesystems =") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_identical_paths_are_allowed() {
        let changes = vec![
            ConfigFileChange {
                path: "/etc/nixos/demo.nix".to_owned(),
                content: "same".to_owned(),
            },
            ConfigFileChange {
                path: "/etc/nixos/demo.nix".to_owned(),
                content: "same".to_owned(),
            },
        ];
        check_protected_changes(&changes).expect("identical duplicate");
    }
}
