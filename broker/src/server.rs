//! Socket setup and the connection loop.
//!
//! # Frame-loop semantics
//!
//! Fail-closed is **per frame**, not per connection: a valid frame that arrived
//! before a malformed one still gets its one result and its one audit record.
//! What a malformed frame never produces is a result.
//!
//! | Condition | Response | Connection |
//! |---|---|---|
//! | valid frame | exactly one response | stays open |
//! | zero-length frame | one `invalid_request` | stays open — the prefix was consumed, so the position is known |
//! | oversize declared length | one `invalid_request`, best effort | **closed** — the body was never read, so the next bytes are ambiguous |
//! | truncated frame | none | **closed** — nothing to correlate a response to |
//! | idle/read timeout | none | closed |
//! | clean EOF | none | closed |

use crate::config::BrokerConfig;
use crate::dispatch::Dispatcher;
use crate::observability::{CallObservation, ObservationSink};
use crate::peercred::{peer_credentials, PeerCredentials};
use agentbed_protocol::frame::{read_frame, write_frame, FrameError, MAX_FRAME_BYTES};
use agentbed_protocol::wire::{ErrorCode, Response, ResponseError};
use std::io;
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the accept loop sleeps between polls when idle.
const ACCEPT_POLL: Duration = Duration::from_millis(5);

/// A bound, running broker.
#[derive(Debug)]
pub struct Server {
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Bind the socket and start serving on a background thread.
    ///
    /// The socket directory is created `0700` and the socket itself `0600`
    /// before anyone can connect: filesystem permissions are the first gate on
    /// *the channel*, and they are set before the listener is reachable rather
    /// than after.
    pub fn start(
        config: &BrokerConfig,
        dispatcher: Arc<Dispatcher>,
        observer: Arc<dyn ObservationSink>,
    ) -> io::Result<Server> {
        let socket_path = config.socket_path.clone();
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        remove_stale_socket(&socket_path)?;

        let listener = UnixListener::bind(&socket_path)?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let accept_thread = {
            let shutdown = Arc::clone(&shutdown);
            let config = config.clone();
            std::thread::Builder::new()
                .name("broker-accept".to_owned())
                .spawn(move || {
                    accept_loop(&listener, &config, &dispatcher, &observer, &shutdown);
                })?
        };

        Ok(Server {
            socket_path,
            shutdown,
            accept_thread: Some(accept_thread),
        })
    }

    /// The path the broker is listening on.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Ask the accept loop to stop and wait for it, bounded.
    ///
    /// Bounded on purpose: a shutdown that can hang forever turns a stuck
    /// connection into a stuck test suite and a stuck service restart.
    pub fn shutdown(&mut self, within: Duration) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.accept_thread.take() {
            let deadline = Instant::now().checked_add(within);
            while !handle.is_finished() {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    break;
                }
                std::thread::sleep(ACCEPT_POLL);
            }
            if handle.is_finished() {
                // Join only when it cannot block. A panicking connection thread
                // is a bug we want surfaced, not swallowed.
                drop(handle.join());
            }
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(2));
    }
}

/// Remove a socket left behind by a previous run, but never anything else.
fn remove_stale_socket(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_socket() {
                std::fs::remove_file(path)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "refusing to replace a non-socket at the socket path",
                ))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn accept_loop(
    listener: &UnixListener,
    config: &BrokerConfig,
    dispatcher: &Arc<Dispatcher>,
    observer: &Arc<dyn ObservationSink>,
    shutdown: &Arc<AtomicBool>,
) {
    let live = Arc::new(AtomicUsize::new(0));
    let mut threads: Vec<JoinHandle<()>> = Vec::new();

    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _addr)) => {
                threads.retain(|t| !t.is_finished());
                if live.load(Ordering::SeqCst) >= config.max_connections {
                    // Refuse by closing: a queue we cannot drain is a slow
                    // resource exhaustion, not a courtesy.
                    drop(stream);
                    continue;
                }
                live.fetch_add(1, Ordering::SeqCst);
                let spawned = spawn_connection(stream, config, dispatcher, observer, &live);
                match spawned {
                    Ok(handle) => threads.push(handle),
                    Err(_) => {
                        live.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(ACCEPT_POLL),
            Err(_) => std::thread::sleep(ACCEPT_POLL),
        }
    }

    for handle in threads {
        drop(handle.join());
    }
}

