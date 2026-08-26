//! Production startup topology verifier for the sealed H-04 watchdog store.

use crate::core::WATCHDOG_MOUNT_ROOT;
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
    "/nix/store",
    "/var/lib/agentbed/broker/state",
    "/var/lib/agentbed/rollback",
];

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

        let store_mount = match find_exact_mount_unique(&mountinfo, &store_abs) {
            Ok(entry) => entry,
            Err(TopologyError::UnavailableStore) => return Err(TopologyError::UnavailableStore),
            Err(_) => {
                let meta = fs::symlink_metadata(store_root);
                return Err(match meta {
                    Ok(m) if m.is_dir() => TopologyError::OrdinaryDirectoryFallback,
                    Ok(_) => TopologyError::NonRegularComponent,
                    Err(_) => TopologyError::UnavailableStore,
                });
            }
        };

        if !path_matches_sealed_mount_root(&store_abs) {
            return Err(TopologyError::MissingMount);
        }

        let store_meta =
            fs::symlink_metadata(store_root).map_err(|_| TopologyError::UnavailableStore)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let store_mount_id = store_mount.mount_id;
            let store_dev = store_meta.dev();

            evaluate_root_dir_metadata(&store_meta, STORE_DIR_MODE)?;

            for domain in PROTECTED_DOMAIN_PATHS {
                let (domain_mount_id, domain_dev) = protected_domain_identity(&mountinfo, domain)?;
                evaluate_mount_device_separation(
                    store_mount_id,
                    store_dev,
                    domain_mount_id,
                    domain_dev,
                )?;
            }

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
    mount_point: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirEvidence {
    uid: u32,
    gid: u32,
    mode: u32,
    is_symlink: bool,
    is_dir: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileEvidence {
    uid: u32,
    gid: u32,
    mode: u32,
    link_count: u64,
    is_symlink: bool,
    is_file: bool,
}

fn path_matches_sealed_mount_root(path: &Path) -> bool {
    normalize_mount_path(path) == Path::new(WATCHDOG_MOUNT_ROOT)
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
    parse_mountinfo(&raw)
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
    fields.next().ok_or(TopologyError::UnavailableStore)?;
    fields.next().ok_or(TopologyError::UnavailableStore)?;
    let mount_point_raw = fields.next().ok_or(TopologyError::UnavailableStore)?;
    let mount_point = unescape_mount_path(mount_point_raw)?;
    Ok(MountEntry {
        mount_id,
        mount_point,
    })
}

fn unescape_mount_path(raw: &str) -> Result<String, TopologyError> {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut idx = 0;
    while let Some(&byte) = bytes.get(idx) {
        if byte == b'\\' {
            let h = bytes.get(idx + 1).ok_or(TopologyError::UnavailableStore)?;
            let t = bytes.get(idx + 2).ok_or(TopologyError::UnavailableStore)?;
            let o = bytes.get(idx + 3).ok_or(TopologyError::UnavailableStore)?;
            let value = (hex_digit(*h)? << 6) | (hex_digit(*t)? << 3) | hex_digit(*o)?;
            out.push(char::from(value));
            idx += 4;
            continue;
        }
        out.push(char::from(byte));
        idx += 1;
    }
    Ok(out)
}

fn hex_digit(byte: u8) -> Result<u8, TopologyError> {
    match byte {
        b'0'..=b'7' => Ok(byte - b'0'),
        _ => Err(TopologyError::UnavailableStore),
    }
}

fn find_exact_mount_unique<'a>(
    entries: &'a [MountEntry],
    path: &Path,
) -> Result<&'a MountEntry, TopologyError> {
    let normalized = normalize_mount_path(path);
    let normalized = normalized.to_string_lossy();
    let mut matches = entries
        .iter()
        .filter(|entry| entry.mount_point == normalized);
    let first = matches
        .next()
        .ok_or(TopologyError::OrdinaryDirectoryFallback)?;
    if matches.next().is_some() {
        return Err(TopologyError::UnavailableStore);
    }
    Ok(first)
}

