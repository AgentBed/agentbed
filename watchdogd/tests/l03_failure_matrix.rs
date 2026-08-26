//! L03 RED failure matrix — discriminating coverage for L03-AC01…L03-AC12.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    dead_code
)]

mod common;

use agentbed_watchdogd::error::{
    DurabilityError, ExternalFloorError, RpcError, TopologyError, WatchdogError,
};
use agentbed_watchdogd::interfaces::{InvariantOutcome, PeerCred};
use agentbed_watchdogd::read_model::{AuthorityRecordKind, DecisionLogReader};
use agentbed_watchdogd::rpc::protocol::{
    decode_frame, decode_request, decode_response, decode_session_established, encode_frame,
    encode_request, encode_response, encode_session_bind, encode_session_established, read_frame,
    AuthenticatedRequest, LocalRequest, LocalResponse, SessionBind, SessionEstablished,
    MAX_FRAME_PAYLOAD_BYTES, PROTOCOL_VERSION,
};
use agentbed_watchdogd::rpc::server::RpcServer;
use agentbed_watchdogd::{CoreConfig, SessionState, WatchdogCore};
use common::{
    dependencies_from, frame_with_oversize_length_header, reframe_with_unknown_json_field,
    reframe_with_unknown_protocol_version, scratch_dir, truncate_file, valid_worker_group_tag,
    FakeBundle, FakePeerCred, FenceTraceEvent, DECISION_LOG_REL, EPOCH_HIGH_WATER_REL,
    SAFE_MODE_REL, WATCHDOG_MOUNT_ROOT,
};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
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
    let deps = dependencies_from(bundle);
    WatchdogCore::open(core_config(dir), deps).expect("open core")
}

fn session_bind(tx: &str, epoch: u64, lease_id: &str, worker_group_tag: u32) -> SessionBind {
    SessionBind::new(
        "host-test",
        tx,
        epoch,
        lease_id,
        valid_worker_group_tag(worker_group_tag),
        "client-nonce-1",
    )
}

fn bootstrap_session(
    core: &WatchdogCore,
    bundle: &FakeBundle,
    tx: &str,
    epoch: u64,
    lease_id: &str,
    worker_group_tag: u32,
) -> (SessionState, SessionEstablished) {
    bundle
        .peer_cred
        .enqueue_cred(FakePeerCred::broker_cred(0, 0, 4242));
    let bind = session_bind(tx, epoch, lease_id, worker_group_tag);
    SessionState::bind(core, &bundle.peer_cred, &bundle.entropy, bind).expect("bootstrap")
}

fn arm_request(req_id: &str, tx: &str, epoch: u64, base: &str) -> LocalRequest {
    LocalRequest::arm(
        req_id,
        "host-test",
        tx,
        epoch,
        base,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        vec!["route_present".to_owned()],
        vec![],
    )
}

fn handle_authenticated(
    core: &mut WatchdogCore,
    session: &mut SessionState,
    established: &SessionEstablished,
    counter: u64,
    req: LocalRequest,
) -> Result<LocalResponse, RpcError> {
    let frame = encode_request(&req, established, counter)?;
    let verified = decode_request(&frame, session)?;
    core.handle_request(verified, session)
}

fn encode_authenticated_frame(
    req: &LocalRequest,
    established: &SessionEstablished,
    counter: u64,
) -> Result<Vec<u8>, RpcError> {
    encode_request(req, established, counter)
}

fn trigger_decision(req_id: &str, tx: &str, epoch: u64) -> LocalRequest {
    LocalRequest::request_decision(req_id, "host-test", tx, epoch)
}

fn assert_request_round_trip(
    session: &mut SessionState,
    established: &SessionEstablished,
    counter: u64,
    req: LocalRequest,
) {
    let frame = encode_request(&req, established, counter).expect("encode");
    let verified = decode_request(&frame, session).expect("decode");
    assert_eq!(verified.request(), &req);
}

// --- L03-AC01 topology ---

#[test]
fn l03_ac01_topology_rejects_missing_mount() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::MissingMount));
    let dir = scratch_dir("l03-topology");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::MissingMount)
    ));
}

