//! RFC 8785 conformance, plus the Agentbed digest vectors.
//!
//! Both live here rather than in `proto/` because canonicalization and digest
//! computation are the broker's (`docs/protocol.md` §6): the bytes a digest
//! covers are a security decision, so the code that produces them sits with the
//! authority that enforces them.
//!
//! The RFC vectors themselves are repository-level fixtures under
//! `tests/fixtures/rfc8785/` — they describe the standard, not this crate, and
//! are asserted byte-for-byte.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use agentbed_broker::digest::{manifest_digest, OperationDigest, OPERATION_DOMAIN};
use agentbed_broker::jcs::{canonicalize, ecma_number_to_string};
use agentbed_protocol::strict;
use serde_json::json;
use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/rfc8785"
    ))
}

/// Canonicalize a fixture's input and compare to its expected bytes exactly.
fn assert_fixture(name: &str) {
    let dir = fixture_dir();
    let input = std::fs::read(dir.join(format!("{name}.input.json")))
        .unwrap_or_else(|e| panic!("read {name} input: {e}"));
    let expected = std::fs::read(dir.join(format!("{name}.expected.json")))
        .unwrap_or_else(|e| panic!("read {name} expected: {e}"));

    let value = strict::parse(&input).expect("fixture input parses");
    let canonical = canonicalize(&value).expect("fixture canonicalizes");

    assert_eq!(
        String::from_utf8_lossy(&canonical),
        String::from_utf8_lossy(&expected),
        "{name}: canonical bytes differ from the RFC 8785 fixture"
    );
    assert_eq!(canonical, expected, "{name}: byte-for-byte mismatch");
}

#[test]
fn rfc8785_key_order_is_utf16_not_utf8() {
    assert_fixture("utf16-key-order");
}

#[test]
fn rfc8785_string_escaping() {
    assert_fixture("string-escapes");
}

#[test]
fn rfc8785_worked_example() {
    // The RFC's own numbers example. Values are built as exact doubles rather
    // than parsed from the literal text: strict decoding refuses fractional
    // literals precisely because parsers disagree about which double they
    // denote (docs/protocol.md §5), so this exercises canonicalization of the
    // values the RFC means — which is what a digest binds.
    let value = json!({
        "numbers": [
            "333333333.33333329".parse::<f64>().expect("correctly-rounded parse"),
            1e30_f64,
            4.50_f64,
            2e-3_f64,
            1e-27_f64,
        ],
        "literals": [(), true, false],
    });
    let expected = concat!(
        r#"{"literals":[null,true,false],"#,
        r#""numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27]}"#
    );
    let bytes = canonicalize(&value).expect("canonicalizes");
    assert_eq!(String::from_utf8(bytes).expect("utf-8"), expected);
}

#[test]
fn ecmascript_number_formatting_boundaries() {
    assert_eq!(ecma_number_to_string(0.0).unwrap(), "0");
    assert_eq!(ecma_number_to_string(-0.0).unwrap(), "0"); // RFC 8785: -0 -> 0
    assert_eq!(ecma_number_to_string(1.0).unwrap(), "1");
    assert_eq!(ecma_number_to_string(-1.5).unwrap(), "-1.5");
    assert_eq!(
        ecma_number_to_string(1e20).unwrap(),
        "100000000000000000000"
    );
    assert_eq!(ecma_number_to_string(1e21).unwrap(), "1e+21");
    assert_eq!(ecma_number_to_string(1e-6).unwrap(), "0.000001");
    assert_eq!(ecma_number_to_string(1e-7).unwrap(), "1e-7");
    assert_eq!(ecma_number_to_string(0.1).unwrap(), "0.1");
    assert_eq!(
        ecma_number_to_string("333333333.33333329".parse().unwrap()).unwrap(),
        "333333333.3333333"
    );
    assert_eq!(ecma_number_to_string(5e-324).unwrap(), "5e-324");
    assert_eq!(
        ecma_number_to_string(f64::MAX).unwrap(),
        "1.7976931348623157e+308"
    );
    assert_eq!(
        ecma_number_to_string(9_007_199_254_740_992.0).unwrap(),
        "9007199254740992"
    );
    assert!(ecma_number_to_string(f64::NAN).is_none());
    assert!(ecma_number_to_string(f64::INFINITY).is_none());
}

// --- Agentbed digest vectors ---------------------------------------------
//
// These pin *our* construction (docs/protocol.md §4), not the RFC's, which is
// why they are not fixtures: they are only meaningful for this system.

#[test]
fn the_operation_digest_matches_its_frozen_vector() {
    // Independently derived, so the vector pins the specification rather than
    // this implementation:
    //
    //   python3 -c 'import hashlib; print(hashlib.sha256(
    //     b"agentbed.operation.v1\0"
    //     b"{\"arguments\":{},\"operation\":\"system.info\",\"operation_version\":1}"
    //   ).hexdigest())'
    let computed = OperationDigest::of("system.info", 1, &json!({})).expect("digests");
    assert_eq!(
        computed.canonical_bytes(),
        br#"{"arguments":{},"operation":"system.info","operation_version":1}"#
    );
    assert_eq!(
        computed.digest().to_string(),
        "sha256:b407fa812a98601a6a123e5f5f5005e6ddd45f98d48bec9189de22c3df5bcbf2"
    );
}

#[test]
fn the_domain_separator_is_the_frozen_string() {
    assert_eq!(OPERATION_DOMAIN, b"agentbed.operation.v1\0");
    assert_eq!(
        OPERATION_DOMAIN.last(),
        Some(&0u8),
        "the separator must end in NUL so no separator is a prefix of another"
    );
}

#[test]
fn operations_and_manifests_live_in_different_domains() {
    // The same document must not produce the same digest under two roles.
    let document = json!({
        "arguments": {}, "operation": "system.info", "operation_version": 1
    });
    let as_operation = OperationDigest::of("system.info", 1, &json!({})).expect("digests");
    let as_manifest = manifest_digest(&document).expect("digests");
    assert_ne!(as_operation.digest(), &as_manifest);
}