fn spawn_connection(
    stream: UnixStream,
    config: &BrokerConfig,
    dispatcher: &Arc<Dispatcher>,
    observer: &Arc<dyn ObservationSink>,
    live: &Arc<AtomicUsize>,
) -> io::Result<JoinHandle<()>> {
    let observer = Arc::clone(observer);
    let dispatcher = Arc::clone(dispatcher);
    let config = config.clone();
    let live = Arc::clone(live);
    std::thread::Builder::new()
        .name("broker-conn".to_owned())
        .spawn(move || {
            serve_connection(stream, &config, &dispatcher, observer.as_ref());
            live.fetch_sub(1, Ordering::SeqCst);
        })
}

fn serve_connection(
    mut stream: UnixStream,
    config: &BrokerConfig,
    dispatcher: &Dispatcher,
    observer: &dyn ObservationSink,
) {
    let Ok(peer) = peer_credentials(&stream) else {
        return;
    };
    if !peer_uid_allowed(peer, config) {
        // The channel itself is not permitted. Nothing is read from it: an
        // unauthorized peer must not get to feed the parser at all.
        observer.record(CallObservation::rejected(
            peer,
            ErrorCode::Unauthenticated,
            "peer_uid_denied",
        ));
        return;
    }
    let _ = stream.set_read_timeout(Some(config.read_timeout));
    let _ = stream.set_write_timeout(Some(config.write_timeout));

    let mut served: u32 = 0;
    loop {
        if served >= config.max_requests_per_connection {
            return;
        }
        match read_frame(&mut stream, MAX_FRAME_BYTES) {
            Ok(body) => {
                served = served.saturating_add(1);
                let response = dispatcher.handle_frame(&body, peer, observer);
                if write_response(&mut stream, &response).is_err() {
                    return;
                }
            }
            Err(FrameError::ZeroLength) => {
                // Position is still known: answer and keep the connection.
                observer.record(CallObservation::rejected(
                    peer,
                    ErrorCode::InvalidRequest,
                    "zero_length_frame",
                ));
                let response =
                    Response::failed(None, ResponseError::new(ErrorCode::InvalidRequest));
                if write_response(&mut stream, &response).is_err() {
                    return;
                }
            }
            Err(err @ FrameError::Oversize { .. }) => {
                observer.record(CallObservation::rejected(
                    peer,
                    ErrorCode::InvalidRequest,
                    "oversize_frame",
                ));
                let response =
                    Response::failed(None, ResponseError::new(ErrorCode::InvalidRequest));
                // Best effort: the peer may not be reading. Either way the
                // connection closes, because `err` says the position is lost.
                let _ = write_response(&mut stream, &response);
                debug_assert!(err.stream_position_lost());
                return;
            }
            Err(FrameError::Truncated { .. }) => {
                // No response: there is nothing to correlate one to, and no
                // audit record either — nothing was processed.
                return;
            }
            Err(FrameError::Eof | FrameError::Io(_)) => return,
        }
    }
}

fn peer_uid_allowed(peer: PeerCredentials, config: &BrokerConfig) -> bool {
    if config.allowed_peer_uids.is_empty() {
        // SAFETY: getuid() cannot fail and touches no memory.
        #[allow(unsafe_code)]
        let own = unsafe { libc::getuid() };
        return peer.uid == own;
    }
    config.allowed_peer_uids.contains(&peer.uid)
}

fn write_response(stream: &mut UnixStream, response: &Response) -> io::Result<()> {
    let body = serde_json::to_vec(response).map_err(io::Error::other)?;
    write_frame(stream, &body, MAX_FRAME_BYTES).map_err(|e| match e {
        FrameError::Io(io) => io,
        other => io::Error::other(other.to_string()),
    })
}
