//! The broker connection.
//!
//! The gateway is a relay. It opens a Unix socket to the broker, writes the
//! frame the protocol crate defines, and reads one back. It cannot decide
//! anything about the call: the broker re-derives identity, re-loads the
//! manifest, recomputes the effect set and the canonical digest, and evaluates
//! policy on its own inputs (`docs/threat-model.md`, boundary 2).
//!
//! A connection per call, closed afterwards. That is the least state the
//! gateway can hold, and at Gate 0 there is no performance reason to hold more.

use agentbed_protocol::frame::{read_frame, write_frame, MAX_FRAME_BYTES};
use agentbed_protocol::wire::Response;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the gateway waits for the broker before giving up on a call.
const BROKER_TIMEOUT: Duration = Duration::from_secs(30);

/// A client for the broker's socket.
#[derive(Debug, Clone)]
pub struct BrokerClient {
    socket_path: PathBuf,
}

impl BrokerClient {
    /// Point the client at the broker's socket.
    #[must_use]
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        BrokerClient {
            socket_path: socket_path.into(),
        }
    }

    /// The socket in use.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Send one request frame and read one response frame.
    pub fn call(&self, request_body: &[u8]) -> io::Result<Response> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(BROKER_TIMEOUT))?;
        stream.set_write_timeout(Some(BROKER_TIMEOUT))?;

        write_frame(&mut stream, request_body, MAX_FRAME_BYTES)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let body = read_frame(&mut stream, MAX_FRAME_BYTES)
            .map_err(|e| io::Error::other(e.to_string()))?;
        serde_json::from_slice(&body).map_err(io::Error::other)
    }
}
