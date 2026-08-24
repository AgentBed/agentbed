//! Broker RPC v2 compatibility boundary (`docs/protocol.md` §7).

mod support;

use agentbed_broker::digest::OperationDigest;
use agentbed_protocol::strict;
use agentbed_protocol::wire::{ErrorCode, OperationResult, Request};
use agentbed_protocol::PROTOCOL_VERSION_V1;
use agentbed_protocol::PROTOCOL_VERSION_V2;
use std::path::PathBuf;
use support::{read_response, send_frame, Harness, TOKEN_A};

const SAMPLE_TX_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn rpc_v2_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/rpc-v2")
        .join(name)
}

#[test]
fn the_v2_wire_fixture_parses_and_is_supported() {
    let raw = std::fs::read(rpc_v2_fixture("request-system-info-v2.json")).expect("read fixture");
    let value = strict::parse(&raw).expect("strict parse");
    let request: Request = serde_json::from_value(value).expect("envelope");
    assert_eq!(request.v, PROTOCOL_VERSION_V2);
    assert!(request.protocol_supported());
    assert!(request.operation_allowed());
}

fn v2_request_body(id: &str, op: &str, params: &str) -> Vec<u8> {
    format!(r#"{{"v":2,"id":"{id}","op":"{op}","auth":{{"token":"{TOKEN_A}"}},"params":{params}}}"#)
        .into_bytes()
}

#[test]
fn v2_system_info_round_trips_with_v2_response_version_and_digest_domain() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(
        &mut stream,
        &v2_request_body("01J-v2-info", "system.info", "{}"),
    );
    let response = read_response(&mut stream).expect("a response");

    assert_eq!(response.v, PROTOCOL_VERSION_V2);
    assert!(response.error.is_none());
    assert!(matches!(
        response.result,
        Some(OperationResult::SystemInfo(_))
    ));
    let binding = response.binding.expect("binding");
    assert!(binding.operation_digest.to_string().starts_with("sha256:"));

    let v1_digest = OperationDigest::of(
        PROTOCOL_VERSION_V1,
        "system.info",
        1,
        &serde_json::json!({}),
    )
    .expect("v1 digest");
    let v2_digest = OperationDigest::of(
        PROTOCOL_VERSION_V2,
        "system.info",
        1,
        &serde_json::json!({}),
    )
    .expect("v2 digest");
    assert_ne!(v1_digest.digest(), v2_digest.digest());
    assert_eq!(
        binding.operation_digest,
        *v2_digest.digest(),
        "binding must use the v2 digest domain"
    );
}

#[test]
fn unknown_protocol_version_is_refused_without_negotiation() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    let body = br#"{"v":99,"id":"01J","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#;
    send_frame(&mut stream, body);
    let response = read_response(&mut stream).expect("a response");
    assert_eq!(
        response.error.expect("error").code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn v1_rejects_v2_only_operations() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(
        &mut stream,
        br#"{"v":1,"id":"01J","op":"tx.status","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{"tx_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}}"#,
    );
    let response = read_response(&mut stream).expect("a response");
    assert_eq!(response.v, PROTOCOL_VERSION_V1);
    assert_eq!(
        response.error.expect("error").code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn v2_unknown_operation_fails_at_parse_time() {
    use agentbed_protocol::strict;
    let body = br#"{"v":2,"id":"01J","op":"tx.missing","auth":{"token":"t"},"params":{}}"#;
    let value = strict::parse(body).expect("strict json");
    assert!(serde_json::from_value::<agentbed_protocol::wire::Request>(value).is_err());
}

#[test]
fn unsupported_operation_version_is_refused_on_v2() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(
        &mut stream,
        &format!(
            r#"{{"v":2,"id":"01J","op":"system.info","op_version":99,"auth":{{"token":"{TOKEN_A}"}},"params":{{}}}}"#
        )
        .into_bytes(),
    );
    let response = read_response(&mut stream).expect("a response");
    assert_eq!(
        response.error.expect("error").code,
        ErrorCode::UnsupportedOperation
    );
}

#[test]
fn v2_tx_status_validates_params_then_refuses_execution_at_l00() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(
        &mut stream,
        &v2_request_body(
            "01J-tx-status",
            "tx.status",
            &format!(r#"{{"tx_id":"{SAMPLE_TX_ID}"}}"#),
        ),
    );
    let response = read_response(&mut stream).expect("a response");
    assert_eq!(response.v, PROTOCOL_VERSION_V2);
    assert_eq!(response.error.expect("error").code, ErrorCode::Internal);
}

#[test]
fn v2_rejects_duplicate_param_keys() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(
        &mut stream,
        &format!(
            r#"{{"v":2,"id":"01J","op":"tx.status","auth":{{"token":"{TOKEN_A}"}},"params":{{"tx_id":"{SAMPLE_TX_ID}","tx_id":"{SAMPLE_TX_ID}"}}}}"#
        )
        .into_bytes(),
    );
    let response = read_response(&mut stream).expect("a response");
    assert_eq!(
        response.error.expect("error").code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn v2_rejects_unknown_param_fields() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(
        &mut stream,
        &v2_request_body(
            "01J-bad-params",
            "tx.status",
            &format!(r#"{{"tx_id":"{SAMPLE_TX_ID}","extra":1}}"#),
        ),
    );
    let response = read_response(&mut stream).expect("a response");
    assert_eq!(
        response.error.expect("error").code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn v2_config_propose_requires_idempotency_key_and_changes() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(
        &mut stream,
        &v2_request_body("01J-propose", "config.propose", r#"{"changes":[]}"#),
    );
    let response = read_response(&mut stream).expect("a response");
    assert_eq!(
        response.error.expect("error").code,
        ErrorCode::InvalidRequest
    );
}
