//! Tool handlers.
//!
//! One per operation, reached from the dispatcher's `match` — never from a
//! table, so the set of reachable code is the set written here.

pub mod system_info;
