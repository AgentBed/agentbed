//! One agent session.
//!
//! # What the gateway holds
//!
//! A socket path, the published schemas, and — for the length of the session —
//! the bearer token its agent presented. That is all.
//!
//! It holds **no verifier**: no token hashes, no signing key, no manifest, no
//! policy. It cannot tell a valid token from an invalid one, and it cannot
//! construct an identity, because identity is what the broker derives from the
//! token and the wire has no field to assert one in
//! (`agentbed_protocol::wire`).
//!
//! So the gateway's schema validation is a *fast rejection*, never an
//! authorization: the broker validates the same call again against the same
//! schema and does not care what happened here. Dropping this check entirely
//! would cost latency and error quality — never safety.

use crate::client::BrokerClient;
use agentbed_protocol::wire::{OperationResult, Token};
use agentbed_schemas::{validate, SchemaKind};
use serde_json::{json, Value};

/// The one tool the Gate 0 gateway exposes.
const SYSTEM_INFO: &str = "system.info";

/// What happened to a `tools/call`.
#[derive(Debug)]
pub enum CallOutcome {
    /// The broker answered.
    Result(Value),
    /// The broker refused, or could not be reached.
    Refused(String),
    /// The gateway does not expose this tool.
    UnknownTool,
    /// The arguments did not match the tool's published schema.
    InvalidArguments(String),
}

/// A live MCP session bound to one agent's token.
#[derive(Debug)]
pub struct Session {
    broker: BrokerClient,
    token: Token,
    next_request: u64,
}

impl Session {
    /// Start a session for an agent that presented `token`.
    #[must_use]
    pub fn new(broker: BrokerClient, token: Token) -> Self {
        Session {
            broker,
            token,
            next_request: 0,
        }
    }

    /// Tool descriptors for `tools/list`, with the published input schema.
    #[must_use]
    pub fn tool_descriptors(&self) -> Value {
        json!([{
            "name": SYSTEM_INFO,
            "description": "Host facts, host adapter state, the per-resource \
        rollback safety vector, and the probed Landlock ABI. Read-only (effect set {R}).",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }])
    }

    /// Validate a tool call and relay it to the broker.
    pub fn call_tool(&mut self, name: &str, arguments: &Value) -> CallOutcome {
        if name != SYSTEM_INFO {
            return CallOutcome::UnknownTool;
        }
        if let Err(e) = validate(SchemaKind::SystemInfoRequest, arguments) {
            return CallOutcome::InvalidArguments(e.to_string());
        }

        let request_id = self.next_request_id();
        let body = json!({
            "v": 1,
            "id": request_id,
            "op": SYSTEM_INFO,
            "auth": { "token": self.token.expose() },
            "params": arguments,
        });
        let Ok(encoded) = serde_json::to_vec(&body) else {
            return CallOutcome::Refused("gateway could not encode the request".to_owned());
        };

        match self.broker.call(&encoded) {
            Ok(response) => {
                if let Some(error) = response.error {
                    // Relay the broker's machine-readable verdict verbatim.
                    // The gateway must not soften, retry or reinterpret a
                    // refusal — it has no basis on which to disagree.
                    let stage = error
                        .stage
                        .map_or_else(|| "-".to_owned(), |s| s.ordinal().to_string());
                    return CallOutcome::Refused(format!(
                        "broker refused: code={:?} precedence_stage={stage}",
                        error.code
                    ));
                }
                match response.result {
                    Some(OperationResult::SystemInfo(info)) => serde_json::to_value(&*info)
                        .map_or_else(
                            |_| {
                                CallOutcome::Refused(
                                    "gateway could not encode the result".to_owned(),
                                )
                            },
                            CallOutcome::Result,
                        ),
                    Some(_) => CallOutcome::Refused(
                        "broker returned an operation this gateway does not expose".to_owned(),
                    ),
                    None => CallOutcome::Refused(
                        "broker returned neither a result nor an error".to_owned(),
                    ),
                }
            }
            Err(e) => CallOutcome::Refused(format!("broker unreachable: {e}")),
        }
    }

    fn next_request_id(&mut self) -> String {
        self.next_request = self.next_request.saturating_add(1);
        format!("gw-{:016x}", self.next_request)
    }
}
