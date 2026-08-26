//! AGB-8 scenario-verification review repair — discriminating RED tests only.
//!
//! Maps to findings F1–F11, F14, F15 from scenario verification NEEDS_FIXES at `522c6bc`.

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
use agentbed_watchdogd::interfaces::{Clock, InvariantOutcome};
use agentbed_watchdogd::read_model::{AuthorityRecordKind, DecisionLogReader};
use agentbed_watchdogd::rpc::protocol::{
    decode_request, encode_request, LocalRequest, LocalResponse, SessionBind,
};
use agentbed_watchdogd::{CoreConfig, SessionState, WatchdogCore};
use common::{
    dependencies_from, scratch_dir, valid_worker_group_tag, DurabilityOp, FakeBundle, FakePeerCred,
    DECISION_LOG_REL, EPOCH_HIGH_WATER_REL, SAFE_MODE_REL,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime};

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

fn bootstrap_session(
    core: &WatchdogCore,
    bundle: &FakeBundle,
    tx: &str,
    epoch: u64,
    lease_id: &str,
    worker_group_tag: u32,
) -> (
    SessionState,
    agentbed_watchdogd::rpc::protocol::SessionEstablished,
) {
    bundle
        .peer_cred
        .enqueue_cred(FakePeerCred::broker_cred(0, 0, 4242));
    let bind = SessionBind::new(
        "host-test",
        tx,
        epoch,
        lease_id,
        valid_worker_group_tag(worker_group_tag),
        "client-nonce-1",
    );
    SessionState::bind(core, &bundle.peer_cred, &bundle.entropy, bind).expect("bootstrap")
}

fn handle_authenticated(
    core: &mut WatchdogCore,
    session: &mut SessionState,
    established: &agentbed_watchdogd::rpc::protocol::SessionEstablished,
    counter: u64,
    req: LocalRequest,
) -> Result<LocalResponse, RpcError> {
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

fn write_framed_record(path: &Path, sequence: u64, epoch: u64, kind: AuthorityRecordKind) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    let payload = serde_json::json!({
        "sequence": sequence,
        "epoch": epoch,
        "kind": kind,
    });
    let payload = serde_json::to_vec(&payload).expect("json");
    let length = u32::try_from(payload.len()).expect("length");
    let mut frame = Vec::new();
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&crc32(&payload).to_be_bytes());
    frame.extend_from_slice(&payload);
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

fn write_epoch_file(store_root: &Path, epoch: u64) {
    let path = store_root.join(EPOCH_HIGH_WATER_REL);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("epoch parent");
    }
    fs::write(path, epoch.to_be_bytes()).expect("epoch write");
}

// --- F1 / F10 / F11: decision log durability contract ---

#[test]
fn review_f1_append_failure_must_not_truncate_log() {
    let src = include_str!("../src/read_model.rs");
    assert!(
        !src.contains("set_len(original_len)"),
        "append failure must not rewrite or truncate the decision log in place"
    );
}

#[test]
fn review_f10_decision_log_rejects_decreasing_epoch() {
    let dir = scratch_dir("review-f10");
    let store = core_config(&dir).store_root.clone();
    fs::create_dir_all(store.join("decisions")).expect("decisions dir");
    let log_path = store.join(DECISION_LOG_REL);
    write_framed_record(&log_path, 1, 5, AuthorityRecordKind::Armed);
    write_framed_record(&log_path, 2, 3, AuthorityRecordKind::BeginCommit);
    let err = DecisionLogReader::open(&log_path).expect_err("decreasing epoch");
    assert!(err.kind() == std::io::ErrorKind::InvalidData);
}

#[test]
fn review_f11_append_uses_no_follow_open_and_post_write_durability() {
    let read_model = include_str!("../src/read_model.rs");
    assert!(
        read_model.contains("O_NOFOLLOW") || read_model.contains("nofollow"),
        "decision log append must use no-follow open semantics"
    );
    let write_pos = read_model
        .find("write_all")
        .expect("write_all present in append path");
    let post_write_fsync = read_model[write_pos..]
        .find("durability.")
        .expect("post-write durability must use injected Durability seam");
    assert!(
        post_write_fsync < 400,
        "durability seam must follow the append write"
    );
}

// --- F2 / F3: safe-mode marker and epoch replacement ---

