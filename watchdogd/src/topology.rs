//! Production startup topology verifier for the sealed H-04 watchdog store.

use crate::error::TopologyError;
use crate::interfaces::TopologyProbe;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Verifies the watchdog dedicated store path before arming or authority work.
#[derive(Debug, Default)]
pub struct ProductionTopologyProbe;

/// Protected domains whose mount identity and `st_dev` must differ from the store.
const PROTECTED_DOMAIN_PATHS: &[&str] = &[
    "/",
    "/nix",
    "/var/lib/agentbed/broker/state",
    "/var/lib/agentbed/rollback",
];

/// Candidate-closure domain under `/nix/store` (watchdog files must never alias into it).
const NIX_STORE_DOMAIN: &str = "/nix/store";

const STORE_DIR_MODE: u32 = 0o700;
const AUTHORITATIVE_FILE_MODE: u32 = 0o600;
const CONFIG_FILE_MODE: u32 = 0o400;
const RUNTIME_BINARY_MODE: u32 = 0o555;

const WATCHDOG_SUBDIRS: &[&str] = &["decisions", "epoch", "state", "config", "runtime"];

const WATCHDOG_AUTHORITATIVE_FILES: &[(&str, u32)] = &[
    ("decisions/decision.log", AUTHORITATIVE_FILE_MODE),
    ("epoch/high-water.json", AUTHORITATIVE_FILE_MODE),
    ("state/safe-mode.json", AUTHORITATIVE_FILE_MODE),
];

const WATCHDOG_OPTIONAL_FILES: &[(&str, u32)] = &[
    ("config/watchdog.json", CONFIG_FILE_MODE),
    ("runtime/agentbed-watchdogd", RUNTIME_BINARY_MODE),
];

