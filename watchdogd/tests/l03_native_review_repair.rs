//! AGB-8 native review repair — discriminating RED for agentos-reviewer findings.
//!
//! Maps to review ID `5032007100` on rejected head `dffcbb5`.

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
    decode_request, encode_request, LocalRequest, LocalResponse, SessionBind,
};
use agentbed_watchdogd::{CoreConfig, SessionState, WatchdogCore};
use common::{
    dependencies_from, scratch_dir, valid_worker_group_tag, FakeBundle, FakePeerCred,
    DECISION_LOG_REL, EPOCH_HIGH_WATER_REL,
};
use std::fs;
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

fn decision_log_reader(store_root: &Path) -> DecisionLogReader {
    DecisionLogReader::open(store_root.join(DECISION_LOG_REL)).expect("decision log reader")
}

fn begin_authority_count(reader: &DecisionLogReader) -> usize {
    usize::from(reader.contains_kind(AuthorityRecordKind::BeginCommit))
        + usize::from(reader.contains_kind(AuthorityRecordKind::BeginRevert))
}

fn armed_record_count(reader: &DecisionLogReader) -> usize {
    usize::from(reader.contains_kind(AuthorityRecordKind::Armed))
}

// 1. Missing high-water after open fails closed.
#[test]
fn native_review_missing_high_water_after_open_fails_closed() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("native-missing-hw");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    assert!(
        store.join(EPOCH_HIGH_WATER_REL).exists(),
        "open must initialize epoch high-water"
    );
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-missing-hw", 1, "lease1", 100);
    fs::remove_file(store.join(EPOCH_HIGH_WATER_REL)).expect("delete high-water");
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-missing-hw",
            "tx-missing-hw",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(7200),
        ),
    )
    .expect_err("missing high-water must fail closed");
    assert!(
        matches!(err, RpcError::SafeModeActive),
        "expected SafeModeActive, got {err:?}"
    );
    let reader = decision_log_reader(&store);
    assert_eq!(
        armed_record_count(&reader),
        0,
        "missing high-water must not append ARMED"
    );
    let latch_err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::report_health("req-health", "host-test", "tx-missing-hw", 1),
    )
    .expect_err("safe-mode latch must refuse follow-up");
    assert!(matches!(latch_err, RpcError::SafeModeActive));
}

// 2. Trailing/corrupt high-water after open fails closed.
#[test]
fn native_review_trailing_high_water_corruption_fails_closed() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("native-trailing-hw");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-trail", 1, "lease1", 100);
    let mut bytes = 1u64.to_be_bytes().to_vec();
    bytes.extend_from_slice(b"TRAILING_GARBAGE");
    fs::write(store.join(EPOCH_HIGH_WATER_REL), &bytes).expect("corrupt high-water");
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-trail",
            "tx-trail",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(7200),
        ),
    )
    .expect_err("trailing high-water corruption must fail closed");
    assert!(
        matches!(err, RpcError::SafeModeActive),
        "expected SafeModeActive, got {err:?}"
    );
    let reader = decision_log_reader(&store);
    assert_eq!(
        armed_record_count(&reader),
        0,
        "corrupt high-water must not append ARMED"
    );
}

// 3. Decision is single-shot.
#[test]
fn native_review_decision_is_single_shot() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Pass));
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Fail));
    let dir = scratch_dir("native-single-shot");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-single", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-single",
            "tx-single",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(7200),
        ),
    )
    .expect("arm");
    let first = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-1", "host-test", "tx-single", 1),
    )
    .expect("first decision");
    assert!(matches!(
        first,
        LocalResponse::AuthorityChosen {
            kind: AuthorityRecordKind::BeginCommit,
            ..
        }
    ));
    let err = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        3,
        LocalRequest::request_decision("req-decide-2", "host-test", "tx-single", 1),
    )
    .expect_err("second decision must be refused");
    assert!(
        matches!(err, RpcError::ConflictingRequest),
        "expected ConflictingRequest, got {err:?}"
    );
    let reader = decision_log_reader(&store);
    assert_eq!(
        begin_authority_count(&reader),
        1,
        "exactly one BEGIN authority record"
    );
    assert!(
        reader.contains_kind(AuthorityRecordKind::BeginCommit)
            && !reader.contains_kind(AuthorityRecordKind::BeginRevert),
        "must never emit both BEGIN_COMMIT and BEGIN_REVERT"
    );
}

