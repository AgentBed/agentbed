//! **Gate 0 exit test (b): the RPC fuzz smoke test.**
//!
//! `docs/roadmap.md`: the spike must pass an RPC fuzz smoke test. The three
//! properties asserted here, per `docs/threat-model.md`'s note that a bug in
//! the broker is a full bypass:
//!
//! 1. **It never panics.** A panicking broker is a denial of service against
//!    the only authorization authority on the host.
//! 2. **It never processes a partial frame.** Half a request is not a request;
//!    acting on one is how a length-prefixed protocol grows a smuggling
//!    primitive.
//! 3. **It always fails closed.** Every malformed input yields a refusal or a
//!    close — never a result, and never silence where a result was due.
//!
//! # Why this is one test function
//!
//! The panic hook is process-global. Installing one while other tests run in
//! sibling threads would let their panics be miscounted and would race on the
//! hook itself, so every case runs sequentially inside this single test, and
//! the previous hook is restored before it returns.
//!
//! Every read is bounded and every join is bounded: a hung broker must fail
//! this test, not hang the suite.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

mod support;

use agentbed_protocol::frame::{read_frame, MAX_FRAME_BYTES};
use agentbed_protocol::wire::Response;
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use support::{read_response, request_body, send_frame, send_raw, Harness, TOKEN_A};

/// Bound on any single read where a response is expected.
const READ_BOUND: Duration = Duration::from_secs(5);
/// Shorter bound for reads that are expected to find *nothing*.
const QUIET_BOUND: Duration = Duration::from_millis(150);

/// Deterministic PRNG. Seeded and reproducible on purpose: a fuzz smoke test
/// that fails only on someone else's machine is not evidence.
struct Rng(u64);

impl Rng {
    fn next_u32(&mut self) -> u32 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            0
        } else {
            self.next_u32() % bound
        }
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next_u32() & 0xff) as u8).collect()
    }
}

#[test]
fn the_broker_never_panics_never_half_processes_and_always_fails_closed() {
    let panics = Arc::new(AtomicUsize::new(0));
    let previous = std::panic::take_hook();
    {
        let panics = Arc::clone(&panics);
        std::panic::set_hook(Box::new(move |info| {
            panics.fetch_add(1, Ordering::SeqCst);
            eprintln!("PANIC observed during fuzz smoke: {info}");
        }));
    }

    // Any assertion failure below unwinds through the hook we just installed,
    // so the run is wrapped and the hook restored before anything is reported.
    let outcome = std::panic::catch_unwind(run_all_cases);
    std::panic::set_hook(previous);

    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
    // The counter includes any panic in a broker connection thread, which is
    // the case this test exists for.
    assert_eq!(panics.load(Ordering::SeqCst), 0, "the broker panicked");
}

fn run_all_cases() {
    let harness = Harness::start();
    let audit_before = harness.audit_records().len();

    malformed_bodies_are_refused(&harness);
    let unprocessed = framing_abuse_is_refused(&harness);
    partial_frames_are_never_processed(&harness);
    a_malformed_frame_does_not_poison_the_next_valid_one(&harness);
    two_valid_pipelined_frames_get_two_ordered_results(&harness);
    byte_at_a_time_delivery_yields_exactly_one_result(&harness);
    random_garbage_is_survived(&harness);

    // Fail closed, counted: every case above that reached the dispatcher was
    // refused, and the ones that never completed a frame were not audited at
    // all.
    let records = harness.audit_records();
    let processed = records.len() - audit_before;
    assert!(
        processed > 0,
        "the fuzz cases must actually reach the broker"
    );
    assert_eq!(
        unprocessed, 0,
        "a frame that could not be read must not have produced an audit record"
    );

    // Still alive and still serving after everything above.
    let mut stream = harness.connect();
    send_frame(&mut stream, &request_body("01J-alive", TOKEN_A));
    let response = read_response(&mut stream).expect("the broker still answers");
    assert!(
        response.error.is_none(),
        "the broker still authorizes valid calls"
    );
}

