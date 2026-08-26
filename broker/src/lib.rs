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

// The broker is Linux-only, and says so at compile time rather than through a
// pile of "cannot find value `SO_PEERCRED`" errors.
//
// This is not an oversight to be portability-patched later: peer credentials
// (`SO_PEERCRED`/`ucred`) and the Landlock probe are Linux interfaces, and the
// design around them assumes systemd, cgroups and nftables (ADR-001 §5.0).
// macOS has analogues for the first (`LOCAL_PEERCRED`, `getpeereid`) but none
// for the rest, so a macOS broker could authenticate a channel and then enforce
// nothing — which is a worse outcome than not building.
//
// The other crates are portable: `agentbed-protocol` and `agentbed-schemas`
// build and test anywhere, and `agentbed-gw` needs only Unix sockets.
#[cfg(not(target_os = "linux"))]
compile_error!(
    "agentbed-broker targets Linux only (SO_PEERCRED, Landlock, systemd, cgroups \
     — see ADR-001 §5.0). Build it on a Linux host or VM. On other platforms the \
     portable crates still work: cargo test -p agentbed-protocol -p agentbed-schemas"
);

pub mod adapter;
pub mod config;
pub mod digest;
pub mod dispatch;
pub mod events;
pub mod host;
pub mod identity;
pub mod jcs;
pub mod manifest;
pub mod nix_host_adapter;
pub mod observability;
pub mod peercred;
pub mod policy;
pub mod quota;
pub mod safety;
pub mod server;
pub mod signals;
pub mod storage;
pub mod tools;
pub mod transaction;
pub mod watchdog;

pub use config::BrokerConfig;
pub use server::Server;
