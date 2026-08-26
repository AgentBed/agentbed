//! AGB-8 request-payload binding repair — discriminating RED for native review `5033388088`.
//!
//! Maps to rejected head `161fa554b07f7ec74a359a3c87e9dea5cb69c35f`.

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
    decode_request, encode_request, LocalRequest, SessionBind,
};
use agentbed_watchdogd::{CoreConfig, SessionState, WatchdogCore};
use common::{
    scratch_dir, valid_worker_group_tag, FakeBundle, FakePeerCred, DECISION_LOG_REL,
    EPOCH_HIGH_WATER_REL, SAFE_MODE_REL,
};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

const HOST_OK: &str = "host-test";
const TX_A: &str = "tx-a";
const TX_B: &str = "tx-b";
const HOST_OTHER: &str = "host-other";
const LEASE_A: &str = "lease-a";
const WORKER_TAG: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestKind {
    Arm,
    ReportHealth,
    RequestLeaseRenewal,
    Heartbeat,
    RequestDecision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MismatchField {
    Host,
    Tx,
    Epoch,
}

fn core_config(dir: &Path) -> CoreConfig {
    CoreConfig {
        store_root: dir.join("store"),
        socket_path: dir.join("watchdog.sock"),
        broker_uid: 0,
        broker_gid: 0,
        host_id: HOST_OK.to_owned(),
    }
}

fn open_core(dir: &Path, bundle: &FakeBundle) -> WatchdogCore {
    WatchdogCore::open(core_config(dir), common::dependencies_from(bundle)).expect("open core")
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

fn decision_log_reader(store: &Path) -> Option<DecisionLogReader> {
    let path = store.join(DECISION_LOG_REL);
    if path.exists() {
        Some(DecisionLogReader::open(path).expect("decision log reader"))
    } else {
        None
    }
}

struct AuthorityObservations {
    high_water: u64,
    safe_mode_present: bool,
    record_count: usize,
    armed_present: bool,
}

fn observe_authority(store: &Path) -> AuthorityObservations {
    let high_water = read_durable_high_water(store);
    let safe_mode_present = store.join(SAFE_MODE_REL).exists();
    match decision_log_reader(store) {
        Some(reader) => AuthorityObservations {
            high_water,
            safe_mode_present,
            record_count: reader.record_count(),
            armed_present: reader.contains_kind(AuthorityRecordKind::Armed),
        },
        None => AuthorityObservations {
            high_water,
            safe_mode_present,
            record_count: 0,
            armed_present: false,
        },
    }
}

fn bind_tx_a(
    core: &mut WatchdogCore,
    bundle: &FakeBundle,
    nonce: &str,
) -> (
    SessionState,
    agentbed_watchdogd::rpc::protocol::SessionEstablished,
) {
    bundle
        .peer_cred
        .enqueue_cred(FakePeerCred::broker_cred(0, 0, 4242));
    let bind = SessionBind::new(
        HOST_OK,
        TX_A,
        0,
        LEASE_A,
        valid_worker_group_tag(WORKER_TAG),
        nonce,
    );
    let (session, established) =
        SessionState::bind(core, &bundle.peer_cred, &bundle.entropy, bind).expect("bind tx-a");
    (session, established)
}

fn request_id_of(req: &LocalRequest) -> &str {
    match req {
        LocalRequest::Arm { request_id, .. }
        | LocalRequest::ReportHealth { request_id, .. }
        | LocalRequest::RequestLeaseRenewal { request_id, .. }
        | LocalRequest::Heartbeat { request_id, .. }
        | LocalRequest::RequestDecision { request_id, .. } => request_id,
    }
}

fn request_kind_of(req: &LocalRequest) -> RequestKind {
    match req {
        LocalRequest::Arm { .. } => RequestKind::Arm,
        LocalRequest::ReportHealth { .. } => RequestKind::ReportHealth,
        LocalRequest::RequestLeaseRenewal { .. } => RequestKind::RequestLeaseRenewal,
        LocalRequest::Heartbeat { .. } => RequestKind::Heartbeat,
        LocalRequest::RequestDecision { .. } => RequestKind::RequestDecision,
    }
}

fn arm_deadline(bundle: &FakeBundle) -> SystemTime {
    bundle.clock.now() + Duration::from_secs(3600)
}

fn build_request(
    kind: RequestKind,
    host: &str,
    tx: &str,
    epoch: u64,
    bundle: &FakeBundle,
    req_id: &str,
) -> LocalRequest {
    match kind {
        RequestKind::Arm => LocalRequest::arm(
            req_id,
            host,
            tx,
            epoch,
            "base-a",
            arm_deadline(bundle),
            vec!["route_present".to_owned()],
            vec![],
        ),
        RequestKind::ReportHealth => LocalRequest::report_health(req_id, host, tx, epoch),
        RequestKind::RequestLeaseRenewal => LocalRequest::request_lease_renewal(
            req_id,
            host,
            tx,
            epoch,
            LEASE_A,
            valid_worker_group_tag(WORKER_TAG),
            1,
        ),
        RequestKind::Heartbeat => LocalRequest::heartbeat(
            req_id,
            host,
            tx,
            epoch,
            LEASE_A,
            valid_worker_group_tag(WORKER_TAG),
            1,
        ),
        RequestKind::RequestDecision => LocalRequest::request_decision(req_id, host, tx, epoch),
    }
}

fn mismatch_values(
    established: &agentbed_watchdogd::rpc::protocol::SessionEstablished,
    field: MismatchField,
) -> (String, String, u64) {
    let host = established.host_id.clone();
    let tx = established.tx_id.clone();
    let epoch = established.epoch;
    match field {
        MismatchField::Host => (HOST_OTHER.to_owned(), tx, epoch),
        MismatchField::Tx => (host, TX_B.to_owned(), epoch),
        MismatchField::Epoch => (host, tx, epoch + 1),
    }
}

fn subcase_label(kind: RequestKind, field: MismatchField) -> String {
    let kind_name = match kind {
        RequestKind::Arm => "Arm",
        RequestKind::ReportHealth => "ReportHealth",
        RequestKind::RequestLeaseRenewal => "RequestLeaseRenewal",
        RequestKind::Heartbeat => "Heartbeat",
        RequestKind::RequestDecision => "RequestDecision",
    };
    let field_name = match field {
        MismatchField::Host => "host_mismatch",
        MismatchField::Tx => "tx_mismatch",
        MismatchField::Epoch => "epoch_mismatch",
    };
    format!("{kind_name}/{field_name}")
}

fn is_payload_binding_rejection(err: &RpcError) -> bool {
    matches!(err, RpcError::WrongBinding | RpcError::WrongEpoch)
}

fn kind_name(kind: RequestKind) -> &'static str {
    match kind {
        RequestKind::Arm => "Arm",
        RequestKind::ReportHealth => "ReportHealth",
        RequestKind::RequestLeaseRenewal => "RequestLeaseRenewal",
        RequestKind::Heartbeat => "Heartbeat",
        RequestKind::RequestDecision => "RequestDecision",
    }
}

// 1. All five request kinds reject payload binding mismatch without consuming counter.
#[test]
fn all_five_request_kinds_reject_payload_binding_mismatch_without_consuming_counter() {
    let kinds = [
        RequestKind::Arm,
        RequestKind::ReportHealth,
        RequestKind::RequestLeaseRenewal,
        RequestKind::Heartbeat,
        RequestKind::RequestDecision,
    ];
    let fields = [MismatchField::Host, MismatchField::Tx, MismatchField::Epoch];
    let mut failures = Vec::new();

    for kind in kinds {
        for field in fields {
            let label = subcase_label(kind, field);
            let bundle = FakeBundle::new();
            let dir = scratch_dir(&format!("payload-bind-{label}"));
            let store = core_config(&dir).store_root.clone();
            let mut core = open_core(&dir, &bundle);
            let (mut session, established) =
                bind_tx_a(&mut core, &bundle, &format!("nonce-{label}"));

            let (host, tx, epoch) = mismatch_values(&established, field);
            let hostile_id = format!("req-hostile-{label}");
            let valid_id = format!("req-valid-{label}");
            let hostile = build_request(kind, &host, &tx, epoch, &bundle, &hostile_id);
            let valid = build_request(
                kind,
                &established.host_id,
                &established.tx_id,
                established.epoch,
                &bundle,
                &valid_id,
            );

            let hostile_frame =
                encode_request(&hostile, &established, 1).expect("encode hostile frame");
            match decode_request(&hostile_frame, &mut session) {
                Ok(_) => failures.push(format!(
                    "{label}: hostile payload accepted-invalid (decode succeeded)"
                )),
                Err(err) if !is_payload_binding_rejection(&err) => {
                    failures.push(format!("{label}: hostile decode wrong error {err:?}"));
                }
                Err(_) => {}
            }

            let valid_frame = encode_request(&valid, &established, 1).expect("encode valid frame");
            match decode_request(&valid_frame, &mut session) {
                Ok(verified) => {
                    if request_id_of(verified.request()) != valid_id {
                        failures.push(format!(
                            "{label}: valid decode request_id mismatch"
                        ));
                    }
                }
                Err(err) => failures.push(format!(
                    "{label}: valid counterpart failed at counter 1 ({err:?}) — counter likely consumed by hostile payload"
                )),
            }

            let obs = observe_authority(&store);
            if obs.high_water != 0 {
                failures.push(format!(
                    "{label}: high-water={}, expected 0",
                    obs.high_water
                ));
            }
            if obs.record_count > 0 {
                failures.push(format!(
                    "{label}: decision record_count={}",
                    obs.record_count
                ));
            }
            if obs.armed_present {
                failures.push(format!("{label}: ARMED record present"));
            }
            if obs.safe_mode_present {
                failures.push(format!("{label}: safe-mode marker present"));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "payload binding mismatch failures ({} subcases): {}",
        failures.len(),
        failures.join("; ")
    );
}

// 2. Cross-transaction Arm cannot persist payload tx or mutate authority.
#[test]
fn cross_transaction_arm_cannot_persist_payload_tx_or_mutate_authority() {
    let bundle = FakeBundle::new();
    let dir = scratch_dir("payload-cross-tx-arm");
    let store = core_config(&dir).store_root.clone();
    let mut core = open_core(&dir, &bundle);
    let (mut session, established) = bind_tx_a(&mut core, &bundle, "nonce-cross-tx");

    let hostile = LocalRequest::arm(
        "req-arm-cross",
        HOST_OK,
        TX_B,
        established.epoch,
        "base-a",
        arm_deadline(&bundle),
        vec!["route_present".to_owned()],
        vec![],
    );
    let frame = encode_request(&hostile, &established, 1).expect("encode cross-tx arm");

    let decode_outcome: String;
    let handle_outcome: String;
    let mut decode_typed_refusal = false;
    let mut handle_typed_refusal = false;

    match decode_request(&frame, &mut session) {
        Ok(verified) => {
            decode_outcome = "decode_accepted_invalid".to_owned();
            handle_outcome = match core.handle_request(verified, &mut session) {
                Ok(resp) => format!("handle_accepted_invalid: {resp:?}"),
                Err(err) if is_payload_binding_rejection(&err) => {
                    handle_typed_refusal = true;
                    format!("handle_refused: {err:?}")
                }
                Err(err) => format!("handle_wrong_error: {err:?}"),
            };
        }
        Err(err) if is_payload_binding_rejection(&err) => {
            decode_typed_refusal = true;
            decode_outcome = format!("decode_refused: {err:?}");
            handle_outcome = "handle_not_attempted".to_owned();
        }
        Err(err) => {
            decode_outcome = format!("decode_wrong_error: {err:?}");
            handle_outcome = "handle_not_attempted".to_owned();
        }
    }

    let obs = observe_authority(&store);
    let mut failures = Vec::new();

    if decode_outcome == "decode_not_attempted" {
        failures.push("decode_not_attempted".to_owned());
    }
    if decode_outcome == "decode_accepted_invalid" {
        failures.push("decode_accepted_invalid".to_owned());
    }
    if decode_outcome.starts_with("decode_wrong_error:") {
        failures.push(decode_outcome.clone());
    }
    if handle_outcome.starts_with("handle_accepted_invalid:") {
        failures.push(handle_outcome.clone());
    }
    if handle_outcome.starts_with("handle_wrong_error:") {
        failures.push(handle_outcome.clone());
    }
    if decode_typed_refusal && handle_outcome != "handle_not_attempted" {
        failures.push(format!(
            "handle ran after typed decode refusal: {handle_outcome}"
        ));
    }
    if !decode_typed_refusal && !handle_typed_refusal {
        failures.push(format!(
            "no typed WrongBinding/WrongEpoch refusal at decode or handle (decode={decode_outcome}, handle={handle_outcome})"
        ));
    }

    if obs.high_water != 0 {
        failures.push(format!(
            "high-water mutation: observed {}, expected 0",
            obs.high_water
        ));
    }
    if obs.record_count > 0 {
        failures.push(format!(
            "decision log mutation: {} record(s)",
            obs.record_count
        ));
    }
    if obs.armed_present {
        failures.push("ARMED record appended for payload tx-b".to_owned());
    }
    if obs.safe_mode_present {
        failures.push("safe-mode marker present".to_owned());
    }

    assert!(
        failures.is_empty(),
        "cross-transaction arm authority mutation: {}",
        failures.join("; ")
    );
}

// 3. Positive control — exact payload binding preserves all five authenticated round trips.
#[test]
fn exact_payload_binding_preserves_all_five_authenticated_round_trips() {
    let kinds = [
        RequestKind::Arm,
        RequestKind::ReportHealth,
        RequestKind::RequestLeaseRenewal,
        RequestKind::Heartbeat,
        RequestKind::RequestDecision,
    ];
    let mut failures = Vec::new();
    let mut passed = 0usize;

    for kind in kinds {
        let label = kind_name(kind);
        let bundle = FakeBundle::new();
        let dir = scratch_dir(&format!("payload-positive-{label}"));
        let store = core_config(&dir).store_root.clone();
        let mut core = open_core(&dir, &bundle);
        let (mut session, established) =
            bind_tx_a(&mut core, &bundle, &format!("nonce-pos-{label}"));

        let req_id = format!("req-pos-{label}");
        let req = build_request(
            kind,
            &established.host_id,
            &established.tx_id,
            established.epoch,
            &bundle,
            &req_id,
        );
        let frame = encode_request(&req, &established, 1).expect("encode positive frame");
        match decode_request(&frame, &mut session) {
            Ok(verified) => {
                let id_ok = request_id_of(verified.request()) == req_id;
                let kind_ok = request_kind_of(verified.request()) == kind;
                if id_ok && kind_ok {
                    passed += 1;
                } else {
                    if !id_ok {
                        failures.push(format!("{label}: request_id mismatch after decode"));
                    }
                    if !kind_ok {
                        failures.push(format!(
                            "{label}: request kind mismatch (expected {:?}, got {:?})",
                            kind,
                            request_kind_of(verified.request())
                        ));
                    }
                }
            }
            Err(err) => failures.push(format!("{label}: decode failed: {err:?}")),
        }

        let obs = observe_authority(&store);
        if obs.high_water != 0 {
            failures.push(format!(
                "{label}: high-water={}, expected 0",
                obs.high_water
            ));
        }
        if obs.record_count > 0 {
            failures.push(format!("{label}: record_count={}", obs.record_count));
        }
    }

    if passed != 5 {
        failures.insert(0, format!("positive round-trips passed {passed}/5"));
    }

    assert!(
        failures.is_empty(),
        "exact payload binding positive control: {}",
        failures.join("; ")
    );
}
