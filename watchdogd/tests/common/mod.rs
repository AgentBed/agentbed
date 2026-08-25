pub mod deps;
pub mod fakes;
pub mod fence_trace;
pub mod paths;

pub use deps::dependencies_from;
pub use fakes::*;
pub use fence_trace::FenceTraceEvent;
pub use paths::*;

use std::path::{Path, PathBuf};

pub fn scratch_dir(prefix: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "{}-{}-{}",
        prefix,
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

pub fn truncate_file(path: &Path, len: usize) {
    let data = std::fs::read(path).expect("read");
    std::fs::write(path, &data[..len.min(data.len())]).expect("truncate");
}

/// Extract UTF-8 JSON payload bytes from a production-encoded frame.
pub fn frame_payload(frame: &[u8]) -> Vec<u8> {
    agentbed_watchdogd::rpc::protocol::decode_frame(frame)
        .expect("decode_frame")
        .payload
        .to_vec()
}

/// Add a top-level unknown JSON field and re-encode through normal `encode_frame`.
pub fn reframe_with_unknown_json_field(frame: &[u8]) -> Vec<u8> {
    let payload = frame_payload(frame);
    let mut value: serde_json::Value = serde_json::from_slice(&payload).expect("json payload");
    let object = value.as_object_mut().expect("json object");
    object.insert(
        "__unknown_red_field__".to_owned(),
        serde_json::Value::Bool(true),
    );
    let augmented = serde_json::to_vec(&value).expect("serialize json");
    agentbed_watchdogd::rpc::protocol::encode_frame(&augmented).expect("encode_frame")
}

/// Bump protocol `version` in JSON payload and re-encode through normal `encode_frame`.
pub fn reframe_with_unknown_protocol_version(frame: &[u8], version: u32) -> Vec<u8> {
    let payload = frame_payload(frame);
    let mut value: serde_json::Value = serde_json::from_slice(&payload).expect("json payload");
    value
        .as_object_mut()
        .expect("json object")
        .insert("version".to_owned(), serde_json::Value::from(version));
    let augmented = serde_json::to_vec(&value).expect("serialize json");
    agentbed_watchdogd::rpc::protocol::encode_frame(&augmented).expect("encode_frame")
}

/// Corrupt only the 4-byte big-endian length header to `MAX+1`.
pub fn frame_with_oversize_length_header(valid_frame: &[u8], max_payload: usize) -> Vec<u8> {
    let mut bad = valid_frame.to_vec();
    let oversize = (max_payload as u32).saturating_add(1);
    bad[..4].copy_from_slice(&oversize.to_be_bytes());
    bad
}
