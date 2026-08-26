//! Fencing-safety RED seam — hermetic recording-fake ordering and source contracts.
//! Sends no real signals; compiles against unchanged production.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    dead_code
)]

mod common;

use agentbed_watchdogd::error::RpcError;
use agentbed_watchdogd::interfaces::InvariantOutcome;
use agentbed_watchdogd::read_model::{AuthorityRecordKind, DecisionLogReader};
use agentbed_watchdogd::rpc::protocol::{
    decode_session_bind, encode_frame, encode_request, LocalRequest, SessionBind,
};
use agentbed_watchdogd::{CoreConfig, SessionState, WatchdogCore};
use common::{
    dependencies_from, scratch_dir, valid_worker_group_tag, FakeBundle, FenceTraceEvent,
    DECISION_LOG_REL,
};
use std::fs;
use std::path::Path;
use std::time::Duration;

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
        .enqueue_cred(common::FakePeerCred::broker_cred(0, 0, 4242));
    let bind = SessionBind::new(
        "host-test",
        tx,
        epoch,
        lease_id,
        valid_worker_group_tag(worker_group_tag),
        "client-nonce-seam",
    );
    SessionState::bind(core, &bundle.peer_cred, &bundle.entropy, bind).expect("bootstrap")
}

fn arm_request(req_id: &str, tx: &str, epoch: u64, base: &str) -> LocalRequest {
    LocalRequest::arm(
        req_id,
        "host-test",
        tx,
        epoch,
        base,
        std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        vec!["route_present".to_owned()],
        vec![],
    )
}

fn handle_authenticated(
    core: &mut WatchdogCore,
    session: &mut SessionState,
    established: &agentbed_watchdogd::rpc::protocol::SessionEstablished,
    counter: u64,
    req: LocalRequest,
) -> Result<agentbed_watchdogd::rpc::protocol::LocalResponse, RpcError> {
    let frame = encode_request(&req, established, counter)?;
    let verified = agentbed_watchdogd::rpc::protocol::decode_request(&frame, session)?;
    core.handle_request(verified, session)
}

/// Future-shape SessionBind wire (GREEN renames `process_group` → `worker_group_tag`).
fn future_session_bind_frame(worker_group_tag: serde_json::Value) -> Vec<u8> {
    let envelope = serde_json::json!({
        "version": 1,
        "payload": {
            "host_id": "host-test",
            "tx_id": "tx-wire",
            "epoch": 1,
            "lease_id": "lease-wire",
            "worker_group_tag": worker_group_tag,
            "client_nonce": "nonce-wire",
        }
    });
    let payload = serde_json::to_vec(&envelope).expect("json");
    encode_frame(&payload).expect("frame")
}

fn is_malformed_request_refusal(err: &RpcError) -> bool {
    format!("{err:?}") == "MalformedRequest"
}

fn assert_worker_group_tag_refused_on_decode(tag: serde_json::Value) {
    let frame = future_session_bind_frame(tag.clone());
    match decode_session_bind(&frame) {
        Ok(bind) => {
            panic!("worker_group_tag {tag} must be refused by production decode/bind, got {bind:?}")
        }
        Err(RpcError::DenyUnknown) => panic!(
            "worker_group_tag {tag}: DenyUnknown is legacy-shape mismatch, not reserved-tag refusal"
        ),
        Err(err) => assert!(
            is_malformed_request_refusal(&err),
            "worker_group_tag {tag}: uniform MalformedRequest required, got {err:?}"
        ),
    }
}

fn decode_and_bind_future_tag(
    core: &WatchdogCore,
    bundle: &FakeBundle,
    tag: serde_json::Value,
) -> Result<(), RpcError> {
    bundle
        .peer_cred
        .enqueue_cred(common::FakePeerCred::broker_cred(0, 0, 4242));
    let frame = future_session_bind_frame(tag);
    let bind = decode_session_bind(&frame)?;
    SessionState::bind(core, &bundle.peer_cred, &bundle.entropy, bind).map(|_| ())
}

