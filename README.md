# Agentbed

**Give any AI agent a governed Linux computer.**

Agentbed is an installable layer for existing Linux distributions (NixOS and Ubuntu first; Fedora/Arch later) that turns a machine into something AI agents can fully operate — and that can always get back to a known-good state.

> Status: **design phase**. No code yet. The architecture is in [ADR-001](docs/adr/ADR-001-agentbed-architecture.md) and is open for review.

## What it does

One privileged daemon (`agentbedd`) plus a CLI (`agentbed`) provides four things:

1. **System API** — the whole machine exposed to agents as typed MCP tools: packages, services, users, network, config, journal and coredumps, screen and input, files, secret handles, plugins, desktops.
2. **Capability manifests** — every agent, skill, plugin and desktop declares what it may touch. The daemon compiles that into real enforcement: systemd hardening, Landlock, seccomp, nftables, a per-identity egress proxy with credential injection.
3. **Transactional change** — every mutation goes through observe → propose → test → apply → verify → rollback. NixOS generations where available; Btrfs/ZFS snapshots plus git-tracked `/etc` elsewhere. The host's *safety level* is always reported, never assumed.
4. **Plugin and desktop runtime** — durable local apps (a CRM, a time tracker, a wrapped n8n) and disposable per-agent desktops with browser, takeover and snapshots, all as rootless Podman/Quadlet units under the same manifests.

## What it deliberately does not do

Chat channels, voice, memory, "dreaming", skill marketplaces and the agent loop itself belong to agent runtimes such as [Hermes Agent](https://github.com/NousResearch/hermes-agent) and [OpenClaw](https://github.com/openclaw/openclaw). Agentbed exposes itself to them over MCP; it does not replace them, fork them, or compete with them.

## Why

A prior-art sweep on 2026-08-22 found every piece in isolation — read-only system MCP servers, NixOS-only rollback tools, coding-agent sandboxes, dead "AI gets its own desktop" projects — and nothing that combines a write-capable system API, enforced per-agent manifests, a distro-agnostic rollback loop, and disposable GUI computers. Details in [docs/research/prior-art.md](docs/research/prior-art.md).

## Documents

- [Goals and user stories](docs/goals.md)
- [ADR-001 — Architecture](docs/adr/ADR-001-agentbed-architecture.md) (decision, manifests, transaction engine, milestones)
- [Prior art](docs/research/prior-art.md)
- [Review brief](docs/REVIEW.md) — how to review this design and what we want challenged

## Roadmap (from ADR-001)

| Phase | Weeks | Outcome |
|---|---|---|
| 0 — Proof | 1–2 | Daemon skeleton, Nix adapter, `tx.*`; a bot breaks networking on purpose and the VM rolls back without a human |
| 1 — Governed | 3–5 | Manifests → enforcement, egress proxy, Landlocked helpers, desktops, audit ledger |
| 2 — Plugins + Ubuntu | 6–8 | Plugin runtime, three reference plugins, apt+Btrfs adapter, installer |

## Licence

Apache-2.0. See [LICENSE](LICENSE).