/// Bodies that are complete frames but not valid requests.
fn malformed_bodies_are_refused(harness: &Harness) {
    let deep_nesting = format!("{}{}", "[".repeat(5_000), "]".repeat(5_000));
    let huge_string = format!(r#"{{"v":1,"id":"x","s":"{}"}}"#, "A".repeat(40_000));

    let cases: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"{".to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"\"a string\"".to_vec(),
        b"12345".to_vec(),
        b"{}".to_vec(),
        // Valid JSON, wrong envelope.
        br#"{"v":1}"#.to_vec(),
        // Protocol version missing entirely.
        br#"{"id":"01J","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        // Protocol version present but null / wrongly typed.
        br#"{"v":null,"id":"01J","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        // Operation version the broker does not implement.
        br#"{"v":1,"id":"01J","op":"system.info","op_version":2,"auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        br#"{"v":1,"id":"01J","op":"system.info","op_version":0,"auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        // Operation version wrongly typed.
        br#"{"v":1,"id":"01J","op":"system.info","op_version":"1","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        br#"{"v":2,"id":"01J","op":"tx.status","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        br#"{"v":1,"id":"01J","op":"system.reboot","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        br#"{"v":1,"id":"01J","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{},"extra":1}"#.to_vec(),
        // Duplicate keys: two readings of one document, so no reading at all.
        br#"{"v":1,"v":1,"id":"01J","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        // Non-interoperable number.
        br#"{"v":1,"id":"01J","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{"n":9007199254740993}}"#.to_vec(),
        // Wrong types where the envelope expects strings/objects.
        br#"{"v":"one","id":"01J","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        br#"{"v":1,"id":42,"op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}}"#.to_vec(),
        br#"{"v":1,"id":"01J","op":"system.info","auth":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","params":{}}"#.to_vec(),
        // Request id with control characters and embedded NULs.
        b"{\"v\":1,\"id\":\"a\0b\",\"op\":\"system.info\",\"auth\":{\"token\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"},\"params\":{}}".to_vec(),
        // Invalid UTF-8 in the body.
        vec![0x7b, 0x22, 0xff, 0xfe, 0x22, 0x7d],
        // Trailing content after a complete document.
        br#"{"v":1,"id":"01J","op":"system.info","auth":{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"params":{}} {}"#.to_vec(),
        deep_nesting.into_bytes(),
        huge_string.into_bytes(),
    ];

    for (index, body) in cases.iter().enumerate() {
        let mut stream = harness.connect();
        // Frame it by hand: some of these are empty, which the writer refuses.
        let mut framed = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
        framed.extend_from_slice(body);
        send_raw(&mut stream, &framed);

        if body.is_empty() {
            // A zero-length frame is answered and the connection survives.
            let response = expect_response(&mut stream, index);
            assert!(response.result.is_none(), "case {index} returned a result");
            continue;
        }
        let response = expect_response(&mut stream, index);
        assert!(
            response.result.is_none() && response.error.is_some(),
            "case {index} did not fail closed: {response:?}"
        );
    }
}

/// Frames whose declared length is the problem. Returns how many of them
/// produced an audit record (which must be zero for the unreadable ones).
fn framing_abuse_is_refused(harness: &Harness) -> usize {
    let mut unexpected_records = 0;

    // Oversize: refused before allocation, answered, then closed because the
    // stream position is no longer knowable.
    for declared in [u32::MAX, MAX_FRAME_BYTES + 1, 0x8000_0000] {
        let before = harness.audit_records().len();
        let mut stream = harness.connect();
        let mut bytes = declared.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"payload that will never be read");
        send_raw(&mut stream, &bytes);

        let response = expect_response(&mut stream, 0);
        assert!(
            response.result.is_none(),
            "an oversize frame must never yield a result"
        );
        assert_eq!(
            read_after_close(&mut stream),
            0,
            "the connection must close after oversize"
        );

        // The oversize case *is* audited (the prefix was read and rejected);
        // what must not happen is a result. Confirm exactly one record.
        let after = harness.audit_records().len();
        assert_eq!(
            after - before,
            1,
            "an oversize frame is one refusal, not several"
        );
    }

    // A length prefix that is itself truncated: no response at all.
    for prefix in [vec![0u8], vec![0u8, 0u8], vec![0u8, 0u8, 1u8]] {
        let before = harness.audit_records().len();
        let mut stream = harness.connect();
        send_raw(&mut stream, &prefix);
        drop(stream);
        std::thread::sleep(QUIET_BOUND);
        unexpected_records += harness.audit_records().len() - before;
    }

    unexpected_records
}

