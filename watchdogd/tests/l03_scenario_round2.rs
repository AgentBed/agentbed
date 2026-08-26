//! AGB-8 bounded closure scenario round-2 — discriminating RED for G1–G3 blockers.
//!
//! Maps to scenario verdict `0f82aaec` on head `7c4e2b7`. Production unchanged at RED.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    dead_code
)]

mod common;

use agentbed_watchdogd::error::{DurabilityError, RpcError, TopologyError};
use agentbed_watchdogd::interfaces::{Clock, TopologyProbe};
use agentbed_watchdogd::read_model::{AuthorityRecordKind, DecisionLogReader};
use agentbed_watchdogd::rpc::protocol::{encode_request, LocalRequest, LocalResponse};
use agentbed_watchdogd::topology::ProductionTopologyProbe;
use agentbed_watchdogd::{CoreConfig, SessionState, WatchdogCore};
use common::{
    dependencies_from, scratch_dir, valid_worker_group_tag, DurabilityOp, FakeBundle, FakePeerCred,
    BROKER_STATE_ROOT, DECISION_LOG_REL, EPOCH_HIGH_WATER_REL, SAFE_MODE_REL,
};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
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
    let bind = agentbed_watchdogd::rpc::protocol::SessionBind::new(
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
    let verified = agentbed_watchdogd::rpc::protocol::decode_request(&frame, session)?;
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

fn seed_epoch_zero(store: &Path) {
    let path = store.join(EPOCH_HIGH_WATER_REL);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("epoch parent");
    }
    fs::write(path, 0u64.to_be_bytes()).expect("seed epoch zero");
}

// When epoch/high-water is pre-seeded, `open` skips startup durability writes.
const OPEN_WITHOUT_STARTUP_FSYNCS: usize = 0;

fn open_core_seeded(dir: &Path, bundle: &FakeBundle) -> WatchdogCore {
    seed_epoch_zero(&core_config(dir).store_root);
    open_core(dir, bundle)
}

fn assert_no_begin_authority_records(store_root: &Path) {
    let log_path = store_root.join(DECISION_LOG_REL);
    if !log_path.exists() {
        return;
    }
    let reader = DecisionLogReader::open(&log_path).expect("reader");
    assert!(
        !reader.contains_kind(AuthorityRecordKind::BeginCommit),
        "BEGIN_COMMIT must not appear after fail-closed durability error"
    );
    assert!(
        !reader.contains_kind(AuthorityRecordKind::BeginRevert),
        "BEGIN_REVERT must not appear after fail-closed durability error"
    );
}

