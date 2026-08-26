//! AGB-8 record-semantics micro-RED — coordinator audit against local GREEN `bb042a4`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    dead_code
)]

mod common;

use agentbed_watchdogd::error::{RpcError, WatchdogError};
use agentbed_watchdogd::read_model::DecisionLogReader;
use agentbed_watchdogd::rpc::protocol::{
    decode_request, encode_request, LocalRequest, SessionBind,
};
use agentbed_watchdogd::{CoreConfig, SessionState, WatchdogCore};
use common::{
    dependencies_from, scratch_dir, valid_worker_group_tag, FakeBundle, FakePeerCred,
    DECISION_LOG_REL,
};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

const T0_SECS: u64 = 1_700_000_000;
const LEASE_POLICY_SECS: u64 = 3600;
const HARD_DEADLINE_OFFSET: u64 = 20_000;

fn core_config(dir: &Path) -> CoreConfig {
    CoreConfig {
        store_root: dir.join("store"),
        socket_path: dir.join("watchdog.sock"),
        broker_uid: 0,
        broker_gid: 0,
        host_id: "host-test".to_owned(),
    }
}

fn open_core(dir: &Path, bundle: &FakeBundle) -> WatchdogCore {
    WatchdogCore::open(core_config(dir), dependencies_from(bundle)).expect("open core")
}

fn bind_session(
    core: &WatchdogCore,
    bundle: &FakeBundle,
    tx: &str,
    epoch: u64,
    lease_id: &str,
    worker_group_tag: u32,
    client_nonce: &str,
) -> Result<
    (
        SessionState,
        agentbed_watchdogd::rpc::protocol::SessionEstablished,
    ),
    RpcError,
> {
    bundle
        .peer_cred
        .enqueue_cred(FakePeerCred::broker_cred(0, 0, 4242));
    let bind = SessionBind::new(
        "host-test",
        tx,
        epoch,
        lease_id,
        valid_worker_group_tag(worker_group_tag),
        client_nonce,
    );
    SessionState::bind(core, &bundle.peer_cred, &bundle.entropy, bind)
}

fn handle_authenticated(
    core: &mut WatchdogCore,
    session: &mut SessionState,
    established: &agentbed_watchdogd::rpc::protocol::SessionEstablished,
    counter: u64,
    req: LocalRequest,
) -> Result<agentbed_watchdogd::rpc::protocol::LocalResponse, RpcError> {
    let frame = encode_request(&req, established, counter)?;
    let verified = decode_request(&frame, session)?;
    core.handle_request(verified, session)
}

fn arm_request(
    req_id: &str,
    tx: &str,
    epoch: u64,
    base: &str,
    deadline: SystemTime,
) -> LocalRequest {
    LocalRequest::arm(
        req_id,
        "host-test",
        tx,
        epoch,
        base,
        deadline,
        vec!["route_present".to_owned()],
        vec![],
    )
}

fn write_framed_json_payload(path: &Path, payload: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    let length = u32::try_from(payload.len()).expect("length");
    let mut frame = Vec::new();
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&crc32(payload).to_be_bytes());
    frame.extend_from_slice(payload);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open");
    file.write_all(&frame).expect("write");
    file.sync_all().expect("sync");
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

fn armed_payload_template() -> serde_json::Value {
    let deadline_secs = T0_SECS + HARD_DEADLINE_OFFSET;
    let policy_lease_secs = T0_SECS + LEASE_POLICY_SECS;
    serde_json::json!({
        "sequence": 1u64,
        "epoch": 1u64,
        "kind": "Armed",
        "host_id": "host-test",
        "tx_id": "tx-schema",
        "base": "base-a",
        "lease_id": "lease1",
        "worker_group_tag": 100,
        "armed_at_secs": T0_SECS,
        "armed_at_nanos": 0,
        "deadline_secs": deadline_secs,
        "deadline_nanos": 0,
        "lease_expires_at_secs": policy_lease_secs,
        "lease_expires_at_nanos": 0,
    })
}

fn probe_reader_rejection(log_path: &Path) -> Result<(), std::io::Error> {
    DecisionLogReader::open(log_path)
        .map(|_| ())
        .map_err(|err| err)
}