fn containing_mount_unique<'a>(
    entries: &'a [MountEntry],
    path: &str,
) -> Result<&'a MountEntry, TopologyError> {
    let candidates: Vec<&MountEntry> = entries
        .iter()
        .filter(|entry| mount_point_contains(&entry.mount_point, path))
        .collect();
    if candidates.is_empty() {
        return Err(TopologyError::UnavailableStore);
    }
    let max_len = candidates
        .iter()
        .map(|entry| entry.mount_point.len())
        .max()
        .unwrap_or(0);
    let longest: Vec<&&MountEntry> = candidates
        .iter()
        .filter(|entry| entry.mount_point.len() == max_len)
        .collect();
    match longest.as_slice() {
        [only] => Ok(*only),
        _ => Err(TopologyError::UnavailableStore),
    }
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
    let domain = Path::new(domain_path);
    reject_lexical_traversal(domain)?;
    reject_symlink_components(domain)?;

    if !domain.exists() {
        return Err(TopologyError::MissingMount);
    }

    let containing = containing_mount_unique(entries, domain_path)?;
    let meta = fs::symlink_metadata(domain).map_err(|_| TopologyError::UnavailableStore)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok((containing.mount_id, meta.dev()))
    }
    #[cfg(not(unix))]
    {
        let _ = (containing, meta);
        Err(TopologyError::UnavailableStore)
    }
}

#[cfg(unix)]
fn evaluate_mount_device_separation(
    store_mount_id: u32,
    store_dev: u64,
    domain_mount_id: u32,
    domain_dev: u64,
) -> Result<(), TopologyError> {
    if store_mount_id == domain_mount_id || store_dev == domain_dev {
        return Err(TopologyError::SameDeviceAlias);
    }
    Ok(())
}

#[cfg(unix)]
fn evaluate_root_dir_metadata(
    meta: &fs::Metadata,
    expected_mode: u32,
) -> Result<(), TopologyError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let evidence = DirEvidence {
        uid: meta.uid(),
        gid: meta.gid(),
        mode: meta.permissions().mode() & 0o777,
        is_symlink: meta.file_type().is_symlink(),
        is_dir: meta.is_dir(),
    };
    evaluate_dir_ownership(evidence, expected_mode)
}

#[cfg(unix)]
fn evaluate_dir_ownership(evidence: DirEvidence, expected_mode: u32) -> Result<(), TopologyError> {
    if evidence.is_symlink {
        return Err(TopologyError::SymlinkComponent);
    }
    if !evidence.is_dir {
        return Err(TopologyError::NonRegularComponent);
    }
    if evidence.uid != 0 || evidence.gid != 0 {
        return Err(TopologyError::WrongOwnershipOrMode);
    }
    if evidence.mode != expected_mode {
        return Err(TopologyError::WrongOwnershipOrMode);
    }
    Ok(())
}

#[cfg(unix)]
fn evaluate_regular_file_metadata(
    meta: &fs::Metadata,
    expected_mode: u32,
) -> Result<(), TopologyError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let evidence = FileEvidence {
        uid: meta.uid(),
        gid: meta.gid(),
        mode: meta.permissions().mode() & 0o777,
        link_count: meta.nlink(),
        is_symlink: meta.file_type().is_symlink(),
        is_file: meta.is_file(),
    };
    evaluate_file_ownership(evidence, expected_mode)
}

#[cfg(unix)]
fn evaluate_file_ownership(
    evidence: FileEvidence,
    expected_mode: u32,
) -> Result<(), TopologyError> {
    if evidence.is_symlink {
        return Err(TopologyError::SymlinkComponent);
    }
    if !evidence.is_file {
        return Err(TopologyError::NonRegularComponent);
    }
    if evidence.uid != 0 || evidence.gid != 0 {
        return Err(TopologyError::WrongOwnershipOrMode);
    }
    if evidence.mode != expected_mode {
        return Err(TopologyError::WrongOwnershipOrMode);
    }
    if evidence.link_count != 1 {
        return Err(TopologyError::WrongLinkCount);
    }
    Ok(())
}

