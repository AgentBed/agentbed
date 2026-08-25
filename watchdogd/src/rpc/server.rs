//! Unix socket RPC server.

use crate::core::WatchdogCore;
use crate::error::RpcError;
use crate::rpc::protocol::{
    decode_request, decode_session_bind, encode_response, encode_session_established, read_frame,
};
use crate::session::SessionState;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::Duration;

const SOCKET_IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct RpcServer {
    listener: UnixListener,
    socket_path: std::path::PathBuf,
}

impl RpcServer {
    pub fn bind(socket_path: &Path) -> Result<Self, std::io::Error> {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
            let mut perms = fs::metadata(parent)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(parent, perms)?;
        }
        let _ = fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path)?;
        let mut perms = fs::metadata(socket_path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(socket_path, perms)?;
        Ok(Self {
            listener,
            socket_path: socket_path.to_path_buf(),
        })
    }

    pub fn serve_one(&self, core: &mut WatchdogCore) -> Result<(), RpcError> {
        let (mut stream, _) = self
            .listener
            .accept()
            .map_err(|_| RpcError::MalformedFrame)?;
        stream
            .set_read_timeout(Some(SOCKET_IO_TIMEOUT))
            .map_err(|_| RpcError::MalformedFrame)?;
        stream
            .set_write_timeout(Some(SOCKET_IO_TIMEOUT))
            .map_err(|_| RpcError::MalformedFrame)?;
        let bind_frame = read_frame(&mut stream)?;
        let bind = decode_session_bind(&bind_frame)?;
        let cred = core
            .deps
            .stream_peer
            .peer_credentials_for_stream(&stream)
            .map_err(|_| RpcError::WrongPeer)?;
        let (mut session, established) =
            SessionState::bind_with_stream_cred(core, &cred, &*core.deps.entropy, bind)?;
        let est_frame = encode_session_established(&established)?;
        stream
            .write_all(&est_frame)
            .map_err(|_| RpcError::MalformedFrame)?;
        let req_frame = read_frame(&mut stream)?;
        let verified = decode_request(&req_frame, &mut session)?;
        let counter = verified.counter();
        let req = verified.request().clone();
        let resp = core.handle_request(verified, &mut session)?;
        let resp_frame = encode_response(&resp, &req, &established, counter)?;
        stream
            .write_all(&resp_frame)
            .map_err(|_| RpcError::MalformedFrame)?;
        Ok(())
    }
}