#[test]
fn review_f2_runtime_safe_mode_persists_durable_marker() {
    let bundle = FakeBundle::new();
    bundle.durability.fail_on(DurabilityOp::Readback);
    let dir = scratch_dir("review-f2");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-sm", 1, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-sm",
            "tx-sm",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("arm should fail closed");
    assert!(matches!(
        err,
        RpcError::SafeModeActive | RpcError::Durability(_)
    ));
    let marker = store.join(SAFE_MODE_REL);
    assert!(
        marker.exists(),
        "runtime safe-mode latch must persist {SAFE_MODE_REL}"
    );
    let reopen_err = WatchdogCore::reopen(core_config(&dir), dependencies_from(&bundle))
        .expect_err("reopen after durable safe mode");
    assert!(matches!(reopen_err, WatchdogError::SafeModeActive));
}

#[test]
fn review_f3_epoch_temp_must_be_unique_o_excl_and_refuse_stale_epoch() {
    let core_src = include_str!("../src/core.rs");
    assert!(
        core_src.contains("O_EXCL") || core_src.contains("o_excl"),
        "epoch replacement must use unique O_EXCL temp files"
    );
    assert!(
        !core_src.contains("epoch.max(read_epoch_file"),
        "stale epoch requests must be refused, not clamped upward"
    );
}

#[test]
fn review_f3_ambiguous_epoch_temp_enters_safe_mode() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("review-f3-temp");
    let store = core_config(&dir).store_root.clone();
    fs::create_dir_all(store.join("epoch")).expect("epoch dir");
    fs::write(store.join("epoch/.tmp-epoch"), b"stale").expect("stale temp");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-tmp", 2, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-tmp",
            "tx-tmp",
            2,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("ambiguous epoch temp");
    assert!(matches!(err, RpcError::SafeModeActive));
}

#[test]
fn review_f3_stale_epoch_below_high_water_refused_on_arm() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("review-f3-stale");
    let store = core_config(&dir).store_root.clone();
    fs::create_dir_all(&store).expect("store");
    write_epoch_file(&store, 10);
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-stale", 5, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-stale",
            "tx-stale",
            5,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("epoch below durable high-water");
    assert!(matches!(
        err,
        RpcError::StaleEpoch | RpcError::WrongEpoch | RpcError::SafeModeActive
    ));
}

// --- F4: production topology verifier ---

#[test]
fn review_f4_production_topology_verifier_exists() {
    let lib_rs = include_str!("../src/lib.rs");
    assert!(
        lib_rs.contains("mod topology"),
        "production topology verifier module must exist"
    );
    let topology_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/topology.rs");
    assert!(
        topology_path.exists(),
        "production topology verifier source file must exist"
    );
    let topology_src = fs::read_to_string(topology_path).expect("topology source");
    assert!(
        topology_src.contains("impl TopologyProbe"),
        "production crate must implement TopologyProbe"
    );
}

// --- F5: production fencing wired and fail-closed ---

#[test]
fn review_f5_production_fencer_implements_process_group_fence() {
    let fencing_src = include_str!("../src/fencing.rs");
    assert!(
        fencing_src.contains("impl ProcessGroupFence for UnavailableProcessGroupFencer"),
        "production fencer must be installable into Dependencies"
    );
}

#[test]
fn review_f5_fence_wait_failure_latches_safe_mode() {
    let bundle = FakeBundle::new();
    bundle.process_group.alive_after_term(true);
    bundle.process_group.alive_after_kill(false);
    bundle.process_group.fail_next_bounded_wait();
    let dir = scratch_dir("review-f5-wait");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-fence", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-fence",
            "tx-fence",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect("arm");
    bundle.clock.advance(Duration::from_secs(7200));
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-fence", "host-test", "tx-fence", 1),
    )
    .expect_err("fence wait failure");
    assert!(matches!(
        err,
        RpcError::FenceIncomplete | RpcError::SafeModeActive
    ));
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        3,
        LocalRequest::report_health("req-health", "host-test", "tx-fence", 1),
    )
    .expect_err("latched safe mode");
    assert!(matches!(err, RpcError::SafeModeActive));
}

// --- F6 / F14: arming conflicts and manifest strictness ---

