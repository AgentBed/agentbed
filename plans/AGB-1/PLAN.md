# Gate 1 — One safe transaction

**Issue:** AGB-1 / GitHub #12
**Workflow:** `workflow:guarded` · planning only
**Baseline:** `d1b2465283ceab524428b1b926da4ae4e93bafdc` (`origin/main`, verified 2026-08-23)
**Roadmap gate / exit:** `docs/roadmap.md` Gate 1. This plan does not authorize implementation, merge, NixOS/Proxmox execution, deployment, or activation.

## 1. Evidence and current state

Inspected: GitHub #12 and its kickoff comment; ADR-001 rev. 6; `docs/threat-model.md`; `docs/effects.md` §§1–4; `docs/protocol.md`; `docs/roadmap.md`; reviews `codex-003`, `codex-004`, and `codex-005`; Gate 0 evidence; workspace/CI/PR governance.

Gate 0 is closed. The existing tree provides only the fixed wire contract and `system.info` path: `broker/src/tools/mod.rs` exposes that one tool; `broker/src/adapter.rs` is the deliberately unresolved adapter; `adapters/nix/src/lib.rs` and `watchdogd/src/lib.rs` are intentional empty Gate 1 stubs. `broker/src/observability.rs` is explicitly not the Gate 2 ledger. CI runs the Rust format, clippy, build, and workspace-test commands on Ubuntu, but has no NixOS VM/OOB chaos harness.

Normative sources outrank the implementation plan: ADR-001, threat model, effects, protocol, then roadmap. A behavior change to any of them must land with its code, or the normative revision must land first.

### Consequential assumptions

1. The initial execution target is a disposable NixOS guest on the spare Proxmox node; no Ubuntu, apt, plugin, desktop, connector, approval, anchored-ledger, or production-host behavior is in Gate 1.
2. OOB is an independently deployed Proxmox-side component with durable storage and a credential/key boundary outside the guest. It is not a broker library or a gateway health probe.
3. The candidate is activated exactly once with `nixos-rebuild test`; COMMITTING only performs the documented profile/boot promotion and must not invoke a live activation path.
4. Every persistence API used for transaction WAL, decision log, epoch high-water store, and OOB mirror has explicit file-and-parent-directory durability semantics. A platform that cannot provide the required storage separation fails closed.
5. No lane is authorized to create a live VM, configure Proxmox, issue credentials, or run the chaos matrix. Those are human/infrastructure gates; code may supply hermetic fakes and automation scripts only.

### Material decisions — resolve before the dependent lane starts

| ID | Decision / question | Why it changes scope | Blocking lanes |
|---|---|---|---|
| H-01 | `docs/protocol.md:3` freezes v1's operation set, while Gate 1 requires `config.propose` and `tx.*`. Should Gate 1 introduce a versioned v2 migration/translation, or should the owner revise the frozen-contract statement before tools land? | The current v1 envelope refuses `v != 1`; silently adding operations would violate a normative public contract. | L01 onward |
| H-02 | Who operates the Proxmox OOB observer, where is its non-guest durable epoch/mirror store, and what is the approved key provisioning/rotation boundary? | Determines the real trust boundary and the integration/chaos environment. No credentials may be created by this plan. | L05–L08 |
| H-03 | What is the approved minimal spare-node NixOS/Proxmox test topology (guest image/version, bridge/Tailscale simulation, console capture mechanism, reset/power-control API)? | Defines reproducible evidence rather than a paper-only OOB implementation. | L07–L08 |
| H-04 | Which local state locations are demonstrably outside both the candidate closure and rollback/WAL domain for the watchdog binary/config, epoch high-water mark, and decision log? | Effects §3a/§4 requires separated durability; an unverified path would invalidate the safety claim. | L03–L06 |

Until H-01 is resolved, the writer may not add or reinterpret a v1 operation. Until H-02 through H-04 are resolved and read back by a human, implementation can proceed only through hermetic unit/component tests; it cannot claim Gate 1 exit evidence.

## 2. Architecture and invariants

### State ownership