// 1. Empty binding components must be rejected on read.
#[test]
fn record_semantics_rejects_empty_binding_components() {
    let dir = scratch_dir("record-semantics-empty-binding");
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    let cases = [
        ("empty_host_id", "host_id", ""),
        ("empty_tx_id", "tx_id", ""),
        ("empty_lease_id", "lease_id", ""),
    ];
    let mut accepted_invalid = Vec::new();
    let mut wrong_error_kind = Vec::new();
    for (name, field, value) in cases {
        if log_path.exists() {
            fs::remove_file(&log_path).expect("reset log");
        }
        let mut payload = armed_payload_template();
        payload.as_object_mut().expect("object").insert(
            field.to_owned(),
            serde_json::Value::String(value.to_owned()),
        );
        let bytes = serde_json::to_vec(&payload).expect("json");
        write_framed_json_payload(&log_path, &bytes);
        match probe_reader_rejection(&log_path) {
            Ok(reader) => accepted_invalid.push(format!("{name}: accepted {reader:?}")),
            Err(err) if err.kind() != ErrorKind::InvalidData => {
                wrong_error_kind.push(format!("{name}: {err:?}"));
            }
            Err(_) => {}
        }
    }
    assert!(
        accepted_invalid.is_empty() && wrong_error_kind.is_empty(),
        "reader must reject empty binding components; accepted-invalid: {accepted_invalid:?}; wrong error kinds: {wrong_error_kind:?}"
    );
}

// 2. Initial lease window must match frozen policy min(armed_at + 3600s, deadline).
#[test]
fn record_semantics_rejects_impossible_initial_lease_window() {
    let dir = scratch_dir("record-semantics-lease-window");
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    let cases = [
        (
            "lease_expires_after_deadline",
            T0_SECS + LEASE_POLICY_SECS + 1_000,
        ),
        ("lease_expires_not_policy_window", T0_SECS + 1_800),
    ];
    let mut accepted_invalid = Vec::new();
    let mut wrong_error_kind = Vec::new();
    for (name, lease_expires_at_secs) in cases {
        if log_path.exists() {
            fs::remove_file(&log_path).expect("reset log");
        }
        let mut payload = armed_payload_template();
        payload.as_object_mut().expect("object").insert(
            "lease_expires_at_secs".to_owned(),
            serde_json::Value::from(lease_expires_at_secs),
        );
        let bytes = serde_json::to_vec(&payload).expect("json");
        write_framed_json_payload(&log_path, &bytes);
        match probe_reader_rejection(&log_path) {
            Ok(reader) => accepted_invalid.push(format!("{name}: accepted {reader:?}")),
            Err(err) if err.kind() != ErrorKind::InvalidData => {
                wrong_error_kind.push(format!("{name}: {err:?}"));
            }
            Err(_) => {}
        }
    }
    assert!(
        accepted_invalid.is_empty() && wrong_error_kind.is_empty(),
        "reader must reject impossible initial lease window; accepted-invalid: {accepted_invalid:?}; wrong error kinds: {wrong_error_kind:?}"
    );
}

// 3. Reopen must reject inflated renewal expiry not matching policy extension.
#[test]
fn record_semantics_reopen_rejects_renewal_expiry_inflation() {
    let bundle = FakeBundle::new();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(T0_SECS);
    let deadline = t0 + Duration::from_secs(HARD_DEADLINE_OFFSET);
    let dir = scratch_dir("record-semantics-renewal-inflate");
    let store = core_config(&dir).store_root.clone();
    let log_path = store.join(DECISION_LOG_REL);
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bind_session(&core, &bundle, "tx-a", 1, "lease-a", 100, "nonce-a").expect("bind");
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request("req-arm-ren", "tx-a", 1, "base-a", deadline),
    )
    .expect("arm");
    drop(core);
    let renewed_at_secs = T0_SECS + 1_800;
    let inflated_expiry_secs = T0_SECS + 10_000;
    let renewal_payload = serde_json::json!({
        "sequence": 2u64,
        "epoch": 1u64,
        "kind": "LeaseRenewed",
        "host_id": "host-test",
        "tx_id": "tx-a",
        "lease_id": "lease-a",
        "worker_group_tag": 100,
        "renewed_at_secs": renewed_at_secs,
        "renewed_at_nanos": 0,
        "lease_expires_at_secs": inflated_expiry_secs,
        "lease_expires_at_nanos": 0,
    });
    let bytes = serde_json::to_vec(&renewal_payload).expect("json");
    write_framed_json_payload(&log_path, &bytes);
    let err = WatchdogCore::reopen(core_config(&dir), dependencies_from(&bundle))
        .expect_err("reopen must fail on inflated renewal expiry");
    assert!(
        matches!(err, WatchdogError::SafeModeActive),
        "expected SafeModeActive, got {err:?}"
    );
}