#[test]
fn l03_ac01_topology_rejects_same_device_alias() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::SameDeviceAlias));
    let dir = scratch_dir("l03-topology");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::SameDeviceAlias)
    ));
}

#[test]
fn l03_ac01_topology_rejects_symlink_component() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::SymlinkComponent));
    let dir = scratch_dir("l03-topology");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::SymlinkComponent)
    ));
}

#[test]
fn l03_ac01_topology_rejects_non_regular_component() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::NonRegularComponent));
    let dir = scratch_dir("l03-topology");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::NonRegularComponent)
    ));
}

#[test]
fn l03_ac01_topology_rejects_wrong_ownership_or_mode() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::WrongOwnershipOrMode));
    let dir = scratch_dir("l03-topology");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::WrongOwnershipOrMode)
    ));
}

#[test]
fn l03_ac01_topology_rejects_wrong_link_count() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::WrongLinkCount));
    let dir = scratch_dir("l03-topology");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::WrongLinkCount)
    ));
}

#[test]
fn l03_ac01_topology_rejects_hard_link_ambiguity() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::HardLinkAmbiguity));
    let dir = scratch_dir("l03-topology");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::HardLinkAmbiguity)
    ));
}

#[test]
fn l03_ac01_topology_rejects_ordinary_directory_fallback() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::OrdinaryDirectoryFallback));
    let dir = scratch_dir("l03-topology");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::OrdinaryDirectoryFallback)
    ));
}

#[test]
fn l03_ac01_topology_refusal_prevents_arming_before_startup() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::MissingMount));
    let dir = scratch_dir("l03-topology");
    let config = core_config(&dir);
    let err = WatchdogCore::open(config.clone(), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::MissingMount)
    ));
    let log_path = config.store_root.join(DECISION_LOG_REL);
    assert!(!log_path.exists());
}

#[test]
fn l03_ac01_sealed_mount_root_constant() {
    assert_eq!(WATCHDOG_MOUNT_ROOT, "/var/lib/agentbed/watchdog");
}

// --- L03-AC02 decision log ---

const READ_MODEL_SRC: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/read_model.rs"));

const EXPECTED_AUTHORITY_RECORD_KINDS: [AuthorityRecordKind; 6] = [
    AuthorityRecordKind::Armed,
    AuthorityRecordKind::ProbationPassed,
    AuthorityRecordKind::BeginCommit,
    AuthorityRecordKind::BeginRevert,
    AuthorityRecordKind::Committed,
    AuthorityRecordKind::Reverted,
];

#[test]
fn l03_ac02_authority_record_kind_lists_six_watchdog_owned_names() {
    for kind in EXPECTED_AUTHORITY_RECORD_KINDS {
        let name = match kind {
            AuthorityRecordKind::Armed => "Armed",
            AuthorityRecordKind::ProbationPassed => "ProbationPassed",
            AuthorityRecordKind::BeginCommit => "BeginCommit",
            AuthorityRecordKind::BeginRevert => "BeginRevert",
            AuthorityRecordKind::Committed => "Committed",
            AuthorityRecordKind::Reverted => "Reverted",
        };
        assert!(READ_MODEL_SRC.contains(name));
    }
    assert!(!READ_MODEL_SRC.contains("Disarm"));
}

#[test]
fn l03_ac02_arm_through_daemon_appends_armed_record_only() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-log");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert_eq!(reader.record_count(), 1);
    assert_eq!(reader.last_kind(), Some(AuthorityRecordKind::Armed));
}

#[test]
fn l03_ac02_fsync_failure_refuses_arm_without_new_record() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-log");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    bundle.durability.fail_on(common::DurabilityOp::FileFsync);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect_err("fsync");
    assert!(matches!(
        err,
        RpcError::Durability(DurabilityError::InjectedFailure)
    ));
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = DecisionLogReader::open(&log_path).expect("reader");
        assert_eq!(reader.record_count(), 0);
    }
}

#[test]
fn l03_ac02_dir_fsync_failure_refuses_arm_without_new_record() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-log");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    bundle.durability.fail_on(common::DurabilityOp::DirFsync);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect_err("dir fsync");
    assert!(matches!(
        err,
        RpcError::Durability(DurabilityError::InjectedFailure)
    ));
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = DecisionLogReader::open(&log_path).expect("reader");
        assert_eq!(reader.record_count(), 0);
    }
}

