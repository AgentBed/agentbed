//! Result payload types.
//!
//! These are *shapes*, not behaviour: the broker decides what goes in them.
//! In particular the safety vector below is a vocabulary of allowed values
//! (`docs/effects.md` §2) — the check "is this resource at or above the
//! manifest's minimum?" is policy and lives in the broker.

pub mod system_info;
pub mod transaction;
