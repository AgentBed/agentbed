# Threat model (v0)

**Status:** Revision 5 · 2026-08-23 · applies to ADR-001 Revision 5

## Persona and deployment assumed

A solo operator ("owner") running Agentbed on machines they own (home-lab VMs, later a home server), on a private network (Tailscale tailnet). All connecting agent runtimes (Hermes, OpenClaw, Claude Code) are operated by the owner. There is no multi-tenant use, no public exposure, and no plugin store in v0.

## What v0 defends against

| Threat | Vector | Primary control |
|---|---|---|
| T1. Honest agent mistakes | wrong package, broken config, deleted file, runaway loop | effect classes + transaction engine + probation + rollback; quotas |
| T2. Prompt-injected agents | agent reads hostile web/email content and is steered to exfiltrate data, reach new hosts, change the system, or spend credentials | manifests (fs/net/system scopes) enforced at broker + kernel (helpers/containers); connector-scoped credentials (agents never see secrets); egress default-deny; approval gates on external effects; ledger |
| T3. Compromised or misbehaving plugin *code* (generated or wrapped) | plugin process abuses its runtime access | rootless containers, per-plugin capabilities, egress allowlists, image digest pinning, no secrets in env |
| T4. Cross-agent interference | one bot damages another's work or reads its data | per-agent identity, separate helpers/desktops/volumes, quotas. **Void within an opt-in shared desktop, which is a single trust domain.** |
| T5. Silent tampering with history | any actor rewriting what happened | hash-chained ledger with off-host anchoring |

## What v0 explicitly does NOT defend against

- A **determined human attacker** with network position on the tailnet or physical/host access. Tailscale ACLs and host hygiene are the owner's responsibility.
- A **malicious plugin author distributing signed packages** — there is no store in v0; installing third-party plugin manifests is at-your-own-risk and off by default.
- **Kernel or hypervisor exploits** from confined processes; Landlock/seccomp/namespaces raise the bar, they are not a hypervisor. Workloads needing that isolation belong in a VM.
- **Model-level identity**: a token proves possession of a credential bound to a manifest. It does not prove which model, prompt, or human was behind the call.
- **Covert channels / timing attacks** between confined processes.

## Trust boundaries

1. **Remote agent ⇄ gateway (unprivileged):** authn (per-agent token, expiry/rotation/revocation; OAuth before any beyond-tailnet exposure), schema validation, rate limits. The gateway holds no privileges and no secrets.
2. **Gateway ⇄ broker (privileged, fixed RPC over Unix socket):** every call re-checked against the manifest *in the broker*; the gateway is untrusted by the broker.
3. **Broker ⇄ executors/helpers/containers:** work is performed by the least-privileged component that can do it; the broker never executes agent-supplied strings itself.
4. **Anything ⇄ secrets:** only connectors hold credentials; they scope use to named operations with field-level bounds and redact both logs and returned responses. Approval decisions are single-use, signed, and bound to {tx digest, diff, agent id, manifest digest, exact effect set, RFC 8785 canonical operation/body digest, expiry, nonce}.

## Known weak points (stated, not hidden)

- **Approval-channel capture:** if the same runtime that requests a change also relays the approval (e.g. the owner approves in Telegram through the same Hermes gateway), approval degrades from authorization to friction. Mitigation now: approvals for high-risk classes go through a channel independent of the requesting runtime (direct Telegram bot owned by Agentbed, or the takeover UI). Documented residual risk in v0.
- **Policy-checked plane:** for remote agents, manifest enforcement is a decision made by trusted Agentbed code, not by the kernel. A bug in the broker is a full bypass; the broker therefore stays small, has no dynamic tool surface, and is the primary audit target.
- **`shell.exec`:** even Landlocked, a shell grants wide behaviour within its scopes. It stays deny-by-default and per-call audited.
- **Skill-level narrowing is advisory** — the host sees runtimes, not skills.
- **Watchdog vs hostile root:** the in-guest watchdog is process-independent but not a security boundary against another root process in the same guest; the out-of-band observer on the hypervisor is the backstop (effects.md §4). Class-F propose-time rejection prevents *transactions* from touching the watchdog; it does not constrain an already-compromised root.
- **Ledger honesty:** anchoring makes post-anchor tampering detectable; it cannot prove the broker recorded events truthfully before anchoring (ADR §5.2). The broker is trusted code and the primary audit target.

## Review requests

Attack T2 hardest: assume the LinkedIn-browsing bot is fully steered by a hostile page and enumerate what it can reach given the sample manifest in ADR §6. Anything reachable that surprises the owner is a finding.