fn assert_worker_group_tag_accepted(tag: serde_json::Value) {
    let core_dir = scratch_dir("fencing-seam-wire-accept");
    let bundle = FakeBundle::new();
    let core = open_core(&core_dir, &bundle);
    decode_and_bind_future_tag(&core, &bundle, tag.clone()).unwrap_or_else(|err| {
        panic!("worker_group_tag {tag} must decode/bind on production, got {err:?}")
    });
}

// --- source absence / trait contract (RED: production still has real signaling) ---

#[test]
fn fencing_seam_source_has_no_signal_or_wait_syscalls() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for entry in fs::read_dir(manifest_dir.join("src")).expect("src dir") {
        let entry = entry.expect("entry");
        if entry.path().extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let src = fs::read_to_string(entry.path()).expect("read source");
        assert!(
            !src.contains("libc::kill"),
            "must not call libc::kill in {}",
            entry.path().display()
        );
        assert!(
            !src.contains("libc::waitpid"),
            "must not call libc::waitpid in {}",
            entry.path().display()
        );
        assert!(
            !src.contains("libc::killpg"),
            "must not call libc::killpg in {}",
            entry.path().display()
        );
        assert!(
            !src.contains("libc::sigqueue"),
            "must not call libc::sigqueue in {}",
            entry.path().display()
        );
    }
    let fencing_src = fs::read_to_string(manifest_dir.join("src/fencing.rs")).expect("fencing.rs");
    assert!(
        !fencing_src.contains("unsafe"),
        "fencing.rs must not contain unsafe blocks"
    );
}

#[test]
fn fencing_seam_trait_signal_has_no_caller_supplied_pgid_parameter() {
    let interfaces_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interfaces.rs"))
            .expect("interfaces.rs");
    assert!(
        interfaces_src.contains("fn signal(&self, kind: SignalKind)"),
        "ProcessGroupFence::signal must not accept a caller-supplied pgid/target parameter"
    );
    assert!(
        !interfaces_src.contains("fn signal(&self, kind: SignalKind, pgid: i32)"),
        "caller-supplied pgid must be unrepresentable on the trait boundary"
    );
}

#[test]
fn fencing_seam_production_unavailable_fencer_only() {
    let fencing_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fencing.rs"))
            .expect("fencing.rs");
    assert!(
        fencing_src.contains("UnavailableProcessGroupFencer"),
        "production must expose UnavailableProcessGroupFencer"
    );
    assert!(
        !fencing_src.contains("ProductionProcessGroupFencer"),
        "real-signal ProductionProcessGroupFencer must be removed from L03"
    );
    assert!(
        !fencing_src.contains("impl ProcessGroupFence for ProductionProcessGroupFencer"),
        "production must not implement real signaling"
    );
}

// --- worker_group_tag wire contract (future-shape JSON via public decode/bind surfaces) ---

#[test]
fn fencing_seam_worker_group_tag_rejects_zero_one_negative_and_above_i32_max() {
    assert_worker_group_tag_refused_on_decode(serde_json::json!(0));
    assert_worker_group_tag_refused_on_decode(serde_json::json!(1));
    assert_worker_group_tag_refused_on_decode(serde_json::json!(-1));
    assert_worker_group_tag_refused_on_decode(serde_json::json!(i64::from(i32::MAX) + 1));
}

#[test]
fn fencing_seam_worker_group_tag_accepts_opaque_correlation_only() {
    assert_worker_group_tag_accepted(serde_json::json!(2));
    assert_worker_group_tag_accepted(serde_json::json!(42));
    assert_worker_group_tag_accepted(serde_json::json!(i32::MAX));
}

// --- hermetic recording-fake ordering branches ---