fn assert_next_request_hits_safe_mode(
    core: &mut WatchdogCore,
    bundle: &FakeBundle,
    session: &mut SessionState,
    established: &agentbed_watchdogd::rpc::protocol::SessionEstablished,
    counter: u64,
) {
    let err = handle_authenticated(
        core,
        session,
        established,
        counter,
        arm_request(
            "req-after-latch",
            "tx-latch",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("latched core must refuse subsequent arm with SafeModeActive");
    assert!(
        matches!(err, RpcError::SafeModeActive),
        "expected SafeModeActive latch, got {err:?}"
    );
}

fn set_mode_0700(path: &Path) {
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o700);
    fs::set_permissions(path, perms).expect("chmod");
}

// --- G1: production topology regression ---

#[test]
fn g1_production_topology_rejects_ordinary_mode_0700_directory() {
    let probe = ProductionTopologyProbe::new();
    let dir = scratch_dir("g1-ordinary");
    fs::create_dir_all(&dir).expect("mkdir");
    set_mode_0700(&dir);
    let err = probe
        .verify_startup(&dir)
        .expect_err("ordinary 0700 directory must be rejected as OrdinaryDirectoryFallback");
    assert_eq!(err, TopologyError::OrdinaryDirectoryFallback);
}

#[test]
fn g1_production_topology_rejects_symlink_component_path() {
    let probe = ProductionTopologyProbe::new();
    let base = scratch_dir("g1-symlink");
    let real = base.join("real_mount");
    fs::create_dir_all(&real).expect("real dir");
    set_mode_0700(&real);
    let link = base.join("link_mount");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let err = probe
        .verify_startup(&link)
        .expect_err("path with symlink component must be rejected");
    assert_eq!(err, TopologyError::SymlinkComponent);
}

#[test]
fn g1_production_topology_rejects_intermediate_symlink_component() {
    let probe = ProductionTopologyProbe::new();
    let base = scratch_dir("g1-symlink-mid");
    let real = base.join("real_root");
    let store = real.join("watchdog");
    fs::create_dir_all(&store).expect("store dir");
    set_mode_0700(&store);
    let middle = base.join("middle");
    std::os::unix::fs::symlink(&real, &middle).expect("middle symlink");
    let via_symlink = middle.join("watchdog");
    let err = probe
        .verify_startup(&via_symlink)
        .expect_err("intermediate symlink component must be rejected");
    assert_eq!(err, TopologyError::SymlinkComponent);
}

#[test]
fn g1_production_topology_source_contract_requires_substantive_evidence() {
    let src = include_str!("../src/topology.rs");
    let must_reference: &[(&str, &[&str])] = &[
        (
            "mount identity / device separation",
            &["st_dev", "mount_id", "mountinfo"],
        ),
        (
            "link-count inspection",
            &["nlink", "link_count", "WrongLinkCount"],
        ),
        ("ownership inspection", &["uid", "gid", "root"]),
        (
            "protected broker state domain",
            &["/var/lib/agentbed/broker/state", "broker/state"],
        ),
        (
            "protected rollback domain",
            &["/var/lib/agentbed/rollback", "rollback"],
        ),
        ("nix store domain", &["/nix/store", "/nix"]),
        (
            "write/fsync/rename capability probe",
            &["fsync", "rename", "Unwritable", "same_directory"],
        ),
        (
            "bind-alias / same-device rejection",
            &["SameDeviceAlias", "st_dev"],
        ),
    ];
    for (label, needles) in must_reference {
        assert!(
            needles.iter().any(|needle| src.contains(needle)),
            "ProductionTopologyProbe must substantively implement {label}"
        );
    }
}

// --- G2: safe-mode latch on durability failures ---

#[test]
fn g2_authority_append_fsync_failure_latches_safe_mode() {
    let bundle = FakeBundle::new();
    bundle
        .durability
        .fail_on_file_fsync_invocation(OPEN_WITHOUT_STARTUP_FSYNCS + 2);
    let dir = scratch_dir("g2-append-fsync");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm",
            "tx1",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("authority append fsync failure");
    assert!(matches!(
        err,
        RpcError::Durability(DurabilityError::InjectedFailure)
    ));
    assert_no_begin_authority_records(&store);
    assert_next_request_hits_safe_mode(&mut core, &bundle, &mut session, &established, 2);
}

#[test]
fn g2_epoch_temp_file_fsync_failure_latches_safe_mode() {
    let bundle = FakeBundle::new();
    bundle
        .durability
        .fail_on_file_fsync_invocation(OPEN_WITHOUT_STARTUP_FSYNCS + 1);
    let dir = scratch_dir("g2-epoch-tmp-fsync");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm",
            "tx1",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("epoch temp file fsync failure");
    assert!(matches!(
        err,
        RpcError::Durability(DurabilityError::InjectedFailure)
    ));
    assert_no_begin_authority_records(&store);
    assert_next_request_hits_safe_mode(&mut core, &bundle, &mut session, &established, 2);
}

#[test]
fn g2_epoch_parent_dir_fsync_failure_latches_safe_mode() {
    let bundle = FakeBundle::new();
    bundle
        .durability
        .fail_on_dir_fsync_invocation(OPEN_WITHOUT_STARTUP_FSYNCS + 1);
    let dir = scratch_dir("g2-epoch-dir-fsync");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm",
            "tx1",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("epoch parent dir fsync failure");
    assert!(matches!(
        err,
        RpcError::Durability(DurabilityError::InjectedFailure)
    ));
    assert_no_begin_authority_records(&store);
    assert_next_request_hits_safe_mode(&mut core, &bundle, &mut session, &established, 2);
}

#[test]
fn g2_safe_mode_marker_persist_failure_keeps_in_memory_latch() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("g2-marker-persist");
    let store = core_config(&dir).store_root.clone();
    seed_epoch_zero(&store);
    fs::create_dir_all(store.join("epoch")).expect("epoch dir");
    fs::write(store.join("epoch/.tmp-epoch"), b"stale").expect("legacy stale temp");
    bundle
        .durability
        .fail_on_file_fsync_invocation(OPEN_WITHOUT_STARTUP_FSYNCS + 1);
    let mut core = open_core_seeded(&dir, &bundle);
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
    .expect_err("safe-mode marker persist failure must not drop latch");
    assert!(
        matches!(
            err,
            RpcError::SafeModeActive | RpcError::Durability(DurabilityError::InjectedFailure)
        ),
        "unexpected first error: {err:?}"
    );
    assert_next_request_hits_safe_mode(&mut core, &bundle, &mut session, &established, 2);
}

