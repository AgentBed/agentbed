# Contributing

Thanks for looking at Agentbed. The project is design-led and pre-alpha
(Gate 0 of [docs/roadmap.md](docs/roadmap.md)): **the most valuable
contributions right now are reviews and issues, not features.**

## Ground rules

- Design-led: contributions are reviews and issues first. See
  [docs/REVIEW.md](docs/REVIEW.md). The Gate 0 spike is now code — review it
  against the documents, which are normative: **where code and document
  disagree, the document wins until the document is changed.**
- Rust for `agentbedd` and `agentbed`; Python allowed under `adapters/` and
  `plugins/`. **`agentbed-broker` builds on Linux only** (peer credentials,
  Landlock, systemd, cgroups — ADR-001 §5.0); `agentbed-protocol` and
  `agentbed-schemas` are portable, so a non-Linux machine can still run
  `cargo test -p agentbed-protocol -p agentbed-schemas`. All manifests
  validate against `schemas/`. Every mutating change to a host goes through
  the transaction engine — including changes to Agentbed itself.

## Where things live

| What | Where |
|---|---|
| What we optimise for, user stories, non-goals | [docs/goals.md](docs/goals.md) |
| Architecture decision + revisions | [docs/adr/](docs/adr/) |
| Normative companions (threat model, effects, protocol) | [docs/](docs/) |
| Plan: gates with exit conditions | [docs/roadmap.md](docs/roadmap.md) |
| Proof a gate closed | [docs/evidence/](docs/evidence/) |
| Day-to-day tasks | GitHub issues, labeled by gate |

## Getting started

Toolchain is pinned by `rust-toolchain.toml`; `rustup` picks it up
automatically. Before pushing, run what CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test  --workspace          # includes schema example validation
```

## How to contribute

**Review findings** — the front door. Open one issue per finding using the
*Review finding* template (titled `[review] <area>: <one-line claim>`), with
the section, a concrete failure scenario, and the fix you would make.
Severity first; no praise needed.

**Bugs** in the spike code: use the *Bug report* template.

**Design changes** — open a *Design proposal* issue **before** writing code.
Changes to a normative document (`docs/adr/`, `threat-model.md`, `effects.md`,
`protocol.md`, `roadmap.md`) land as a new ADR or an explicit revision of the
affected document, so reviewers can see what changed and why. Code-only PRs
that silently diverge from the documents will be declined — change the
document first or in the same PR.

**Security issues**: never a public issue — see [SECURITY.md](SECURITY.md).

## Pull requests

- Keep PRs small and bound to one issue; link it in the description.
- CI must be green: fmt, clippy (`-D warnings`), build, tests, cargo-deny,
  and the internal doc-link check.
- New behavior needs a test; changes near gate exit conditions should say
  which condition they serve.
- Sign off your commits (`git commit -s`). By adding the `Signed-off-by:`
  line you certify the [Developer Certificate of Origin](https://developercertificate.org/)
  — that you have the right to submit the work under Apache-2.0.

## Conduct

Everyone interacting in this project's spaces is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
