# ADR-001: Agentbed — an AI-native layer for Linux

**Status:** Proposed
**Date:** 2026-08-22
**Deciders:** L-P (owner). Reviewers: Hermes "architect" bot, Claude.
**Scope:** Phase 0–2 (first 8 weeks). Later phases are sketched, not decided.

---

## 1. Context

We want machines that AI agents operate as first-class users: any agent (Hermes bots on Node-D, Claude Code, ChatGPT, local models) can be handed a governed computer, change anything on it, and the OS can always get back to a known-good state. Humans supervise and take over rather than drive.

Prior-art sweep (2026-08-22, three independent searches) found every piece in isolation and nothing that combines them:

| Need | Exists today | Gap |
|---|---|---|
| Chat/voice/SMS control, cron, memory, "dreaming" | OpenClaw (387k★), Hermes Agent (234k★) | None — **build on them, not beside them** |
| Linux desktop control for agents | agent-sh/computer-use-linux (AT-SPI, ships a Hermes skill) | Wrap, don't rebuild |
| System MCP servers | Red Hat linux-mcp-server (read-only by design), mvo5/systemd-mcp (varlink, "no guardrails"), openSUSE/systemd-mcp (polkit) | None are write-capable **and** governed |
| NixOS apply/rollback | osModa SafeSwitch (~110★), nix-agent, ClawNix (stalled) | NixOS-only; osModa admits its approval layer is bypassed |
| Agent sandboxing + secret proxying | nono (3.7k★), Anthropic srt, greywall | Scoped to coding agents in a repo, unaware of OS generations/snapshots |
| Per-bot disposable GUI computers | Bytebot (dead since 09/2025), Cua (VC, cloud upsell) | No self-hosted, snapshot-able, Linux-native option |
| Hardened "box per agent" | NVIDIA NemoClaw (alpha, DGX/WSL), Red Hat Tank-OS (forbids self-modification) | Incumbents treat self-modification as unsafe — that is the bar to clear |
| Spec identical to ours | ElephantClock "Agentic Linux Runtime" whitepaper | Paper only, no code, no licence |

Constraints: one owner plus AI pair-programming; must run on Ubuntu (Node-D), NixOS (bot VMs on the spare Supermicro node), later Fedora/Arch; no GPU required; open source (Apache-2.0 proposed); must not fork Hermes/OpenClaw.

## 2. Decision

Build **one privileged daemon** (`agentbedd`) plus a CLI, installable on existing Linux distributions, that provides four things and nothing else:

1. **System API** — the whole machine exposed to agents as typed MCP tools (packages, services, users, network, config, journal/coredumps, screen/input, files, secret handles, plugins, desktops).
2. **Capability manifests** — every agent, skill and plugin declares what it may touch; the daemon compiles that declaration into real enforcement (systemd exec directives, Landlock, seccomp, nftables, secret handles).
3. **Transactional change** — every mutation goes through observe → propose → test → apply → verify → rollback, using NixOS generations where available and Btrfs/ZFS snapshots + git-tracked `/etc` elsewhere.
4. **Plugin & desktop runtime** — durable user apps and disposable per-agent desktops run as rootless Podman/Quadlet systemd units under the same manifests.

Channels, voice, memory, dreaming and skill marketplaces stay in the agent runtimes (Hermes, OpenClaw). The layer exposes itself to them; it does not replace them.

## 3. Options considered

### Option A — New distro (NixOS-based "AI OS" image)
| Dimension | Assessment |
|---|---|
| Complexity | High (installer, ISO, hardware support, release engineering) |
| Cost | Ongoing distro maintenance, ~1 FTE forever |
| Reach | Only users willing to switch OS |
| Safety | Best: declarative + atomic rollback |

**Pros:** cleanest model; osModa shows it works. **Cons:** tiny addressable audience; competes with osModa on its home turf; maintenance swamp.

### Option B — Installable layer, NixOS-only
| Dimension | Assessment |
|---|---|
| Complexity | Medium |
| Reach | NixOS users only (small) |
| Safety | Best |

**Pros:** fastest to a safe prototype. **Cons:** Node-D is Ubuntu; excludes nearly every potential user; osModa already owns this niche.

### Option C — Installable layer, distro-agnostic with host adapters (**chosen**)
| Dimension | Assessment |
|---|---|
| Complexity | Medium-High (two adapters at launch: Nix, apt+snapshots) |
| Reach | Ubuntu/Debian, NixOS at launch; Fedora/Arch later |
| Safety | Tiered: generation rollback on NixOS, snapshot rollback elsewhere |
| Maintenance | Daemon talks only to stable interfaces (systemd, D-Bus/varlink, journald, nix CLI, apt, btrfs/zfs) |

