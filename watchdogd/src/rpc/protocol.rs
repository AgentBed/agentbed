//! Bounded, versioned frame codec and authenticated local RPC data types.

use crate::error::RpcError;
use crate::read_model::AuthorityRecordKind;
use crate::session::SessionState;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::time::SystemTime;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_PAYLOAD_BYTES: usize = 64 * 1024;
const FRAME_HEADER_BYTES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBind {
    pub host_id: String,
    pub tx_id: String,
    pub epoch: u64,
    pub lease_id: String,
    pub process_group: i32,
    pub client_nonce: String,
}

impl SessionBind {
    #[must_use]
    pub fn new(
        host_id: &str,
        tx_id: &str,
        epoch: u64,
        lease_id: &str,
        process_group: i32,
        client_nonce: &str,
    ) -> Self {
        Self {
            host_id: host_id.to_owned(),
            tx_id: tx_id.to_owned(),
            epoch,
            lease_id: lease_id.to_owned(),
            process_group,
            client_nonce: client_nonce.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionEstablished {
    pub capability: Vec<u8>,
    pub server_nonce: String,
    pub host_id: String,
    pub tx_id: String,
    pub epoch: u64,
    pub counter: u64,
}

impl SessionEstablished {
    #[must_use]
    pub fn counter(&self) -> u64 {
        self.counter
    }

    #[must_use]
    pub fn capability(&self) -> &[u8] {
        &self.capability
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalRequest {
    Arm {
        request_id: String,
        host_id: String,
        tx_id: String,
        epoch: u64,
        base: String,
        deadline_secs: u64,
        deadline_nanos: u32,
        mandatory_invariants: Vec<String>,
        additive_manifest_checks: Vec<String>,
    },
    ReportHealth {
        request_id: String,
        host_id: String,
        tx_id: String,
        epoch: u64,
    },
    RequestLeaseRenewal {
        request_id: String,
        host_id: String,
        tx_id: String,
        epoch: u64,
        lease_id: String,
        process_group: i32,
        renewal_seq: u64,
    },
    Heartbeat {
        request_id: String,
        host_id: String,
        tx_id: String,
        epoch: u64,
        lease_id: String,
        process_group: i32,
        heartbeat_seq: u64,
    },
    RequestDecision {
        request_id: String,
        host_id: String,
        tx_id: String,
        epoch: u64,
    },
}

impl LocalRequest {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn arm(
        request_id: &str,
        host_id: &str,
        tx_id: &str,
        epoch: u64,
        base: &str,
        deadline: SystemTime,
        mandatory_invariants: Vec<String>,
        additive_manifest_checks: Vec<String>,
    ) -> Self {
        let duration = deadline
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        Self::Arm {
            request_id: request_id.to_owned(),
            host_id: host_id.to_owned(),
            tx_id: tx_id.to_owned(),
            epoch,
            base: base.to_owned(),
            deadline_secs: duration.as_secs(),
            deadline_nanos: duration.subsec_nanos(),
            mandatory_invariants,
            additive_manifest_checks,
        }
    }

    #[must_use]
    pub fn report_health(request_id: &str, host_id: &str, tx_id: &str, epoch: u64) -> Self {
        Self::ReportHealth {
            request_id: request_id.to_owned(),
            host_id: host_id.to_owned(),
            tx_id: tx_id.to_owned(),
            epoch,
        }
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn request_lease_renewal(
        request_id: &str,
        host_id: &str,
        tx_id: &str,
        epoch: u64,
        lease_id: &str,
        process_group: i32,
        renewal_seq: u64,
    ) -> Self {
        Self::RequestLeaseRenewal {
            request_id: request_id.to_owned(),
            host_id: host_id.to_owned(),
            tx_id: tx_id.to_owned(),
            epoch,
            lease_id: lease_id.to_owned(),
            process_group,
            renewal_seq,
        }
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn heartbeat(
        request_id: &str,
        host_id: &str,
        tx_id: &str,
        epoch: u64,
        lease_id: &str,
        process_group: i32,
        heartbeat_seq: u64,
    ) -> Self {
        Self::Heartbeat {
            request_id: request_id.to_owned(),
            host_id: host_id.to_owned(),
            tx_id: tx_id.to_owned(),
            epoch,
            lease_id: lease_id.to_owned(),
            process_group,
            heartbeat_seq,
        }
    }

    #[must_use]
    pub fn request_decision(request_id: &str, host_id: &str, tx_id: &str, epoch: u64) -> Self {
        Self::RequestDecision {
            request_id: request_id.to_owned(),
            host_id: host_id.to_owned(),
            tx_id: tx_id.to_owned(),
            epoch,
        }
    }

    pub(crate) fn request_id(&self) -> &str {
        match self {
            Self::Arm { request_id, .. }
            | Self::ReportHealth { request_id, .. }
            | Self::RequestLeaseRenewal { request_id, .. }
            | Self::Heartbeat { request_id, .. }
            | Self::RequestDecision { request_id, .. } => request_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalResponse {
    Armed {
        request_id: String,
    },
    HealthAck {
        request_id: String,
    },
    LeaseRenewed {
        request_id: String,
    },
    HeartbeatAck {
        request_id: String,
    },
    AuthorityChosen {
        request_id: String,
        kind: AuthorityRecordKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedRequest {
    request: LocalRequest,
    counter: u64,
}

impl AuthenticatedRequest {
    #[must_use]
    pub fn request(&self) -> &LocalRequest {
        &self.request
    }

    pub(crate) fn counter(&self) -> u64 {
        self.counter
    }

    pub(crate) fn into_request(self) -> LocalRequest {
        self.request
    }
}

#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub payload: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlEnvelope<T> {
    version: u32,
    payload: T,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    version: u32,
    capability: Vec<u8>,
    counter: u64,
    request_id: String,
    host_id: String,
    tx_id: String,
    epoch: u64,
    request: LocalRequest,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseEnvelope {
    version: u32,
    capability: Vec<u8>,
    counter: u64,
    request_id: String,
    host_id: String,
    tx_id: String,
    epoch: u64,
    response: LocalResponse,
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, RpcError> {
    if payload.len() > MAX_FRAME_PAYLOAD_BYTES {
        return Err(RpcError::OversizeFrame);
    }
    let length = u32::try_from(payload.len()).map_err(|_| RpcError::OversizeFrame)?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES.saturating_add(payload.len()));
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&crc32(payload).to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8]) -> Result<DecodedFrame, RpcError> {
    if frame.len() < FRAME_HEADER_BYTES {
        return Err(RpcError::MalformedFrame);
    }
    let length = read_u32(frame, 0)? as usize;
    if length > MAX_FRAME_PAYLOAD_BYTES {
        return Err(RpcError::OversizeFrame);
    }
    let expected_len = FRAME_HEADER_BYTES
        .checked_add(length)
        .ok_or(RpcError::MalformedFrame)?;
    if frame.len() < expected_len {
        return Err(RpcError::MalformedFrame);
    }
    if frame.len() != expected_len {
        return Err(RpcError::MalformedFrame);
    }
    let expected_crc = read_u32(frame, 4)?;
    let payload = frame
        .get(FRAME_HEADER_BYTES..expected_len)
        .ok_or(RpcError::MalformedFrame)?;
    if crc32(payload) != expected_crc {
        return Err(RpcError::CrcMismatch);
    }
    Ok(DecodedFrame {
        payload: payload.to_vec(),
    })
}

pub fn encode_session_bind(bind: &SessionBind) -> Result<Vec<u8>, RpcError> {
    encode_json(&ControlEnvelope {
        version: PROTOCOL_VERSION,
        payload: bind,
    })
}

pub fn decode_session_bind(frame: &[u8]) -> Result<SessionBind, RpcError> {
    let envelope: ControlEnvelope<SessionBind> = decode_control(frame)?;
    Ok(envelope.payload)
}

pub fn encode_session_established(established: &SessionEstablished) -> Result<Vec<u8>, RpcError> {
    encode_json(&ControlEnvelope {
        version: PROTOCOL_VERSION,
        payload: established,
    })
}

pub fn decode_session_established(frame: &[u8]) -> Result<SessionEstablished, RpcError> {
    let envelope: ControlEnvelope<SessionEstablished> = decode_control(frame)?;
    Ok(envelope.payload)
}

pub fn encode_request(
    request: &LocalRequest,
    established: &SessionEstablished,
    counter: u64,
) -> Result<Vec<u8>, RpcError> {
    encode_json(&RequestEnvelope {
        version: PROTOCOL_VERSION,
        capability: established.capability.clone(),
        counter,
        request_id: request.request_id().to_owned(),
        host_id: established.host_id.clone(),
        tx_id: established.tx_id.clone(),
        epoch: established.epoch,
        request: request.clone(),
    })
}

pub fn decode_request(
    frame: &[u8],
    session: &mut SessionState,
) -> Result<AuthenticatedRequest, RpcError> {
    let decoded = decode_frame(frame).map_err(|err| {
        if err == RpcError::OversizeFrame
            && frame.len() < FRAME_HEADER_BYTES.saturating_add(MAX_FRAME_PAYLOAD_BYTES + 1)
        {
            RpcError::MalformedFrame
        } else {
            err
        }
    })?;
    let value = parse_json(&decoded.payload)?;
    check_version(&value)?;
    reject_unknown_top_level(
        &value,
        &[
            "version",
            "capability",
            "counter",
            "request_id",
            "host_id",
            "tx_id",
            "epoch",
            "request",
        ],
    )?;
    let envelope: RequestEnvelope =
        serde_json::from_value(value).map_err(|error| classify_json_error(&error))?;
    session.verify_authenticated(
        &envelope.capability,
        envelope.counter,
        &envelope.host_id,
        &envelope.tx_id,
        envelope.epoch,
    )?;
    if envelope.request_id != envelope.request.request_id() {
        return Err(RpcError::WrongBinding);
    }
    session.advance_counter(envelope.counter);
    Ok(AuthenticatedRequest {
        request: envelope.request,
        counter: envelope.counter,
    })
}

pub fn encode_response(
    response: &LocalResponse,
    request: &LocalRequest,
    established: &SessionEstablished,
    counter: u64,
) -> Result<Vec<u8>, RpcError> {
    encode_json(&ResponseEnvelope {
        version: PROTOCOL_VERSION,
        capability: established.capability.clone(),
        counter,
        request_id: request.request_id().to_owned(),
        host_id: established.host_id.clone(),
        tx_id: established.tx_id.clone(),
        epoch: established.epoch,
        response: response.clone(),
    })
}

pub fn decode_response(
    frame: &[u8],
    request: &LocalRequest,
    established: &SessionEstablished,
    counter: u64,
) -> Result<LocalResponse, RpcError> {
    let decoded = decode_frame(frame)?;
    let value = parse_json(&decoded.payload)?;
    check_version(&value)?;
    let envelope: ResponseEnvelope =
        serde_json::from_value(value).map_err(|error| classify_json_error(&error))?;
    if envelope.capability != established.capability
        || envelope.counter != counter
        || envelope.request_id != request.request_id()
        || envelope.host_id != established.host_id
        || envelope.tx_id != established.tx_id
        || envelope.epoch != established.epoch
    {
        return Err(RpcError::ResponseBindingMismatch);
    }
    Ok(envelope.response)
}

pub fn read_frame(reader: &mut impl Read) -> Result<Vec<u8>, RpcError> {
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    reader
        .read_exact(&mut header)
        .map_err(|error| RpcError::Transport(error.to_string()))?;
    let length = read_u32(&header, 0)? as usize;
    if length > MAX_FRAME_PAYLOAD_BYTES {
        return Err(RpcError::OversizeFrame);
    }
    let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES.saturating_add(length));
    frame.extend_from_slice(&header);
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|error| RpcError::Transport(error.to_string()))?;
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn decode_control<T>(frame: &[u8]) -> Result<ControlEnvelope<T>, RpcError>
where
    T: for<'de> Deserialize<'de>,
{
    let decoded = decode_frame(frame)?;
    let value = parse_json(&decoded.payload)?;
    check_version(&value)?;
    reject_unknown_top_level(&value, &["version", "payload"])?;
    serde_json::from_value(value).map_err(|error| classify_json_error(&error))
}

fn encode_json(value: &impl Serialize) -> Result<Vec<u8>, RpcError> {
    let payload = serde_json::to_vec(value).map_err(|_| RpcError::MalformedFrame)?;
    encode_frame(&payload)
}

fn parse_json(payload: &[u8]) -> Result<serde_json::Value, RpcError> {
    serde_json::from_slice(payload).map_err(|_| RpcError::MalformedFrame)
}

fn check_version(value: &serde_json::Value) -> Result<(), RpcError> {
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(version) if version == u64::from(PROTOCOL_VERSION) => Ok(()),
        Some(_) => Err(RpcError::UnknownVersion),
        None => Err(RpcError::MalformedFrame),
    }
}

fn reject_unknown_top_level(value: &serde_json::Value, allowed: &[&str]) -> Result<(), RpcError> {
    let object = value.as_object().ok_or(RpcError::MalformedFrame)?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(RpcError::DenyUnknown);
    }
    Ok(())
}

fn classify_json_error(error: &serde_json::Error) -> RpcError {
    if error.to_string().contains("unknown field") {
        RpcError::DenyUnknown
    } else {
        RpcError::MalformedFrame
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, RpcError> {
    let end = offset.checked_add(4).ok_or(RpcError::MalformedFrame)?;
    let array: [u8; 4] = bytes
        .get(offset..end)
        .ok_or(RpcError::MalformedFrame)?
        .try_into()
        .map_err(|_| RpcError::MalformedFrame)?;
    Ok(u32::from_be_bytes(array))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & mask);
        }
    }
    !crc
}
