# ADR-001: Agentbed — an AI-native layer for Linux

**Status:** **ACCEPTED WITH CONDITIONS** (reviews codex-003/004/005) — **Revision 6** (2026-08-23), incorporating reviews [codex-001](../review-responses/codex-001.md) through [codex-005](../review-responses/codex-005.md). Gate 0 open; per-gate conditions in [roadmap.md](../roadmap.md).
**Date:** 2026-08-22 (rev. 2026-08-23)
**Deciders:** L-P (owner). Reviewers: Codex (independent), Claude, Hermes "architect" bot.
**Scope:** Gates 0–3 (NixOS-only alpha). Later gates are sketched, not decided. See [roadmap](../roadmap.md).
**Normative companions:** [threat-model.md](../threat-model.md) · [effects.md](../effects.md) (effect classes, per-resource safety vector, transaction contract, watchdog)

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

Build **Agentbed**: a small set of cooperating processes (not one monolithic root daemon — see §5.0) plus a CLI, installable on existing Linux distributions, providing four things and nothing else:

1. **System API** — the whole machine exposed to agents as typed MCP tools (packages, services, users, network, config, journal/coredumps, screen/input, files, secret handles, plugins, desktops).
2. **Capability manifests** — every agent, skill and plugin declares what it may touch; the daemon compiles that declaration into real enforcement (systemd exec directives, Landlock, seccomp, nftables, secret handles).
3. **Transactional change** — every *host* mutation goes through observe → propose → test → apply → verify → rollback, using NixOS generations where available and Btrfs/ZFS snapshots + git-tracked `/etc` elsewhere. Reversibility is claimed **per computed effect set**, never blanketly: declarative host changes roll back automatically; data mutations restore from tested snapshots; external effects (email, SaaS mutations, browser input on desktops with egress) are irreversible and gated on per-transaction approval or explicit scoped pre-authorization. Normative detail in [effects.md](../effects.md).
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

- **Safety vs reach.** Option C accepts a weaker rollback on non-Nix hosts in exchange for being installable anywhere. Mitigation: the daemon reports a **per-resource safety vector** (effects.md §2) and manifests state per-resource minimums; D/M steps below the minimum, or at `none`, are refused.
- **Own enforcement vs reuse nono/srt.** We reuse ideas (Landlock profiles, phantom credentials) and may vendor nono's Landlock crate, but enforcement must be compiled from *our* manifest so agents, plugins and desktops share one policy language.
- **Typed tools vs raw shell.** Typed tools are more work and will never be complete; keep a `shell.exec` tool but make it the most restricted and most audited tool, so usage of it is the metric we drive down.
- **Rust vs Python.** Daemon in **Rust** (privileged, long-lived, Landlock/seccomp bindings mature, single static binary eases install). Host adapters and generated plugins may be Python. Decision can be revisited after Gate 1 if velocity suffers.

## 5. Architecture

### 5.0 Process split

The trust boundary is not one root daemon. Four components, landing incrementally (gate in brackets, see [roadmap](../roadmap.md)):

| Component | Privilege | Role |
|---|---|---|
| `agentbed-gw` | unprivileged (`DynamicUser`) | MCP front: HTTP + stdio shim, authn, schema validation, sessions, rate limits. Holds no secrets and no privileges. [G0] |
| `agentbed-broker` | root, minimal surface | Fixed, narrow RPC over a Unix socket with peer credentials. Re-checks every call against the manifest itself — the gateway is untrusted by the broker. Owns the transaction engine and ledger writes. Never executes agent-supplied strings. [G0–G1] |
| per-agent executors/helpers | unprivileged, per-agent `DynamicUser` + Landlock + seccomp; distinct netns later | Perform `shell.exec`, `file.*` and adapter work at least privilege. [G3, deepened later] |
| `agentbed-watchdogd` | root, independent of broker and of the candidate generation | Armed before every activation; commits require its affirmative act, and it executes the precommitted revert on failed probation even if the broker is dead. Class F: no tool can modify it. Paired with an out-of-band observer on the hypervisor. [G1] |

In prose, "agentbedd" refers to the gateway + broker pair. Logical surface:

