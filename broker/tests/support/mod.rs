//! Shared test harness: a real broker on a real Unix socket.
//!
//! Real listener, not a socketpair: `SO_PEERCRED` is part of what these tests
//! exercise, and the socket's `0600` mode and its `0700` parent directory are
//! part of the transport's contract.

// Test scaffolding: panicking on a broken invariant is the point, and the
// module is shared by several test binaries so not every item is used by each.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    unreachable_pub,
    dead_code
)]

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::config::BrokerConfig;
use agentbed_broker::dispatch::Dispatcher;
use agentbed_broker::identity::{Enrollment, TokenStore};
use agentbed_broker::manifest::ManifestStore;
use agentbed_broker::observability::{CallObservation, CollectingObserver};
use agentbed_broker::server::Server;
use agentbed_protocol::frame::{read_frame, write_frame, MAX_FRAME_BYTES};
use agentbed_protocol::wire::Response;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tokens used across the broker tests. Long enough to pass the store's
/// minimum-entropy check; fixed so failures are reproducible.
pub const TOKEN_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const TOKEN_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const TOKEN_UNKNOWN: &str = "cccccccccccccccccccccccccccccccc";
pub const TOKEN_REVOKED: &str = "dddddddddddddddddddddddddddddddd";
pub const TOKEN_TIGHT_QUOTA: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

pub const AGENT_A: &str = "mcp-client:gate0-reader";
pub const AGENT_B: &str = "mcp-client:denied-agent";
pub const AGENT_TIGHT_QUOTA: &str = "mcp-client:gate0-tight-quota";

/// The budget declared by `agent.tight-quota.yaml`.
pub const TIGHT_QUOTA_LIMIT: usize = 3;

/// Every read in these tests is bounded: a hung broker must fail the test, not
/// hang the suite.
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// A running broker plus its scratch directory.
pub struct Harness {
    server: Server,
    audit: CollectingObserver,
    dir: PathBuf,
}

impl Harness {
    /// Start a broker whose token store enrols `TOKEN_A` -> `AGENT_A`
    /// (allowed), `TOKEN_B` -> `AGENT_B` (manifest denies at stage 3), and
    /// `TOKEN_REVOKED` -> a revoked credential for `AGENT_A`.
    pub fn start() -> Harness {
        Self::start_with(|_| {})
    }

    /// Start a broker, letting the caller adjust the configuration first.
    pub fn start_with(customize: impl FnOnce(&mut BrokerConfig)) -> Harness {
        let dir = scratch_dir();
        let enrol = |agent: &str, manifest: &str, token: &str, revoked: bool| Enrollment {
            agent_id: agent.to_owned(),
            manifest_ref: manifest.to_owned(),
            token: token.to_owned(),
            revoked,
            expires_at_unix: None,
        };
        let tokens = TokenStore::from_enrollments([
            enrol(AGENT_A, "agent.reader.yaml", TOKEN_A, false),
            enrol(AGENT_B, "agent.denied.yaml", TOKEN_B, false),
            // A credential that was valid and no longer is. Revocation has to
            // be enforced by the broker, so this is a real revoked entry — an
            // absent one would make the revocation assertion pass for the
            // wrong reason (unknown token, same wire error).
            enrol(AGENT_A, "agent.reader.yaml", TOKEN_REVOKED, true),
            enrol(
                AGENT_TIGHT_QUOTA,
                "agent.tight-quota.yaml",
                TOKEN_TIGHT_QUOTA,
                false,
            ),
        ])
        .expect("token store");

        let mut config = BrokerConfig {
            socket_path: dir.join("broker.sock"),
            manifest_dir: Some(manifest_dir()),
            state_dir: Some(dir.join("state")),
            read_timeout: IO_TIMEOUT,
            write_timeout: IO_TIMEOUT,
            ..BrokerConfig::default()
        };
        customize(&mut config);
        let state_dir = config
            .state_dir
            .clone()
            .unwrap_or_else(|| dir.join("state"));
        let audit = CollectingObserver::default();
        let dispatcher = Arc::new(Dispatcher::new(
            tokens,
            ManifestStore::new(manifest_dir()),
            Box::new(UnresolvedAdapter),
            state_dir,
        ));
        let server = Server::start(&config, dispatcher, Arc::new(audit.clone()))
            .expect("broker binds its socket");
        Harness { server, audit, dir }
    }

    /// A broker that admits only a uid this process does not have, so a
    /// connection from these tests is refused on peer credentials.
    pub fn start_allowing_foreign_peer_uid() -> Harness {
        // SAFETY: getuid() cannot fail and touches no memory.
        #[allow(unsafe_code)]
        let own = unsafe { libc::getuid() };
        Self::start_with(|config| {
            // Any uid but ours. Wrapping keeps this valid when running as root.
            config.allowed_peer_uids = vec![own.wrapping_add(1)];
        })
    }

    pub fn socket_path(&self) -> &Path {
        self.server.socket_path()
    }

    /// Connect as a client would — including as a *forged gateway* would, since
    /// nothing about this connection differs from the real gateway's.
    pub fn connect(&self) -> UnixStream {
        let stream = UnixStream::connect(self.socket_path()).expect("connect to broker");
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .expect("read timeout");
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .expect("write timeout");
        stream
    }

    pub fn audit_records(&self) -> Vec<CallObservation> {
        self.audit.records()
    }

    /// Wait until the broker has recorded at least `n` audit records, bounded.
    pub fn wait_for_records(&self, n: usize) -> Vec<CallObservation> {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            let records = self.audit.records();
            if records.len() >= n || Instant::now() >= deadline {
                return records;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.shutdown(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn scratch_dir() -> PathBuf {
    // Unique per harness without pulling in a temp-dir dependency.
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("agentbed-broker-test-{pid}-{n}"));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/manifests"))
}

/// A well-formed request frame body.
pub fn request_body(id: &str, token: &str) -> Vec<u8> {
    format!(
        r#"{{"v":1,"id":"{id}","op":"system.info","auth":{{"token":"{token}"}},"params":{{}}}}"#
    )
    .into_bytes()
}

pub fn send_frame(stream: &mut UnixStream, body: &[u8]) {
    write_frame(stream, body, MAX_FRAME_BYTES).expect("write frame");
}

/// Send raw bytes with no framing help at all.
pub fn send_raw(stream: &mut UnixStream, bytes: &[u8]) {
    stream.write_all(bytes).expect("write raw");
    stream.flush().expect("flush");
}

/// Read one response frame, or `None` if the broker closed or timed out first.
pub fn read_response(stream: &mut UnixStream) -> Option<Response> {
    let body = read_frame(stream, MAX_FRAME_BYTES).ok()?;
    Some(serde_json::from_slice(&body).expect("broker emits a well-formed response"))
}

/// Assert the peer closed the connection without sending anything more.
pub fn assert_closed_without_response(stream: &mut UnixStream) {
    let mut buf = [0u8; 64];
    match stream.read(&mut buf) {
        Ok(0) => {}
        Ok(n) => panic!("expected close, got {n} bytes: {:?}", &buf[..n]),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            panic!("expected close, connection stayed open")
        }
        Err(_) => {}
    }
}
