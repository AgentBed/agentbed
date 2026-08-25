//! Broker configuration.
//!
//! Gate 0 runs the broker as a normal user (`docs/roadmap.md`); root, systemd
//! socket activation and `DynamicUser` for the gateway come with Gate 1. What
//! is already true here is the shape of the trust: the socket is reachable only
//! by a listed set of uids, and that set gates *the channel*, never a call.

use std::path::PathBuf;
use std::time::Duration;

/// Runtime settings.
#[derive(Debug, Clone)]
pub struct BrokerConfig {
    /// Where to bind the Unix socket.
    pub socket_path: PathBuf,
    /// JSON token store (see [`crate::identity`]).
    pub token_store_path: Option<PathBuf>,
    /// Directory of agent manifests.
    pub manifest_dir: Option<PathBuf>,
    /// Durable broker state (WAL, events, idempotency).
    pub state_dir: Option<PathBuf>,
    /// Peer uids permitted to connect at all. Empty means "this process's uid".
    pub allowed_peer_uids: Vec<u32>,
    /// Concurrent connections served before new ones are refused.
    pub max_connections: usize,
    /// Requests accepted on one connection before it is closed.
    pub max_requests_per_connection: u32,
    /// How long a connection may stall mid-frame before it is dropped.
    pub read_timeout: Duration,
    /// How long a write may block before the connection is dropped.
    pub write_timeout: Duration,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        BrokerConfig {
            socket_path: PathBuf::from("/run/agentbed/broker.sock"),
            token_store_path: None,
            manifest_dir: None,
            state_dir: None,
            allowed_peer_uids: Vec::new(),
            max_connections: 32,
            max_requests_per_connection: 1024,
            read_timeout: Duration::from_secs(15),
            write_timeout: Duration::from_secs(15),
        }
    }
}