impl ProductionTopologyProbe {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl TopologyProbe for ProductionTopologyProbe {
    fn verify_startup(&self, store_root: &Path) -> Result<(), TopologyError> {
        reject_lexical_traversal(store_root)?;
        reject_symlink_components(store_root)?;

        if !store_root.exists() {
            return Err(TopologyError::MissingMount);
        }

        let store_abs = absolute_lexical_path(store_root)?;
        let mountinfo = read_mount_table()?;

        let store_mount = find_exact_mount(&mountinfo, &store_abs).ok_or_else(|| {
            let meta = fs::symlink_metadata(store_root);
            match meta {
                Ok(m) if m.is_dir() => TopologyError::OrdinaryDirectoryFallback,
                Ok(_) => TopologyError::NonRegularComponent,
                Err(_) => TopologyError::UnavailableStore,
            }
        })?;

        let store_meta =
            fs::symlink_metadata(store_root).map_err(|_| TopologyError::UnavailableStore)?;
        if !store_meta.is_dir() {
            return Err(TopologyError::NonRegularComponent);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let store_mount_id = store_mount.mount_id;
            let store_dev = store_meta.dev();

            for domain in PROTECTED_DOMAIN_PATHS {
                let (domain_mount_id, domain_dev) = protected_domain_identity(&mountinfo, domain)?;
                if store_mount_id == domain_mount_id || store_dev == domain_dev {
                    return Err(TopologyError::SameDeviceAlias);
                }
            }

            // Reject alias into the `/nix/store` candidate-closure domain.
            if let Ok((nix_store_mount_id, nix_store_dev)) =
                protected_domain_identity(&mountinfo, NIX_STORE_DOMAIN)
            {
                if store_mount_id == nix_store_mount_id || store_dev == nix_store_dev {
                    return Err(TopologyError::SameDeviceAlias);
                }
            }

            check_root_owned_dir_mode(store_root, STORE_DIR_MODE)?;
            inspect_existing_layout(store_root)?;
            prove_writable_same_directory_atomic(store_root)?;
        }

        #[cfg(not(unix))]
        {
            let _ = store_mount;
            return Err(TopologyError::UnavailableStore);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MountEntry {
    mount_id: u32,
    major: u32,
    minor: u32,
    mount_point: String,
}

fn reject_lexical_traversal(path: &Path) -> Result<(), TopologyError> {
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(TopologyError::UnavailableStore);
        }
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<(), TopologyError> {
    let mut cumulative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => {
                cumulative.push("/");
            }
            Component::Prefix(prefix) => {
                cumulative.push(prefix.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(TopologyError::UnavailableStore),
            Component::Normal(name) => cumulative.push(name),
        }
        if cumulative.as_os_str().is_empty() {
            continue;
        }
        let meta =
            fs::symlink_metadata(&cumulative).map_err(|_| TopologyError::UnavailableStore)?;
        if meta.file_type().is_symlink() {
            return Err(TopologyError::SymlinkComponent);
        }
    }
    Ok(())
}

fn absolute_lexical_path(path: &Path) -> Result<PathBuf, TopologyError> {
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().map_err(|_| TopologyError::UnavailableStore)?
    };
    Ok(normalize_mount_path(&base.join(path)))
}

fn normalize_mount_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => {
                out.push("/");
            }
            Component::Prefix(prefix) => {
                out.push(prefix.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

fn read_mount_table() -> Result<Vec<MountEntry>, TopologyError> {
    let raw =
        fs::read_to_string("/proc/self/mountinfo").map_err(|_| TopologyError::UnavailableStore)?;
    parse_mountinfo(&raw).map_err(|_| TopologyError::UnavailableStore)
}

fn parse_mountinfo(raw: &str) -> Result<Vec<MountEntry>, TopologyError> {
    let mut entries = Vec::new();
    for line in raw.lines() {
        if line.is_empty() {
            continue;
        }
        entries.push(parse_mountinfo_line(line)?);
    }
    Ok(entries)
}

fn parse_mountinfo_line(line: &str) -> Result<MountEntry, TopologyError> {
    let (left, _right) = line
        .split_once(" - ")
        .ok_or(TopologyError::UnavailableStore)?;
    let mut fields = left.split_whitespace();
    let mount_id = fields
        .next()
        .ok_or(TopologyError::UnavailableStore)?
        .parse()
        .map_err(|_| TopologyError::UnavailableStore)?;
    fields.next().ok_or(TopologyError::UnavailableStore)?;
    let major_minor = fields.next().ok_or(TopologyError::UnavailableStore)?;
    let (major, minor) = parse_major_minor(major_minor)?;
    fields.next().ok_or(TopologyError::UnavailableStore)?;
    let mount_point_raw = fields.next().ok_or(TopologyError::UnavailableStore)?;
    let mount_point = unescape_mount_path(mount_point_raw);
    Ok(MountEntry {
        mount_id,
        major,
        minor,
        mount_point,
    })
}

fn parse_major_minor(token: &str) -> Result<(u32, u32), TopologyError> {
    let (major, minor) = token
        .split_once(':')
        .ok_or(TopologyError::UnavailableStore)?;
    let major = major.parse().map_err(|_| TopologyError::UnavailableStore)?;
    let minor = minor.parse().map_err(|_| TopologyError::UnavailableStore)?;
    Ok((major, minor))
}

fn unescape_mount_path(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut idx = 0;
    while let Some(&byte) = bytes.get(idx) {
        if byte == b'\\' {
            if let (Some(&h), Some(&t), Some(&o)) =
                (bytes.get(idx + 1), bytes.get(idx + 2), bytes.get(idx + 3))
            {
                if let (Some(h), Some(t), Some(o)) = (hex_digit(h), hex_digit(t), hex_digit(o)) {
                    let value = (h << 6) | (t << 3) | o;
                    out.push(char::from(value));
                    idx += 4;
                    continue;
                }
            }
        }
        out.push(char::from(byte));
        idx += 1;
    }
    out
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'7' => Some(byte - b'0'),
        _ => None,
    }
}

fn find_exact_mount<'a>(entries: &'a [MountEntry], path: &Path) -> Option<&'a MountEntry> {
    let normalized = normalize_mount_path(path);
    let normalized = normalized.to_string_lossy();
    entries.iter().find(|entry| entry.mount_point == normalized)
}

fn containing_mount<'a>(entries: &'a [MountEntry], path: &str) -> Option<&'a MountEntry> {
    entries
        .iter()
        .filter(|entry| mount_point_contains(&entry.mount_point, path))
        .max_by_key(|entry| entry.mount_point.len())
}