The broker owns the serialized D/M transaction WAL and requests transitions; it does not decide terminal commit/revert. The watchdog owns the separate, append-only, fsync'd decision log and is the sole durable writer of `ARMED`, `PROBATION_PASSED`, `BEGIN_COMMIT`, `BEGIN_REVERT`, `COMMITTED`, and `REVERTED`. The OOB observer mirrors the epoch and its transaction handshake state, monitors a signed watchdog heartbeat, and may fence/reset only under the two normative conditions. It never decides a committed candidate from guest/gateway availability.

```
Broker WAL:   IDLE → PROPOSED → TESTING → APPLYING → PROBATION → ...
                                │                         │
                                │ arm/request             │ health result / lease
                                ▼                         ▼
Watchdog log: ARMED → PROBATION_PASSED → BEGIN_COMMIT → COMMITTED
                  └────────────── BEGIN_REVERT → REVERTED
OOB mirror:   armed → COMMIT_PREPARED → terminal (receipt)
```

The broker persists before each WAL state: ULID, idempotency key, resolved identity/manifest digest, immutable base revision, full diff/resources/effect set, pre-snapshots where applicable, precommitted revert, and approval reference. Replay returns the original outcome. A moved generation, `/etc` commit, or config digest refuses apply. A corrupted or truncated WAL/decision log, an unknown transaction, an epoch mismatch, or an epoch-store rollback enters safe mode and refuses new D/M work.

### Commit and recovery boundary

Before `TESTING → APPLYING`, the watchdog durably arms `{host_id, tx_id, epoch, base, deadline, mandatory invariants, additive manifest checks}`. The candidate runs once under `nixos-rebuild test`; mandatory invariants are immutable, and manifest checks can only add strictness. After probation, the watchdog grants a renewable commit-worker lease. It alone appends `BEGIN_COMMIT` or `BEGIN_REVERT`.

COMMITTING pins the candidate closure, advances `/nix/var/nix/profiles/system`, runs the pinned closure's `switch-to-configuration boot`, flushes profile/boot filesystem-or-ESP/decision storage, and verifies profile, boot default, and closure agree. It submits no live systemd activation. On lease expiry, watchdog fencing is ordered: terminate the entire commit-worker process group, confirm it exited, verify that no candidate-submitted systemd jobs remain, then inspect durable state and decide.

`BEGIN_COMMIT` without local `COMMITTED` is always uncommitted. Recovery acquires a fresh epoch/lease, fences survivors, reruns invariants against the current system, then either completes only a verified matching candidate or writes `BEGIN_REVERT` and restores the base/profile/boot target. Before local `COMMITTED`, watchdog obtains OOB `COMMIT_PREPARED`; after local fsync it sends the receipt. Lost messages on either side cause OOB fencing into decision-log-led recovery, not an uninformed base selection.

### Trust and forbidden boundaries

Gateway remains unprivileged and untrusted by the root broker. The broker may request watchdog actions only over a narrow local RPC; no caller selects decision-log records, epochs, health results, boot paths, or terminal state. The watchdog binary/config/state/revert path/OOB protocol are class F and must be semantically rejected by `config.propose`. An in-guest watchdog is not a hostile-root boundary; independent OOB is the backstop. Gate 1 does not add secrets, token issuance, approvals, external effects, or an anchored audit ledger (Gate 2).

## 3. Acceptance traceability

| AC | Planned owner lanes | Verification / evidence |
|---|---|---|
| AC-01 complete scope | L00–L08 | Lane-to-tracker/roadmap matrix below; review verifies no #12/effects §4 item is orphaned. |
| AC-02 transaction invariants | L01–L04 | State-machine, persistence, idempotency/base-revision, recovery and decision-authority tests. |
| AC-03 watchdog/OOB boundaries | L03–L06 | Local-RPC authorization, signed-heartbeat/epoch/handshake property tests; isolated OOB integration fixtures. |
| AC-04 dependency order | L00–L08 | One-writer lane sequence and stop conditions below; no overlapping writer scope. |
| AC-05 verification and chaos | L07–L08 | Hermetic fault injection plus approved spare-node VM/OOB matrix, console and WAL/decision-log capture. |
| AC-06 durable events/status | L01, L07 | Durable cursor replay, restart/resume, R-class latency budget tests and informal gate measurement. |
| AC-07 safety/activation gates | L00, L05, L08 | PR gates and explicit human approvals; no activation in code PRs. |
| AC-08 governance readiness | L00, L07 | Protocol resolution, VM/OOB fixture proposal, CI/test-harness lane; no silent policy edits. |

