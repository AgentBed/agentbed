//! Strict JSON parsing.
//!
//! `docs/effects.md` §1 requires that "duplicate keys and non-interoperable
//! numeric forms are rejected at validation". Both rules exist so that the
//! canonical bytes an approval or ledger record binds are the *only* reading of
//! the document:
//!
//! - **Duplicate keys.** A permissive parser silently keeps one of them. If two
//!   components keep different ones, the digest no longer identifies what was
//!   executed. Rejected anywhere in the document, at any depth.
//! - **Non-interoperable numbers.** An integer outside ±(2^53−1) does not
//!   survive an IEEE-754 double round-trip, so two implementations can
//!   canonicalize it differently. Rejected at parse time rather than silently
//!   truncated.

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

/// Largest integer magnitude that survives an IEEE-754 double round-trip.
pub const MAX_INTEROPERABLE_INT: u64 = 9_007_199_254_740_991; // 2^53 - 1

/// Why a document was refused.
#[derive(Debug)]
pub struct StrictJsonError(String);

impl StrictJsonError {
    /// The reason, safe to log (it names structure, never values).
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StrictJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StrictJsonError {}

/// Parse UTF-8 JSON bytes into a [`Value`], rejecting duplicate object keys and
/// non-interoperable numbers at every depth.
///
/// Nesting depth is bounded by `serde_json`'s recursion limit, which returns an
/// error rather than overflowing the stack.
pub fn parse(bytes: &[u8]) -> Result<Value, StrictJsonError> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let strict = StrictValue::deserialize(&mut de).map_err(|e| StrictJsonError(e.to_string()))?;
    de.end().map_err(|e| StrictJsonError(format!("trailing content: {e}")))?;
    Ok(strict.0)
}

/// A [`Value`] that enforces the strict rules while it is being built.
struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value with unique object keys and interoperable numbers")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        let magnitude = v.unsigned_abs();
        if magnitude > MAX_INTEROPERABLE_INT {
            return Err(E::custom("integer outside the interoperable range (2^53-1)"));
        }
        Ok(StrictValue(Value::from(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        if v > MAX_INTEROPERABLE_INT {
            return Err(E::custom("integer outside the interoperable range (2^53-1)"));
        }
        Ok(StrictValue(Value::from(v)))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        if !v.is_finite() {
            return Err(E::custom("non-finite number"));
        }
        serde_json::Number::from_f64(v)
            .map(|n| StrictValue(Value::Number(n)))
            .ok_or_else(|| E::custom("number not representable"))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(v)))
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(StrictValue(item)) = seq.next_element::<StrictValue>()? {
            items.push(item);
        }
        Ok(StrictValue(Value::Array(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let StrictValue(value) = map.next_value::<StrictValue>()?;
            if out.insert(key, value).is_some() {
                // Name the rule, never the key: keys can carry caller data.
                return Err(de::Error::custom("duplicate object key"));
            }
        }
        Ok(StrictValue(Value::Object(out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_documents() {
        let v = parse(br#"{"b":1,"a":[1,2,{"c":null}]}"#).unwrap();
        assert!(v.is_object());
    }

    #[test]
    fn rejects_duplicate_keys_at_top_level() {
        let err = parse(br#"{"a":1,"a":2}"#).unwrap_err();
        assert!(err.reason().contains("duplicate object key"));
    }

    #[test]
    fn rejects_duplicate_keys_when_nested() {
        let err = parse(br#"{"outer":{"deep":[{"k":1,"k":2}]}}"#).unwrap_err();
        assert!(err.reason().contains("duplicate object key"));
    }

    #[test]
    fn rejects_non_interoperable_integers() {
        assert!(parse(b"{\"n\":9007199254740993}").is_err());
        assert!(parse(b"{\"n\":-9007199254740993}").is_err());
        assert!(parse(b"{\"n\":9007199254740991}").is_ok());
    }

    #[test]
    fn rejects_invalid_utf8_and_trailing_content() {
        assert!(parse(&[0x7b, 0x22, 0xff, 0x22, 0x7d]).is_err());
        assert!(parse(b"{} {}").is_err());
        assert!(parse(b"{}\0").is_err());
    }
}
