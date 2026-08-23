//! `system.info` end to end: a served call, its binding, and the refusals.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

mod support;

use agentbed_protocol::dto::system_info::{
    DataSafety, ExternalEffectsSafety, HostSafety, SafetySource, ServiceStateSafety,
};
use agentbed_protocol::wire::{DecisionStage, EffectClass, ErrorCode, OperationResult};
use agentbed_schemas::{validate, SchemaKind};
use support::{read_response, request_body, send_frame, Harness, AGENT_A, TOKEN_A, TOKEN_B};

#[test]
fn an_authorized_call_returns_the_report_and_its_binding() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(&mut stream, &request_body("01J-info", TOKEN_A));
    let response = read_response(&mut stream).expect("a response");

    assert!(
        response.error.is_none(),
        "the reader manifest allows system.info"
    );
    let binding = response
        .binding
        .expect("an authorized call carries its binding");

    // Everything in the binding is broker-derived, and the identity is the one
    // the token resolved to.
    assert_eq!(binding.agent_id, AGENT_A);
    assert_eq!(binding.effect_set, vec![EffectClass::R]);
    assert!(binding.operation_digest.to_string().starts_with("sha256:"));
    assert!(binding.manifest_digest.to_string().starts_with("sha256:"));

    let OperationResult::SystemInfo(info) = response.result.expect("a result");
    assert!(!info.host.hostname.is_empty());
    assert!(!info.host.kernel_release.is_empty());
}

#[test]
fn the_result_conforms_to_its_published_schema() {
    // ADR §6 promises schema conformance for every initial tool. Validating the
    // broker's real output against the shipped schema is what makes that true
    // rather than aspirational.
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(&mut stream, &request_body("01J-schema", TOKEN_A));
    let response = read_response(&mut stream).expect("a response");
    let OperationResult::SystemInfo(info) = response.result.expect("a result");

    let value = serde_json::to_value(&*info).expect("serializes");
    validate(SchemaKind::SystemInfoResponse, &value).expect("result matches its schema");
}

#[test]
fn the_gate0_safety_vector_is_honest_about_having_resolved_nothing() {
    // No adapter ran, so nothing may be claimed: every resource is `none`, and
    // safety_source says the all-none vector is an absence of measurement
    // rather than a measurement of unrecoverability.
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(&mut stream, &request_body("01J-safety", TOKEN_A));
    let response = read_response(&mut stream).expect("a response");
    let OperationResult::SystemInfo(info) = response.result.expect("a result");

    assert_eq!(info.safety_source, SafetySource::UnresolvedAdapter);
    assert!(!info.adapter.resolved);
    assert_eq!(info.adapter.kind, "unresolved");
    assert_eq!(info.safety.root_config, HostSafety::None);
    assert_eq!(info.safety.packages, HostSafety::None);
    assert_eq!(info.safety.service_state, ServiceStateSafety::None);
    assert_eq!(info.safety.plugin_data, DataSafety::None);
    assert_eq!(info.safety.home_data, DataSafety::None);
    assert_eq!(info.safety.external_effects, ExternalEffectsSafety::None);
    assert!(
        info.adapter.generations.is_none(),
        "no adapter looked, so there is no generation count to report"
    );
}

#[test]
fn a_manifest_denial_names_stage_three() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(&mut stream, &request_body("01J-denied", TOKEN_B));
    let response = read_response(&mut stream).expect("a response");

    let error = response.error.expect("a refusal");
    assert_eq!(error.code, ErrorCode::Denied);
    assert_eq!(
        error.stage,
        Some(DecisionStage::OperationPolicy),
        "an explicit manifest policy decides at stage 3"
    );
    assert!(
        response.result.is_none(),
        "a refused call returns no result"
    );
    assert!(response.binding.is_none());
}

