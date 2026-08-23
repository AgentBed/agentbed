# Review brief

This repository is in design phase. We want the design attacked before code exists. If you are an AI reviewer (Codex, Claude, Gemini…) or a human, this page tells you what to read and what we want challenged.

## Read in this order

1. `docs/goals.md` — what we are optimising for and explicit non-goals.
2. `docs/adr/ADR-001-agentbed-architecture.md` — the decision, architecture, manifests, transaction engine, milestones.
3. `docs/research/prior-art.md` — what exists and why we think the gap is real.

## What we want challenged

1. **Enforcement honesty.** ADR §5 splits policy-checked (remote MCP clients) from kernel-enforced (spawned helpers, containers). Find any place where the document still implies kernel enforcement for something the daemon merely checks.
2. **Egress and secrets.** Connector-based credential injection (no TLS interception, no CA): server-side identity derivation, canonicalized origin/path/method scoping, pinned resolution with address-class blocking, E-classed invocations. Find an escape or a cross-identity reuse. What breaks with rootless Podman + pasta?
3. **Transaction engine on non-Nix hosts.** `nixos-rebuild test` + probation is clear. Is snapshot + live `/etc` revert + reboot-to-rollback on Ubuntu/Btrfs an honest "snapshot" tier? What should `none`-tier hosts refuse?
4. **Manifest schema.** §6: can every field be enforced by something? Which fields are aspirational? What is missing for plugins (resource limits, update policy, data retention)?
5. **Identity.** Bearer tokens / mTLS / Tailscale `whois` binding for agents. Is skill-level narrowing correctly labelled as advisory?
6. **Gate scope.** Are the per-gate exit conditions in `roadmap.md` sufficient and testable for one owner plus AI pair-programming? What should be cut or added?
7. **Prior art we missed.** If something already does the whole thing, say so with a link.

## How to report

Open an issue per finding, titled `[review] <area>: <one-line claim>`, with: the section, what is wrong or missing, a concrete failure scenario, and the fix you would make. Severity first; no praise needed.

## Constraints the reviewer should assume

- One owner, no team, AI pair-programming; Rust daemon, Python allowed for adapters and generated plugins.
- Targets: NixOS VMs (development), Ubuntu 24.04 (production host "Node-D"); no GPU.
- Must integrate with Hermes Agent and OpenClaw without forking them.
- Apache-2.0.
