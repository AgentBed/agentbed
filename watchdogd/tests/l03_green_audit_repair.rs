//! AGB-8 convergence micro-RED — coordinator audit against local GREEN `89582ae`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    dead_code
)]

mod common;

use agentbed_watchdogd::error::RpcError;
use agentbed_watchdogd::interfaces::{Clock, InvariantOutcome};
use agentbed_watchdogd::read_model::{AuthorityRecordKind, DecisionLogReader};
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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

fn future_deadline(clock: &FakeBundle, seconds: u64) -> SystemTime {
    clock
        .clock
        .now()
        .checked_add(Duration::from_secs(seconds))
        .expect("future deadline")
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

fn lease_renewed_count(store: &Path) -> usize {
    let reader = DecisionLogReader::open(store.join(DECISION_LOG_REL)).expect("reader");
    usize::from(reader.contains_kind(AuthorityRecordKind::LeaseRenewed))
}

fn armed_record_count(store: &Path) -> usize {
    let reader = DecisionLogReader::open(store.join(DECISION_LOG_REL)).expect("reader");
    usize::from(reader.contains_kind(AuthorityRecordKind::Armed))
}

fn begin_authority_count(store: &Path) -> usize {
    let reader = DecisionLogReader::open(store.join(DECISION_LOG_REL)).expect("reader");
    usize::from(reader.contains_kind(AuthorityRecordKind::BeginCommit))
        + usize::from(reader.contains_kind(AuthorityRecordKind::BeginRevert))
}

// 1. Public `open` must reconstruct durable history.
#[test]
fn green_audit_open_reconstructs_durable_binding() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("green-open-reconstruct");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bind_session(&core, &bundle, "tx-a", 1, "lease-a", 100, "nonce-a").expect("bind a");
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-open",
            "tx-a",
            1,
            "base-a",
            future_deadline(&bundle, 10_000),
        ),
    )
    .expect("arm");
    drop(core);
    let reopened = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle))
        .expect("public open on nonempty store");
    let err = bind_session(&reopened, &bundle, "tx-b", 1, "lease-b", 200, "nonce-b")
        .expect_err("conflicting bind after public open");
    assert!(
        matches!(err, RpcError::StaleReconnect),
        "expected StaleReconnect, got {err:?}"
    );
    bind_session(&reopened, &bundle, "tx-a", 1, "lease-a", 100, "nonce-a2")
        .expect("exact binding reconnect");
    assert_eq!(armed_record_count(&store), 1);
}

// 2. Renewal restart preserves observation time, not expiry as clock high-water.
#[test]
fn green_audit_reopen_preserves_renewal_observation_time() {
    let bundle = FakeBundle::new();
    let t0 = bundle.clock.now();
    let dir = scratch_dir("green-renewal-obs");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bind_session(&core, &bundle, "tx-ren", 1, "lease1", 100, "nonce-ren").expect("bind");
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-ren",
            "tx-ren",
            1,
            "base-a",
            t0.checked_add(Duration::from_secs(20_000))
                .expect("deadline"),
        ),
    )
    .expect("arm");
    bundle.clock.advance(Duration::from_secs(1800));
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_lease_renewal(
            "req-ren-1",
            "host-test",
            "tx-ren",
            1,
            "lease1",
            valid_worker_group_tag(100),
            1,
        ),
    )
    .expect("first renewal");
    assert_eq!(lease_renewed_count(&store), 1);
    drop(core);
    let mut core =
        WatchdogCore::reopen(core_config(&dir), dependencies_from(&bundle)).expect("reopen");
    let (mut session2, established2) =
        bind_session(&core, &bundle, "tx-ren", 1, "lease1", 100, "nonce-ren2").expect("rebind");
    bundle.clock.advance(Duration::from_secs(30));
    handle_authenticated(
        &mut core,
        &mut session2,
        &established2,
        1,
        LocalRequest::request_lease_renewal(
            "req-ren-2",
            "host-test",
            "tx-ren",
            1,
            "lease1",
            valid_worker_group_tag(100),
            2,
        ),
    )
    .expect("second renewal after reopen");
    assert_eq!(
        lease_renewed_count(&store),
        2,
        "must append second durable LeaseRenewed"
    );
    assert!(
        bundle.fence_trace.snapshot().is_empty(),
        "renewal must not invoke fencing"
    );
}

