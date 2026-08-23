# Effect taxonomy, safety vector, and transaction contracts

**Status:** Revision 6 · 2026-08-23 · normative for ADR-001. This file is the **sole source of effect and safety vocabulary**; other documents defer to it.

## 1. Effect classes and effect sets

Classes, ordered from weakest to strongest: **R** (read) < **D** (declarative host change) < **M** (data mutation) < **E** (external effect). **F** (forbidden) is outside the ordering: it is refused, never authorized.

| Class | Meaning | Rollback story |
|---|---|---|
| **R** | no mutation | n/a |
| **D** | expressible as config/package/service state | automatic: generation or snapshot + probation (§3) |
| **M** | plugin data, files in agent workspaces | restore-tested snapshot of the affected resource taken before the step (§3) |
| **E** | anything leaving the host with consequences we cannot undo: send email, post, place order, mutate SaaS records, spend credentials, **browser or desktop input on any desktop with external egress** (an anonymous form submission or download is external too), **any credential-bearing connector invocation** | **none.** Separate outcome contract (§3b). Optional *compensation plan* recorded, never promised |
| **F** | kernel, bootloader, storage layout, firewall management plane, the watchdog and its closure, Agentbed self-modification | refused in v0 |

**Effect sets, not single classes.** Each tool *call* is assigned an effect **set** computed pre-execution from tool + arguments + manifest (e.g. `plugin.install` → `{D,M}`; `shell.exec` with network egress granted → `{M,E}`; without → `{M}`). Authorization, approval and quota rules apply to the **highest class in the set**. A tool whose set cannot be computed from its arguments is refused, not guessed. Static per-tool classes in the ADR table are the *minimum* set; arguments can only raise them.

**Mixed tasks are not atomic.** A logical task spanning D/M/E steps has no all-or-nothing guarantee: D/M steps roll back per §3, E steps follow §3b, and the ledger records each step's own outcome.

**Pre-authorization of E.** A manifest may pre-authorize a *narrow, explicit* E scope (`effects.external: pre_authorized` with named connector operations and field-level bounds); everything outside that scope requires a per-transaction approval (ADR §5). "External effects are gated" always means: approval **or** explicit scoped pre-authorization.

**Policy precedence (normative).** Every call is evaluated through all five stages in order; any stage may refuse or add an approval requirement, and later stages never relax an earlier one:

1. **F / explicit deny** → refuse. Terminal.
2. **Safety minimum** — any D/M member of the effect set below the per-resource minimum (or on `none`) → refuse. Terminal.
3. **Explicit operation policy** — if the manifest carries a policy for this operation (`deny` | `requires_approval` | `pre_authorized` scope), it governs: `deny` refuses, `requires_approval` **always requires a per-call approval regardless of the class ceiling**, and a matching `pre_authorized` scope allows. An operation whose arguments fall **outside** its `pre_authorized` bounds does **not** fall through to the class ceiling: it requires a per-call approval, or is refused if the operation declares `out_of_bounds: deny`. Stage 4 applies only to operations with no explicit policy at all.
4. **Class ceiling** — for calls with no explicit operation policy: highest class in the effect set ≤ `max_unapproved_class` → allow; otherwise per-call approval required.
5. **Quota** — a **mandatory final veto** applied to every outcome above (tx/day, resource quotas); quota exhaustion refuses even an approved or pre-authorized call.

A scoped E pre-authorization thus overrides the class ceiling (stage 3 before stage 4), while an explicit `requires_approval` can never be bypassed by a low class (`config.apply: requires_approval` demands approval even though D ≤ `max_unapproved_class`).

**Approvals and the ledger bind the exact computed effect set** plus a canonical digest of the operation and request body — never a singular "effect class". The canonical form is **RFC 8785 (JCS)** serialization of the fully validated, schema-projected typed operation, computed **inside the broker**: duplicate keys and non-interoperable numeric forms are rejected at validation, and the approval UI, ledger, connectors, and replay checks all use those exact bytes — never a re-serialization. Tools whose D/M footprint cannot be computed (a service unit or package script with undeclared side effects) are refused, not guessed: service and package entries in manifests declare their affected resources, and unknown footprint is a refusal reason.