## 4. Dependency-ordered implementation lanes

All implementation lanes use one writer at a time, exact-head independent review, current-head CI, and signed-off commits. Each code lane also runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, and `cargo test --workspace`. A failure to prove an invariant is a stop condition, not a reason to weaken a test.

| Lane | Scope and likely paths | Depends on / acceptance | Tests and exit evidence | Failure/rollback proof and stop condition |
|---|---|---|---|---|
| **L00 — Freeze transition and test-contract design** | Resolve H-01 via a normative PR if required; define operation/version migration, error/status schemas, persisted record schemas, failure-injection interfaces, event cursor format, and a VM/OOB evidence manifest. Likely `docs/protocol.md`, `docs/effects.md` only if owner-approved, `proto/`, `schemas/`, `tests/fixtures/`, `.github/` or `docs/` harness design. | H-01; AC-01, 04, 06–08. | Schema/compatibility tests and a reviewable test-matrix artifact. Protocol v1 behavior remains tested byte-for-byte. | Stop if a new `tx.*` operation is proposed under v1 without an explicit normative resolution. Roll back by reverting this isolated contract PR; do not start L01. |
| **L01 — Durable transaction core and read surface** | Add serialized D/M transaction model, WAL persistence/recovery interface, idempotency/base-revision checks, `tx.status`, and the first-release `agentbed://events` durable append/cursor-replay resource with cheap activity/status query shape. Likely `broker/src/{transaction,storage,dispatch,tools}/`, `proto/`, `schemas/`, `broker/tests/`. Explicitly do not implement Gate 2 ledger anchoring. | L00; AC-01, 02, 06. | State-transition table tests; fsync/atomic-rename error injection; restart at every WAL state; replay/original-result and base-moved refusal; `agentbed://events` cursor resume with no loss/duplication contract; benchmark/assertion demonstrating sub-second local R-class queries. | Stop on any path which derives a terminal decision in broker state. Tests must show corrupted/truncated WAL refuses. Revert safe because no adapter activation/OOB integration is in this lane. |
| **L02 — Nix proposal, test activation, and boot-promotion primitives** | Implement NixOS probe/safety vector, protected-path diff rejection, `config.propose`, immutable candidate/base capture, and adapter primitives for `nixos-rebuild build/test`, closure pinning, profile advance, boot configuration, flush, and agreement readback. Likely `adapters/nix/`, `broker/src/{adapter,host,tools}/`, NixOS fixture definitions, `broker/tests/`. | L00–L01; AC-01–02. | Fake command runner unit tests with exact argument/path assertions; NixOS VM tests for test activation, profile/boot agreement, and crash boundaries; prove COMMITTING makes no live activation/systemd job. | Stop if watchdog/self/OOB/rollback paths can be proposed or if adapter cannot distinguish verified generation safety. Rollback is recovery to base profile/boot target; no production run. |
| **L03 — Watchdog decision authority and durable local protocol** | Implement watchdog daemon, single-writer decision log, local authenticated broker↔watchdog RPC, arming, immutable-invariant evaluation, leases/heartbeats, decision responses, fencing process-group protocol, safe mode, and outside-domain epoch-store abstraction. Likely `watchdogd/`, `broker/src/`, service/unit definitions, test fakes. | L00–L02 and H-04 design; AC-02–03. | Assert broker cannot append `BEGIN_*`/terminal records; record plus parent-directory fsync tests; duplicate/stale/unknown request refusal; lease renewal/expiry; SIGTERM→SIGKILL→process-group-exit ordering; no surviving candidate job test. | Stop if decision log shares a rollback/WAL domain or if an expired lease can overlap a worker/revert. Revert leaves watchdog unarmed and candidate/base unchanged. |
| **L04 — Commit/recovery orchestration** | Wire broker requests to watchdog answers; implement probation, compound promotion, post-fencing recovery precedence, invariant reruns, and rollback procedure execution. Likely `broker/src/transaction*`, `watchdogd/`, `adapters/nix/`, integration tests. | L01–L03; AC-01–03. | Fault inject after every durable write and promotion substep; profile-advanced/boot-not-updated and boot-updated/no-COMMITTED cases; normal and post-fencing invariant tests; process/job non-survival proof. | Stop if `BEGIN_COMMIT` can be inferred as committed, any recovery skips invariants, or COMMITTING performs a second activation. Rollback test must restore base profile/boot selection. |
| **L05 — Proxmox OOB observer and prepared handshake** | Implement separately deployable OOB state machine, mirrored epoch high-water, signed watchdog heartbeat verification, explicit base selection, fenced recovery handoff, `COMMIT_PREPARED` CAS and receipt terminalization. Likely new `oob/` component plus deployment/test documentation; exact placement awaits H-02. | L03–L04; H-02/H-04; AC-03, 07. | Isolated fake Proxmox API and durable-store tests; signature/host/tx/epoch replay negatives; dead-watchdog/live-gateway detection; prepared-without-COMMITTED and committed-with-lost-receipt tests. | Stop if OOB uses gateway health as watchdog health, selects current boot default rather than explicit base, or terminalizes before the local decision record. No OOB credentials or VM mutations without human authority. |
| **L06 — End-to-end guarded transaction fixture** | Compose transaction core, Nix adapter, watchdog, and OOB behind test-only interfaces; finalize `config.propose`, `tx.*`, event/status behavior and protected-resource failures. Likely cross-crate integration tests, fixture configs, docs/evidence format. | L01–L05; AC-01–03, 06. | Deterministic happy path; deliberate network-breaking config automatically reverts; event cursors survive broker restart; status/activity stays cheap; class-F watchdog diff rejected before staging. | Stop on any uninstrumented effect or non-deterministic assertion. Fixture teardown must ensure watchdog is disarmed and base selection is restored. |
| **L07 — Reproducible chaos harness and governance fixture lane** | Add scripts/configuration to run the effects §4 matrix only against an approved disposable NixOS/Proxmox environment; capture console, WAL, decision log, OOB state, and exact artifact manifest. Add bounded CI support only for hermetic components; do not claim hosted runners can substitute for OOB evidence. Likely `tests/`, `images/nixos-vm/`, `docs/evidence/`, CI workflow changes if approved. | L00, L06, H-03; AC-05, 06, 08. | Harness self-test proves each injector is actually reached; artifact completeness check rejects missing console/log/decision evidence; CI runs hermetic subset. | Stop if the harness can target a non-disposable host or lacks an explicit teardown/base reset. No policy/ruleset modification unless separately reviewed. |
| **L08 — Authorized spare-node evidence run and Gate 1 evidence record** | Human-authorized execution only: run every chaos case and network-breaking revert on the approved spare-node VM; persist reviewed evidence in `docs/evidence/` and a `RESULT.md`. No new product behavior except evidence/documentation corrections discovered in prior lanes. | L05–L07; H-02–H-04; AC-01–08. | Matrix below passes with artifact readback; independent exact-head reviewer maps every AC and every case to current-head results; L-P gives explicit merge authority. | Stop and fail closed on any missing or ambiguous artifact, failed recovery, stale head, or unavailable OOB. Rollback is OOB-selected base plus human recovery; production/live activation remains out of scope. |

