//! **Gate 0 exit test (a): the broker, not the gateway, is the authorization
//! authority.**
//!
//! `docs/roadmap.md` closes Gate 0 partly on this demonstration: *a forged
//! gateway request without valid identity is refused by the broker*.
//!
//! Every request below arrives on the **trusted socket** with **valid peer
//! credentials** — `SO_PEERCRED` passes, the uid is the allowed one, the
//! filesystem permissions were satisfied. Nothing distinguishes these
//! connections from the real gateway's, because at this layer nothing can: one
//! gateway process serves many agents, so the channel cannot identify the
//! caller. If peer credentials were treated as authorization, every case here
//! would succeed.
//!
//! What each case asserts is that the broker refuses anyway, and that the one
//! authorized case resolves to the identity the *token* names — read from the
//! audit record the handler actually acted on, not from the response.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

mod support;

use agentbed_protocol::wire::{DecisionStage, ErrorCode};
use support::{
    read_response, request_body, send_frame, Harness, AGENT_A, AGENT_B, TOKEN_A, TOKEN_B,
    TOKEN_REVOKED, TOKEN_UNKNOWN,
};

/// Send one frame on a fresh connection and return the response.
fn exchange(harness: &Harness, body: &[u8]) -> agentbed_protocol::wire::Response {
    let mut stream = harness.connect();
    send_frame(&mut stream, body);
    read_response(&mut stream).expect("the broker answers every complete frame")
}

#[test]
fn a_request_with_no_credential_is_refused() {
    let harness = Harness::start();
    // A frame that is well-formed apart from carrying no auth at all — the
    // shape a gateway would send if it believed the socket vouched for it.
    let body = br#"{"v":1,"id":"01J-noauth","op":"system.info","params":{}}"#;
    let response = exchange(&harness, body);

    assert!(response.result.is_none(), "no identity, no result");
    assert!(response.binding.is_none());
    assert_eq!(
        response.error.expect("a refusal").code,
        ErrorCode::InvalidRequest
    );

    let records = harness.wait_for_records(1);
    assert!(
        records.iter().all(|r| r.agent_id.is_none()),
        "nothing may be attributed to an agent that was never identified"
    );
}

#[test]
fn an_unknown_or_revoked_credential_is_refused_identically() {
    let harness = Harness::start();

    for (label, token) in [("unknown", TOKEN_UNKNOWN), ("revoked", TOKEN_REVOKED)] {
        let response = exchange(&harness, &request_body("01J-cred", token));
        let error = response.error.expect("a refusal");
        assert_eq!(error.code, ErrorCode::Unauthenticated, "{label} token");
        assert!(
            response.result.is_none(),
            "{label} token must yield no result"
        );
        assert!(
            response.binding.is_none(),
            "{label} token must yield no binding"
        );
        // Both answer `unauthenticated` with no further detail: distinguishing
        // them would confirm that a credential exists, or once did.
        assert_eq!(error.stage, None, "{label} token");
    }
}

#[test]
fn a_caller_cannot_assert_an_identity_alongside_a_valid_token() {
    // Token A is genuinely valid. The frame additionally claims to be agent B.
    // There is no recognized field for that claim — the envelope is
    // deny_unknown_fields — so the frame is refused outright at the parser
    // rather than being resolved to either identity.
    let harness = Harness::start();
    let body = format!(
        r#"{{"v":1,"id":"01J-claim","op":"system.info","auth":{{"token":"{TOKEN_A}"}},"agent_id":"{AGENT_B}","params":{{}}}}"#
    );
    let response = exchange(&harness, body.as_bytes());

    assert_eq!(
        response.error.expect("a refusal").code,
        ErrorCode::InvalidRequest
    );
    assert!(response.result.is_none());

    let records = harness.wait_for_records(1);
    let last = records.last().expect("an audit record");
    assert_eq!(last.reason, "envelope_rejected");
    assert!(
        last.agent_id.is_none(),
        "a rejected envelope resolves to nobody"
    );
}

