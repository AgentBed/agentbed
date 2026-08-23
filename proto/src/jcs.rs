//! RFC 8785 (JSON Canonicalization Scheme).
//!
//! `docs/effects.md` §1 makes these bytes load-bearing: approvals, the ledger,
//! connectors and replay checks all bind *these exact bytes*, never a
//! re-serialization. The rules implemented here:
//!
//! - object members sorted by key, compared as UTF-16 code units (§3.2.3);
//! - no insignificant whitespace;
//! - strings escaped with the short forms where they exist, otherwise
//!   `\u00xx` for control characters, everything else emitted as UTF-8 (§3.2.2.2);
//! - numbers serialized as ECMAScript `Number::toString` (§3.2.2.3), with
//!   `-0` normalized to `0`.

use crate::strict::MAX_INTEROPERABLE_INT;
use serde_json::Value;
use std::fmt::Write as _;

/// Why a value could not be canonicalized.
#[derive(Debug)]
pub enum JcsError {
    /// A non-finite or non-interoperable number reached canonicalization.
    /// Values parsed through [`crate::strict`] cannot hit this.
    NonInteroperableNumber,
    /// Formatting failure (allocation).
    Format,
}

impl std::fmt::Display for JcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JcsError::NonInteroperableNumber => f.write_str("non-interoperable number"),
            JcsError::Format => f.write_str("format error"),
        }
    }
}

impl std::error::Error for JcsError {}

/// Canonicalize a value to RFC 8785 bytes.
pub fn canonicalize(value: &Value) -> Result<Vec<u8>, JcsError> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out.into_bytes())
}

fn write_value(value: &Value, out: &mut String) -> Result<(), JcsError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            let repr = number_to_string(n)?;
            out.push_str(&repr);
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            // serde_json's Map is a BTreeMap keyed by String, i.e. sorted by
            // UTF-8 bytes. RFC 8785 sorts by UTF-16 code units, which differs
            // for keys containing characters above U+FFFF, so sort explicitly.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| utf16_cmp(a, b));
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                let Some(v) = map.get(key.as_str()) else { continue };
                write_value(v, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Lexicographic comparison of UTF-16 code-unit sequences (RFC 8785 §3.2.3).
fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{9}' => out.push_str("\\t"),
            '\u{a}' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\u{d}' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn number_to_string(n: &serde_json::Number) -> Result<String, JcsError> {
    if let Some(u) = n.as_u64() {
        if u > MAX_INTEROPERABLE_INT {
            return Err(JcsError::NonInteroperableNumber);
        }
        return Ok(u.to_string());
    }
    if let Some(i) = n.as_i64() {
        if i.unsigned_abs() > MAX_INTEROPERABLE_INT {
            return Err(JcsError::NonInteroperableNumber);
        }
        return Ok(i.to_string());
    }
    let f = n.as_f64().ok_or(JcsError::NonInteroperableNumber)?;
    ecma_number_to_string(f).ok_or(JcsError::NonInteroperableNumber)
}

/// ECMAScript `Number::toString` (base 10) for a finite double.
///
/// Rust's `{:e}` yields the shortest round-tripping digit string and its
/// decimal exponent, which is precisely the `(s, k, n)` triple the ECMAScript
/// algorithm is defined over: value = s × 10^(n−k), where `s` has `k` digits.
#[must_use]
pub fn ecma_number_to_string(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    if value == 0.0 {
        // Covers -0.0: RFC 8785 normalizes negative zero to "0".
        return Some("0".to_owned());
    }

    let negative = value < 0.0;
    let magnitude = value.abs();
    let exp_form = format!("{magnitude:e}"); // e.g. "1.234e-7"
    let (mantissa, exponent) = exp_form.split_once('e')?;
    let exponent: i32 = exponent.parse().ok()?;
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };

    let k = i32::try_from(digits.len()).ok()?;
    let n = exponent.checked_add(1)?; // value = 0.digits × 10^n

    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(digits);
        for _ in 0..n.saturating_sub(k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        let split = usize::try_from(n).ok()?;
        let (head, tail) = digits.split_at_checked(split)?;
        out.push_str(head);
        out.push('.');
        out.push_str(tail);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..n.saturating_neg() {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        let (first, rest) = digits.split_at_checked(1)?;
        out.push_str(first);
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        out.push('e');
        let e = n.checked_sub(1)?;
        if e >= 0 {
            out.push('+');
        } else {
            out.push('-');
        }
        let _ = write!(out, "{}", e.unsigned_abs());
    }
    Some(out)
}