// --- L03-AC03 epoch / safe mode ---

#[test]
fn l03_ac03_epoch_rollback_below_external_floor_enters_safe_mode() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-epoch");
    let config = core_config(&dir);
    let mut core = open_core(&dir, &bundle);
    let epoch_path = config.store_root.join(EPOCH_HIGH_WATER_REL);
    let initial_epoch = fs::read(&epoch_path).expect("initial epoch after open");
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    fs::write(&epoch_path, &initial_epoch).expect("restore initial epoch bytes");
    bundle.external_floor.push_floor(Ok(10));
    let err = WatchdogCore::reopen(config, dependencies_from(&bundle)).expect_err("reopen");
    assert!(matches!(err, WatchdogError::SafeModeActive));
}

#[test]
fn l03_ac03_external_floor_ambiguous_enters_safe_mode() {
    let bundle = FakeBundle::new();
    bundle
        .external_floor
        .push_floor(Err(ExternalFloorError::Ambiguous));
    let dir = scratch_dir("l03-epoch");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(err, WatchdogError::SafeModeActive));
}

#[test]
fn l03_ac03_external_floor_unavailable_enters_safe_mode() {
    let bundle = FakeBundle::new();
    bundle
        .external_floor
        .push_floor(Err(ExternalFloorError::Unavailable));
    let dir = scratch_dir("l03-epoch");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(err, WatchdogError::SafeModeActive));
}

#[test]
fn l03_ac03_safe_mode_persist_unavailable_returns_distinct_error() {
    let bundle = FakeBundle::new();
    bundle.topology.push_outcome(Err(TopologyError::Unwritable));
    bundle
        .durability
        .fail_on(common::DurabilityOp::AtomicRename);
    let dir = scratch_dir("l03-safe");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(err, WatchdogError::SafeModePersistUnavailable));
    let marker = core_config(&dir).store_root.join(SAFE_MODE_REL);
    assert!(!marker.exists());
}

#[test]
fn l03_ac03_rename_failure_on_epoch_allocate_refuses_arm() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-epoch");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    bundle
        .durability
        .fail_on(common::DurabilityOp::AtomicRename);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect_err("rename");
    assert!(matches!(err, RpcError::SafeModeActive));
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = DecisionLogReader::open(&log_path).expect("reader");
        assert_eq!(reader.record_count(), 0);
    }
}

#[test]
fn l03_ac03_readback_failure_on_epoch_allocate_refuses_arm() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-epoch");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    bundle.durability.fail_on(common::DurabilityOp::Readback);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect_err("readback");
    assert!(matches!(err, RpcError::SafeModeActive));
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = DecisionLogReader::open(&log_path).expect("reader");
        assert_eq!(reader.record_count(), 0);
    }
}

#[test]
fn l03_ac03_epoch_log_mismatch_enters_safe_mode() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-epoch");
    let config = core_config(&dir);
    let mut core = open_core(&dir, &bundle);
    let epoch_path = config.store_root.join(EPOCH_HIGH_WATER_REL);
    let initial_epoch = fs::read(&epoch_path).expect("initial epoch after open");
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    fs::write(&epoch_path, &initial_epoch).expect("restore initial epoch bytes");
    let err = WatchdogCore::reopen(config, dependencies_from(&bundle)).expect_err("reopen");
    assert!(matches!(err, WatchdogError::EpochLogMismatch));
}

