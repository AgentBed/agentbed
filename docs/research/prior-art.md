# Prior art (sweep of 2026-08-22)

Three independent searches (system-API daemons; assistant and voice platforms; safe self-modification and sandboxing), run 2026-08-22 via web search plus repository page inspection; star counts and activity are as-seen on that date and are approximate. **The verdict is a market hypothesis current as of the sweep date, not a proven negative** — a reproducible sweep (queries, inclusion criteria, per-claim source dates) and an adjacent-technology pass (polkit/Cockpit, OPA/Cedar, SPIFFE/SPIRE, secretless brokers, gVisor/Kata/Firecracker, transactional-update/bootc/rpm-ostree — several are candidate *dependencies*, not just prior art) are Gate 0 tasks. Verdict first, evidence after.

## Verdict

Nothing shipped combines a write-capable system API, per-agent manifests compiled to kernel enforcement, a distro-agnostic rollback loop, and disposable GUI computers. Every piece exists separately. The defensible gap is narrow and specific:

1. a distro-agnostic transaction loop (NixOS generations **and** Btrfs/ZFS snapshots + git-tracked `/etc`) gated by machine-checkable health checks;
2. a manifest that compiles to systemd + Landlock + seccomp + nftables + egress proxy **and** to the set of OS mutations an identity may commit;
3. per-bot disposable GUI desktops on Linux with snapshot/restore and takeover;
4. first-class MCP exposure of the *whole machine* to any external agent.

Channels, voice, memory, dreaming, skill marketplaces: already done at enormous scale by OpenClaw and Hermes. Build on them.

## Closest projects

