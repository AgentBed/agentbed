#![allow(unsafe_code)]

//! `SO_PEERCRED` for accepted Unix stream peers.

use crate::interfaces::PeerCred;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPeerCred {
    pub uid: u32,
    pub gid: u32,
    pub pid: i32,
}

impl From<StreamPeerCred> for PeerCred {
    fn from(cred: StreamPeerCred) -> Self {
        Self {
            uid: cred.uid,
            gid: cred.gid,
            pid: cred.pid,
        }
    }
}

/// Production stream peer authentication via `SO_PEERCRED`.
#[derive(Debug, Default)]
pub struct ProductionStreamPeerAuth;

impl ProductionStreamPeerAuth {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl crate::interfaces::StreamPeerAuth for ProductionStreamPeerAuth {
    fn peer_credentials_for_stream(
        &self,
        stream: &UnixStream,
    ) -> Result<PeerCred, crate::error::PeerCredError> {
        peer_credentials(stream)
            .map(PeerCred::from)
            .map_err(|_| crate::error::PeerCredError::Unavailable)
    }
}

/// Read the kernel-reported credentials for the remote end of a Unix socket.
pub fn peer_credentials(stream: &UnixStream) -> std::io::Result<StreamPeerCred> {
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
        Ok(StreamPeerCred {
            uid: cred.uid,
            gid: cred.gid,
            pid: cred.pid,
        })
    }
}