#[test]
fn a_caller_cannot_supply_its_own_verdict_or_binding() {
    // The other half of the same property: a compromised gateway cannot hand
    // the broker a pre-computed authorization, effect set, manifest digest or
    // canonical digest. Each is a broker output and none is representable as
    // an input.
    let harness = Harness::start();
    for injected in [
        r#""authorized":true"#,
        r#""effect_set":["R"]"#,
        r#""manifest_digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000""#,
        r#""operation_digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000""#,
        r#""decision":{"allowed":true,"stage":"class_ceiling"}"#,
        r#""binding":{"agent_id":"anyone"}"#,
    ] {
        let body = format!(
            r#"{{"v":1,"id":"01J-verdict","op":"system.info","auth":{{"token":"{TOKEN_A}"}},{injected},"params":{{}}}}"#
        );
        let response = exchange(&harness, body.as_bytes());
        assert_eq!(
            response.error.expect("a refusal").code,
            ErrorCode::InvalidRequest,
            "the broker accepted a caller-supplied verdict field: {injected}"
        );
        assert!(response.result.is_none(), "{injected} produced a result");
    }
}

#[test]
fn the_broker_evaluates_the_manifest_itself() {
    // Token B is valid and resolves to an agent whose manifest denies
    // system.info at stage 3. No gateway was involved in that decision — the
    // broker loaded the manifest from its own directory and applied the ladder.
    let harness = Harness::start();
    let response = exchange(&harness, &request_body("01J-manifest", TOKEN_B));

    let error = response.error.expect("a refusal");
    assert_eq!(error.code, ErrorCode::Denied);
    assert_eq!(error.stage, Some(DecisionStage::OperationPolicy));
    assert!(response.result.is_none());

    let records = harness.wait_for_records(1);
    let last = records.last().expect("an audit record");
    assert_eq!(last.agent_id.as_deref(), Some(AGENT_B));
    assert_eq!(last.reason, "operation_denied_by_manifest");
    assert!(!last.allowed);
    assert!(
        last.manifest_digest.is_some(),
        "the decision is bound to a manifest the broker read"
    );
}

#[test]
fn the_authorized_control_resolves_to_the_identity_the_token_names() {
    // The control for every case above: the same socket, the same peer
    // credentials, a valid token — and now it works. The resolved identity is
    // read from the audit record, i.e. the value the handler acted on.
    let harness = Harness::start();
    let response = exchange(&harness, &request_body("01J-control", TOKEN_A));

    assert!(
        response.error.is_none(),
        "a valid credential must be served"
    );
    let binding = response.binding.expect("a binding");
    assert_eq!(binding.agent_id, AGENT_A);

    let records = harness.wait_for_records(1);
    let authorized = records
        .iter()
        .rev()
        .find(|r| r.request_id.as_deref() == Some("01J-control"))
        .expect("an audit record for the control call");
    assert!(authorized.allowed);
    assert_eq!(
        authorized.agent_id.as_deref(),
        Some(AGENT_A),
        "the handler acted as the identity the token resolved to"
    );
    assert_eq!(
        authorized.agent_id.as_deref(),
        Some(binding.agent_id.as_str())
    );
    assert!(authorized.operation_digest.is_some());
    assert!(authorized.manifest_digest.is_some());
}

#[test]
fn refusals_disclose_nothing_about_the_host() {
    // A refused caller learns that it was refused and which precedence stage
    // decided — never a hostname, a kernel version, an adapter state, or which
    // agents exist.
    let harness = Harness::start();
    for token in [TOKEN_UNKNOWN, TOKEN_REVOKED, TOKEN_B] {
        let response = exchange(&harness, &request_body("01J-quiet", token));
        let encoded = serde_json::to_string(&response).expect("serializes");
        assert!(response.result.is_none());
        for leak in [
            "hostname", "kernel", "safety", "adapter", "landlock", AGENT_A,
        ] {
            assert!(!encoded.contains(leak), "refusal leaked {leak}: {encoded}");
        }
    }
}