// 3. Decision rejects clock regression before evaluation/append.
#[test]
fn green_audit_decision_rejects_clock_regression() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Pass));
    let dir = scratch_dir("green-decision-clock");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bind_session(&core, &bundle, "tx-clock", 1, "lease1", 100, "nonce-clock").expect("bind");
    let t0 = bundle.clock.now();
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-clock",
            "tx-clock",
            1,
            "base-a",
            t0.checked_add(Duration::from_secs(10_000))
                .expect("deadline"),
        ),
    )
    .expect("arm");
    bundle.clock.set(
        t0.checked_sub(Duration::from_secs(5))
            .expect("pre-arm time"),
    );
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-clock", "host-test", "tx-clock", 1),
    )
    .expect_err("clock regression at decision");
    assert!(
        matches!(err, RpcError::ClockRegression),
        "expected ClockRegression, got {err:?}"
    );
    assert_eq!(begin_authority_count(&store), 0);
    assert!(bundle.fence_trace.snapshot().is_empty());
}

// 4. Pre-UNIX authority time fails closed.
#[test]
fn green_audit_pre_unix_arm_time_fails_closed() {
    let bundle = FakeBundle::new();
    let pre_unix = UNIX_EPOCH
        .checked_sub(Duration::from_secs(1))
        .expect("pre-unix instant");
    bundle.clock.set(pre_unix);
    let dir = scratch_dir("green-pre-unix");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bind_session(&core, &bundle, "tx-pre", 1, "lease1", 100, "nonce-pre").expect("bind");
    let deadline = pre_unix
        .checked_add(Duration::from_secs(3600))
        .expect("future deadline");
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request("req-arm-pre", "tx-pre", 1, "base-a", deadline),
    )
    .expect_err("pre-unix arm time must fail closed");
    assert!(
        matches!(err, RpcError::SafeModeActive),
        "expected SafeModeActive, got {err:?}"
    );
    assert_eq!(armed_record_count(&store), 0);
    let latch = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::report_health("req-health", "host-test", "tx-pre", 1),
    )
    .expect_err("safe-mode latch");
    assert!(matches!(latch, RpcError::SafeModeActive));
}

// 5. Strict per-kind schema and timestamp validation on read.
#[test]
fn green_audit_reader_rejects_invalid_record_schema() {
    let dir = scratch_dir("green-schema");
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    let cases = [
        (
            "armed_invalid_nanoseconds",
            serde_json::json!({
                "sequence": 1u64,
                "epoch": 1u64,
                "kind": "Armed",
                "host_id": "host-test",
                "tx_id": "tx-schema",
                "base": "base-a",
                "lease_id": "lease1",
                "worker_group_tag": 100,
                "armed_at_secs": 1_700_000_000u64,
                "armed_at_nanos": 1_000_000_000u32,
                "deadline_secs": 1_800_000_000u64,
                "deadline_nanos": 0,
                "lease_expires_at_secs": 1_700_003_600u64,
                "lease_expires_at_nanos": 0,
            }),
        ),
        (
            "lease_renewed_forbidden_armed_fields",
            serde_json::json!({
                "sequence": 1u64,
                "epoch": 1u64,
                "kind": "LeaseRenewed",
                "host_id": "host-test",
                "tx_id": "tx-schema",
                "lease_id": "lease1",
                "worker_group_tag": 100,
                "base": "base-a",
                "deadline_secs": 1_800_000_000u64,
                "deadline_nanos": 0,
                "lease_expires_at_secs": 1_700_003_600u64,
                "lease_expires_at_nanos": 0,
            }),
        ),
    ];
    for (name, value) in cases {
        if log_path.exists() {
            fs::remove_file(&log_path).expect("reset log");
        }
        let payload = serde_json::to_vec(&value).expect("json");
        write_framed_json_payload(&log_path, &payload);
        let open_result = DecisionLogReader::open(&log_path);
        assert!(
            open_result.is_err(),
            "{name}: reader must reject invalid schema, got {open_result:?}"
        );
        if let Err(err) = open_result {
            assert_eq!(
                err.kind(),
                ErrorKind::InvalidData,
                "{name}: expected InvalidData, got {err:?}"
            );
        }
    }
}
