//! `agentbed-broker` — the authorization authority.
//!
//! # The Gate 0 property
//!
//! `docs/roadmap.md` closes Gate 0 on a demonstration that **the broker, not
//! the gateway, decides**. Everything in this crate is arranged around that:
//!
//! - identity comes from the presented token, verified here
//!   ([`identity`]) — never from the caller's word and never from the peer
//!   credential, which authenticates the channel only ([`peercred`]);
//! - the manifest is re-loaded and re-checked in this process, because the
//!   gateway is untrusted by the broker (`docs/threat-model.md`, boundary 2);
//! - the RPC surface is a closed enum with one `match`, so there is no dynamic
//!   dispatch and no way to reach code by naming it ([`dispatch`]);
//! - the effect set and the RFC 8785 canonical digest are computed *here*, over
//!   the operation as this process validated it.
//!
//! The broker is the primary audit target (`docs/threat-model.md`, known weak
//! points: "a bug in the broker is a full bypass"), which is why it stays this
//! small and why the workspace lints refuse panicking constructs in it.

pub mod audit;
pub mod config;
pub mod dispatch;
pub mod identity;
pub mod peercred;
pub mod server;

pub use config::BrokerConfig;
pub use server::Server;
