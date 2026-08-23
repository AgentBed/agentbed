//! Transport-layer behaviour: socket permissions, peer credentials, and the
//! per-frame fail-closed rules.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

mod support;

use agentbed_protocol::wire::ErrorCode;
use std::os::unix::fs::PermissionsExt as _;
use support::{
    assert_closed_without_response, read_response, request_body, send_frame, send_raw, Harness,
    TOKEN_A, TOKEN_UNKNOWN,
};

#[test]
fn socket_and_directory_are_private_to_the_owner() {
    let harness = Harness::start();
    let socket = harness.socket_path();
    let mode = std::fs::metadata(socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "socket must not be reachable by other users");

    let parent = socket.parent().unwrap();
    let parent_mode = std::fs::metadata(parent).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        parent_mode, 0o700,
        "socket directory must not be traversable by others"
    );
}

#[test]
fn a_valid_peer_is_recorded_but_does_not_authorize() {
    // The connection succeeds (SO_PEERCRED passes: same uid) and the request is
    // still refused, because the token is unknown. Peer credentials
    // authenticate the channel; they never authorize a call.
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_frame(&mut stream, &request_body("01J-peer", TOKEN_UNKNOWN));

    let response = read_response(&mut stream).expect("a response");
    assert_eq!(response.error.unwrap().code, ErrorCode::Unauthenticated);

    let records = harness.wait_for_records(1);
    let record = records.last().expect("an audit record");
    assert!(
        record.agent_id.is_none(),
        "no identity may be recorded for an unknown token"
    );
    assert!(
        record.peer.uid == unsafe_getuid(),
        "the channel's peer is recorded for attribution"
    );
}

#[test]
fn zero_length_frame_is_answered_and_the_connection_survives() {
    // The length prefix was consumed and the body is empty, so the stream
    // position is still known: answer, keep reading.
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_raw(&mut stream, &0u32.to_be_bytes());

    let response = read_response(&mut stream).expect("a response to the empty frame");
    assert_eq!(response.error.unwrap().code, ErrorCode::InvalidRequest);

    send_frame(&mut stream, &request_body("01J-after-zero", TOKEN_A));
    let next = read_response(&mut stream).expect("the connection still serves requests");
    assert_eq!(next.id.unwrap().as_str(), "01J-after-zero");
}

#[test]
fn oversize_declared_length_closes_the_connection() {
    // The oversized body was never read, so the following bytes cannot be
    // located. Answering and continuing would let the peer choose the framing
    // of whatever comes next.
    let harness = Harness::start();
    let mut stream = harness.connect();
    send_raw(&mut stream, &u32::MAX.to_be_bytes());

    let response = read_response(&mut stream).expect("a refusal before the close");
    assert_eq!(response.error.unwrap().code, ErrorCode::InvalidRequest);
    assert_closed_without_response(&mut stream);
}

#[test]
fn truncated_frame_yields_no_response_and_no_audit_record() {
    let harness = Harness::start();
    let before = harness.audit_records().len();

    let mut stream = harness.connect();
    let body = request_body("01J-truncated", TOKEN_A);
    let mut bytes = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    bytes.extend_from_slice(&body[..body.len() / 2]);
    send_raw(&mut stream, &bytes);
    drop(stream);

    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(
        harness.audit_records().len(),
        before,
        "a partial frame must not be processed, so it must not be audited"
    );
}

#[test]
fn pipelined_frames_get_ordered_responses() {
    let harness = Harness::start();
    let mut stream = harness.connect();
    for id in ["01J-one", "01J-two", "01J-three"] {
        send_frame(&mut stream, &request_body(id, TOKEN_A));
    }
    for id in ["01J-one", "01J-two", "01J-three"] {
        let response = read_response(&mut stream).expect("a response per frame, in order");
        assert_eq!(response.id.unwrap().as_str(), id);
    }
}

fn unsafe_getuid() -> u32 {
    // Test-only: the broker's own uid, to compare against SO_PEERCRED.
    #[allow(unsafe_code)]
    unsafe {
        libc::getuid()
    }
}
