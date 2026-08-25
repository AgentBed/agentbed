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

    let semantic = normalize_nix_content(&change.content);
    if content_selects_watchdog(&semantic) {
        return Err(ProtectedRejectReason::Watchdog);
    }
    if content_selects_kernel(&semantic) {
        return Err(ProtectedRejectReason::Kernel);
    }
    if content_selects_bootloader(&semantic) {
        return Err(ProtectedRejectReason::Bootloader);
    }
    if content_selects_firewall(&semantic) {
        return Err(ProtectedRejectReason::Firewall);
    }
    if content_selects_storage_layout(&semantic) {
        return Err(ProtectedRejectReason::StorageLayout);
    }

    let _ = normalized;
    Ok(())
}

fn normalize_nix_content(content: &str) -> String {
    content
        .to_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

fn content_selects_watchdog(content: &str) -> bool {
    content.contains("agentbed-watchdogd")
        || content.contains("watchdogd.package")
        || content.contains("watchdogd.enable")
        || content.contains("services.agentbed-watchdogd")
        || content.contains("systemd.services.agentbed-watchdogd")
}

fn content_selects_kernel(content: &str) -> bool {
    content.contains("boot.kernelpackages")
        || (content.contains("kernelpackages")
            && (content.contains("boot={") || content.contains("boot=")))
}

fn content_selects_bootloader(content: &str) -> bool {
    content.contains("boot.loader")
        || content.contains("systemd-boot")
        || (content.contains("boot={") && content.contains("loader"))
        || (content.contains("boot=") && content.contains("loader="))
}

fn content_selects_firewall(content: &str) -> bool {
    content.contains("networking.firewall")
        || (content.contains("networking={") && content.contains("firewall"))
}

fn content_selects_storage_layout(content: &str) -> bool {
    content.contains("filesystems.")
        || content.contains("filesystems=")
        || content.contains("filesystems={")
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
