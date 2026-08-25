#![allow(unsafe_code)]

//! `SO_PEERCRED` for accepted Unix stream peers.

use crate::interfaces::PeerCred;
use std::os::unix::io::AsRawFd;
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
    let fd = stream.as_raw_fd();
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(StreamPeerCred {
        uid: cred.uid,
        gid: cred.gid,
        pid: cred.pid,
    })
}
