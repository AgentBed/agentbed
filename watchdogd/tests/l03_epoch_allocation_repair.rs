//! AGB-8 epoch-allocation repair — discriminating RED for native review `5032875711`.
//!
//! Maps to rejected head `52984ce7d7b044e41f4a810e660f54404e4ce758`.

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
use agentbed_watchdogd::interfaces::Clock;
use agentbed_watchdogd::read_model::{AuthorityRecordKind, DecisionLogReader};
use agentbed_watchdogd::rpc::protocol::{
    decode_request, encode_request, LocalRequest, LocalResponse, SessionBind,
};
use agentbed_watchdogd::{CoreConfig, SessionState, WatchdogCore};
use common::{
    dependencies_from, scratch_dir, valid_worker_group_tag, FakeBundle, FakePeerCred,
    DECISION_LOG_REL, EPOCH_HIGH_WATER_REL, SAFE_MODE_REL,
};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

const MALICIOUS_EPOCH: u64 = 4_000_000_000;
const EXPECTED_ISSUED_EPOCH: u64 = 1;

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

fn read_durable_high_water(store: &Path) -> u64 {
    let bytes = fs::read(store.join(EPOCH_HIGH_WATER_REL)).expect("high-water exists");
    assert_eq!(
        bytes.len(),
        8,
        "high-water must be exactly 8 big-endian bytes"
    );
    let arr: [u8; 8] = bytes.try_into().expect("8 bytes");
    u64::from_be_bytes(arr)
}

fn decision_log_reader(store: &Path) -> DecisionLogReader {
    DecisionLogReader::open(store.join(DECISION_LOG_REL)).expect("decision log reader")
}

struct PostArmObservations {
    high_water: u64,
    safe_mode_present: bool,
    record_count: usize,
    armed_present: bool,
}

fn observe_post_arm(store: &Path) -> PostArmObservations {
    let high_water = read_durable_high_water(store);
    let safe_mode_present = store.join(SAFE_MODE_REL).exists();
    let log_path = store.join(DECISION_LOG_REL);
    if log_path.exists() {
        let reader = decision_log_reader(store);
        PostArmObservations {
            high_water,
            safe_mode_present,
            record_count: reader.record_count(),
            armed_present: reader.contains_kind(AuthorityRecordKind::Armed),
        }
    } else {
        PostArmObservations {
            high_water,
            safe_mode_present,
            record_count: 0,
            armed_present: false,
        }
    }
}

