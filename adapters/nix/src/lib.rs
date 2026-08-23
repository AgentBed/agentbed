//! `agentbed-adapter-nix` — **Gate 1 stub, intentionally empty.**
//!
//! The Nix adapter (`config.propose`, `tx.*` over `nixos-rebuild test`, and a
//! resolved safety vector reporting `generation` for `root_config`/`packages`)
//! lands at Gate 1 (`docs/roadmap.md`).
//!
//! At Gate 0 the broker uses its built-in **unresolved** adapter, which
//! resolves nothing and therefore reports `none` for every resource
//! (`docs/effects.md` §2). See `broker/src/adapter.rs`.
