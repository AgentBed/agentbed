//! `SO_PEERCRED`: who is on the other end of the socket.
//!
//! # What this authenticates, and what it does not
//!
//! The kernel tells us the uid, gid and pid of the connecting *process*. That
//! authenticates the **channel** — it answers "may this process talk to the
//! broker at all". It says nothing whatsoever about **which agent** is calling:
//! one gateway process serves many agents, and the gateway is untrusted by the
//! broker (`docs/threat-model.md`, boundary 2).
//!
//! Treating a valid peer credential as an authorization would be exactly the
//! confused-deputy bug the split-process design exists to prevent, so identity
//! resolution lives in [`crate::identity`] and starts from the presented token,
//! never from this.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

/// Kernel-reported credentials of the connected process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    /// Effective uid of the peer process.
    pub uid: u32,
    /// Effective gid of the peer process.
    pub gid: u32,
    /// Peer pid, for the audit trail only — it is racy by nature and must not
    /// be used for a decision.
    pub pid: i32,
}

/// Read the peer's credentials from a connected Unix stream.
pub fn peer_credentials(stream: &UnixStream) -> std::io::Result<PeerCredentials> {
    // SAFETY: `ucred` is a plain-old-data struct that the kernel fills in;
    // `getsockopt` is given the matching level/option and the exact size of the
    // buffer, and the borrowed stream keeps the fd valid for the call. The
    // result is only read after a successful return.
    #[allow(unsafe_code)]
    unsafe {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = u32::try_from(std::mem::size_of::<libc::ucred>())
            .map_err(|_| std::io::Error::other("ucred size does not fit socklen_t"))?;
        let rc = libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(cred).cast::<libc::c_void>(),
            std::ptr::addr_of_mut!(len),
        );
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(PeerCredentials {
            uid: cred.uid,
            gid: cred.gid,
            pid: cred.pid,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_this_process_for_a_socketpair() {
        let (a, _b) = UnixStream::pair().expect("socketpair");
        let creds = peer_credentials(&a).expect("peer credentials");
        // Both ends are this process, so the kernel must agree with getuid().
        #[allow(unsafe_code)]
        let (uid, pid) = unsafe { (libc::getuid(), libc::getpid()) };
        assert_eq!(creds.uid, uid);
        assert_eq!(creds.pid, pid);
    }
}
