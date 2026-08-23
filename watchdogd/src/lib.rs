//! `agentbed-watchdogd` — **Gate 1 stub, intentionally empty.**
//!
//! The watchdog owns the single-writer decision log and executes the
//! precommitted revert (`docs/effects.md` §3a, §4). None of that exists at
//! Gate 0: the Gate 0 spike has no transaction engine, so there is nothing to
//! arm, no epoch to fence with, and no lease to grant.
//!
//! Deliberately no placeholder types: a stub that pretends to have an API
//! invites callers to depend on a shape we have not designed yet.