fn bind_session(
    core: &mut WatchdogCore,
    bundle: &FakeBundle,
    tx: &str,
    proposed_epoch: u64,
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
        proposed_epoch,
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

// 1. Fresh bind epoch is watchdog-issued, not broker-selected.
#[test]
fn fresh_bind_epoch_is_watchdog_issued_not_broker_selected() {
    let proposals: [(u64, &str); 2] = [(0, "nonce-zero"), (MALICIOUS_EPOCH, "nonce-malicious")];
    let mut failures = Vec::new();

    for (proposed, nonce) in proposals {
        let bundle = FakeBundle::new();
        let dir = scratch_dir("epoch-bind-proposal");
        let store = core_config(&dir).store_root.clone();
        let mut core = open_core(&dir, &bundle);
        let tx = format!("tx-bind-{proposed}");

        let (session, established) =
            bind_session(&mut core, &bundle, &tx, proposed, "lease-bind", 100, nonce)
                .expect("bind succeeds on rejected code");

        if established.epoch != EXPECTED_ISSUED_EPOCH {
            failures.push(format!(
                "proposal {proposed}: SessionEstablished.epoch={}, expected {EXPECTED_ISSUED_EPOCH}",
                established.epoch
            ));
        }
        if established.epoch == proposed {
            failures.push(format!(
                "proposal {proposed}: broker proposal echoed as authoritative epoch"
            ));
        }

        let high_water = read_durable_high_water(&store);
        if high_water != 0 {
            failures.push(format!(
                "proposal {proposed}: high-water={high_water}, expected 0 before Arm"
            ));
        }

        if store.join(DECISION_LOG_REL).exists() {
            let reader = decision_log_reader(&store);
            if reader.record_count() > 0 {
                failures.push(format!(
                    "proposal {proposed}: decision log has {} records before Arm",
                    reader.record_count()
                ));
            }
        }

        let _ = session;
    }

    assert!(
        failures.is_empty(),
        "fresh bind must issue watchdog epoch 1, keep high-water 0, and leave log empty: {}",
        failures.join("; ")
    );
}

// 2. Broker epoch jump cannot mutate high-water or append ARMED.
#[test]
fn broker_epoch_jump_cannot_mutate_high_water_or_append_armed() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("epoch-jump-refused");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);

    let (mut session, established) = bind_session(
        &mut core,
        &bundle,
        "tx-jump",
        MALICIOUS_EPOCH,
        "lease-jump",
        101,
        "client-jump",
    )
    .expect("bind accepts malicious proposal on rejected code");

    let arm_result = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-jump",
            "tx-jump",
            MALICIOUS_EPOCH,
            "base-jump",
            bundle.clock.now() + Duration::from_secs(7200),
        ),
    );

    let obs = observe_post_arm(&store);
    let mut failures = Vec::new();

    match arm_result {
        Ok(response) => {
            failures.push(format!("malicious Arm accepted-invalid: {response:?}"));
        }
        Err(err) => {
            if !matches!(
                err,
                RpcError::WrongBinding | RpcError::WrongEpoch | RpcError::StaleEpoch
            ) {
                failures.push(format!("malicious Arm wrong refusal kind: {err:?}"));
            }
        }
    }

    if obs.high_water != 0 {
        failures.push(format!(
            "high-water mutation: observed {}, expected 0",
            obs.high_water
        ));
    }
    if obs.safe_mode_present {
        failures.push("safe-mode marker mutation after hostile Arm".to_string());
    }
    if obs.armed_present {
        failures.push(format!(
            "decision log mutation: {} ARMED record(s), record_count={}",
            usize::from(obs.armed_present),
            obs.record_count
        ));
    }

    assert!(
        failures.is_empty(),
        "malicious Arm must be refused without mutating high-water/log/safe-mode: {}",
        failures.join("; ")
    );
}

// 3. Watchdog-issued exact successor arms and persists once.
#[test]
fn watchdog_issued_exact_successor_arms_and_persists_once() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("epoch-successor-arm");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);

    let (mut session, established) = bind_session(
        &mut core,
        &bundle,
        "tx-successor",
        MALICIOUS_EPOCH,
        "lease-successor",
        102,
        "client-successor",
    )
    .expect("bind");

    let issued_epoch = established.epoch;

    let arm_result = handle_authenticated(
        &mut core,
        &mut session,
        &established,
        1,
        arm_request(
            "req-arm-successor",
            "tx-successor",
            issued_epoch,
            "base-successor",
            bundle.clock.now() + Duration::from_secs(7200),
        ),
    );

    let obs = observe_post_arm(&store);
    let mut failures = Vec::new();

    if issued_epoch != EXPECTED_ISSUED_EPOCH {
        failures.push(format!(
            "issuance mismatch: SessionEstablished.epoch={issued_epoch}, expected watchdog-issued {EXPECTED_ISSUED_EPOCH}"
        ));
    }

    match arm_result {
        Err(err) => failures.push(format!("Arm with established epoch failed: {err:?}")),
        Ok(response) => {
            if !matches!(response, LocalResponse::Armed { .. }) {
                failures.push(format!("Arm wrong response: {response:?}"));
            }
        }
    }

    if obs.high_water != EXPECTED_ISSUED_EPOCH {
        failures.push(format!(
            "high-water mismatch: observed {}, expected issued successor {}",
            obs.high_water, EXPECTED_ISSUED_EPOCH
        ));
    }
    if obs.record_count != 1 {
        failures.push(format!(
            "log mismatch: record_count={}, expected 1",
            obs.record_count
        ));
    }
    if !obs.armed_present {
        failures.push("log mismatch: ARMED record missing after Arm".to_string());
    }

    assert!(
        failures.is_empty(),
        "watchdog must issue exact successor 1, Arm successfully, and persist once: {}",
        failures.join("; ")
    );
}