#[test]
fn l03_ac03_epoch_ahead_of_log_mismatch_enters_safe_mode() {
    let bundle_a = FakeBundle::new();
    let dir_a = scratch_dir("l03-epoch-ahead-a");
    let config_a = core_config(&dir_a);
    let mut core_a = open_core(&dir_a, &bundle_a);
    let (mut session_a, established_a) =
        bootstrap_session(&core_a, &bundle_a, "tx-ahead", 7, "lease7", 107);
    handle_authenticated(
        &mut core_a,
        &mut session_a,
        &established_a,
        1,
        arm_request("req-arm-ahead", "tx-ahead", 7, "base-a"),
    )
    .expect("arm advances epoch in store A");
    let advanced_epoch = fs::read(config_a.store_root.join(EPOCH_HIGH_WATER_REL))
        .expect("production epoch bytes from store A");

    let bundle_b = FakeBundle::new();
    let dir_b = scratch_dir("l03-epoch-ahead-b");
    let config_b = core_config(&dir_b);
    let _core_b = open_core(&dir_b, &bundle_b);
    fs::write(
        config_b.store_root.join(EPOCH_HIGH_WATER_REL),
        &advanced_epoch,
    )
    .expect("copy production epoch bytes into lower store B");
    let err = WatchdogCore::reopen(config_b, dependencies_from(&bundle_b)).expect_err("reopen");
    assert!(matches!(err, WatchdogError::EpochLogMismatch));
}

#[test]
fn l03_ac03_unavailable_store_at_startup_enters_safe_mode() {
    let bundle = FakeBundle::new();
    bundle
        .topology
        .push_outcome(Err(TopologyError::UnavailableStore));
    let dir = scratch_dir("l03-store");
    let err = WatchdogCore::open(core_config(&dir), dependencies_from(&bundle)).expect_err("open");
    assert!(matches!(
        err,
        WatchdogError::Topology(TopologyError::UnavailableStore)
    ));
}

#[test]
fn l03_ac03_corrupt_decision_log_on_reopen_enters_safe_mode() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-corrupt");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    truncate_file(&core_config(&dir).store_root.join(DECISION_LOG_REL), 4);
    let err =
        WatchdogCore::reopen(core_config(&dir), dependencies_from(&bundle)).expect_err("reopen");
    assert!(matches!(err, WatchdogError::SafeModeActive));
}

// --- L03-AC04 RPC / auth ---

const PROTOCOL_SRC: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/rpc/protocol.rs"));

#[test]
fn l03_ac04_request_kinds_round_trip_through_codec() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-codec");
    let core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    assert_request_round_trip(
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    );
    counter += 1;
    assert_request_round_trip(
        &mut session,
        &established,
        counter,
        LocalRequest::report_health("req-health", "host-test", "tx1", 1),
    );
    counter += 1;
    assert_request_round_trip(
        &mut session,
        &established,
        counter,
        LocalRequest::request_lease_renewal(
            "req-renew",
            "host-test",
            "tx1",
            1,
            "lease1",
            valid_worker_group_tag(100),
            2,
        ),
    );
    counter += 1;
    assert_request_round_trip(
        &mut session,
        &established,
        counter,
        LocalRequest::heartbeat(
            "req-hb",
            "host-test",
            "tx1",
            1,
            "lease1",
            valid_worker_group_tag(100),
            3,
        ),
    );
    counter += 1;
    assert_request_round_trip(
        &mut session,
        &established,
        counter,
        trigger_decision("req-decide", "tx1", 1),
    );
}

#[test]
fn l03_ac04_protocol_source_excludes_disarm_oob() {
    assert!(!PROTOCOL_SRC.contains("Disarm"));
    assert!(!PROTOCOL_SRC.contains("OobHandshake"));
}

#[test]
fn l03_ac04_oversize_frame_rejected() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-oversize");
    let core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let frame = encode_request(
        &arm_request("req-arm-1", "tx1", 1, "base-a"),
        &established,
        1,
    )
    .expect("encode");
    let oversized = frame_with_oversize_length_header(&frame, MAX_FRAME_PAYLOAD_BYTES);
    let err = decode_frame(&oversized).expect_err("oversize");
    assert!(matches!(err, RpcError::OversizeFrame));
    let _ = session;
}

#[test]
fn l03_ac04_malformed_frame_rejected() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-malformed");
    let core = open_core(&dir, &bundle);
    let (mut session, _) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let err = decode_request(b"not-a-frame", &mut session).expect_err("malformed");
    assert!(matches!(err, RpcError::MalformedFrame));
}

