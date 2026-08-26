//! Opaque worker correlation tag — never a signal target.

use crate::error::RpcError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerGroupTag(u32);

impl WorkerGroupTag {
    pub fn try_from_raw(value: u32) -> Result<Self, RpcError> {
        if (2..=i32::MAX as u32).contains(&value) {
            Ok(Self(value))
        } else {
            Err(RpcError::MalformedRequest)
        }
    }

    #[must_use]
    pub fn raw(self) -> u32 {
        self.0
    }

    /// Constructor helper for trusted in-crate/test call sites with known-valid tags.
    #[must_use]
    #[allow(clippy::expect_used, clippy::cast_sign_loss)]
    pub fn from_trusted_i32(value: i32) -> Self {
        Self::try_from_raw(value as u32).expect("trusted worker_group_tag")
    }
}

impl Serialize for WorkerGroupTag {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl<'de> Deserialize<'de> for WorkerGroupTag {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        parse_worker_group_tag_value(&value)
            .map_err(|_| serde::de::Error::custom("MalformedRequest"))
    }
}

pub(crate) fn parse_worker_group_tag_value(
    value: &serde_json::Value,
) -> Result<WorkerGroupTag, RpcError> {
    match value {
        serde_json::Value::Number(number) => {
            if let Some(signed) = number.as_i64() {
                if signed < 0 {
                    return Err(RpcError::MalformedRequest);
                }
                let raw = u32::try_from(signed).map_err(|_| RpcError::MalformedRequest)?;
                return WorkerGroupTag::try_from_raw(raw);
            }
            if let Some(unsigned) = number.as_u64() {
                let raw = u32::try_from(unsigned).map_err(|_| RpcError::MalformedRequest)?;
                return WorkerGroupTag::try_from_raw(raw);
            }
            Err(RpcError::MalformedRequest)
        }
        _ => Err(RpcError::MalformedRequest),
    }
}
