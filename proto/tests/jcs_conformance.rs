//! RFC 8785 conformance vectors.
//!
//! `docs/effects.md` §1 binds approvals, the ledger and replay checks to these
//! exact bytes, so "our canonicalization" and "RFC 8785" must not be allowed to
//! drift apart. The vectors below are the ones the RFC itself works through
//! (§3.2.3 sorting, §3.2.2.2 escaping, appendix B number formatting), plus the
//! ECMAScript `Number::toString` boundaries where a naive implementation is
//! most likely to be wrong.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use agentbed_protocol::jcs::{canonicalize, ecma_number_to_string};
use agentbed_protocol::strict;

fn canon(raw: &str) -> String {
    let value = strict::parse(raw.as_bytes()).expect("input parses");
    String::from_utf8(canonicalize(&value).expect("canonicalizes")).expect("utf-8")
}

#[test]
fn rfc8785_worked_example() {
    // RFC 8785 §3.2.3 / appendix: the RFC's own input and canonical output.
    //
    // The numbers are built here as exact doubles rather than parsed from the
    // literal text: `strict` refuses fractional literals precisely because
    // parsers disagree about which double they denote (see strict's module
    // docs), so this vector exercises canonicalization of the *values* the RFC
    // means — which is what the digest binds.
    let value = serde_json::json!({
        "numbers": [
            "333333333.33333329".parse::<f64>().expect("correctly-rounded parse"),
            1e30_f64,
            4.50_f64,
            2e-3_f64,
            1e-27_f64,
        ],
        "string": "\u{20ac}$\u{f}\nA'B\"\\\\\"/",
        "literals": [(), true, false],
    });
    let expected = concat!(
        r#"{"literals":[null,true,false],"#,
        r#""numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],"#,
        "\"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}"
    );
    let bytes = canonicalize(&value).expect("canonicalizes");
    assert_eq!(String::from_utf8(bytes).expect("utf-8"), expected);
}

#[test]
fn keys_sort_by_utf16_code_units_not_utf8_bytes() {
    // RFC 8785 §3.2.3's own example. U+10140 sorts *before* U+00E4 in UTF-16
    // (its high surrogate D800.. is below E4), but after it in UTF-8 order —
    // so a BTreeMap<String> ordering would produce the wrong bytes here.
    let input = r#"{"\u20ac":"Euro Sign","\r":"Carriage Return","\ufb33":"Hebrew Letter Dalet With Dagesh","1":"One","\ud83d\ude00":"Emoji: Grinning Face","\u0080":"Control","\u00f6":"Latin Small Letter O With Diaeresis"}"#;
    let expected = concat!(
        "{\"\\r\":\"Carriage Return\",",
        "\"1\":\"One\",",
        "\"\u{80}\":\"Control\",",
        "\"\u{f6}\":\"Latin Small Letter O With Diaeresis\",",
        "\"\u{20ac}\":\"Euro Sign\",",
        // U+1F600 is the surrogate pair D83D DE00; D83D < FB33, so the emoji
        // sorts *before* U+FB33 here, while UTF-8 byte order would put it
        // after. This assertion is the whole reason the sort is explicit.
        "\"\u{1f600}\":\"Emoji: Grinning Face\",",
        "\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
    );
    assert_eq!(canon(input), expected);
}

#[test]
fn strings_use_short_escapes_and_hex_only_for_controls() {
    assert_eq!(
        canon(r#"{"a":"\u0000\u001f\u0008\u0009\u000a\u000c\u000d\"\\"}"#),
        {
            let mut s = String::from(r#"{"a":"#);
            s.push_str("\"\\u0000\\u001f\\b\\t\\n\\f\\r\\\"\\\\\"}");
            s
        }
    );
    // Non-ASCII is emitted as UTF-8, never escaped.
    assert_eq!(canon(r#"{"k":"\u00e9\u0080"}"#), "{\"k\":\"\u{e9}\u{80}\"}");
}

#[test]
fn ecmascript_number_formatting_boundaries() {
    // Integers and the 21-digit switch to exponential form.
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
    // Shortest round-tripping digits, not 17 significant digits.
    assert_eq!(ecma_number_to_string(0.1).unwrap(), "0.1");
    assert_eq!(
        ecma_number_to_string("333333333.33333329".parse().unwrap()).unwrap(),
        "333333333.3333333"
    );
    // Extremes.
    assert_eq!(ecma_number_to_string(5e-324).unwrap(), "5e-324");
    assert_eq!(
        ecma_number_to_string(f64::MAX).unwrap(),
        "1.7976931348623157e+308"
    );
    assert_eq!(
        ecma_number_to_string(9_007_199_254_740_992.0).unwrap(),
        "9007199254740992"
    );
    // Non-finite values have no canonical form.
    assert!(ecma_number_to_string(f64::NAN).is_none());
    assert!(ecma_number_to_string(f64::INFINITY).is_none());
}

#[test]
fn canonical_form_is_stable_across_input_orderings() {
    let a = canon(r#"{"b":{"y":1,"x":[2,3]},"a":"z"}"#);
    let b = canon(r#"{"a":"z","b":{"x":[2,3],"y":1}}"#);
    assert_eq!(a, b);
    assert_eq!(a, r#"{"a":"z","b":{"x":[2,3],"y":1}}"#);
    // Array order is data, not formatting: it must survive.
    assert_ne!(canon(r#"{"a":[1,2]}"#), canon(r#"{"a":[2,1]}"#));
}