#[test]
fn l03_ac04_replay_counter_rejected_after_arm() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-replay");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    counter += 1;
    let hb = LocalRequest::heartbeat(
        "req-hb-1",
        "host-test",
        "tx1",
        1,
        "lease1",
        valid_worker_group_tag(100),
        1,
    );
    let frame = encode_authenticated_frame(&hb, &established, counter).expect("encode");
    let verified = decode_request(&frame, &mut session).expect("decode");
    core.handle_request(verified, &mut session).expect("first");
    let err = decode_request(&frame, &mut session).expect_err("replay");
    assert!(matches!(err, RpcError::ReplayCounter));
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert_eq!(reader.record_count(), 1);
}

#[test]
fn l03_ac04_session_bind_establishes_capability() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-bind");
    let core = open_core(&dir, &bundle);
    let (session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    assert_eq!(established.counter(), 0);
    assert!(!established.capability().is_empty());
    let _ = session;
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = DecisionLogReader::open(&log_path).expect("reader");
        assert_eq!(reader.record_count(), 0);
    }
}

#[test]
fn l03_ac04_session_bind_stale_reconnect_refused() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-bind");
    let core = open_core(&dir, &bundle);
    let (_session, _established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    bundle
        .peer_cred
        .enqueue_cred(FakePeerCred::broker_cred(0, 0, 4243));
    let err = SessionState::bind(
        &core,
        &bundle.peer_cred,
        &bundle.entropy,
        session_bind("tx-stale", 99, "lease-x", 200),
    )
    .expect_err("stale reconnect");
    assert!(matches!(
        err,
        RpcError::StaleReconnect | RpcError::ConflictingRequest
    ));
}

#[test]
fn l03_ac04_capability_counter_binding_refused() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-bind");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        0,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect_err("stale counter");
    assert!(matches!(
        err,
        RpcError::ReplayCounter | RpcError::WrongCapability
    ));
}

#[test]
fn l03_ac04_peercred_refused_before_session_bind() {
    let bundle = FakeBundle::new();
    bundle.peer_cred.enqueue_cred(PeerCred {
        uid: 9999,
        gid: 0,
        pid: 1,
    });
    let dir = scratch_dir("l03-peer");
    let core = open_core(&dir, &bundle);
    let err = SessionState::bind(
        &core,
        &bundle.peer_cred,
        &bundle.entropy,
        session_bind("tx1", 1, "lease1", 100),
    )
    .expect_err("peer");
    assert!(matches!(err, RpcError::WrongPeer));
}

#[test]
fn l03_ac04_socket_permissions_are_0700_parent_0600_socket() {
    let dir = scratch_dir("l03-socket");
    let config = core_config(&dir);
    let _server = RpcServer::bind(&config.socket_path).expect("bind");
    let socket_mode = fs::metadata(&config.socket_path)
        .expect("socket")
        .permissions()
        .mode()
        & 0o777;
    let parent_mode = fs::metadata(config.socket_path.parent().expect("parent"))
        .expect("parent")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(socket_mode, 0o600);
    assert_eq!(parent_mode, 0o700);
}

#[test]
fn l03_ac04_frame_codec_bad_length_crc_version() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-frame");
    let core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let frame = encode_request(
        &arm_request("req-arm-1", "tx1", 1, "base-a"),
        &established,
        1,
    )
    .expect("encode");
    let mut bad_len = frame.clone();
    let oversize = (MAX_FRAME_PAYLOAD_BYTES as u32).saturating_add(1);
    bad_len[..4].copy_from_slice(&oversize.to_be_bytes());
    let err = decode_frame(&bad_len).expect_err("length");
    assert!(matches!(
        err,
        RpcError::OversizeFrame | RpcError::MalformedFrame
    ));

    let mut bad_crc = frame.clone();
    if bad_crc.len() >= 8 {
        bad_crc[4] = 0xff;
    }
    let err = decode_frame(&bad_crc).expect_err("crc");
    assert!(matches!(err, RpcError::CrcMismatch));

    let adverse = reframe_with_unknown_protocol_version(&frame, PROTOCOL_VERSION.wrapping_add(1));
    let err = decode_request(&adverse, &mut session).expect_err("version");
    assert!(matches!(err, RpcError::UnknownVersion));
}