| Project | What it is | Why it is not this |
|---|---|---|
| [osModa](https://github.com/bolivian-peru/os-moda) (~110★, active) | NixOS-only "OS for agents": 92 MCP tools, SafeSwitch blue-green rebuild with health probation and auto-rollback, hash-chained audit ledger, egress daemon | NixOS only; README concedes the approval/capability layer is bypassed by the default Claude Code driver and unverified end-to-end; secrets encrypted at rest, not handle-based. Closest in spirit — cite and learn from it. |
| [ClawNix](https://github.com/jacopone/clawnix) (3★, stalled Feb 2026) | Self-evolving agent platform on NixOS: human-gated evolve loop, per-agent systemd hardening, sops-nix secrets | No post-switch health probe, systemd-only enforcement, single author, dormant |
| ElephantClock "[Agentic Linux Runtime](https://www.elephantclock.ae/research/agentic-linux-runtime/)" (Jun 2026) | Whitepaper: systemd + Landlock + seccomp + cgroups + nftables, YAML manifests with approval gates, Btrfs/ZFS rollback, systemd-creds secrets | Paper only. No code, no licence. Validates the design. |
| [NVIDIA NemoClaw](https://github.com/NVIDIA/NemoClaw) (~22k★, alpha) | Hardened runtime (OpenShell) to contain OpenClaw/Hermes agents: egress policies with operator approval, cap drops, snapshots | Contains agents; does not expose the host to them. DGX/WSL only, headless. Could move toward host tooling — watch it. |
| Red Hat [linux-mcp-server](https://github.com/rhel-lightspeed/linux-mcp-server) / [Tank-OS](https://www.redhat.com/en/blog/building-hardened-image-based-foundation-ai-agents) | Vendor-shipped system MCP (RHEL 10), read-only by design; bootc image that forbids agent self-modification | The incumbents treat write-capable agents as unsafe without exactly the enforcement layer we propose. That is the bar. |

## System MCP servers

- [mvo5/systemd-mcp](https://github.com/mvo5/systemd-mcp) (Rust, varlink introspection, 50–80 tools, "no guardrails") — cheapest route to a broad systemd surface; systemd upstream could ship something similar.
- [openSUSE/systemd-mcp](https://github.com/openSUSE/systemd-mcp) (Go, sd-bus, polkit on stdio, OAuth scopes on HTTP) — coarse but real permission model.
- [JEFF7712/nix-agent](https://github.com/JEFF7712/nix-agent) (eval/build/diff/switch/generations for NixOS) and [utensils/mcp-nixos](https://github.com/utensils/mcp-nixos) (659★, nixpkgs/option metadata) — use as dependencies.
- [nihalxkumar/arch-mcp](https://github.com/nihalxkumar/arch-mcp), [signal-slot/mcp-systemd-coredump](https://github.com/signal-slot/mcp-systemd-coredump) — narrow.

## Sandboxing, manifests, secrets

- [nono](https://github.com/nolabs-ai/nono) (3.7k★) — Landlock profiles, per-tool child sandboxes, credential proxy with "phantom credentials". Strongest existing manifests+secrets combo, scoped to coding agents in a repo. Licence check before reuse.
- [Anthropic sandbox-runtime](https://github.com/anthropic-experimental/sandbox-runtime) (4.6k★), [greywall](https://github.com/GreyhavenHQ/greywall), [hivebox](https://github.com/TetiAI/hivebox), [sandlock](https://github.com/multikernel/sandlock), [ActPlane](https://github.com/eunomia-bpf/ActPlane) (BPF-LSM), [agent-sandbox.nix](https://github.com/archie-judd/agent-sandbox.nix) — each picks one primitive; none know about generations or snapshots.
- MCP policy gateways: [agentgateway](https://github.com/agentgateway/agentgateway), [Kuadrant mcp-gateway](https://github.com/Kuadrant/mcp-gateway) — tool-level authz, not OS-level.
- Manifest vocabularies without enforcement: [agent-manifest](https://github.com/agent-manifest/agent-manifest), JSON Agents, AgentSpec.
- Secrets brokers: [systemd-vaultd](https://github.com/numtide/systemd-vaultd), [shuru](https://github.com/superhq-ai/shuru).

## Rollback primitives

- NixOS generations; `bootc rollback`; `transactional-update` (MicroOS); snapper/timeshift (not agent-aware).
- Agent-specific: [stratumpf](https://github.com/whitecell-dev/stratumpf) (Btrfs per-op snapshots, dir-level), [os4agent](https://github.com/WukLab/os4agent) (research, custom kernel), [Stockyard](https://github.com/prime-radiant-inc/stockyard) (Firecracker + ZFS).
- NixOS-for-agents configs: [jacopone/nixos-config](https://github.com/jacopone/nixos-config), [amamival/agentsandbox](https://github.com/amamival/agentsandbox).

## Desktops and computer use

- [agent-sh/computer-use-linux](https://github.com/agent-sh/computer-use-linux) (421★, Rust MCP, AT-SPI + ydotool, ships a Hermes skill) — wrap, do not rebuild.
- [Bytebot](https://github.com/bytebot-ai/bytebot) (11k★, **no commits since 2025-09**) — the "AI gets its own Linux desktop" product, now dead.
- [Cua](https://github.com/trycua/cua) (21.8k★, VC-backed, cloud upsell) — disposable Linux/macOS/Windows VMs; MCP driver mostly mac/win. Candidate for Windows/macOS guests later.
- Hermes Computer Use (`cua-driver`, AT-SPI, "bounded" manifest mode) — app-level, not kernel-enforced.
- Agent-S, UI-TARS desktop, self-operating-computer — agent brains, not infrastructure.

## Agent runtimes (build on, not beside)

- [OpenClaw](https://github.com/openclaw/openclaw) (387k★): 24+ channels incl. SMS/voice via Twilio plugins, Ollama/llama.cpp, Claude/Codex subscription auth, cron, built-in Dreaming, per-agent Docker sandboxes, **runs as an MCP server** (`openclaw mcp serve`) and ACP bridge. Known malicious-skill incidents.
- [Hermes Agent](https://github.com/NousResearch/hermes-agent) (234k★): channels, voice, 7 terminal backends, Bot Mode with bot-to-bot messaging and `hermes peer`, per-bot model/tool pins; dreaming via third-party plugins.
- ZeroClaw, nanobot, OpenFang — lighter alternatives, same category.

## "AI OS"-branded projects

Omarchy 4 (bundles coding agents in an Arch desktop; no OS API), AIOS/agiresearch (Python agent scheduler), rivet agentos (backend library), OpenDAN (dead), Warmwind (closed cloud), Lilux (vision doc). Name collisions to avoid: AIOS, AgentOS, agent-os.