**Pros:** users keep their OS and update habits; layer updates independently; develops on NixOS VMs where mistakes are cheapest; ships to Ubuntu where users are. **Cons:** two rollback paths to keep honest; "snapshot" rollback is weaker than "generation" rollback and must be labelled as such.

### Option D — Extend Hermes or OpenClaw directly
**Rejected:** both are agent runtimes, not OS layers; a PR adding privileged OS mutation with kernel enforcement would not be accepted and would lock us to one runtime. We ship Hermes and OpenClaw integrations instead.

## 4. Trade-off analysis

- **Safety vs reach.** Option C accepts a weaker rollback on non-Nix hosts in exchange for being installable anywhere. Mitigation: the daemon reports a *safety level* per host (`generation`, `snapshot`, `none`) and manifests can require a minimum level for risky tools.
- **Own enforcement vs reuse nono/srt.** We reuse ideas (Landlock profiles, phantom credentials) and may vendor nono's Landlock crate, but enforcement must be compiled from *our* manifest so agents, plugins and desktops share one policy language.
- **Typed tools vs raw shell.** Typed tools are more work and will never be complete; keep a `shell.exec` tool but make it the most restricted and most audited tool, so usage of it is the metric we drive down.
- **Rust vs Python.** Daemon in **Rust** (privileged, long-lived, Landlock/seccomp bindings mature, single static binary eases install). Host adapters and generated plugins may be Python. Decision can be revisited after Phase 0 if velocity suffers.

## 5. Architecture

```
 Agents (Hermes bots, Claude Code, ChatGPT, local models)
        │  MCP (stdio / streamable HTTP over Tailscale)
        ▼
 ┌─────────────────────────────────────────────────────────┐
 │ agentbedd (root, systemd service)                          │
 │  ├─ MCP front: tool dispatch + per-agent policy check    │
 │  ├─ Manifest compiler → systemd drop-ins, Landlock,      │
 │  │     seccomp, nftables sets, secret handles            │
 │  ├─ Transaction engine: observe/propose/test/apply/      │
 │  │     verify/rollback + audit ledger (append-only)      │
 │  ├─ Host adapter: nix | apt+btrfs/zfs | dnf+btrfs (later)│
 │  ├─ Plugin runtime: Podman Quadlet units                 │
 │  └─ Desktop runtime: Xvfb/Wayland + XFCE + KasmVNC       │
 │        containers, one per agent, snapshot-able          │
 └─────────────────────────────────────────────────────────┘
        │ D-Bus / varlink / journald / CLI
        ▼
 systemd · kernel (Landlock, seccomp, netns) · Podman · Nix/apt · Btrfs/ZFS
```

Agents never get root and never talk to the host directly; they talk to `agentbedd`, which acts on their behalf within their manifest.

**Two enforcement planes — be honest about which applies.**

| Plane | Applies to | Mechanism |
|---|---|---|
| **Policy-checked** | Remote MCP clients (Hermes on Node-D, Claude Code over Tailscale) | `agentbedd` validates every tool call against the manifest before acting. This is a root daemon making a decision, not the kernel. |
| **Kernel-enforced** | Anything `agentbedd` spawns: `shell.exec` children, `file.*` I/O, plugins, desktops | Per-agent unprivileged helper process (forked, `DynamicUser`, Landlock, seccomp) performs the work; containers get Quadlet/OCI sandboxing (`DropCapability=`, `SeccompProfile=`, `UserNS=`, `Network=`). |

Rule: `file.read`/`file.write`/`shell.exec` are **never** executed by the root daemon itself; they are delegated to the agent's Landlocked helper so the manifest's filesystem scope is real.

**Agent identity.** HTTP transport requires a per-agent bearer token (or mTLS), optionally bound to Tailscale `whois` identity; stdio transport binds to the spawning unit. A self-asserted client id is never trusted. `agentbedd` cannot see which *skill* inside a runtime issued a call, so skill-level narrowing (§6.3) is enforced by the runtime and treated as advisory by the layer.

**Egress and secrets.** nftables matches IPs and cgroups, not hostnames. Each agent/plugin/desktop therefore gets a default-deny nftables rule keyed on its cgroup that allows traffic only to a per-identity egress proxy; the proxy enforces the hostname allowlist (SNI/CONNECT) and injects credentials ("phantom credentials") only toward allowed destinations. Injection into TLS requires the client to trust a Agentbed-issued CA; plugins and desktops get it automatically, remote agents opt in. Rootless containers egress via pasta, so the cgroup rule targets the pasta process's cgroup.