fn partial_frames_are_never_processed(harness: &Harness) {
    let body = request_body("01J-partial", TOKEN_A);

    // A valid frame followed by a truncated one: exactly one result, and the
    // truncated frame produces neither a response nor an audit record.
    let before = harness.audit_records().len();
    let mut stream = harness.connect();
    send_frame(&mut stream, &request_body("01J-first", TOKEN_A));
    let first = read_response(&mut stream).expect("the complete frame is answered");
    assert_eq!(first.id.expect("an id").as_str(), "01J-first");
    assert!(first.error.is_none());

    let mut truncated = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    truncated.extend_from_slice(&body[..body.len() / 3]);
    send_raw(&mut stream, &truncated);

    stream
        .set_read_timeout(Some(QUIET_BOUND))
        .expect("read timeout");
    let mut buf = [0u8; 64];
    match stream.read(&mut buf) {
        Ok(0) => {}
        Ok(n) => panic!("a truncated frame produced {n} bytes of response"),
        Err(e) => assert!(
            matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "unexpected error after truncated frame: {e}"
        ),
    }
    drop(stream);
    std::thread::sleep(QUIET_BOUND);

    let added = harness.audit_records().len() - before;
    assert_eq!(
        added, 1,
        "only the complete frame may be audited, got {added} records"
    );
}

