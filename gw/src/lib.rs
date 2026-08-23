//! `agentbed-gw` — the unprivileged MCP front.
//!
//! ADR §5.0 gives the gateway one job: speak MCP, authenticate the agent,
//! validate the shape of a call, and hand it to the broker. It **holds no
//! privileges and no secrets** — no verifier material, no manifest, no policy,
//! no signing key — and the broker treats everything it says as untrusted
//! input (`docs/threat-model.md`, boundary 2).
//!
//! The practical consequence, and the reason the Gate 0 forged-gateway test is
//! meaningful: if this process were replaced wholesale by a hostile one, it
//! would gain nothing. It cannot mint an identity, because the wire has no
//! field to assert one in; it cannot pass its own verdict, because the broker
//! computes its own; it cannot widen a manifest, because it never sees one.

pub mod client;
pub mod mcp;
pub mod session;

pub use client::BrokerClient;
pub use session::Session;