## 2. Per-resource safety vector

`system.info` reports rollback coverage per resource. Allowed values form a total order per resource kind:

- **Host resources** (`root_config`, `packages`, `bootloader`, `kernel`): `none < snapshot_reboot < snapshot_live < generation`
- **Runtime state** (`service_state`): `none < desired_state` — rollback restores only the unit's desired active/inactive state; consequences of a start/stop/restart (a webhook fired, a database migrated on boot) are **not** rolled back and must appear in the operation's `added_effects` as M/E
- **Data resources** (`plugin_data`, `desktop_data` (desktop profiles/home volumes), `home_data`, agent workspaces): `none < dedicated_snapshot` (a dedicated, separately-mounted subvolume/dataset with its own snapshot schedule and an exercised restore procedure)
- `external_effects` is always `none` (definitionally)
- `recovery_requires`: `none | remote_reboot | oob_console` (informational, not ordered)

```yaml
safety:
  root_config: generation
  packages: generation
  bootloader: none
  kernel: none
  plugin_data: dedicated_snapshot
  desktop_data: dedicated_snapshot
  home_data: none
  external_effects: none
  recovery_requires: oob_console
```

**Refusal rule (uniform for D and M):** a class-D or class-M step targeting a resource whose reported safety is below the manifest's per-resource minimum — or `none` — is refused. There is **no manifest opt-in to mutate at `none`**; prior wording to the contrary is withdrawn. (Example consequence: with `home_data: none`, workspace `file.write` is refused unless the workspace is moved onto a `dedicated_snapshot` volume — which the standard install does.)

## 3. D/M transaction state machine

All D/M transactions on a host are **serialized** behind one lock. Parallelism is a later optimization.

```
IDLE → PROPOSED → TESTING → APPLYING → PROBATION → PROBATION_PASSED → COMMITTING → COMMITTED
                     │           │          │                │              │
                     ▼           ▼          ▼                ▼              ▼
                  REJECTED   REVERTING ← ─ ─┴─ (deadline/health fail) ─ ─ ─ ┘
                                 │
                                 ▼
                              REVERTED
```

Persisted (write-ahead, fsync) before each state entry: `tx_id` (ULID), idempotency key (replays return the original result), agent identity + manifest digest, base revision (generation/snapshot id + `/etc` git commit + config digest — apply refuses if the base moved), full diff and affected-resource set, effect set, per-step pre-snapshot ids (M), precommitted rollback procedure, approval reference.

### 3a. Atomic commit protocol (with the watchdog)

**Single durable decision writer.** The watchdog (`agentbed-watchdogd`) owns the **decision log**: a single-writer, append-only log (records fsync'd, containing-directory fsync'd on creation) separate from the broker's WAL. Only the watchdog appends decision records (`ARMED`, `PROBATION_PASSED`, `BEGIN_COMMIT`, `BEGIN_REVERT`, `COMMITTED`, `REVERTED`); the broker *requests* transitions over local RPC and acts only on the watchdog's durable answer. Mutual exclusion is by construction — one writer — not by CAS between two writers.

**Epochs.** Each arming carries a monotonically increasing epoch (fencing token). The epoch high-water mark is persisted **outside the rollback and WAL domain** (a dedicated non-snapshotted store) and mirrored at the OOB controller; on any mismatch, truncation, or apparent rollback of the epoch store, the host **fails closed** (safe mode, no new transactions). Every decision record, heartbeat, and OOB action is bound to {host_id, tx_id, epoch}; actions carrying a stale epoch or unknown tx_id are rejected.