// 4. Restart reconstructs armed binding and rejects conflicting rebind.
#[test]
fn native_review_restart_rejects_conflicting_rebind() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("native-rebind");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bootstrap_session(&core, &bundle, "tx-a", 1, "lease-a", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-rebind",
            "tx-a",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(7200),
        ),
    )
    .expect("arm");
    let armed_before = armed_record_count(&decision_log_reader(&store));
    assert_eq!(armed_before, 1);
    let reopened = WatchdogCore::reopen(core_config(&dir), dependencies_from(&bundle))
        .expect("reopen after arm");
    bundle
        .peer_cred
        .enqueue_cred(FakePeerCred::broker_cred(0, 0, 4243));
    let err = SessionState::bind(
        &reopened,
        &bundle.peer_cred,
        &bundle.entropy,
        SessionBind::new(
            "host-test",
            "tx-b",
            1,
            "lease-b",
            valid_worker_group_tag(200),
            "client-nonce-2",
        ),
    )
    .expect_err("conflicting rebind after restart");
    assert!(
        matches!(err, RpcError::StaleReconnect),
        "expected StaleReconnect, got {err:?}"
    );
    let reader = decision_log_reader(&store);
    assert_eq!(
        armed_record_count(&reader),
        1,
        "durable log must not receive a second ARMED on conflicting rebind"
    );
}

// 5. Timely renewal extends the bounded lease window.
#[test]
fn native_review_timely_renewal_extends_lease_window() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Pass));
    let dir = scratch_dir("native-renewal");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-renew", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-renew",
            "tx-renew",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(10_000),
        ),
    )
    .expect("arm");
    bundle.clock.advance(Duration::from_secs(3599));
    handle_authenticated(
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
    .expect("timely renewal");
    bundle.clock.advance(Duration::from_secs(101));
    let resp = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        3,
        LocalRequest::request_decision("req-decide-renew", "host-test", "tx-renew", 1),
    )
    .expect("decision after timely renewal");
    assert!(matches!(
        resp,
        LocalResponse::AuthorityChosen {
            kind: AuthorityRecordKind::BeginCommit,
            ..
        }
    ));
    let trace = bundle.fence_trace.snapshot();
    assert!(
        trace.is_empty(),
        "timely renewal must avoid fencing; trace={trace:?}"
    );
    let reader = decision_log_reader(&store);
    assert!(reader.contains_kind(AuthorityRecordKind::BeginCommit));
}

// 6. Restart preserves chosen authority and refuses repeat decision.
#[test]
fn native_review_restart_preserves_chosen_authority_refuses_repeat() {
    let bundle = FakeBundle::new();
    bundle.invariants.push_outcome(Ok(InvariantOutcome::Pass));
    let dir = scratch_dir("native-repeat");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) =
        bootstrap_session(&core, &bundle, "tx-repeat", 1, "lease1", 100);
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-repeat",
            "tx-repeat",
            1,
            "base-a",
            bundle.clock.now() + Duration::from_secs(7200),
        ),
    )
    .expect("arm");
    handle_authenticated(
        &mut core,
        &mut session,
        &established,
        2,
        LocalRequest::request_decision("req-decide-repeat", "host-test", "tx-repeat", 1),
    )
    .expect("first decision");
    let mut reopened = WatchdogCore::reopen(core_config(&dir), dependencies_from(&bundle))
        .expect("reopen after decision");
    let (mut session2, established2) =
        bootstrap_session(&reopened, &bundle, "tx-repeat", 1, "lease1", 100);
    let err = handle_authenticated(
        &mut reopened,
        &mut session2,
        &established2,
        1,
        LocalRequest::request_decision("req-decide-repeat-2", "host-test", "tx-repeat", 1),
    )
    .expect_err("repeat decision after restart");
    assert!(
        matches!(err, RpcError::ConflictingRequest),
        "expected ConflictingRequest (not UnknownTransaction), got {err:?}"
    );
    let reader = decision_log_reader(&store);
    assert_eq!(begin_authority_count(&reader), 1);
    assert!(
        reader.contains_kind(AuthorityRecordKind::BeginCommit)
            && !reader.contains_kind(AuthorityRecordKind::BeginRevert),
        "must preserve single BEGIN_COMMIT without BEGIN_REVERT"
    );
}
