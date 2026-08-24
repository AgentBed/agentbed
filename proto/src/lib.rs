//! `agentbed-protocol` — the wire contract between `agentbed-gw` and
//! `agentbed-broker`.
//!
//! # What this crate is for
//!
//! The gateway and the broker are separate processes across a trust boundary
//! (`docs/threat-model.md`, boundary 2: *the gateway is untrusted by the
//! broker*). They must nonetheless agree, byte for byte, on how a frame is
//! delimited and how an operation is canonicalized — divergence there is a
//! parsing-differential bug, which is exactly the class of bug that turns a
//! policy check into a bypass. So the encoding lives here, once, and both
//! sides execute it **independently**.
//!
//! # What this crate deliberately does NOT own
//!
//! Sharing code across the boundary is safe only while the shared code reaches
//! no conclusions. This crate therefore contains no:
//!
//! - peer-credential authorization (`SO_PEERCRED` handling),
//! - token lookup, expiry, or revocation,
//! - agent identity resolution,
//! - manifest loading or semantic validation,
//! - effect-set computation, policy precedence, safety-vector checks, quotas,
//! - observability writing, adapter behaviour, or MCP translation,
//! - **RFC 8785 canonicalization or digest computation.** Which bytes a digest
//!   covers is a security decision (`docs/protocol.md` §4), so it lives with
//!   the authority that enforces it. This crate can carry a digest on the wire
//!   and reject a malformed one; it cannot produce one.
//!
//! All of those are the broker's, and the broker performs them on its own
//! inputs. A gateway conclusion is never an input to a broker decision: see
//! [`wire::Request`], whose field set makes an asserted identity, effect set,
//! manifest digest, canonical digest, or authorization verdict
//! *unrepresentable* on the wire.
//!
//! # Modules
//!
//! - [`frame`] — 4-byte big-endian length framing with a hard maximum checked
//!   before allocation.
//! - [`strict`] — JSON parsing that rejects duplicate keys and
//!   non-interoperable numbers anywhere in the document.
//! - [`wire`] — envelope DTOs, the closed operation enum, and the
//!   machine-readable error / decision-stage enums.
//! - [`digest`] — the wire *rendering* of a digest (`sha256:<hex>`): parse and
//!   format only. Canonicalization and hashing are the broker's.

pub mod digest;
pub mod dto;
pub mod frame;
pub mod strict;
pub mod wire;

/// Wire protocol version 1 — frozen at Gate 0 (`docs/protocol.md` §2).
pub const PROTOCOL_VERSION_V1: u8 = 1;

/// Wire protocol version 2 — Gate 1 contract (`docs/protocol.md` §7).
pub const PROTOCOL_VERSION_V2: u8 = 2;

/// The only protocol version understood at Gate 0. Kept as an alias so v1
/// call sites stay readable.
pub const PROTOCOL_VERSION: u8 = PROTOCOL_VERSION_V1;

/// Every protocol version this crate recognizes on the wire.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u8] = &[PROTOCOL_VERSION_V1, PROTOCOL_VERSION_V2];