fn mount_point_contains(mount_point: &str, path: &str) -> bool {
    if mount_point == "/" {
        return path.starts_with('/');
    }
    path == mount_point || path.starts_with(&format!("{mount_point}/"))
}

fn protected_domain_identity(
    entries: &[MountEntry],
    domain_path: &str,
) -> Result<(u32, u64), TopologyError> {
    let containing =
        containing_mount(entries, domain_path).ok_or(TopologyError::UnavailableStore)?;
    let mount_id = containing.mount_id;
    let domain_dev = if Path::new(domain_path).exists() {
        let meta =
            fs::symlink_metadata(domain_path).map_err(|_| TopologyError::UnavailableStore)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            meta.dev()
        }
        #[cfg(not(unix))]
        {
            return Err(TopologyError::UnavailableStore);
        }
    } else {
        dev_from_major_minor(containing.major, containing.minor)
    };
    Ok((mount_id, domain_dev))
}

#[cfg(unix)]
fn dev_from_major_minor(major: u32, minor: u32) -> u64 {
    let major = u64::from(major);
    let minor = u64::from(minor);
    ((major & 0xfff) << 8) | (minor & 0xff) | ((major & !0xfff) << 32)
}

#[cfg(unix)]
fn check_root_owned_dir_mode(path: &Path, expected_mode: u32) -> Result<(), TopologyError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = fs::symlink_metadata(path).map_err(|_| TopologyError::UnavailableStore)?;
    if meta.is_symlink() {
        return Err(TopologyError::SymlinkComponent);
    }
    if !meta.is_dir() {
        return Err(TopologyError::NonRegularComponent);
    }
    if meta.uid() != 0 || meta.gid() != 0 {
        return Err(TopologyError::WrongOwnershipOrMode);
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != expected_mode {
        return Err(TopologyError::WrongOwnershipOrMode);
    }
    Ok(())
}