1. **Arm.** Before TESTING→APPLYING, the broker requests arming: {tx_id, epoch, base generation, probation deadline, mandatory invariants + manifest checks (§4)}. Watchdog appends `ARMED`, fsyncs.
2. **Probation.** The watchdog — not the broker — evaluates health. On deadline or failed mandatory invariant it appends `BEGIN_REVERT` and executes the precommitted revert.
3. **Commit lease.** On passing probation the watchdog appends `PROBATION_PASSED` and grants a **time-bounded, renewable commit lease**: while the commit worker proves liveness (heartbeat with progress state), the watchdog renews; renewal stops the moment liveness stops. The watchdog stays armed through COMMITTING.
4. **COMMITTING — boot promotion, never a second activation.** The broker requests `BEGIN_COMMIT`; the watchdog appends it, then the commit worker performs boot promotion as a **crash-recoverable compound operation**: (i) pin and record the exact candidate closure path; (ii) **advance the persistent system profile** (`/nix/var/nix/profiles/system`) to that closure, creating the generation — standalone `switch-to-configuration` without this step produces missing or inconsistent bootloader entries; (iii) run that closure's `switch-to-configuration boot` so bootloader entries are built from the advanced profile; (iv) flush the profile store, boot filesystem/ESP, and decision log; (v) verify that system profile, boot default, and pinned closure all agree. (Snapshot hosts: promote/pin the post-tx snapshot with the analogous verify step.) The system was activated once, by `nixos-rebuild test`, before probation; COMMITTING submits **no systemd jobs and runs no activation scripts**. Recovery explicitly handles a crash **between profile advancement and bootloader update**: under the uncommitted rule the recovering watchdog either completes steps (iii)–(v) (only if invariants pass) or rolls the profile back to the base generation before reverting. (If any code path ever performs live activation during COMMITTING, fencing must cover systemd jobs and activation scripts — systemd does not abort already-running jobs when a client dies — but the normative design forbids that path.)
5. **Final lease expiry.** If the lease expires without `COMMITTED`, the watchdog first **fences and terminates the commit worker** (SIGTERM, then SIGKILL, then waits for confirmed exit of the whole process group), then verifies via systemd that no candidate-submitted jobs remain running — only then does it inspect durable state and decide. No revert ever runs concurrently with a live commit worker or a live activation job.
6. **Verify durably, invariants last.** After boot promotion returns (or after fencing), the deciding party verifies durable state directly — boot-default store path / bootloader entry matches the candidate, with the boot filesystem/ESP and log storage flushed, not merely re-read from cache — **and re-runs the mandatory invariants immediately before `COMMITTED` on every path, normal completion and post-fencing alike**. Then the watchdog completes the prepared OOB handshake (§4): obtain `COMMIT_PREPARED` from the OOB, append local `COMMITTED`, send the receipt that moves the OOB to terminal, and disarm.

**Recovery precedence (one rule).** A `BEGIN_COMMIT` without `COMMITTED` is **always uncommitted**, regardless of what the boot target currently says. To resolve it, the recovering watchdog must: acquire a fresh recovery lease under a new epoch, fence any surviving commit worker, re-run the mandatory invariants against the *currently active* system, and then atomically choose — append `COMMITTED` (completing the interrupted commit, only if the durable boot target already matches the candidate **and** invariants pass) or `BEGIN_REVERT` + revert (including rolling back a partially advanced system profile and resetting a boot target that promotion had already moved). "The boot default happens to be the candidate" is evidence, never a decision.

Power loss during APPLYING/PROBATION: boot default is still the base (NixOS `test` semantics) / pre-tx snapshot exists; the watchdog confirms base on boot and appends `REVERTED(crash)`. Power loss during COMMITTING: resolved by the recovery precedence rule above. An OOB reset after promotion moved the boot target may boot the candidate — which is why recovery re-runs invariants before completing a commit rather than assuming the base (§4).

### 3b. External-effect (E) outcome contract

E steps do not use the D/M machine. Per E step:

```
APPROVED → DISPATCHING → SUCCEEDED | FAILED | OUTCOME_UNKNOWN
```

- The approval (or pre-authorization reference) is **consumed durably before dispatch** — fsync first, send second. A crash between the two yields DISPATCHING, never a reusable approval.
- Connectors pass a **stable provider idempotency key** whenever the provider supports one, and persist the provider's operation/request id with the outcome.
- `OUTCOME_UNKNOWN` (crash after dispatch, timeout, ambiguous provider response) is terminal for automation: it is **never retried automatically**. The connector's provider-specific reconciliation (query by idempotency key / list recent operations) runs first; only a human may then order a retry, which is a new E step with a new approval.
- The ledger records every E step with its approval reference, provider id, and outcome — including UNKNOWN.