### 5.1 System API — initial tool surface (Phase 0/1)

| Tool | Mutating | Notes |
|---|---|---|
| `system.info` | no | host, adapter, safety level, generations/snapshots |
| `journal.query` | no | journald filters, unit, priority, since |
| `crash.list` / `crash.backtrace` | no | coredumpctl + gdb |
| `service.list` / `service.status` | no | systemd units |
| `service.control` | yes | start/stop/restart/enable — transactional |
| `package.search` / `package.list` | no | adapter-backed |
| `package.install` / `package.remove` | yes | transactional |
| `config.propose` | yes (staged) | returns a diff + test plan; Nix: config edit; apt hosts: git-tracked `/etc` |
| `tx.test` / `tx.apply` / `tx.rollback` / `tx.status` | yes | the transaction engine |
| `file.read` / `file.write` | yes | Landlock-scoped to manifest paths |
| `secret.use` | no* | returns a *handle* usable by proxies, never plaintext |
| `desktop.create` / `desktop.screenshot` / `desktop.input` / `desktop.snapshot` | yes | wraps computer-use-linux inside a desktop container |
| `plugin.install` / `plugin.list` / `plugin.control` | yes | Quadlet lifecycle |
| `shell.exec` | yes | runs in the agent's Landlocked helper; always audited; manifest default `deny` |

### 5.2 Transaction engine

1. **Observe** — journald, failed units, coredumps, disk/cert/update events → `agentbed://events` (MCP resource + webhook). The agent runtime decides who handles it.
2. **Propose** — agent calls `config.propose` / `package.install` etc. The daemon stages a change set and returns a human-readable diff.
3. **Test** — Nix: `nixos-rebuild build` (evaluation + build pre-flight, optionally `build-vm`), then `nixos-rebuild test` which activates the new configuration *without* a bootloader entry, so a failed probation or a reboot returns to the previous generation. apt hosts: snapshot first; on Btrfs, optionally pre-flight in an ephemeral `systemd-nspawn -x` clone of `/` (not available on ZFS); then apply on the live system under probation.
4. **Apply** — Nix: `switch` (bootloader entry) only after probation passes. apt: dpkg has no transactions, so "apply" is the package operation bracketed by the snapshot; the snapshot is the rollback unit.
5. **Verify** — probation window (default 120 s): health checks from the manifest (units active, ports answering, network reachable, daemon itself reachable).
6. **Rollback** — automatic on failed verify; manual via `tx.rollback`. Every step written to the audit ledger with agent id, manifest version, diff and outcome.

### 5.3 Safety levels by host

| Host | Level | Rollback unit |
|---|---|---|
| NixOS | `generation` | boot-selectable generation |
| Ubuntu/Fedora/Arch on Btrfs/ZFS | `snapshot` | root subvolume snapshot + git commit of `/etc`. `/etc` changes revert live; package/root rollback is **reboot-to-rollback** (subvolume swap). `/boot` and the ESP are outside the guarantee. |
| Image-based (Silverblue/bootc, MicroOS) | `snapshot` (adapter later) | `bootc rollback` / `transactional-update` |
| ext4 without snapshots | `none` | only `/etc` git history; mutating tools above "low" risk refuse unless manifest opts in |

Note: Ubuntu's default install is ext4. **Node-D's filesystem must be confirmed before Phase 2**; if ext4, Node-D is `none`-tier until reinstalled on Btrfs/ZFS or a separate Btrfs volume is added for plugin data.

**Plugin data is excluded from system rollback.** Plugin `data.dir` lives on a dedicated subvolume/dataset (`/var/lib/agentbed/plugins`, mounted separately) with its own snapshot schedule, so rolling back a config change never rolls back the CRM database, and removing a plugin is a separate, explicit operation.

## 6. Manifests

Four manifest kinds (`agent`, `skill`, `plugin`, `desktop`) share a `capabilities` block so one compiler serves all of them. Format: YAML with a published JSON Schema. Version field mandatory.

JSON Schema validates shape only. The compiler performs **semantic validation** beyond it: `services.control` must name existing units; a skill's capabilities must be a subset of the calling agent's; `min_host_safety` is checked against the host's actual level; `config.apply: requires_approval` requires `approvals.channel`; Landlock ABI is probed at start and features the kernel lacks are reported in `system.info` and degrade to deny, never to silent allow.