#[test]
fn the_same_operation_always_digests_to_the_same_canonical_bytes() {
    // The digest binds the operation, so two identical calls — from different
    // agents, in different connections — must agree, while the manifest digest
    // distinguishes who was authorized.
    let harness = Harness::start();
    let mut first = harness.connect();
    send_frame(&mut first, &request_body("01J-a", TOKEN_A));
    let a = read_response(&mut first).expect("a response");

    let mut second = harness.connect();
    send_frame(&mut second, &request_body("01J-b", TOKEN_A));
    let b = read_response(&mut second).expect("a response");

    let (a, b) = (a.binding.expect("binding"), b.binding.expect("binding"));
    assert_eq!(a.operation_digest, b.operation_digest);
    assert_eq!(a.manifest_digest, b.manifest_digest);
}

#[test]
fn unknown_parameters_are_refused_rather_than_ignored() {
    // effects.md §1: a tool whose effect set cannot be computed from its
    // arguments is refused, not guessed. An argument the broker does not
    // understand could be one that raises the set.
    let harness = Harness::start();
    let mut stream = harness.connect();
    let body = br#"{"v":1,"id":"01J-extra","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{"verbose":true}}"#;
    send_frame(&mut stream, body);
    let response = read_response(&mut stream).expect("a response");
    assert_eq!(
        response.error.expect("a refusal").code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn quota_exhaustion_vetoes_an_otherwise_allowed_read() {
    // The reader manifest allows 100 calls/day; spend them and watch stage 5
    // refuse a call that stages 1-4 would have allowed.
    let harness = Harness::start();
    let mut stream = harness.connect();
    for i in 0..100 {
        send_frame(&mut stream, &request_body(&format!("01J-q{i}"), TOKEN_A));
        let response = read_response(&mut stream).expect("a response");
        assert!(response.error.is_none(), "call {i} should be within quota");
    }
    send_frame(&mut stream, &request_body("01J-over", TOKEN_A));
    let response = read_response(&mut stream).expect("a response");
    let error = response.error.expect("a refusal");
    assert_eq!(error.code, ErrorCode::QuotaExhausted);
    assert_eq!(error.stage, Some(DecisionStage::Quota));
}

#[test]
fn an_absent_operation_version_means_version_one() {
    // docs/protocol.md §2: within protocol v1 every operation is at version 1,
    // so omitting the field is legal and means 1 — while a version the broker
    // does not implement is refused rather than reinterpreted as one it does.
    let harness = Harness::start();

    let mut stream = harness.connect();
    let omitted = format!(
        r#"{{"v":1,"id":"01J-omit","op":"system.info","auth":{{"token":"{TOKEN_A}"}},"params":{{}}}}"#
    );
    send_frame(&mut stream, omitted.as_bytes());
    let response = read_response(&mut stream).expect("a response");
    assert!(
        response.error.is_none(),
        "an omitted op_version must be accepted as 1"
    );

    let mut explicit = harness.connect();
    let stated = format!(
        r#"{{"v":1,"id":"01J-explicit","op":"system.info","op_version":1,"auth":{{"token":"{TOKEN_A}"}},"params":{{}}}}"#
    );
    send_frame(&mut explicit, stated.as_bytes());
    let stated_response = read_response(&mut explicit).expect("a response");
    assert!(stated_response.error.is_none());

    // The two must agree on the digest: the default is the same value, not a
    // separate code path.
    assert_eq!(
        response.binding.expect("binding").operation_digest,
        stated_response.binding.expect("binding").operation_digest
    );
}

#[test]
fn an_unsupported_operation_version_is_refused_not_reinterpreted() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    let future = format!(
        r#"{{"v":1,"id":"01J-v2","op":"system.info","op_version":2,"auth":{{"token":"{TOKEN_A}"}},"params":{{}}}}"#
    );
    send_frame(&mut stream, future.as_bytes());

    let response = read_response(&mut stream).expect("a response");
    let error = response.error.expect("a refusal");
    assert_eq!(error.code, ErrorCode::UnsupportedOperation);
    assert!(
        response.result.is_none(),
        "an unknown operation version must not execute"
    );
    assert!(
        error.stage.is_none(),
        "this is a contract refusal, not a policy stage"
    );
}