// --- G3: same-directory temps and ambiguous epoch temp detection ---

#[test]
fn g3_epoch_temp_parent_matches_high_water_parent_and_dir_fsynced() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("g3-epoch-same-dir");
    let store = core_config(&dir).store_root.clone();
    let epoch_target = store.join(EPOCH_HIGH_WATER_REL);
    let epoch_parent = epoch_target.parent().expect("epoch parent").to_path_buf();

    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm",
            "tx1",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect("arm records epoch rename paths");

    let epoch_rename = {
        let renames = bundle.durability.rename_ops.lock().expect("lock");
        renames
            .iter()
            .find(|(_, to)| *to == epoch_target)
            .expect("epoch high-water rename must be recorded")
            .clone()
    };
    assert_eq!(
        epoch_rename.0.parent().expect("epoch temp parent"),
        epoch_parent.as_path(),
        "epoch temp must live in the same directory as high-water.json"
    );
    assert!(
        bundle
            .durability
            .dir_fsync_paths
            .lock()
            .expect("lock")
            .contains(&epoch_parent),
        "epoch destination parent must be dir-fsynced after rename"
    );
}

#[test]
fn g3_safe_mode_temp_parent_matches_marker_parent_and_dir_fsynced() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("g3-safe-same-dir");
    let store = core_config(&dir).store_root.clone();
    let safe_mode_target = store.join(SAFE_MODE_REL);
    let safe_parent = safe_mode_target
        .parent()
        .expect("safe-mode parent")
        .to_path_buf();
    fs::create_dir_all(store.join("epoch")).expect("epoch dir");
    fs::write(store.join("epoch/.tmp-epoch"), b"stale").expect("legacy stale temp");

    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-sm", 2, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-safe",
            "tx-sm",
            2,
            "base-b",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("legacy epoch temp must latch safe mode");
    assert!(matches!(err, RpcError::SafeModeActive));

    let safe_rename = {
        let renames = bundle.durability.rename_ops.lock().expect("lock");
        renames
            .iter()
            .find(|(_, to)| *to == safe_mode_target)
            .expect("safe-mode marker rename must be recorded")
            .clone()
    };
    assert_eq!(
        safe_rename.0.parent().expect("safe-mode temp parent"),
        safe_parent.as_path(),
        "safe-mode temp must live in the same directory as safe-mode.json"
    );
    assert!(
        bundle
            .durability
            .dir_fsync_paths
            .lock()
            .expect("lock")
            .contains(&safe_parent),
        "safe-mode destination parent must be dir-fsynced after rename"
    );
}