#[test]
fn review_f6_conflicting_arm_while_other_tx_armed_refused() {
    let core_src = include_str!("../src/core.rs");
    assert!(
        !core_src.contains("some_and(|a| a.tx_id == tx_id)"),
        "arming must refuse while any other transaction remains armed, not only duplicate tx_id"
    );
}

#[test]
fn review_f14_additive_manifest_checks_enforced() {
    let core_src = include_str!("../src/core.rs");
    assert!(
        !core_src.contains("additive_manifest_checks: _"),
        "additive manifest checks must be validated, not ignored"
    );
}

// --- F7 / F15: decision-time base/deadline and late renewal ---

#[test]
fn review_f7_moved_base_rejected_at_decision_time() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Pass));
    let dir = scratch_dir("review-f7-base");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-base", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-base",
            "tx-base",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect("arm");
    bundle.base_revision.set_observed("base-b");
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-base", "host-test", "tx-base", 1),
    )
    .expect_err("moved base at decision");
    assert!(matches!(err, RpcError::MovedBase));
}

#[test]
fn review_f7_expired_arming_deadline_rejected_at_decision() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Pass));
    let dir = scratch_dir("review-f7-deadline");
    let mut core = open_core(&dir, &bundle);
    let deadline = bundle.clock.now() + Duration::from_secs(5);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-dead", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request("req-arm-dead", "tx-dead", 1, "base-a", deadline),
    )
    .expect("arm");
    bundle.clock.advance(Duration::from_secs(10));
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-dead", "host-test", "tx-dead", 1),
    )
    .expect_err("deadline at decision");
    assert!(matches!(err, RpcError::ExpiredDeadline));
}

#[test]
fn review_f15_late_lease_renewal_rejected() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("review-f15-renewal");
    let mut core = open_core(&dir, &bundle);
    let deadline = bundle.clock.now() + Duration::from_secs(5);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-renew", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request("req-arm-renew", "tx-renew", 1, "base-a", deadline),
    )
    .expect("arm");
    bundle.clock.advance(Duration::from_secs(10));
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_lease_renewal(
            "req-renew",
            "host-test",
            "tx-renew",
            1,
            "lease1",
            valid_worker_group_tag(100),
            1,
        ),
    )
    .expect_err("late renewal");
    assert!(matches!(err, RpcError::ExpiredDeadline));
}

// --- F8 / F9: RPC authentication and epoch authority ---

#[test]
fn review_f8_rpc_server_sets_socket_deadlines() {
    let server_src = include_str!("../src/rpc/server.rs");
    assert!(
        server_src.contains("set_read_timeout") && server_src.contains("set_write_timeout"),
        "RPC server must set read/write deadlines on accepted streams"
    );
}

#[test]
fn review_f8_capability_bound_to_peer_credentials() {
    let session_src = include_str!("../src/session.rs");
    assert!(
        session_src.contains("pid") && session_src.contains("capability"),
        "capability minting must bind peer pid/uid/gid into the session tuple"
    );
    let server_src = include_str!("../src/rpc/server.rs");
    assert!(
        !server_src.contains("deps.peer_cred"),
        "session bind must authenticate the accepted stream peer, not only injected deps"
    );
}

#[test]
fn review_f9_arm_epoch_must_match_durable_high_water_not_session_only() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("review-f9-epoch");
    let store = core_config(&dir).store_root.clone();
    fs::create_dir_all(&store).expect("store");
    write_epoch_file(&store, 8);
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-epoch", 3, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-epoch",
            "tx-epoch",
            3,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("caller-selected epoch below durable high-water");
    assert!(matches!(
        err,
        RpcError::StaleEpoch | RpcError::WrongEpoch | RpcError::SafeModeActive
    ));
}

// --- F15: corrupt log at decision time ---

#[test]
fn review_f15_corrupt_log_at_decision_enters_safe_mode() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Pass));
    let dir = scratch_dir("review-f15-corrupt");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-corrupt", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-corrupt",
            "tx-corrupt",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect("arm");
    let log_path = store.join(DECISION_LOG_REL);
    let mut file = OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("open log");
    file.write_all(b"CORRUPT").expect("corrupt tail");
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-corrupt", "host-test", "tx-corrupt", 1),
    )
    .expect_err("corrupt log at decision");
    assert!(matches!(err, RpcError::SafeModeActive));
}