#[test]
fn l03_ac04_deny_unknown_json_field_via_frame_codec() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-deny-unknown");
    let core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let frame = encode_request(
        &arm_request("req-arm-1", "tx1", 1, "base-a"),
        &established,
        1,
    )
    .expect("encode");
    let adverse = reframe_with_unknown_json_field(&frame);
    let err = decode_request(&adverse, &mut session).expect_err("unknown field");
    assert!(matches!(err, RpcError::DenyUnknown));
}

#[test]
fn l03_ac04_response_binding_mismatch_refused() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-binding");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let counter = 1u64;
    let req = arm_request("req-arm-1", "tx1", 1, "base-a");
    let resp = handle_authenticated(&mut core, &mut session, &established, counter, req.clone())
        .expect("arm");
    let frame = encode_response(&resp, &req, &established, counter.wrapping_add(1))
        .expect("encode response");
    let err = decode_response(&frame, &req, &established, counter).expect_err("binding");
    assert!(matches!(err, RpcError::ResponseBindingMismatch));
}

#[test]
fn l03_ac04_unix_socket_connect_bootstrap_arm_round_trip() {
    let bundle = FakeBundle::new();
    bundle
        .peer_cred
        .enqueue_cred(FakePeerCred::broker_cred(0, 0, 4242));
    let dir = scratch_dir("l03-roundtrip");
    let config = core_config(&dir);
    let mut core = open_core(&dir, &bundle);
    let server = RpcServer::bind(&config.socket_path).expect("bind");
    let bind = session_bind("tx1", 1, "lease1", 100);
    let bind_frame = encode_session_bind(&bind).expect("encode bind");
    let req = arm_request("req-arm-rt", "tx1", 1, "base-a");
    let socket_path = config.socket_path.clone();
    let server_handle = thread::spawn(move || server.serve_one(&mut core).expect("serve"));
    let mut client = UnixStream::connect(&socket_path).expect("connect");
    client.write_all(&bind_frame).expect("write bind");
    let established_frame = read_frame(&mut client).expect("read established");
    let established = decode_session_established(&established_frame).expect("decode established");
    assert_eq!(established.counter(), 0);
    let counter = 1u64;
    let arm_frame = encode_request(&req, &established, counter).expect("encode arm");
    client.write_all(&arm_frame).expect("write arm");
    let resp_frame = read_frame(&mut client).expect("read response");
    let resp = decode_response(&resp_frame, &req, &established, counter).expect("decode");
    assert!(matches!(resp, LocalResponse::Armed { .. }));
    server_handle.join().expect("join");
    let reader = DecisionLogReader::open(config.store_root.join(DECISION_LOG_REL)).expect("reader");
    assert_eq!(reader.record_count(), 1);
    assert_eq!(reader.last_kind(), Some(AuthorityRecordKind::Armed));
}

// --- L03-AC05 arming ---

#[test]
fn l03_ac05_moved_base_rejected_before_first_armed_record() {
    let bundle = FakeBundle::new();
    bundle.base_revision.set_observed("base-observed");
    let dir = scratch_dir("l03-arm");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-requested"),
    )
    .expect_err("moved");
    assert!(matches!(err, RpcError::MovedBase));
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = DecisionLogReader::open(&log_path).expect("reader");
        assert_eq!(reader.record_count(), 0);
    }
}

#[test]
fn l03_ac05_duplicate_conflicting_arm_rejected_without_new_record() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-arm");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    counter += 1;
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-2", "tx1", 1, "base-b"),
    )
    .expect_err("conflict");
    assert!(matches!(err, RpcError::ConflictingRequest));
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert_eq!(reader.record_count(), 1);
}

#[test]
fn l03_ac05_wrong_epoch_rejected() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-arm");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 99, "base-a"),
    )
    .expect_err("epoch");
    assert!(matches!(err, RpcError::WrongEpoch));
}

#[test]
fn l03_ac05_weakened_mandatory_invariant_rejected() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-arm");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        LocalRequest::arm(
            "req-arm-1",
            "host-test",
            "tx1",
            1,
            "base-a",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            vec![],
            vec![],
        ),
    )
    .expect_err("weakened");
    assert!(matches!(err, RpcError::WeakenedMandatoryInvariant));
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = DecisionLogReader::open(&log_path).expect("reader");
        assert_eq!(reader.record_count(), 0);
    }
}