#[test]
fn g3_stale_unique_epoch_temp_in_target_dir_latches_safe_mode() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("g3-unique-temp");
    let store = core_config(&dir).store_root.clone();
    fs::create_dir_all(store.join("epoch")).expect("epoch dir");
    fs::write(store.join("epoch/.tmp-epoch-1700000000000000000"), b"stale")
        .expect("stale unique epoch temp");

    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-uniq", 2, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-uniq",
            "tx-uniq",
            2,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("stale unique epoch temp must fail closed before replacement");
    assert!(matches!(err, RpcError::SafeModeActive));
    assert_next_request_hits_safe_mode(&mut core, &bundle, &mut session, &established, 2);
}

#[test]
fn g3_legacy_epoch_temp_refusal_preserved() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("g3-legacy-temp");
    let store = core_config(&dir).store_root.clone();
    fs::create_dir_all(store.join("epoch")).expect("epoch dir");
    fs::write(store.join("epoch/.tmp-epoch"), b"stale").expect("legacy stale temp");

    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-leg", 2, "lease1", 100);
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-leg",
            "tx-leg",
            2,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("legacy .tmp-epoch must be refused");
    assert!(matches!(err, RpcError::SafeModeActive));
    let high_water = store.join(EPOCH_HIGH_WATER_REL);
    let bytes = fs::read(high_water).expect("seeded epoch file must remain");
    assert_eq!(
        u64::from_be_bytes(bytes[..8].try_into().expect("epoch bytes")),
        0,
        "epoch advance must not proceed past ambiguous legacy temp"
    );
}

const REPLACEMENT_DURABILITY_ORDER: &[DurabilityOp] = &[
    DurabilityOp::FileFsync,
    DurabilityOp::AtomicRename,
    DurabilityOp::DirFsync,
    DurabilityOp::Readback,
];

fn durability_ops_since(bundle: &FakeBundle, start: usize) -> Vec<DurabilityOp> {
    bundle.durability.ops.lock().expect("lock")[start..].to_vec()
}

fn assert_replacement_durability_order(ops: &[DurabilityOp], label: &str) {
    assert!(
        ops.windows(REPLACEMENT_DURABILITY_ORDER.len())
            .any(|window| window == REPLACEMENT_DURABILITY_ORDER),
        "{label}: expected file-fsync(temp) → atomic rename → destination-parent dir-fsync → readback, observed {ops:?}"
    );
}

#[test]
fn g3_epoch_advance_durability_order_is_fsync_rename_dir_fsync_readback() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("g3-epoch-order");
    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx1", 1, "lease1", 100);
    let ops_before = bundle.durability.ops.lock().expect("lock").len();
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-order",
            "tx1",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect("epoch advance must succeed for sequencing observation");
    let ops = durability_ops_since(&bundle, ops_before);
    assert_replacement_durability_order(&ops, "epoch high-water replacement");
}

#[test]
fn g3_safe_mode_marker_durability_order_is_fsync_rename_dir_fsync_readback() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("g3-safe-order");
    let store = core_config(&dir).store_root.clone();
    seed_epoch_zero(&store);
    fs::create_dir_all(store.join("epoch")).expect("epoch dir");
    fs::write(store.join("epoch/.tmp-epoch"), b"stale").expect("legacy stale temp");
    let mut core = open_core_seeded(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-sm", 2, "lease1", 100);
    let ops_before = bundle.durability.ops.lock().expect("lock").len();
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-safe-order",
            "tx-sm",
            2,
            "base-b",
            bundle.clock.now() + Duration::from_secs(60),
        ),
    )
    .expect_err("legacy epoch temp must trigger safe-mode persistence before replacement");
    assert!(matches!(err, RpcError::SafeModeActive));
    let ops = durability_ops_since(&bundle, ops_before);
    assert_replacement_durability_order(&ops, "safe-mode marker persistence");
}

// Reference sealed domains for source-contract readability (not executed).
#[allow(dead_code)]
const _SEALED_PROTECTED_DOMAINS: &[&str] = &[
    common::WATCHDOG_MOUNT_ROOT,
    BROKER_STATE_ROOT,
    "/var/lib/agentbed/rollback",
];