Compiler backends: **native** (systemd drop-ins + Landlock + seccomp for helpers) and **container** (Quadlet/OCI keys for plugins and desktops). The same `capabilities` block feeds both.

### 6.1 Capabilities block (shared)

```yaml
capabilities:
  fs:
    read:  [/home/agent/work, /etc/nixos]
    write: [/home/agent/work]
  net:
    egress: [api.openrouter.ai:443, "*.hubspot.com:443"]   # nftables set; "none" | "all" allowed
    listen: []                                              # ports a plugin may expose
  system:
    services: { control: [caddy.service] }                  # explicit units only
    packages: { install: true, remove: false }
    config:   { propose: true, apply: requires_approval }   # auto | requires_approval | deny
    shell:    deny                                          # deny | audited | allow
  desktop:
    own: true                 # may request a disposable desktop
    takeover: owner           # owner | any_human | none — who may seize the screen
  secrets:
    use: [openrouter-key, hubspot-session]                 # handles, never values
  risk:
    max_level: medium    # low | medium | high; tools above it are denied (and hidden from listings)
    min_host_safety: snapshot
```

The compiler maps this to: a systemd drop-in (`ProtectSystem=strict`, `ReadWritePaths=`, `DynamicUser=`, `RestrictAddressFamilies=`, `SystemCallFilter=`), a Landlock ruleset applied at spawn, an nftables set keyed by the unit's cgroup, and a secrets proxy entry that injects credentials only into allowed egress destinations (the "phantom credential" pattern).

### 6.2 Agent manifest

```yaml
kind: agent
version: 1
name: linkedin-researcher
runtime: hermes            # hermes | openclaw | mcp-client (external)
identity:
  mcp_client_id: hermes:linkedin-researcher   # how agentbedd recognises it
  owner: lp
capabilities: { ...see 6.1... }
approvals:
  channel: telegram:lp     # where requires_approval prompts go
  timeout: 30m
budget:
  tx_per_day: 20
```

### 6.3 Skill manifest (thin — defers to Hermes/OpenClaw skill formats)

```yaml
kind: skill
version: 1
name: social-media-digest
runtime_format: hermes-skill   # the actual SKILL.md lives in the runtime
requires:
  tools: [desktop.*, secret.use]
  plugins: [crm>=0.2]
capabilities: { ... }          # intersected with the calling agent's manifest, never widened
```

Rule: a skill can only *narrow* the agent's capabilities. Union is never allowed.

### 6.4 Plugin manifest

```yaml
kind: plugin
version: 1
name: crm
summary: Local CRM with contacts, companies, deals and a pipeline view
tier: container            # native | container
image:
  build: ./Containerfile   # or image: ghcr.io/...
  blessed_stack: sqlite+fastapi+htmx   # informs generators and reviewers
data:
  dir: /var/lib/agentbed/plugins/crm      # snapshotted + backed up by the OS
  migrate: ["python", "manage.py", "migrate"]
  export:  ["python", "manage.py", "export", "--format", "jsonl"]
health:
  http: http://127.0.0.1:8731/healthz
  interval: 30s
expose:
  ui:  { port: 8731, path: / }         # reachable via the layer's reverse proxy (Tailscale only by default)
  mcp: { port: 8732 }                  # tools the plugin offers agents (crm.add_lead, crm.search…)
capabilities: { ...see 6.1... }        # what the plugin itself may reach
lifecycle:
  rebuild_from_manifest: true          # must be reproducible
  backup: nightly
```

Compiles to a rootless Podman Quadlet unit (`crm.container`) run by a dedicated service user (`agentbed-plugins`, `loginctl enable-linger`, managed via `systemctl --user -M agentbed-plugins@`), a data volume on the plugin subvolume, nftables + egress-proxy rules, and a registration of its MCP endpoint with `agentbedd` so agents see `crm.*` tools. Quadlet requires Podman ≥ 4.4: supported hosts are Ubuntu 24.04+, Debian 13+, Fedora, NixOS; Ubuntu 22.04 / Debian 12 need the upstream Podman repo. AppArmor (Ubuntu) and SELinux (Fedora) volume labelling is handled by the adapter (`:Z` / profile generation). A plugin can also **wrap** an existing OSS app (Twenty CRM, Kimai, n8n) by pointing `image:` at it and adding an MCP sidecar; the generator should prefer wrapping when a mature app exists.

### 6.5 Desktop (disposable computer) spec

