//! A minimal MCP server over stdio.
//!
//! Enough of the protocol for the Gate 0 spike: `initialize`, `tools/list`,
//! `tools/call`. Streamable HTTP, sessions and rate limits are the gateway's
//! job too (ADR §5.0) and land with Gate 2's stdio-shim-to-socket work.

use crate::session::{CallOutcome, Session};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The MCP revision this gateway speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC error codes used here.
mod codes {
    pub(super) const PARSE_ERROR: i32 = -32700;
    pub(super) const INVALID_REQUEST: i32 = -32600;
    pub(super) const METHOD_NOT_FOUND: i32 = -32601;
    pub(super) const INVALID_PARAMS: i32 = -32602;
}

/// An incoming JSON-RPC message.
#[derive(Debug, Deserialize)]
struct Incoming {
    #[serde(default)]
    jsonrpc: String,
    /// Absent for notifications.
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

/// An outgoing JSON-RPC response.
#[derive(Debug, Serialize)]
struct Outgoing {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// Handle one line of stdin, returning the line to write to stdout.
///
/// `None` means "no reply", which is correct for notifications — answering one
/// is a protocol violation, not a courtesy.
#[must_use]
pub fn handle_line(session: &mut Session, line: &str) -> Option<String> {
    if line.trim().is_empty() {
        return None;
    }
    let Ok(message) = serde_json::from_str::<Incoming>(line) else {
        return Some(error_line(
            Value::Null,
            codes::PARSE_ERROR,
            "malformed JSON-RPC message",
        ));
    };
    if message.jsonrpc != "2.0" {
        let id = message.id.clone().unwrap_or(Value::Null);
        return Some(error_line(
            id,
            codes::INVALID_REQUEST,
            "jsonrpc must be \"2.0\"",
        ));
    }

    let Some(id) = message.id.clone() else {
        // Notifications: acknowledged by doing nothing.
        return None;
    };

    let response = match message.method.as_str() {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(tools_list_result(session)),
        "tools/call" => tools_call(session, &message.params),
        "ping" => Ok(json!({})),
        _ => Err((
            codes::METHOD_NOT_FOUND,
            format!("unknown method: {}", message.method),
        )),
    };

    Some(match response {
        Ok(result) => result_line(id, result),
        Err((code, message)) => error_line(id, code, &message),
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "agentbed-gw", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tools_list_result(session: &Session) -> Value {
    json!({ "tools": session.tool_descriptors() })
}

fn tools_call(session: &mut Session, params: &Value) -> Result<Value, (i32, String)> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err((
            codes::INVALID_PARAMS,
            "tools/call requires a tool name".to_owned(),
        ));
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match session.call_tool(name, &arguments) {
        CallOutcome::Result(value) => Ok(json!({
            "content": [{ "type": "text", "text": value.to_string() }],
            "structuredContent": value,
            "isError": false,
        })),
        // A refusal is a *tool* error, not a transport error: the agent asked a
        // well-formed question and the broker said no. Reporting it as
        // isError keeps the refusal visible to the model instead of looking
        // like a broken connection.
        CallOutcome::Refused(reason) => Ok(json!({
            "content": [{ "type": "text", "text": reason }],
            "isError": true,
        })),
        CallOutcome::UnknownTool => Err((codes::INVALID_PARAMS, format!("unknown tool: {name}"))),
        CallOutcome::InvalidArguments(reason) => Err((codes::INVALID_PARAMS, reason)),
    }
}

fn result_line(id: Value, result: Value) -> String {
    let outgoing = Outgoing {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    };
    serde_json::to_string(&outgoing).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal error"}}"#
            .to_owned()
    })
}

fn error_line(id: Value, code: i32, message: &str) -> String {
    let outgoing = Outgoing {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(RpcError {
            code,
            message: message.to_owned(),
        }),
    };
    serde_json::to_string(&outgoing).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal error"}}"#
            .to_owned()
    })
}
