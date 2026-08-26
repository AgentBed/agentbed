//! `agentbed-watchdogd` — Gate 1 watchdog decision authority owner.

pub mod core;
pub mod durability_store;
pub mod error;
pub mod fencing;
pub mod interfaces;
pub mod peercred;
pub mod read_model;
pub mod rpc;
pub mod session;
pub mod topology;
pub mod worker_group_tag;

pub use core::{CoreConfig, WatchdogCore, WATCHDOG_MOUNT_ROOT};
pub use error::RpcError;
pub use fencing::UnavailableProcessGroupFencer;
pub use session::SessionState;
pub use worker_group_tag::WorkerGroupTag;