#[cfg(unix)]
fn inspect_existing_layout(store_root: &Path) -> Result<(), TopologyError> {
    for subdir in WATCHDOG_SUBDIRS {
        let path = store_root.join(subdir);
        if path.exists() {
            check_root_owned_dir_mode(&path, STORE_DIR_MODE)?;
        }
    }
    for (rel, mode) in WATCHDOG_AUTHORITATIVE_FILES {
        let path = store_root.join(rel);
        if path.exists() {
            check_root_owned_regular_file(&path, *mode)?;
        }
    }
    for (rel, mode) in WATCHDOG_OPTIONAL_FILES {
        let path = store_root.join(rel);
        if path.exists() {
            check_root_owned_regular_file(&path, *mode)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn check_root_owned_regular_file(path: &Path, expected_mode: u32) -> Result<(), TopologyError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = fs::symlink_metadata(path).map_err(|_| TopologyError::UnavailableStore)?;
    if meta.is_symlink() {
        return Err(TopologyError::SymlinkComponent);
    }
    if !meta.is_file() {
        return Err(TopologyError::NonRegularComponent);
    }
    if meta.uid() != 0 || meta.gid() != 0 {
        return Err(TopologyError::WrongOwnershipOrMode);
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != expected_mode {
        return Err(TopologyError::WrongOwnershipOrMode);
    }
    let link_count = meta.nlink();
    if link_count != 1 {
        return Err(TopologyError::WrongLinkCount);
    }
    Ok(())
}

#[cfg(unix)]
fn prove_writable_same_directory_atomic(store_root: &Path) -> Result<(), TopologyError> {
    use std::os::unix::fs::OpenOptionsExt;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let tmp = store_root.join(format!(".tmp-topology-probe-{nanos}"));
    let renamed = store_root.join(format!(".tmp-topology-probe-renamed-{nanos}"));
    let payload = b"agentbed-topology-probe";
    let _cleanup = ProbeResidueGuard::new(store_root, vec![tmp.clone(), renamed.clone()]);

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(AUTHORITATIVE_FILE_MODE)
        .open(&tmp)
        .map_err(|_| TopologyError::Unwritable)?;
    file.write_all(payload)
        .map_err(|_| TopologyError::Unwritable)?;
    file.sync_all().map_err(|_| TopologyError::Unwritable)?;

    // same_directory atomic replacement: rename within the store mount root.
    fs::rename(&tmp, &renamed).map_err(|_| TopologyError::Unwritable)?;
    dir_fsync(store_root)?;

    let readback = fs::read(&renamed).map_err(|_| TopologyError::Unwritable)?;
    if readback != payload {
        return Err(TopologyError::Unwritable);
    }
    Ok(())
}

#[cfg(unix)]
fn dir_fsync(path: &Path) -> Result<(), TopologyError> {
    let dir = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| TopologyError::Unwritable)?;
    dir.sync_all().map_err(|_| TopologyError::Unwritable)
}

#[cfg(unix)]
struct ProbeResidueGuard {
    parent: PathBuf,
    paths: Vec<PathBuf>,
}

#[cfg(unix)]
impl ProbeResidueGuard {
    fn new(parent: &Path, paths: Vec<PathBuf>) -> Self {
        Self {
            parent: parent.to_path_buf(),
            paths,
        }
    }
}

#[cfg(unix)]
impl Drop for ProbeResidueGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
        let _ = dir_fsync(&self.parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_unescapes_octal_space_in_mount_point() {
        let line = "36 35 98:0 / /mnt/with\\040space rw,relatime - ext4 /dev/sda1 rw";
        let entry = parse_mountinfo_line(line).expect("parse");
        assert_eq!(entry.mount_point, "/mnt/with space");
    }

    #[test]
    fn mountinfo_rejects_malformed_line_without_separator() {
        let err = parse_mountinfo_line("36 35 98:0 / /mnt rw").expect_err("malformed");
        assert_eq!(err, TopologyError::UnavailableStore);
    }

    #[test]
    fn containing_mount_uses_longest_component_boundary_prefix() {
        let entries = vec![
            MountEntry {
                mount_id: 1,
                major: 8,
                minor: 1,
                mount_point: "/".to_string(),
            },
            MountEntry {
                mount_id: 2,
                major: 8,
                minor: 2,
                mount_point: "/var".to_string(),
            },
            MountEntry {
                mount_id: 3,
                major: 8,
                minor: 3,
                mount_point: "/var/lib/agentbed/broker/state".to_string(),
            },
        ];
        let found = containing_mount(&entries, "/var/lib/agentbed/broker/state/wal")
            .expect("containing mount");
        assert_eq!(found.mount_id, 3);
    }

    #[test]
    fn exact_mount_requires_mount_point_not_ordinary_directory() {
        let entries = vec![MountEntry {
            mount_id: 10,
            major: 8,
            minor: 10,
            mount_point: "/var/lib/agentbed/watchdog".to_string(),
        }];
        assert!(find_exact_mount(&entries, Path::new("/var/lib/agentbed/watchdog")).is_some());
        assert!(find_exact_mount(&entries, Path::new("/var/lib/agentbed/watchdog/sub")).is_none());
    }

    #[test]
    fn mount_id_alias_detected_for_same_containing_mount() {
        let entries = vec![
            MountEntry {
                mount_id: 42,
                major: 8,
                minor: 1,
                mount_point: "/".to_string(),
            },
            MountEntry {
                mount_id: 42,
                major: 8,
                minor: 1,
                mount_point: "/var/lib/agentbed/watchdog".to_string(),
            },
        ];
        let store = find_exact_mount(&entries, Path::new("/var/lib/agentbed/watchdog")).unwrap();
        let root = containing_mount(&entries, "/").unwrap();
        assert_eq!(store.mount_id, root.mount_id);
    }

    #[test]
    fn device_alias_detected_from_major_minor() {
        let dev = dev_from_major_minor(8, 1);
        assert_ne!(dev, 0);
        assert_eq!(dev, dev_from_major_minor(8, 1));
        assert_ne!(dev, dev_from_major_minor(8, 2));
    }

    #[test]
    fn mount_point_contains_respects_component_boundary() {
        assert!(mount_point_contains("/var", "/var/lib"));
        assert!(!mount_point_contains("/var", "/var-lib"));
    }

    #[test]
    fn parse_mountinfo_table_rejects_ambiguous_major_minor() {
        let err = parse_major_minor("not-a-device").expect_err("bad dev");
        assert_eq!(err, TopologyError::UnavailableStore);
    }
}