## 4. Watchdog: authority, limits, and self-protection

- **Single recovery coordinator:** the in-guest watchdog decides revert/commit. The out-of-band (OOB) observer on the hypervisor monitors a **signed watchdog heartbeat** `{host_id, tx_id, epoch, state, lease_deadline}` — not merely guest/MCP reachability, so a dead watchdog inside a responsive guest is detected. **Prepared handshake (two records, three steps):** OOB arming is a durable lease. Before appending local `COMMITTED`, the watchdog obtains **`COMMIT_PREPARED`** from the OOB (a CAS on its mirrored transaction record). While prepared, the OOB **retains fencing authority but never blindly selects the base**. The watchdog then appends local `COMMITTED` and sends a receipt that moves the OOB to terminal. If either message is lost — OOB prepared but the watchdog died before local `COMMITTED`, or local `COMMITTED` written but the receipt lost — the OOB fences the VM into recovery, and recovery consults the **authoritative local decision log**: `COMMITTED` present preserves the candidate; absent, the uncommitted rule applies. A prepared OOB thus never strands a hung guest and never wrongly reverts a committed one. The OOB otherwise acts in exactly two cases: (a) the heartbeat stops past its own deadline while a transaction is armed — it forces reset and — unless the transaction is in the prepared window, where recovery consults the decision log first — **explicitly selects the base generation** in the bootloader (never trusting the current boot default, which promotion may already have moved); (b) the guest fails an authenticated MCP round-trip past the lease deadline — same action. OOB actions are bound to {tx_id, epoch}; stale or replayed OOB messages are rejected against the mirrored epoch high-water mark.
- **Mandatory invariants vs manifest checks:** mandatory invariants are immutable and identical for every transaction (default route present, sshd or gateway reachable from outside, systemd not in degraded activation failure, watchdog's own heartbeat); manifest health checks are additive and can only make probation *stricter*, never relax invariants.
- **Budget and hysteresis:** at most **one automatic revert per transaction** and at most one OOB reset per transaction epoch. After `N` (default 3) consecutive failed transactions the host enters **owner-recovery safe mode**: new D/M transactions are refused, the owner is paged, existing state is untouched.
- **Self-protection:** the transaction engine **semantically rejects** any candidate diff that modifies the watchdog's package/closure, unit files, state directory, WAL path, precommitted rollback path, or the OOB protocol — this is the class-F rule enforced at propose time, not merely labelled. The watchdog binary and config live in a pinned store path / dedicated dataset referenced outside the candidate closure, so candidate activation cannot replace them. Honest limit (also in the threat model): a root process in the same guest is process-independent, not a security boundary against another root process — the OOB observer is the backstop for a hostile root.

Gate 1 chaos matrix (all with evidence in `docs/evidence/`): network loss · Tailscale loss with local networking healthy · `agentbed-broker` killed during probation · broker killed during COMMITTING (lease + fencing behaviour) · **watchdog killed during COMMITTING with gateway healthy (OOB heartbeat detection)** · lease overrun during boot promotion (fence-terminate-then-decide) · **power loss between profile advancement and bootloader update, and between bootloader update and `COMMITTED` (both sides of the promotion boundary)** · systemd activation failure · power loss during APPLYING · power loss during COMMITTING · reboot during probation · **decision-log/WAL truncation and corruption (fail closed)** · **epoch-store rollback via snapshot restore (fail closed)** · **stale OOB action replay (rejected)** · **OOB reset after boot-target change (recovery re-runs invariants, does not assume base)** · **OOB `COMMIT_PREPARED` then watchdog death before local `COMMITTED` (recovery via decision log)** · **local `COMMITTED` with final receipt to OOB lost (no wrong revert)** · **a candidate-submitted systemd job outliving CLI termination (must be impossible on the boot-promotion path; verified)** · watchdog-modifying diff rejected at propose.