#[cfg(unix)]
fn inspect_existing_layout(store_root: &Path) -> Result<(), TopologyError> {
    for subdir in WATCHDOG_SUBDIRS {
        let path = store_root.join(subdir);
        if path.exists() {
            let meta = fs::symlink_metadata(&path).map_err(|_| TopologyError::UnavailableStore)?;
            evaluate_root_dir_metadata(&meta, STORE_DIR_MODE)?;
        }
    }
    for (rel, mode) in WATCHDOG_AUTHORITATIVE_FILES {
        let path = store_root.join(rel);
        if path.exists() {
            let meta = fs::symlink_metadata(&path).map_err(|_| TopologyError::UnavailableStore)?;
            evaluate_regular_file_metadata(&meta, *mode)?;
        }
    }
    for (rel, mode) in WATCHDOG_OPTIONAL_FILES {
        let path = store_root.join(rel);
        if path.exists() {
            let meta = fs::symlink_metadata(&path).map_err(|_| TopologyError::UnavailableStore)?;
            evaluate_regular_file_metadata(&meta, *mode)?;
        }
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
    let mut cleanup = ProbeResidueGuard::new(store_root, vec![tmp.clone(), renamed.clone()]);

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

    cleanup_probe_residue(store_root, &[renamed])?;
    cleanup.disarm();
    Ok(())
}

#[cfg(unix)]
fn cleanup_probe_residue(parent: &Path, paths: &[PathBuf]) -> Result<(), TopologyError> {
    for path in paths {
        let _ = fs::remove_file(path);
    }
    dir_fsync(parent)
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
    disabled: bool,
}

#[cfg(unix)]
impl ProbeResidueGuard {
    fn new(parent: &Path, paths: Vec<PathBuf>) -> Self {
        Self {
            parent: parent.to_path_buf(),
            paths,
            disabled: false,
        }
    }

    fn disarm(&mut self) {
        self.disabled = true;
    }
}

#[cfg(unix)]
impl Drop for ProbeResidueGuard {
    fn drop(&mut self) {
        if self.disabled {
            return;
        }
        let _ = cleanup_probe_residue(&self.parent, &self.paths);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_unescapes_valid_octal_sequences() {
        assert_eq!(
            unescape_mount_path("/mnt/with\\040space").expect("space"),
            "/mnt/with space"
        );
        assert_eq!(
            unescape_mount_path("/tab\\011here").expect("tab"),
            "/tab\there"
        );
        assert_eq!(
            unescape_mount_path("/line\\012feed").expect("lf"),
            "/line\nfeed"
        );
        assert_eq!(
            unescape_mount_path("/slash\\134end").expect("backslash"),
            "/slash\\end"
        );
    }

    #[test]
    fn mountinfo_rejects_incomplete_escape() {
        let err = unescape_mount_path("/bad\\04").expect_err("incomplete");
        assert_eq!(err, TopologyError::UnavailableStore);
    }

    #[test]
    fn mountinfo_rejects_non_octal_escape() {
        let err = unescape_mount_path("/bad\\980").expect_err("non-octal");
        assert_eq!(err, TopologyError::UnavailableStore);
    }

    #[test]
    fn mountinfo_rejects_malformed_line_without_separator() {
        let err = parse_mountinfo_line("36 35 98:0 / /mnt rw").expect_err("malformed");
        assert_eq!(err, TopologyError::UnavailableStore);
    }

    #[test]
    fn containing_mount_unique_uses_longest_component_boundary_prefix() {
        let entries = vec![
            MountEntry {
                mount_id: 1,
                mount_point: "/".to_string(),
            },
            MountEntry {
                mount_id: 2,
                mount_point: "/var".to_string(),
            },
            MountEntry {
                mount_id: 3,
                mount_point: "/var/lib/agentbed/broker/state".to_string(),
            },
        ];
        let found =
            containing_mount_unique(&entries, "/var/lib/agentbed/broker/state/wal").expect("mount");
        assert_eq!(found.mount_id, 3);
    }

    #[test]
    fn containing_mount_unique_rejects_ambiguous_equal_longest_prefix() {
        let entries = vec![
            MountEntry {
                mount_id: 1,
                mount_point: "/var/lib".to_string(),
            },
            MountEntry {
                mount_id: 2,
                mount_point: "/var/lib".to_string(),
            },
        ];
        let err = containing_mount_unique(&entries, "/var/lib/agentbed").expect_err("ambiguous");
        assert_eq!(err, TopologyError::UnavailableStore);
    }

    #[test]
    fn exact_mount_unique_rejects_duplicate_entries() {
        let entries = vec![
            MountEntry {
                mount_id: 10,
                mount_point: "/var/lib/agentbed/watchdog".to_string(),
            },
            MountEntry {
                mount_id: 11,
                mount_point: "/var/lib/agentbed/watchdog".to_string(),
            },
        ];
        let err = find_exact_mount_unique(&entries, Path::new("/var/lib/agentbed/watchdog"))
            .expect_err("duplicate");
        assert_eq!(err, TopologyError::UnavailableStore);
    }

    #[test]
    fn sealed_mount_root_requires_exact_watchdog_path() {
        assert!(path_matches_sealed_mount_root(Path::new(
            WATCHDOG_MOUNT_ROOT
        )));
        assert!(!path_matches_sealed_mount_root(Path::new(
            "/mnt/other-watchdog"
        )));
    }

    #[test]
    fn evaluate_mount_device_separation_rejects_same_mount_id() {
        let err = evaluate_mount_device_separation(42, 100, 42, 200).expect_err("mount id");
        assert_eq!(err, TopologyError::SameDeviceAlias);
    }

    #[test]
    fn evaluate_mount_device_separation_rejects_same_st_dev() {
        let err = evaluate_mount_device_separation(1, 77, 2, 77).expect_err("st_dev");
        assert_eq!(err, TopologyError::SameDeviceAlias);
    }

    #[test]
    fn evaluate_dir_ownership_rejects_bad_root_uid_gid_mode() {
        let err = evaluate_dir_ownership(
            DirEvidence {
                uid: 1000,
                gid: 0,
                mode: STORE_DIR_MODE,
                is_symlink: false,
                is_dir: true,
            },
            STORE_DIR_MODE,
        )
        .expect_err("uid");
        assert_eq!(err, TopologyError::WrongOwnershipOrMode);

        let err = evaluate_dir_ownership(
            DirEvidence {
                uid: 0,
                gid: 1000,
                mode: STORE_DIR_MODE,
                is_symlink: false,
                is_dir: true,
            },
            STORE_DIR_MODE,
        )
        .expect_err("gid");
        assert_eq!(err, TopologyError::WrongOwnershipOrMode);

        let err = evaluate_dir_ownership(
            DirEvidence {
                uid: 0,
                gid: 0,
                mode: 0o755,
                is_symlink: false,
                is_dir: true,
            },
            STORE_DIR_MODE,
        )
        .expect_err("mode");
        assert_eq!(err, TopologyError::WrongOwnershipOrMode);
    }

    #[test]
    fn evaluate_dir_ownership_rejects_symlink_and_non_directory() {
        let err = evaluate_dir_ownership(
            DirEvidence {
                uid: 0,
                gid: 0,
                mode: STORE_DIR_MODE,
                is_symlink: true,
                is_dir: false,
            },
            STORE_DIR_MODE,
        )
        .expect_err("symlink");
        assert_eq!(err, TopologyError::SymlinkComponent);

        let err = evaluate_dir_ownership(
            DirEvidence {
                uid: 0,
                gid: 0,
                mode: STORE_DIR_MODE,
                is_symlink: false,
                is_dir: false,
            },
            STORE_DIR_MODE,
        )
        .expect_err("non-dir");
        assert_eq!(err, TopologyError::NonRegularComponent);
    }

    #[test]
    fn evaluate_file_ownership_rejects_bad_uid_gid_mode_and_link_count() {
        let err = evaluate_file_ownership(
            FileEvidence {
                uid: 1,
                gid: 0,
                mode: AUTHORITATIVE_FILE_MODE,
                link_count: 1,
                is_symlink: false,
                is_file: true,
            },
            AUTHORITATIVE_FILE_MODE,
        )
        .expect_err("uid");
        assert_eq!(err, TopologyError::WrongOwnershipOrMode);

        let err = evaluate_file_ownership(
            FileEvidence {
                uid: 0,
                gid: 1,
                mode: AUTHORITATIVE_FILE_MODE,
                link_count: 1,
                is_symlink: false,
                is_file: true,
            },
            AUTHORITATIVE_FILE_MODE,
        )
        .expect_err("gid");
        assert_eq!(err, TopologyError::WrongOwnershipOrMode);

        let err = evaluate_file_ownership(
            FileEvidence {
                uid: 0,
                gid: 0,
                mode: 0o644,
                link_count: 1,
                is_symlink: false,
                is_file: true,
            },
            AUTHORITATIVE_FILE_MODE,
        )
        .expect_err("mode");
        assert_eq!(err, TopologyError::WrongOwnershipOrMode);

        let err = evaluate_file_ownership(
            FileEvidence {
                uid: 0,
                gid: 0,
                mode: AUTHORITATIVE_FILE_MODE,
                link_count: 2,
                is_symlink: false,
                is_file: true,
            },
            AUTHORITATIVE_FILE_MODE,
        )
        .expect_err("nlink");
        assert_eq!(err, TopologyError::WrongLinkCount);
    }

    #[test]
    fn mount_point_contains_respects_component_boundary() {
        assert!(mount_point_contains("/var", "/var/lib"));
        assert!(!mount_point_contains("/var", "/var-lib"));
    }
}