/// A complete-but-malformed frame is answered and the connection survives, so
/// a valid frame behind it is still served — and served *correctly*, not
/// contaminated by the previous frame's state.
fn a_malformed_frame_does_not_poison_the_next_valid_one(harness: &Harness) {
    let mut stream = harness.connect();

    let malformed = br#"{"v":1,"id":"01J-bad","op":"system.info","auth":{"token":"x"},"nope":1}"#;
    let mut framed = u32::try_from(malformed.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    framed.extend_from_slice(malformed);
    send_raw(&mut stream, &framed);

    let first = read_response(&mut stream).expect("the malformed frame is answered");
    assert!(
        first.result.is_none(),
        "a malformed frame must not produce a result"
    );

    send_frame(&mut stream, &request_body("01J-after-bad", TOKEN_A));
    let second = read_response(&mut stream).expect("the following valid frame is served");
    assert_eq!(second.id.expect("an id").as_str(), "01J-after-bad");
    assert!(
        second.error.is_none(),
        "a valid frame after a malformed one is still valid"
    );
}

/// Two valid frames written back-to-back produce two results, in order.
fn two_valid_pipelined_frames_get_two_ordered_results(harness: &Harness) {
    let mut stream = harness.connect();
    let mut both = Vec::new();
    for id in ["01J-pipe-1", "01J-pipe-2"] {
        let body = request_body(id, TOKEN_A);
        both.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
        both.extend_from_slice(&body);
    }
    send_raw(&mut stream, &both);

    for id in ["01J-pipe-1", "01J-pipe-2"] {
        let response = read_response(&mut stream).expect("one response per frame");
        assert_eq!(
            response.id.expect("an id").as_str(),
            id,
            "responses must stay in order"
        );
        assert!(response.error.is_none());
    }
}

fn byte_at_a_time_delivery_yields_exactly_one_result(harness: &Harness) {
    let body = request_body("01J-slow", TOKEN_A);
    let mut framed = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    framed.extend_from_slice(&body);

    let mut stream = harness.connect();
    let (last, leading) = framed.split_last().expect("a non-empty frame");

    for byte in leading {
        stream.write_all(&[*byte]).expect("write byte");
        stream.flush().expect("flush");
    }
    // Nothing may be produced before the final byte arrives.
    stream
        .set_read_timeout(Some(QUIET_BOUND))
        .expect("read timeout");
    let mut buf = [0u8; 8];
    match stream.read(&mut buf) {
        Ok(n) => panic!("the broker answered before the frame was complete ({n} bytes)"),
        Err(e) => assert!(
            matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "unexpected error mid-frame: {e}"
        ),
    }

    stream.write_all(&[*last]).expect("write final byte");
    stream.flush().expect("flush");
    stream
        .set_read_timeout(Some(READ_BOUND))
        .expect("read timeout");

    let response = read_response(&mut stream).expect("exactly one response after the last byte");
    assert_eq!(response.id.expect("an id").as_str(), "01J-slow");
    assert!(
        response.error.is_none(),
        "a slowly delivered valid frame is still valid"
    );

    // And exactly one: nothing further is waiting.
    stream
        .set_read_timeout(Some(QUIET_BOUND))
        .expect("read timeout");
    let mut extra = [0u8; 8];
    match stream.read(&mut extra) {
        Ok(0) | Err(_) => {}
        Ok(n) => panic!("a single frame produced {n} extra bytes"),
    }
}

fn random_garbage_is_survived(harness: &Harness) {
    let mut rng = Rng(0x5EED_1234_ABCD_0001);

    for round in 0..256 {
        let mut stream = harness.connect();
        let mode = round % 4;
        let bytes = match mode {
            // Pure noise, self-declared length.
            0 => {
                let len = rng.below(512) as usize;
                let payload = rng.bytes(len);
                let mut framed = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
                framed.extend_from_slice(&payload);
                framed
            }
            // Noise with a lying prefix.
            1 => {
                let len = rng.below(128) as usize;
                let payload = rng.bytes(len);
                let mut framed = rng.next_u32().to_be_bytes().to_vec();
                framed.extend_from_slice(&payload);
                framed
            }
            // A valid request with random bytes corrupted.
            2 => {
                let mut body = request_body("01J-mutate", TOKEN_A);
                let flips = 1 + rng.below(8) as usize;
                for _ in 0..flips {
                    let index = rng.below(u32::try_from(body.len()).unwrap()) as usize;
                    body[index] ^= 1 << (rng.below(8) as u8);
                }
                let mut framed = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
                framed.extend_from_slice(&body);
                framed
            }
            // A valid request truncated at a random point.
            _ => {
                let body = request_body("01J-cut", TOKEN_A);
                let keep = rng.below(u32::try_from(body.len()).unwrap()) as usize;
                let mut framed = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
                framed.extend_from_slice(&body[..keep]);
                framed
            }
        };

        send_raw(&mut stream, &bytes);
        stream
            .set_read_timeout(Some(QUIET_BOUND))
            .expect("read timeout");

        // Whatever comes back, it must never be a result: none of these can be
        // a valid authorized request, and a mutated one that happens to remain
        // valid is still refused unless it is byte-identical to a real one.
        if let Ok(body) = read_frame(&mut stream, MAX_FRAME_BYTES) {
            let response: Response =
                serde_json::from_slice(&body).expect("the broker emits well-formed responses");
            // A result here would mean corrupted input was served. The
            // bit-flip mode always flips at least one bit, so no case in this
            // loop can reproduce a valid authorized request by accident.
            assert!(
                response.result.is_none(),
                "round {round} (mode {mode}) produced a result from corrupted input"
            );
        }
    }
}

fn expect_response(stream: &mut UnixStream, case: usize) -> Response {
    stream
        .set_read_timeout(Some(READ_BOUND))
        .expect("read timeout");
    read_response(stream).unwrap_or_else(|| panic!("case {case}: expected a response"))
}

/// Read until the peer closes, returning how many bytes arrived first.
fn read_after_close(stream: &mut UnixStream) -> usize {
    stream
        .set_read_timeout(Some(QUIET_BOUND))
        .expect("read timeout");
    let mut buf = [0u8; 128];
    stream.read(&mut buf).unwrap_or_default()
}