```
 Agents (Hermes bots, Claude Code, ChatGPT, local models)
        │  MCP (streamable HTTP over Tailscale / stdio shim → Unix socket)
        ▼
 ┌─────────────────────────────────────────────────────────┐
 │ agentbed-gw (unprivileged) ⇄ agentbed-broker (root)      │
 │  ├─ MCP front: tool dispatch + per-agent policy check    │
 │  ├─ Manifest compiler → systemd drop-ins, Landlock,      │
 │  │     seccomp, nftables sets, secret handles            │
 │  ├─ Transaction engine (serialized, WAL — effects.md §3) │
 │  │     + audit ledger (hash chain, off-host anchor)      │
 │  ├─ agentbed-watchdogd (independent revert on failure)   │
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

**Agent identity.** HTTP transport requires a per-agent token with expiry, rotation and revocation (stored via systemd-creds), optionally bound to Tailscale `whois` identity; stdio uses a shim connected to the gateway's Unix socket, identified by peer credentials. A self-asserted client id is never trusted. What a token proves is *possession of a credential bound to a manifest* — not which model, prompt or human was behind the call; the ledger records exactly that claim. Standards-compliant OAuth (resource-server behaviour, audience validation, scopes per the MCP authorization spec) is a **hard precondition for any deployment reachable beyond a private tailnet**, and a later gate; static tokens are the v0 contract. `agentbedd` cannot see which *skill* inside a runtime issued a call, so skill-level narrowing (§6.3) is enforced by the runtime and is **advisory-only** at the host.

**Approvals.** An approval is a single-use signed decision bound to {transaction digest, exact diff, agent identity, manifest digest, **exact computed effect set, canonical operation/request-body digest (RFC 8785 JCS of the validated typed operation, computed in the broker — effects.md §1; exact construction frozen in [protocol.md](../protocol.md) §4)**, expiry, nonce}, issued on a channel independent of the requesting runtime (Agentbed's own Telegram bot or the takeover UI). If the approval channel is relayed by the same runtime that made the request, approval degrades to friction, not authorization — stated in the [threat model](../threat-model.md).

**Egress and secrets (TLS interception dropped; connector contract specified).** nftables matches IPs and cgroups, not hostnames. Each identity gets a default-deny nftables rule keyed on its cgroup allowing traffic only to its egress path. Two egress paths exist:

(a) **Connectors** — per-service processes that hold the credential; agents get `hubspot.request`-style tools and never see secrets. The connector contract:

- **Caller identity is derived server-side.** The agent's authenticated MCP identity (gateway token → broker) selects the connector grant; a connector invocation is a broker RPC on the agent's behalf. Agents are never given raw socket paths, file descriptors, or transferable bearer handles; `secret.use` returns an *opaque reference valid only within the calling identity's session*, and the broker rejects it from any other identity. Where executors talk to connectors directly (Gate 3+), each identity has a stable unique UID, a `0700` per-identity runtime directory, and the connector verifies `SO_PEERCRED` against the expected UID; FD-passing and cross-identity replay are Gate 3 exit tests.
- **Typed RPC only — never serialized HTTP.** A connector accepts a typed operation call and constructs **one fresh outbound request** itself. No caller-supplied framing of any kind is accepted: no pseudo-headers (`:authority`/`:scheme`/`:path`), `Host`, `Transfer-Encoding`/`Content-Length`, trailers, chunk extensions, obsolete folding, CR/LF, connection-control or Upgrade fields. HTTP extended CONNECT / CONNECT-UDP / MASQUE are disabled. This removes request-smuggling ambiguity (RFC 9112/9113) by construction rather than by filtering.
- **One parser, one normalization.** URLs are handled by a single standards-compliant parser (RFC 3986): components separated *before* percent-decoding, never double-decoded, dot-segments removed; origin = scheme + normalized hostname + **explicit port** (the sample manifest below reflects this). Hostnames in manifests are **ASCII IDNA A-labels only** in v0; the owner-facing UI displays both A-label and decoded U-label so confusables are visible (RFC 5890).
- **Resolution is pinned and address-classed on binary form.** The connector resolves destinations itself, pins the resolved address per request, parses IPs to binary before classification (IPv4-mapped IPv6 classified by the embedded IPv4; IPv6 zone identifiers rejected; compressed forms, ULA, NAT64 ranges and unspecified addresses handled by class), and blocks loopback, link-local, RFC1918/tailnet, multicast and Unix-socket destinations unless explicitly designated. Kernel-level (nftables) restriction backs this. No wildcard credential origins in v0.
- **Operation grants are field-aware.** Grants name connector operations with field-level constraints, not just method/path. Fields that carry outbound destinations — webhook, callback, redirect, import/template URLs — are marked *external destinations*: they get the same normalization + address-class validation, and default to per-transaction approval even inside a pre-authorized operation. Operations whose credential-reflection or downstream-fetch behaviour cannot be bounded are rejected at manifest-compile time.
- **Responses are projected, not passed through.** Constrained content type, wire size *and decompressed size*; responses projected into typed allowlisted fields — raw response headers are never returned; scanned/redacted for credential values **before returning to the caller and before logging** (an upstream debug endpoint reflecting the credential must not reach the agent).
- **Redirects are governed, not library-default.** Automatic redirect following is disabled. Where a named operation explicitly permits redirects: each hop is parsed and authorized as a fresh destination (same normalization, address-class and grant checks), all credentials are stripped before re-evaluation and reinjected only for the newly authorized exact origin, resolution is re-pinned per hop, and hop count is limited (default 3). A `307`/`308` from a provider can therefore never carry a credential to an unauthorized origin.
- **Plain proxy tunnel binding.** The credential-less proxy requires the canonical CONNECT authority and the visible TLS SNI to match the allowlisted hostname; absent or encrypted SNI (ECH) and alternate tunnel protocols are rejected.
- **Every credential-bearing connector invocation carries effect class E in its set** (effects.md §1) and follows the E outcome contract (§3b): durable approval consumption, provider idempotency keys, `OUTCOME_UNKNOWN` never auto-retried.

(b) a **plain proxy** for credential-less browsing, enforcing a hostname allowlist via SNI/CONNECT without decrypting — with the same pinned resolution and address-class blocking as connectors (the "DNS rebinding moot" claim holds only under that rule, so it applies to both paths).

There is no Agentbed CA and no TLS interception in v0. Rootless containers egress via pasta; the nftables rule targets the pasta process's cgroup, and rule lifecycle across user-service restarts is a Gate 3 probe with recorded evidence, not an assumption.

### 5.1 System API — initial tool surface (Gates 0–3)

Every tool call is assigned an **effect set** computed pre-execution from tool + arguments + manifest ([effects.md](../effects.md) §1); the table lists each tool's *minimum* set, which arguments can only raise. The highest class in the set governs authorization and approval.

| Tool | Min. effect set | Notes |
|---|---|---|
| `system.info` | {R} | host, adapter, **per-resource safety vector** (effects.md §2), generations/snapshots, probed Landlock ABI |
| `journal.query` | {R} | journald filters, unit, priority, since |
| `crash.list` / `crash.backtrace` | {R} | coredumpctl + gdb |
| `service.list` / `service.status` | {R} | systemd units |
| `service.control` | {D} (+declared) | {unit, action} pairs from the manifest allowlist, each declaring its affected resources / added effects; unknown footprint → refused — transactional |
| `package.search` / `package.list` | {R} | adapter-backed |
| `package.install` / `package.remove` | {D} | transactional; exact-name allowlist with repo + version/range + resolved digest, signatures required — **no globs** (post-install scripts run as root) |
| `config.propose` | {D} (staged) | returns a diff + test plan; Nix: config edit; apt hosts: git-tracked `/etc`; diffs touching the watchdog closure/unit/state/rollback path are rejected at propose (class F, effects.md §4) |
| `tx.test` / `tx.apply` / `tx.rollback` | {D} | serialized engine with atomic commit protocol (effects.md §3–3a); `tx.rollback` of an older tx is a *revert* — a new forward transaction, never a state restore |
| `tx.status` | {R} | read-only |
| `file.read` / `file.write` | {R} / {M} | executed in the agent's Landlocked helper, scoped to manifest paths; writes snapshot-before-step on a `dedicated_snapshot` resource, refused otherwise (effects.md §2) |
| `secret.use` | {R} | returns an opaque session-bound reference (see connector contract, §5); never plaintext, never transferable |
| `desktop.create` / `desktop.snapshot` | {M} | desktop container lifecycle |
| `desktop.screenshot` | {R} | |
| `desktop.input` | {M}, +**E** whenever the desktop has external egress or any other external-effect channel | even an unauthenticated browser can submit forms or mutate anonymous services; approval or scoped pre-authorization per manifest |
| `plugin.install` / `plugin.list` / `plugin.control` | {D,M} / {R} / {D} | Quadlet lifecycle; data operations add M |
| `shell.exec` | {M}, +E if manifest grants egress | runs in the Landlocked helper; always audited; manifest default `deny` |
| connector invocations (`hubspot.request`, …) | {E} | always E when credential-bearing (effects.md §1) |
| kernel / bootloader / storage layout / firewall plane / watchdog / Agentbed self-modification | **F** | refused in v0 |

### 5.2 Transaction engine

1. **Observe** — journald, failed units, coredumps, disk/cert/update events → `agentbed://events` (MCP resource + webhook). The agent runtime decides who handles it.
2. **Propose** — agent calls `config.propose` / `package.install` etc. The daemon stages a change set and returns a human-readable diff.
3. **Test** — Nix: `nixos-rebuild build` (evaluation + build pre-flight, optionally `build-vm`), then `nixos-rebuild test` which activates the new configuration *without* a bootloader entry, so a failed probation or a reboot returns to the previous generation. apt hosts: snapshot first; on Btrfs, optionally pre-flight in an ephemeral `systemd-nspawn -x` clone of `/` (not available on ZFS); then apply on the live system under probation.
4. **Apply/Commit** — Nix: after probation passes, boot promotion as the compound operation of effects.md §3a step 4 (advance the system profile, then the closure's `switch-to-configuration boot`, then verify agreement) — never a second live activation. apt: dpkg has no transactions, so "apply" is the package operation bracketed by the snapshot; the snapshot is the rollback unit.
5. **Verify** — probation window (default 120 s): health checks from the manifest (units active, ports answering, network reachable, daemon itself reachable).
6. **Rollback** — automatic on failed probation, executed by `agentbed-watchdogd` under the atomic commit protocol (effects.md §3a: epochs/fencing, renewable commit lease, watchdog armed through boot promotion, durable profile/boot-default/closure agreement verified before disarm); manual via `tx.rollback`. Every step written to the audit ledger with agent id, manifest digest, diff, effect set and outcome.

Concurrency and crash recovery are normative in [effects.md](../effects.md) §3–3b: all D/M transactions serialized behind one lock; WAL with idempotency keys, epochs and base-revision conflict checks; defined recovery for every interrupted state including power loss during COMMITTING; external effects follow the separate E outcome contract (durable approval consumption, provider idempotency keys, `OUTCOME_UNKNOWN` never auto-retried).

**Audit ledger (claim narrowed).** Hash-chained records; the chain head is anchored off-host at every transaction commit and at least every N minutes. The anchor must be **WORM**: an object store with compliance-mode retention (governance mode is bypassable by permitted principals) or equivalent, written with a host credential that can append new objects but cannot delete, overwrite, or change retention. Plain `git push` does **not** qualify as an anchor (refs are rewritable) and may be kept only as a convenience mirror. The anchoring/signing key lives outside the broker — in the OOB controller, a TPM, or a separate minimal service. Offline behaviour: a commit that cannot anchor is recorded `COMMITTED_UNANCHORED` and further D/M transactions are refused until anchoring resumes. **What this proves, exactly:** modifications *after* an anchored head are detectable; it does not prove the broker truthfully recorded events *before* anchoring — a compromised broker can lie into the ledger. Detecting a lying broker is out of scope for v0 (threat model: the broker is trusted code and the primary audit target). Crash consistency shares the transaction WAL. Redaction: diffs may embed paths, never secret values.

### 5.3 Safety by host — a per-resource vector, not a scalar

`system.info` reports rollback coverage per resource (root_config, packages, bootloader, kernel, plugin_data, desktop_data, home_data, external_effects, recovery_requires — schema in [effects.md](../effects.md) §2), and manifests state per-resource minimums. Class-D **and class-M** steps targeting a resource below its minimum, or at `none`, are refused. The table below gives the typical vector headline per host family:

| Host | Typical headline | Rollback unit |
|---|---|---|
| NixOS | `generation` | boot-selectable generation |
| Ubuntu/Fedora/Arch on Btrfs/ZFS | `snapshot` | root subvolume snapshot + git commit of `/etc`. `/etc` changes revert live; package/root rollback is **reboot-to-rollback** (subvolume swap). `/boot` and the ESP are outside the guarantee. |
| Image-based (Silverblue/bootc, MicroOS) | `snapshot` (adapter later) | `bootc rollback` / `transactional-update` |
| ext4 without snapshots | `none` (most resources) | only `/etc` git history; D/M steps on `none` resources are **refused — no manifest opt-in** (effects.md §2) |

Note: Ubuntu's default install is ext4. **Node-D's filesystem must be confirmed before Gate 5**; if ext4, most of Node-D's resources report `none` until it gains Btrfs/ZFS (or a separate Btrfs volume for plugin data).

**Plugin data is excluded from system rollback.** Plugin `data.dir` lives on a dedicated subvolume/dataset (`/var/lib/agentbed/plugins`, mounted separately) with its own snapshot schedule, so rolling back a config change never rolls back the CRM database, and removing a plugin is a separate, explicit operation.

## 6. Manifests

Four manifest kinds (`agent`, `skill`, `plugin`, `desktop`) share a `capabilities` block so one compiler serves all of them. Format: YAML with a published JSON Schema. Version field mandatory.

JSON Schema validates shape only. The compiler performs **semantic validation** beyond it: `services.control` must name existing units; a skill's capabilities must be a subset of the calling agent's; `min_safety` is checked per resource against the host's safety vector; `config.apply: requires_approval` requires `approvals.channel`; diffs touching class-F resources are rejected at propose; Landlock ABI is probed at start and features the kernel lacks are reported in `system.info` and degrade to deny, never to silent allow. Schema conformance examples for every initial tool ship in `schemas/examples/`.

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
    services:
      control:
        - unit: caddy.service
          actions: [restart, reload]
          affected_resources: [service_state]   # runtime restart is service_state, never root_config
          added_effects: []                      # e.g. [E] for a unit that sends webhooks on restart; unknown behaviour → refused
    packages:
      install:            # exact names only — no globs; digest resolved at propose time
        allow: [{name: htop, repo: official, version: ">=3", affected_resources: [packages], added_effects: []}]
        require_signatures: true
      # footprint fields are required; adapter-resolved metadata (package scriptlet inspection,
      # unit analysis) overrides self-declaration, and an unresolved footprint is refused
      remove: deny
    config:   { propose: true, apply: requires_approval }   # auto | requires_approval | deny
    shell:    deny                                          # deny | audited | allow
  quotas:
    cpu: "50%"           # cgroup weight for helpers/containers
    memory: 2G
    pids: 256
    disk: 10G
    tx_per_day: 20
  desktop:
    own: true                 # may request a disposable desktop
    takeover: owner           # owner | any_human | none — who may seize the screen
  secrets:
    use:
      - connector: hubspot
        origins: ["https://api.hubspot.com:443"]     # scheme + A-label host + explicit port
        operations: [{name: crm.contacts.search, fields: {limit: {max: 100}}}]  # named ops + field bounds
  effects:
    external: requires_approval    # requires_approval | pre_authorized (named operations + field bounds) | deny
  risk:
    max_unapproved_class: M   # highest effect class invocable without approval; scoped pre-authorization overrides (effects.md §1 precedence)
    min_safety: {packages: snapshot_reboot, root_config: snapshot_live}   # per-resource minimums, enums per effects.md §2
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

Rule: a skill can only *narrow* the agent's capabilities; union is never allowed. **This is advisory-only at the host** — Agentbed sees runtimes, not skills, so narrowing is enforced by the runtime. It becomes a host boundary only if runtimes ever issue short-lived attenuated credentials per skill invocation (future work, outside our control).

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
  image_pin: sha256:…                  # digest-pinned; updates are transactions
  update_policy: manual                # manual | patch-auto
  backup: nightly
  restore_test: monthly                # restores are exercised, not assumed
  retention: {backups: 90d, on_removal: export_then_archive}
```

Compiles to a rootless Podman Quadlet unit (`crm.container`) run by a **dedicated service user per plugin trust domain** (`agentbed-p-crm`, own subuid/subgid range, own Podman storage and runtime directory, `loginctl enable-linger`, managed via `systemctl --user -M agentbed-p-crm@`) — one compromised plugin must not see a sibling's Podman socket, storage or files, so the earlier single shared `agentbed-plugins` user is withdrawn; plugins that explicitly declare mutual trust may share a domain. Plus: a data volume on the plugin subvolume, nftables + egress rules, and a registration of its MCP endpoint with `agentbedd` so agents see `crm.*` tools. Cross-plugin container control and volume-read attempts are Gate 4 exit tests. Quadlet requires Podman ≥ 4.4: supported hosts are Ubuntu 24.04+, Debian 13+, Fedora, NixOS; Ubuntu 22.04 / Debian 12 need the upstream Podman repo. AppArmor (Ubuntu) and SELinux (Fedora) volume labelling is handled by the adapter (`:Z` / profile generation). A plugin can also **wrap** an existing OSS app (Twenty CRM, Kimai, n8n) by pointing `image:` at it and adding an MCP sidecar; the generator should prefer wrapping when a mature app exists.

### 6.5 Desktop (disposable computer) spec

`kind: desktop` is the fourth manifest kind, built on the plugin runtime: XFCE + Chromium + computer-use-linux MCP + KasmVNC/noVNC, persistent profile volume, snapshot on demand, `takeover` exposed through the reverse proxy. One per agent by default. A `shared: true` desktop implements the Grok Bot model: Chromium locks its profile directory, so shared logins are delivered as **one Chromium instance with one window per agent, driven via CDP targets**, while desktop-app work uses separate X displays on the same home volume.

**Trust statement:** a shared desktop merges its agents into **one trust domain by design** — that is the feature. Within it, attribution is best-effort (the ledger records which agent's session drove which CDP target, but agents can read each other's state), isolation claims are void, and **best-effort attribution is never used for security authorization or non-repudiation**. It is opt-in; agents that must be isolated from each other get separate desktops. `desktop.input` on a desktop with external egress carries E in its effect set.

## 7. Consequences

Easier: giving any agent a governed computer; auditing what agents did; rebuilding a bot's machine from a manifest; replacing long-tail SaaS with reproducible local plugins; integrating with Hermes and OpenClaw without forking.

Harder: two rollback paths to keep honest; typed tools lag behind what agents want, so `shell.exec` pressure is constant; Landlock needs kernel ≥ 6.2 for truncate control (ABI 3), ≥ 6.7 for TCP bind/connect scoping (ABI 4), ≥ 6.12 for signal/abstract-socket scoping — features are probed and absent ones degrade to deny; rootless Podman with cgroup v2 and Podman ≥ 4.4 required; a plugin store is a supply-chain target and must not launch before signing and review exist.

Open security items (Gate 1–2 action items, one paragraph each in `docs/security.md`): journald FSS sealing alongside the anchored hash chain; **Agentbed self-update and its own config are class F for agents and go through the transaction engine when the owner applies them**; AppArmor/SELinux interaction with Podman volumes and nspawn; licence check before vendoring any nono code (Apache-2.0 compatible).

Revisit after Gate 3: Rust vs Python for velocity; whether to vendor nono's Landlock layer; whether nspawn pre-flight on apt hosts is worth the complexity or snapshot+probation suffices.

## 8. Milestones

Superseded by the gated roadmap: see [docs/roadmap.md](../roadmap.md). Summary: G0 ground truth (threat model, effect taxonomy, split-process spike) → G1 one safe serialized transaction + watchdog with the chaos matrix of effects.md §4 → G2 identity, approvals, anchored audit → G3 enforcement (Landlock helpers, nftables, one connector) = **NixOS-only alpha**. Plugins (G4), Ubuntu (G5), desktops (G6), and production Node-D come after, each behind evidence. The original 8-week horizon buys G0–G3, not the earlier phase list.

## 9. Action items

1. [ ] Pick the project name (avoid AIOS, AgentOS, agent-os, osModa-adjacent names); register repo under Apache-2.0.
2. [ ] L-P: provide Tailscale SSH access to the spare Supermicro node (Proxmox) for Claude-driven iteration.
3. [ ] Claude: scaffold repo (`gw/`, `broker/`, `watchdogd/`, `adapters/nix`, `adapters/apt`, `schemas/`, `images/nixos-vm/`, `plugins/reference/`), write JSON Schemas for the four manifest kinds from §6.
4. [ ] Claude: Gate 0 spike (gateway ⇄ broker RPC, `system.info`), then Gate 1 tools (`journal.query`, `service.*`, `config.propose`, `tx.*`) and the Nix adapter + watchdog.
5. [ ] L-P: decide approval policy defaults (what auto-applies vs asks on Telegram) — this is the one decision that cannot be delegated.
5b. [ ] L-P: report Node-D's root filesystem (`findmnt /`), kernel version (`uname -r`) and Podman version (`podman --version`) so the Ubuntu adapter is planned against reality.
6. [ ] Both: run the Gate 1 chaos matrix (effects.md §4) and record evidence in `docs/evidence/`.