### Required review gates

L00–L08 are guarded: one implementation writer, no concurrent overlapping lane writer; independent current-head security/technical review after each candidate PR; re-review after repairs; current-head CI. L03–L06 require scenario verification because they change transaction/recovery authority. L08 additionally requires explicit L-P authorization for the disposable infrastructure action and the final guarded merge. A green CI result never overrides an unclosed safety finding.

## 5. Chaos and evidence matrix

The target is a spare-node NixOS VM with a dedicated OOB controller, console capture, deterministic fault injector, fresh epoch store for each test, and an artifact bundle containing: test manifest/version/head SHA; serial/Proxmox console capture; broker WAL; watchdog decision log; epoch-store/OOB mirror snapshots; process/systemd-job observations; boot profile/default/closure readbacks; invariant outcomes; and teardown/base-reset result.

| Effects §4 / #12 case | Injector and expected outcome | Evidence owner |
|---|---|---|
| Network loss; Tailscale loss with local networking healthy | Break the declared path during probation; watchdog invariant failure writes `BEGIN_REVERT`; no human action. | L06/L08 |
| Broker killed in probation / COMMITTING | Kill broker; watchdog continues probation/recovery and follows lease rules. | L04/L08 |
| Watchdog killed during COMMITTING with gateway healthy | Keep gateway responsive, stop signed heartbeat; OOB detects watchdog independently, fences to decision-log recovery. | L05/L08 |
| Lease overrun during promotion | Stall worker; watchdog terminates full process group, confirms zero candidate jobs, reruns invariants, then decides. | L03–L04/L08 |
| Power loss around profile advance / boot update; APPLYING / COMMITTING; reboot in probation | Cut/reset at each named durable boundary. Missing `COMMITTED` follows uncommitted rule; base or verified candidate only. | L02/L04/L08 |
| Systemd activation failure / candidate job outlives CLI | Force test activation failure and attempt a job survivor; no COMMITTING live activation and no surviving job before revert. | L02–L04/L08 |
| WAL/decision-log truncation or corruption | Mutate records/checkpoint at restart; safe mode, no new D/M transaction. | L01/L03/L08 |
| Epoch-store rollback; stale OOB replay | Restore stale mirror/store or replay old signed action; high-water mismatch/replay is rejected and safe mode entered. | L03/L05/L08 |
| OOB reset after boot-target change | Reset after promotion movement; recovery reruns invariants rather than assuming current boot target/base. | L04–L05/L08 |
| `COMMIT_PREPARED` then watchdog death | Drop watchdog after prepared response, before local `COMMITTED`; OOB fences and recovery selects using local decision log. | L05/L08 |
| Local `COMMITTED`, receipt lost | Persist local record, drop receipt; OOB does not wrongly revert a committed candidate. | L05/L08 |
| Watchdog-modifying diff | Propose change to watchdog package/unit/state/WAL/revert/OOB closure; semantic class-F refusal before staging. | L02/L06 |