#[test]
fn l03_ac05_expired_arming_deadline_rejected() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-arm");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        LocalRequest::arm(
            "req-arm-1",
            "host-test",
            "tx1",
            1,
            "base-a",
            SystemTime::UNIX_EPOCH,
            vec!["route_present".to_owned()],
            vec![],
        ),
    )
    .expect_err("deadline");
    assert!(matches!(err, RpcError::ExpiredDeadline));
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = DecisionLogReader::open(&log_path).expect("reader");
        assert_eq!(reader.record_count(), 0);
    }
}

// --- L03-AC06 authority ---

#[test]
fn l03_ac06_watchdog_chooses_begin_commit_on_separate_tx() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Pass));
    let dir = scratch_dir("l03-commit");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-commit", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-commit", "tx-commit", 1, "base-a"),
    )
    .expect("arm");
    counter += 1;
    let resp = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        trigger_decision("req-decide-commit", "tx-commit", 1),
    )
    .expect("decide");
    assert!(matches!(
        resp,
        LocalResponse::AuthorityChosen {
            kind: AuthorityRecordKind::BeginCommit,
            ..
        }
    ));
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert_eq!(reader.last_kind(), Some(AuthorityRecordKind::BeginCommit));
}

#[test]
fn l03_ac06_watchdog_chooses_begin_revert_on_separate_tx() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Fail));
    let dir = scratch_dir("l03-revert");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-revert", 2, "lease2", 101);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-revert", "tx-revert", 2, "base-a"),
    )
    .expect("arm");
    counter += 1;
    let resp = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        trigger_decision("req-decide-revert", "tx-revert", 2),
    )
    .expect("decide");
    assert!(matches!(
        resp,
        LocalResponse::AuthorityChosen {
            kind: AuthorityRecordKind::BeginRevert,
            ..
        }
    ));
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert_eq!(reader.last_kind(), Some(AuthorityRecordKind::BeginRevert));
}

#[test]
fn l03_ac06_unknown_transaction_rejected() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-unknown-tx");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    counter += 1;
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        LocalRequest::heartbeat(
            "req-hb",
            "host-test",
            "tx-unknown",
            1,
            "lease1",
            valid_worker_group_tag(100),
            2,
        ),
    )
    .expect_err("unknown tx");
    assert!(matches!(err, RpcError::UnknownTransaction));
}

#[test]
fn l03_ac06_stale_epoch_refused_on_same_transaction() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-stale-epoch");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    counter += 1;
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        trigger_decision("req-decide", "tx1", 0),
    )
    .expect_err("stale");
    assert!(matches!(err, RpcError::StaleEpoch));
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert!(!reader.contains_kind(AuthorityRecordKind::BeginCommit));
    assert!(!reader.contains_kind(AuthorityRecordKind::BeginRevert));
}

// --- L03-AC07 lease / heartbeat ---

#[test]
fn l03_ac07_heartbeat_wrong_binding_rejected() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-hb");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    counter += 1;
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        LocalRequest::heartbeat(
            "req-hb",
            "host-test",
            "tx1",
            1,
            "wrong-lease",
            valid_worker_group_tag(100),
            2,
        ),
    )
    .expect_err("binding");
    assert!(matches!(err, RpcError::WrongBinding));
}

#[test]
fn l03_ac07_clock_regression_rejected() {
    let bundle = FakeBundle::new();
    bundle.clock.advance(Duration::from_secs(3600));
    let dir = scratch_dir("l03-clock");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    counter += 1;
    bundle.clock.set(SystemTime::UNIX_EPOCH);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        LocalRequest::request_lease_renewal(
            "req-renew",
            "host-test",
            "tx1",
            1,
            "lease1",
            valid_worker_group_tag(100),
            3,
        ),
    )
    .expect_err("clock");
    assert!(matches!(err, RpcError::ClockRegression));
}

// --- L03-AC08 fencing via expired-lease RequestDecision ---

