//! Narrow watchdog RPC client — wire DTOs only, no authority append surface.

use agentbed_watchdogd::rpc::protocol::{
    encode_request, encode_session_bind, read_frame, LocalRequest, SessionBind, SessionEstablished,
};
use agentbed_watchdogd::RpcError;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// Transport and bootstrap failures from the watchdog client boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogClientError {
    Transport(String),
    Bootstrap(String),
}

/// Local-process watchdog RPC client stub.
#[derive(Debug)]
pub struct WatchdogClient {
    socket_path: PathBuf,
}

impl WatchdogClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn bootstrap_session(
        &self,
        bind: &SessionBind,
    ) -> Result<SessionEstablished, WatchdogClientError> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| WatchdogClientError::Transport(e.to_string()))?;
        let frame = encode_session_bind(bind)
            .map_err(|e| WatchdogClientError::Bootstrap(format!("{e:?}")))?;
        stream
            .write_all(&frame)
            .map_err(|e| WatchdogClientError::Transport(e.to_string()))?;
        let established_frame = read_frame(&mut stream)
            .map_err(|e| WatchdogClientError::Bootstrap(format!("{e:?}")))?;
        agentbed_watchdogd::rpc::protocol::decode_session_established(&established_frame)
            .map_err(|e| WatchdogClientError::Bootstrap(format!("{e:?}")))
    }

    pub fn encode_authenticated_request(
        &self,
        request: &LocalRequest,
        established: &SessionEstablished,
        counter: u64,
    ) -> Result<Vec<u8>, WatchdogClientError> {
        encode_request(request, established, counter)
            .map_err(|e: RpcError| WatchdogClientError::Bootstrap(format!("{e:?}")))
    }
}