## 6. Rollback, failures, and non-goals

A failed proposed candidate is not rolled forward. While armed, watchdog owns the rollback path; at lease expiry it fences before deciding. OOB may reset/fence only with the bound host/transaction/epoch authorization and must recover from the local decision log. Consecutive failures enter owner-recovery safe mode after the configured threshold; D/M work is refused, state is not modified. Any durability ambiguity, epoch mismatch, unavailable required OOB acknowledgement, corrupted critical log, or failed mandatory invariant fails closed.

**Explicit non-goals:** Gate 2 identity/token issuance, approvals, WORM anchor, or E outcomes; Gate 3 manifest enforcement/helpers/nftables/connectors; Gate 4 plugins; Gate 5 Ubuntu/apt; Gate 6 desktops; public exposure; production Node-D changes; deployment; and activation outside an explicitly authorized spare-node test. This plan does not alter router/reconciler configuration, GitHub protections, or use credentials.

## 7. Gate exit mapping

Gate 1 closes only when L08's current-head evidence proves all of the following, not when code compiles: watchdog-only durable `BEGIN_COMMIT`/`BEGIN_REVERT`; single test activation plus crash-recoverable compound boot promotion; prepared OOB handshake across both lost-message gaps; lease fencing/job confirmation before revert; invariant rerun on normal and post-fencing paths; epoch durability/replay rejection; heartbeat-based watchdog failure detection and explicit base selection; profile/boot/WAL flush before disarm; durable cursor-resumable events and cheap R status; every listed chaos scenario and network-breaking auto-revert on the authorized spare-node VM with logs and console capture. An independent exact-head review and explicit L-P guarded merge authorization are final gates.
