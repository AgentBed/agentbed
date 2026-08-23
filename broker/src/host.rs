//! Host facts for `system.info`.
//!
//! Kept thin on purpose: `system.info` is class R and every field is
//! reconnaissance for a prompt-injected agent (`docs/threat-model.md` T2). The
//! fields here are the ones ADR §5.1 names — enough for an agent to know what
//! kind of machine it is on and what it may not do.

use agentbed_protocol::dto::system_info::{HostInfo, LandlockInfo};

/// Collect host facts, degrading to `unknown` rather than failing: an
/// unreadable `/etc/os-release` is not a reason to refuse a read call.
#[must_use]
pub fn host_info() -> HostInfo {
    let (kernel_release, architecture) = uname_fields();
    HostInfo {
        hostname: read_trimmed("/proc/sys/kernel/hostname"),
        os_id: os_release_field("ID"),
        os_version_id: os_release_field("VERSION_ID"),
        kernel_release,
        architecture,
    }
}

/// Probe Landlock support.
///
/// ADR §6: the ABI is probed at start and features the kernel lacks are
/// reported in `system.info` and **degrade to deny, never to silent allow**.
/// Gate 0 only reports; the helpers that act on it arrive at Gate 3.
#[must_use]
pub fn landlock_info() -> LandlockInfo {
    match landlock_abi_version() {
        Some(version) if version > 0 => LandlockInfo {
            supported: true,
            abi_version: Some(version),
        },
        _ => LandlockInfo {
            supported: false,
            abi_version: None,
        },
    }
}

/// `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` returns
/// the highest supported ABI version, or an error when Landlock is absent or
/// disabled.
fn landlock_abi_version() -> Option<i32> {
    /// `LANDLOCK_CREATE_RULESET_VERSION`
    const CREATE_RULESET_VERSION: libc::c_ulong = 1;

    // SAFETY: the version query is the documented way to probe Landlock. It
    // takes a null attribute pointer with size 0 and creates nothing — no fd is
    // returned and no kernel object is allocated, so there is nothing to leak.
    #[allow(unsafe_code)]
    let rc = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            CREATE_RULESET_VERSION,
        )
    };
    i32::try_from(rc).ok().filter(|v| *v > 0)
}

fn uname_fields() -> (String, String) {
    // SAFETY: `utsname` is filled in by the kernel; the buffer is owned here and
    // only read after a successful return.
    #[allow(unsafe_code)]
    unsafe {
        let mut buf: libc::utsname = std::mem::zeroed();
        if libc::uname(std::ptr::addr_of_mut!(buf)) != 0 {
            return ("unknown".to_owned(), "unknown".to_owned());
        }
        (
            c_chars_to_string(&buf.release),
            c_chars_to_string(&buf.machine),
        )
    }
}

#[allow(unsafe_code)]
unsafe fn c_chars_to_string(field: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| u8::try_from(*c).unwrap_or(b'?'))
        .collect();
    String::from_utf8(bytes).unwrap_or_else(|_| "unknown".to_owned())
}

fn read_trimmed(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn os_release_field(key: &str) -> String {
    let Ok(contents) = std::fs::read_to_string("/etc/os-release") else {
        return "unknown".to_owned();
    };
    for line in contents.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            return value.trim().trim_matches('"').to_owned();
        }
    }
    "unknown".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_facts_are_populated_or_explicitly_unknown() {
        let info = host_info();
        for field in [
            &info.hostname,
            &info.os_id,
            &info.kernel_release,
            &info.architecture,
        ] {
            assert!(
                !field.is_empty(),
                "a field must be a value or the string 'unknown'"
            );
        }
    }

    #[test]
    fn landlock_probe_is_consistent() {
        let info = landlock_info();
        // Whatever the kernel says, "supported" and "a version" agree — a
        // half-populated probe result is what leads to silent allow later.
        assert_eq!(info.supported, info.abi_version.is_some());
        if let Some(version) = info.abi_version {
            assert!(version > 0);
        }
    }
}