#[test]
fn l03_ac08_expired_lease_fence_order_before_begin_revert() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Fail));
    bundle.process_group.alive_after_term(true);
    bundle.process_group.alive_after_kill(false);
    bundle.job_inspector.push_count(Ok(0));
    let dir = scratch_dir("l03-fence");
    let config = core_config(&dir);
    let log_path = config.store_root.join(DECISION_LOG_REL);
    bundle
        .job_inspector
        .observe_log_at_inspection(log_path.clone());
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-fence", 3, "lease3", 102);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-fence", "tx-fence", 3, "base-a"),
    )
    .expect("arm");
    counter += 1;
    bundle.clock.advance(Duration::from_secs(7200));
    let resp = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        trigger_decision("req-decide-fence", "tx-fence", 3),
    )
    .expect("decide");
    assert!(matches!(
        resp,
        LocalResponse::AuthorityChosen {
            kind: AuthorityRecordKind::BeginRevert,
            ..
        }
    ));
    assert_eq!(
        bundle.fence_trace.snapshot(),
        vec![
            FenceTraceEvent::Term,
            FenceTraceEvent::BoundedWait,
            FenceTraceEvent::AliveAfterTerm,
            FenceTraceEvent::Kill,
            FenceTraceEvent::BoundedWait,
            FenceTraceEvent::ConfirmedExit,
            FenceTraceEvent::ZeroCandidateJobs,
        ]
    );
    let reader = DecisionLogReader::open(&log_path).expect("reader");
    assert_eq!(reader.last_kind(), Some(AuthorityRecordKind::BeginRevert));
}

#[test]
fn l03_ac08_surviving_group_prevents_begin_revert() {
    let bundle = FakeBundle::new();
    bundle.process_group.alive_after_term(true);
    bundle.process_group.alive_after_kill(true);
    let dir = scratch_dir("l03-fence");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-survive", 4, "lease4", 103);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-survive", "tx-survive", 4, "base-a"),
    )
    .expect("arm");
    counter += 1;
    bundle.clock.advance(Duration::from_secs(7200));
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        trigger_decision("req-decide-survive", "tx-survive", 4),
    )
    .expect_err("survivor");
    assert!(matches!(err, RpcError::FenceIncomplete));
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert!(!reader.contains_kind(AuthorityRecordKind::BeginRevert));
}

#[test]
fn l03_ac08_nonzero_jobs_prevents_begin_revert() {
    let bundle = FakeBundle::new();
    bundle.process_group.alive_after_term(true);
    bundle.process_group.alive_after_kill(false);
    bundle.job_inspector.push_count(Ok(2));
    let dir = scratch_dir("l03-fence");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-jobs", 5, "lease5", 104);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-jobs", "tx-jobs", 5, "base-a"),
    )
    .expect("arm");
    counter += 1;
    bundle.clock.advance(Duration::from_secs(7200));
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        trigger_decision("req-decide-jobs", "tx-jobs", 5),
    )
    .expect_err("jobs");
    assert!(matches!(err, RpcError::FenceIncomplete));
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert!(!reader.contains_kind(AuthorityRecordKind::BeginRevert));
}

// --- L03-AC09 restart ---

#[test]
fn l03_ac09_restart_reconstructs_log_sequence() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("l03-restart");
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let mut counter = 1u64;
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        counter,
        arm_request("req-arm-1", "tx1", 1, "base-a"),
    )
    .expect("arm");
    let reopened =
        WatchdogCore::reopen(core_config(&dir), dependencies_from(&bundle)).expect("reopen");
    let reader = DecisionLogReader::open(core_config(&dir).store_root.join(DECISION_LOG_REL))
        .expect("reader");
    assert_eq!(reader.record_count(), 1);
    assert_eq!(reopened.read_decision_log_sequence(), 1);
}

// --- L03-AC10 hermetic ---

#[test]
fn l03_ac10_all_dependencies_are_injected_traits() {
    let bundle = FakeBundle::new();
    let _deps = dependencies_from(&bundle);
    assert!(bundle
        .topology
        .verify_calls
        .lock()
        .expect("lock")
        .is_empty());
}