`kind: desktop` is the fourth manifest kind, built on the plugin runtime: XFCE + Chromium + computer-use-linux MCP + KasmVNC/noVNC, persistent profile volume, snapshot on demand, `takeover` exposed through the reverse proxy. One per agent by default. A `shared: true` desktop implements the Grok Bot model: Chromium locks its profile directory, so shared logins are delivered as **one Chromium instance with one window per agent, driven via CDP targets**, while desktop-app work uses separate X displays on the same home volume.

## 7. Consequences

Easier: giving any agent a governed computer; auditing what agents did; rebuilding a bot's machine from a manifest; replacing long-tail SaaS with reproducible local plugins; integrating with Hermes and OpenClaw without forking.

Harder: two rollback paths to keep honest; typed tools lag behind what agents want, so `shell.exec` pressure is constant; Landlock needs kernel ≥ 6.2 for truncate control (ABI 3), ≥ 6.7 for TCP bind/connect scoping (ABI 4), ≥ 6.12 for signal/abstract-socket scoping — features are probed and absent ones degrade to deny; rootless Podman with cgroup v2 and Podman ≥ 4.4 required; a plugin store is a supply-chain target and must not launch before signing and review exist.

Open security items (Phase 1 action items, one paragraph each in `docs/security.md`): audit ledger mechanism (hash-chained entries plus journald FSS sealing, optional remote forward); `agentbedd` self-update and its own config changes must go through the transaction engine like any other change; AppArmor/SELinux interaction with Podman volumes and nspawn; licence check before vendoring any nono code (Apache-2.0 compatible).

Revisit after Phase 1: Rust vs Python for velocity; whether to vendor nono's Landlock layer; whether `dry-activate`-style testing on apt hosts is worth nspawn complexity or whether snapshot+probation is enough.

## 8. Milestones

**Phase 0 — Proof (week 1–2).** Repo, licence, CI. `agentbedd` skeleton in Rust with MCP front (stdio + HTTP with bearer tokens), `system.info`, `journal.query`, `service.list/status/control`, and the Nix adapter with `config.propose` + `tx.test/apply/rollback/status` using `nixos-rebuild test` + probation. Runs on a hand-built NixOS VM on the spare Supermicro node (Proxmox); Hermes bot on Node-D drives it over Tailscale. Exit criterion: a bot proposes a config change that breaks networking on purpose, and the VM returns to the previous generation without a human.

**Phase 1 — Governed (week 3–5).** Manifest schemas + compiler → systemd/Landlock helpers, cgroup-keyed nftables, egress proxy with credential injection. Agent manifests for three Hermes bots. `shell.exec`/`file.*` via Landlocked helpers. Reproducible NixOS VM image (flake) with agentbedd + Hermes + XFCE + KasmVNC + Tailscale; `desktop.*` wrapping computer-use-linux inside a desktop container. Audit ledger + `agentbed audit` CLI. Exit: a bot whose egress is limited to HubSpot cannot reach LinkedIn, and the blocked attempt is attributed to that bot's token in the ledger.

**Phase 2 — Plugins + Ubuntu (week 6–8).** Preconditions: confirm Node-D's filesystem and Podman version. Plugin manifest + Quadlet runtime, three reference plugins (time tracker, CRM, "wrap n8n"), apt+Btrfs host adapter, `curl | sh`-free installer (`.deb` + Nix module). Install on Node-D. Exit: "build me a time tracker" from Telegram yields a running plugin with MCP tools, snapshots and export, and `tx.rollback` removes it cleanly.

**Later (not decided):** Fedora/bootc adapter, public store with signing and review, Windows/macOS desktops via Cua, OpenClaw integration module, security audit.

## 9. Action items

1. [ ] Pick the project name (avoid AIOS, AgentOS, agent-os, osModa-adjacent names); register repo under Apache-2.0.
2. [ ] L-P: provide Tailscale SSH access to the spare Supermicro node (Proxmox) for Claude-driven iteration.
3. [ ] Claude: scaffold repo (`agentbedd/`, `adapters/nix`, `adapters/apt`, `schemas/`, `images/nixos-vm/`, `plugins/reference/`), write JSON Schemas for the three manifests from §6.
4. [ ] Claude: Phase 0 tools (`system.info`, `journal.query`, `service.*`, `config.propose`, `tx.*`) and the Nix adapter.
5. [ ] L-P: decide approval policy defaults (what auto-applies vs asks on Telegram) — this is the one decision that cannot be delegated.
5b. [ ] L-P: report Node-D's root filesystem (`findmnt /`), kernel version (`uname -r`) and Podman version (`podman --version`) so the Ubuntu adapter is planned against reality.
6. [ ] Both: run the Phase 0 exit test and record results in `docs/evidence/`.