#[test]
fn fencing_seam_after_term_absent_skips_kill_and_reaches_zero_jobs() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Fail));
    bundle.process_group.alive_after_term(false);
    bundle.job_inspector.push_count(Ok(0));
    let dir = scratch_dir("fencing-seam-term-success");
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    bundle
        .job_inspector
        .observe_log_at_inspection(log_path.clone());
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-term-ok", 7, "lease7", 102);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request("req-arm-term-ok", "tx-term-ok", 7, "base-a"),
    )
    .expect("arm");
    bundle.clock.advance(Duration::from_secs(7200));
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-term-ok", "host-test", "tx-term-ok", 7),
    )
    .expect("decide after term-success fence");
    assert_eq!(
        bundle.fence_trace.snapshot(),
        vec![
            FenceTraceEvent::Term,
            FenceTraceEvent::BoundedWait,
            FenceTraceEvent::AliveAfterTerm,
            FenceTraceEvent::ZeroCandidateJobs,
        ],
        "AfterTerm absent must skip Kill and second bounded_wait"
    );
    let reader = DecisionLogReader::open(&log_path).expect("reader");
    assert_eq!(reader.last_kind(), Some(AuthorityRecordKind::BeginRevert));
}

#[test]
fn fencing_seam_after_term_survivor_requires_kill_wait_afterkill_then_zero_jobs() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Fail));
    bundle.process_group.alive_after_term(true);
    bundle.process_group.alive_after_kill(false);
    bundle.job_inspector.push_count(Ok(0));
    let dir = scratch_dir("fencing-seam-term-survivor");
    let log_path = core_config(&dir).store_root.join(DECISION_LOG_REL);
    bundle
        .job_inspector
        .observe_log_at_inspection(log_path.clone());
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-survivor", 8, "lease8", 103);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request("req-arm-survivor", "tx-survivor", 8, "base-a"),
    )
    .expect("arm");
    bundle.clock.advance(Duration::from_secs(7200));
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-survivor", "host-test", "tx-survivor", 8),
    )
    .expect("decide after full survivor fence");
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
        ],
        "AfterTerm survivor must Kill then bounded_wait(Kill) then AfterKill absent"
    );
    let reader = DecisionLogReader::open(&log_path).expect("reader");
    assert_eq!(reader.last_kind(), Some(AuthorityRecordKind::BeginRevert));
}

#[test]
fn fencing_seam_unavailable_production_fencer_refuses_begin_authority() {
    let lib_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).expect("lib");
    assert!(
        lib_src.contains("UnavailableProcessGroupFencer"),
        "library must export unavailable production fencer type"
    );
    let fencing_src =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fencing.rs"))
            .expect("fencing.rs");
    assert!(
        fencing_src.contains("FenceError::Unavailable"),
        "unavailable fencer must fail closed without signaling"
    );
}

// --- constructor safety REVIEW RED (additive; existing eight tests unchanged) ---

#[test]
fn fencing_seam_try_from_raw_runtime_accepts_and_rejects() {
    use agentbed_watchdogd::WorkerGroupTag;

    for raw in [2_u32, 42, i32::MAX as u32] {
        WorkerGroupTag::try_from_raw(raw).expect("valid worker_group_tag");
    }
    for raw in [0_u32, 1, i32::MAX as u32 + 1] {
        let err = WorkerGroupTag::try_from_raw(raw).expect_err("reserved worker_group_tag");
        assert_eq!(
            err,
            RpcError::MalformedRequest,
            "reserved tag {raw} must refuse as MalformedRequest"
        );
    }
}

#[test]
fn fencing_seam_production_has_no_panic_constructor_or_raw_i32_api() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut production_src = String::new();
    for entry in fs::read_dir(manifest_dir.join("src")).expect("src dir") {
        let entry = entry.expect("entry");
        if entry.path().extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        production_src.push_str(&fs::read_to_string(entry.path()).expect("read source"));
    }
    assert!(
        !production_src.contains("from_trusted_i32"),
        "production must not export from_trusted_i32 panic constructor"
    );
    assert!(
        !production_src.contains(".expect(\"trusted worker_group_tag\")"),
        "production must not panic on trusted worker_group_tag construction"
    );
    let protocol_src =
        fs::read_to_string(manifest_dir.join("src/rpc/protocol.rs")).expect("protocol.rs");
    assert!(
        !protocol_src.contains("worker_group_tag: i32"),
        "public SessionBind/lease/heartbeat constructors must not accept raw i32 worker_group_tag"
    );
    assert!(
        protocol_src.contains("worker_group_tag: WorkerGroupTag")
            || protocol_src.contains("-> Result<"),
        "constructors must consume validated WorkerGroupTag or return Result"
    );
}
