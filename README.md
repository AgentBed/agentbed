# Agentbed

**Give any AI agent a governed Linux computer.**

Agentbed is an installable layer for existing Linux distributions (NixOS and Ubuntu first; Fedora/Arch later) that turns a machine into something AI agents can fully operate — and that can always get back to a known-good state.

> Status: **design phase, Revision 6 — ACCEPTED WITH CONDITIONS** after iterative independent review ([responses](docs/review-responses/)). Gate 0 (split-process spike) is open; remaining conditions are bound to gate exits in [roadmap.md](docs/roadmap.md). No code yet. The architecture is in [ADR-001](docs/adr/ADR-001-agentbed-architecture.md) with normative companions [threat-model.md](docs/threat-model.md), [effects.md](docs/effects.md) and [roadmap.md](docs/roadmap.md).

## What it does

A small set of cooperating processes — an unprivileged gateway, a minimal privileged broker, per-agent sandboxed executors, and an independent rollback watchdog (collectively "agentbedd") — plus a CLI (`agentbed`) provides four things:

1. **System API** — the whole machine exposed to agents as typed MCP tools: packages, services, users, network, config, journal and coredumps, screen and input, files, secret handles, plugins, desktops.
2. **Capability manifests** — every agent, skill, plugin and desktop declares what it may touch. The daemon compiles that into real enforcement: systemd hardening, Landlock, seccomp, nftables, a per-identity egress proxy with credential injection.
3. **Transactional change** — every host mutation goes through observe → propose → test → apply → verify → rollback, watched by an independent watchdog. Reversibility is honest and per computed effect set: declarative changes roll back automatically, data restores from tested snapshots, external effects (email, SaaS actions) are irreversible and gated on approval or explicit, narrowly scoped pre-authorization. The host's rollback strength is reported per resource, never assumed.
4. **Plugin and desktop runtime** — durable local apps (a CRM, a time tracker, a wrapped n8n) and disposable per-agent desktops with browser, takeover and snapshots, all as rootless Podman/Quadlet units under the same manifests.

## What it deliberately does not do

Chat channels, voice, memory, "dreaming", skill marketplaces and the agent loop itself belong to agent runtimes such as [Hermes Agent](https://github.com/NousResearch/hermes-agent) and [OpenClaw](https://github.com/openclaw/openclaw). Agentbed exposes itself to them over MCP; it does not replace them, fork them, or compete with them.

## Why

A prior-art sweep on 2026-08-22 found every piece in isolation — read-only system MCP servers, NixOS-only rollback tools, coding-agent sandboxes, dead "AI gets its own desktop" projects — and, as of that date, nothing that combines a write-capable system API, enforced per-agent manifests, a distro-agnostic rollback loop, and disposable GUI computers (a market hypothesis we keep re-testing, not a proven negative). Details in [docs/research/prior-art.md](docs/research/prior-art.md).

## Documents

- [Goals and user stories](docs/goals.md)
- [ADR-001 — Architecture](docs/adr/ADR-001-agentbed-architecture.md) (decision, manifests, transaction engine, milestones)
- [Prior art](docs/research/prior-art.md)
- [Review brief](docs/REVIEW.md) — how to review this design and what we want challenged

## Roadmap (gated — see [docs/roadmap.md](docs/roadmap.md))

| Gate | Outcome |
|---|---|
| G0 — Ground truth | Threat model, effect taxonomy, split-process spike |
| G1 — One safe transaction | Serialized tx engine + independent watchdog; the effects.md §4 chaos matrix passes |
| G2 — Identity, approvals, audit | Tokens, signed single-use approvals, anchored ledger |
| G3 — Enforcement | Landlock helpers, nftables, first connector → **NixOS-only alpha** |
| G4–G6 + later | Plugins, Ubuntu adapter, desktops, production, store |

## Licence

Apache-2.0. See [LICENSE](LICENSE).
