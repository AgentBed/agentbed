# Contributing

Design-led: contributions are reviews and issues first. See `docs/REVIEW.md`. The Gate 0 spike is now code — review it against the documents, which are normative: where code and document disagree, the document wins until the document is changed.

Rust for `agentbedd` and `agentbed`; Python allowed under `adapters/` and `plugins/`. **`agentbed-broker` builds on Linux only** (peer credentials, Landlock, systemd, cgroups — ADR-001 §5.0); `agentbed-protocol` and `agentbed-schemas` are portable, so a non-Linux machine can still run `cargo test -p agentbed-protocol -p agentbed-schemas`. All manifests validate against `schemas/`. Every mutating change to a host goes through the transaction engine — including changes to Agentbed itself.
