# AGB-8 — L03 Watchdog decision authority and durable local protocol

**Issue:** AGB-8 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c` (`origin/main`, verified 2026-08-25)
**Roadmap gate / exit:** Gate 1 L03 (`plans/AGB-1/PLAN.md` lane L03). Gate 1 remains open after this lane.

## 1. Evidence and scope

Inspected: `plans/AGB-1/PLAN.md` L03 row; ADR-001 rev. 6; `docs/threat-model.md`; `docs/effects.md` §§3a, 4; `docs/protocol.md`; L01 `plans/AGB-4/RESULT.md`; L02 `plans/AGB-6/RESULT.md`; `watchdogd/src/lib.rs` (empty stub); `broker/src/transaction/{state,engine,recovery}.rs`; `broker/src/peercred.rs`; `adapters/nix/src/protected.rs`; `broker/tests/l01_repair_review.rs` (watchdog-owned WAL → safe mode).

L00–L02 merged: v2 wire contract, durable broker WAL/events/idempotency, Nix propose with semantic class-F rejection and hermetic promotion primitives. `watchdogd` is still an intentional empty stub. Broker engine refuses watchdog-owned WAL states at recovery; happy path stops at `Probation`. L02 protected-path matcher names `/var/lib/agentbed/wal/` but production broker state (and WAL) lives at `/var/lib/agentbed/broker/state` — L03 must close that real-path gap without weakening existing aliases.

This lane implements **watchdog-only durable authority** and the narrow authenticated local broker↔watchdog protocol. It does **not** wire broker transaction orchestration (L04), perform actual promotion/revert execution, install/start the daemon, provision mounts, or implement OOB.

### Consequential assumptions

1. **Hermetic only.** Filesystem durability, topology proof, clock, local authentication, entropy, signals/process groups, job inspection, and invariant observations are injected behind narrow interfaces with fakes. Tests require no root, NixOS VM, systemd daemon, Proxmox, credentials, or live host mutation.
2. **H-04 sealed design.** The exact domains, paths, permissions, durability semantics, and startup refusal rules below are authoritative for L03. No improvisation of alternate mount roots, fallback directories, or cross-filesystem stores.
3. **Single writer.** Only watchdog-owned code appends decision-log records (`ARMED`, `PROBATION_PASSED`, `BEGIN_COMMIT`, `BEGIN_REVERT`, `COMMITTED`, `REVERTED`). Broker crates hold request/response types and RPC client stubs only — no append API, no epoch allocation, no invariant evaluation, no terminal-state selection.
4. **Process independence, not hostile-root boundary.** Permission/ownership checks and transaction self-protection are real; a malicious root can still alter watchdog files. OOB is the backstop (H-02/H-05, L05+). L03 must state this honestly (L03-AC10).
5. **External epoch floor is injected only.** L03 provides the outside-domain epoch-store abstraction and an injectable external-floor interface for hermetic mismatch tests. It does not implement OOB mirror placement, signing, or live rollback detection against an external store.

### Hard non-goals (L03-AC12)

No L04 commit/recovery orchestration or actual promotion/revert execution; no L05 OOB implementation; no live watchdog/systemd install/start; no live NixOS/profile/boot/network/firewall/Proxmox mutation; no deployment or activation; no credentials/keys/external epoch store; no Gate 2+; no router/reconciler/agent-instruction or repository-setting changes; no push to `main`; no merge.

## 2. Sealed H-04 topology and storage contract

### Exact domains and locations

| Domain | Path | Notes |
|---|---|---|
| Watchdog dedicated store (mount root) | `/var/lib/agentbed/watchdog` | Required dedicated, non-snapshotted mount; excluded from Nix generation switching and rollback snapshot sets |
| Watchdog binary | `/var/lib/agentbed/watchdog/runtime/agentbed-watchdogd` | `root:root`, mode `0555`, regular file |
| Watchdog config | `/var/lib/agentbed/watchdog/config/watchdog.json` | `root:root`, mode `0400`, regular file |
| Decision log | `/var/lib/agentbed/watchdog/decisions/decision.log` | Watchdog-only, append-only |
| Epoch high-water | `/var/lib/agentbed/watchdog/epoch/high-water.json` | Outside rollback/WAL domain |
| Safe-mode marker | `/var/lib/agentbed/watchdog/state/safe-mode.json` | Durable fail-closed flag |
| Volatile RPC socket | `/run/agentbed/watchdog/watchdog.sock` | Not authoritative after restart |
| Candidate closure domain | `/nix/store` + `/nix/var/nix/profiles/system` | Watchdog durable files must never resolve below either |
| Broker/WAL domain | `/var/lib/agentbed/broker/state` (WAL at `…/wal`) | Close L02 matcher gap for real production path |
| Precommitted rollback | `/var/lib/agentbed/rollback` | Class F at propose; topology must reject alias into watchdog store |

### Topology verifier (startup)

Before arming or accepting D/M work, prove:

- `/var/lib/agentbed/watchdog` is the **exact** mount point (not symlink/bind alias into broker, rollback, `/`, or `/nix`).
- Distinct mount ID and filesystem device from `/`, `/nix`, `/var/lib/agentbed/broker/state`, and `/var/lib/agentbed/rollback`.
- Reject: missing paths, ordinary-directory fallback, same-device/bind aliases, path traversal, symlink components, read-only/unwritable storage, filesystems that cannot satisfy file+parent-directory fsync and same-directory atomic rename.
- Never auto-create or auto-mount the dedicated store.
- Any failed or ambiguous proof → durable safe mode; refuse arming and new D/M work.

Nix protected-resource matrix must semantically reject watchdog root, runtime/config/state paths, actual broker state/WAL root, rollback root, watchdog unit/package selectors, and aliases **before** any WAL/event/idempotency side effect.

### Durability semantics

**Decision log:** watchdog-only, append-only, single-writer, versioned, length-framed, integrity-checked, monotonic sequence with epoch binding. Open with append/no-follow; never truncate, rewrite, or repair in place. On first creation: fsync initialized file and parent directory. Every accepted record fully written and file-fsynced before RPC response. Parent-directory fsync mandatory on creation; append does not replace file fsync.

**Epoch and safe-mode files:** same-directory temp (`O_EXCL`, mode `0600`), complete write, file fsync, atomic rename, parent-directory fsync, readback. No cross-filesystem rename or in-place overwrite.

**Fail-closed triggers:** checksum/frame/sequence/hash/epoch inconsistency; partial tail; truncation; missing expected file; ambiguous temp file; rollback below maximum epoch observed in decision log or injected external floor; failed fsync/rename/readback; unavailable storage. No auto-truncation, best-effort continuation, or default epoch.

**RPC refusal without new authority record:** stale epoch; unknown transaction; duplicate/conflicting request; moved base; expired lease; malformed/oversized frame; authentication failure; corrupt log; ambiguous state.

### Ownership and update boundary

- Mount root and subdirectories: `root:root`, `0700`.
- Reject symlinks, non-regular files, unexpected owner/group/mode, hard-link ambiguity, digest/config mismatch.
- L03 exposes **no** binary/config update API. Binary/config immutable while armed. Owner-controlled replacement (outside L03) requires disarm, same-directory temp + file fsync + atomic rename + parent fsync + readback; candidate transactions can never perform it.

### H-02/H-03 boundaries held

L03 must not implement or claim: Proxmox API calls; OOB durable mirror placement; OOB signing/key provisioning/rotation; production signed-heartbeat keys; explicit base-generation selection by Proxmox; `COMMIT_PREPARED`/receipt orchestration; live dedicated-mount provisioning; live watchdog/systemd installation or start; live systemd-job observation; disposable VM topology; console capture; reset/power control; spare-node chaos evidence.

## 3. Architecture

### Authority split

```
Broker WAL:   PROPOSED → TESTING → APPLYING → PROBATION  (broker-owned)
              requests only via local RPC ───────────────┐
                                                         ▼
Watchdog log:  ARMED → PROBATION_PASSED → BEGIN_COMMIT → COMMITTED
                    └──────────── BEGIN_REVERT → REVERTED
```

Broker may request transitions; it cannot select or append watchdog-owned records. Only the watchdog durably chooses `BEGIN_COMMIT` versus `BEGIN_REVERT`. L04 wires broker engine to these answers; L03 delivers the daemon/library boundary and hermetic proof.

### Component layout

```
watchdogd/
  topology.rs           — mount/device proof, path normalization, permission checks (L03-AC01)
  storage/
    decision_log.rs     — framed append-only log, single writer (L03-AC02)
    epoch.rs            — high-water allocation/refusal, external-floor hook (L03-AC03)
    safe_mode.rs        — durable marker + entry/exit rules (L03-AC03)
    durability.rs       — shared temp/rename/fsync/readback helpers
  rpc/
    protocol.rs         — versioned frame schema, deny-unknown (L03-AC04)
    server.rs           — Unix listener, deadlines, response binding (L03-AC04)
  auth.rs               — peer-credential + request authenticator binding (L03-AC04, AC07)
  arming.rs             — invariant set validation, immutable capture (L03-AC05)
  authority.rs          — sole BEGIN_* selection from health/invariant results (L03-AC06)
  lease.rs              — renewable lease + heartbeat/progress (L03-AC07)
  fencing.rs            — SIGTERM→wait→SIGKILL→group exit→job inventory (L03-AC08)
  invariants.rs         — mandatory invariant observation interface (injected)
  interfaces.rs         — Clock, Entropy, ProcessGroup, JobInspector, Topology, ExternalFloor
  daemon.rs             — startup: topology → storage open → RPC serve
  fakes/                — hermetic doubles for all injected interfaces (L03-AC10)
  tests/                — crate integration + spawned-fixture fencing test (L03-AC08, AC09)

broker/
  watchdog/
    client.rs           — request/response types, RPC client stub (no append API) (L03-AC02, AC04)
    types.rs            — broker-visible enums mirroring wire, not log append structs
  transaction/engine.rs — optional early refusal if watchdog client reports safe mode (minimal; full wire in L04)

adapters/nix/
  protected.rs          — add `/var/lib/agentbed/broker/state` and alias normalization (L03-AC01)

proto/ or watchdogd/rpc/
  (optional shared frame types if broker client needs them without importing watchdog internals)

schemas/
  watchdog-rpc.schema.json, decision-record.schema.json (if JSON schema aids RED fixtures)
```

### Narrow authenticated local RPC (broker ↔ watchdog)

- Transport: Unix socket at `/run/agentbed/watchdog/watchdog.sock` (volatile; tests use temp paths).
- Authorization: `SO_PEERCRED` (reuse broker `peercred` pattern); allowlisted broker uid/gid only.
- Framing: bounded length-prefixed frames; version field; strict schema; deny-unknown fields.
- Request types (broker → watchdog): `Arm`, `ReportHealth`, `RequestLeaseRenewal`, `Heartbeat`, `RequestDecision` (health/invariant results only — **not** a decision choice), `Disarm` (test/admin hook behind fake in hermetic tests).
- Forbidden caller inputs: log record bodies; epoch choice; invariant evaluation result forgery as authority; terminal state; filesystem paths; signal targets; `BEGIN_*` selection.
- Response binding: each response carries `{request_id, host_id, tx_id, epoch}`; replays/duplicates fail closed.
- Uniform errors: no partial durable side effects on refusal.

### Arming payload

Validate and durably record `{host_id, tx_id, epoch, immutable base, deadline, mandatory invariant set, additive manifest checks}`:

- Reject moved base, wrong epoch, unknown/duplicate/conflicting arming, weakened/removed mandatory invariants, expired deadlines, ambiguity before `ARMED`.
- Manifest checks additive only (can increase strictness, never relax mandatory set).

### Lease and heartbeat

Bounded renewable lease bound to `{host_id, tx_id, epoch, lease_id, process_group, deadline}`:

- Authenticated local heartbeat/progress messages; deterministic clock and authenticator fakes.
- Replays, wrong binding, clock regression, late renewal, liveness ambiguity → fail closed.
- Production OOB signing remains blocked on H-02 (heartbeat is local-authenticated only in L03).

### Fencing sequence (on lease expiry)

Ordered, no overlap with recovery decision:

1. `SIGTERM` to entire commit-worker process group.
2. Bounded wait.
3. `SIGKILL` to entire process group.
4. Confirmed group exit.
5. Candidate-job inventory empty (injected `JobInspector`).
6. Only then inspect durable state and emit recovery decision response.

Failure to signal, wait, prove exit, or prove zero jobs → safe mode / refusal. Expired lease must never overlap a surviving commit worker or revert (L03-AC08, AC09).

## 4. Acceptance traceability

| AC | Intended paths | Verification |
|---|---|---|
| **L03-AC01** | `watchdogd/topology.rs`, `adapters/nix/protected.rs`, `broker/tests/l03_topology.rs` (planned) | Hermetic topology fakes: exact mount point, distinct device/mount ID, no fallback, startup refusal matrix; broker/state WAL path protected alongside `/wal/` alias; permissions/ownership/symlink rejection; no live mount/install |
| **L03-AC02** | `watchdogd/storage/decision_log.rs`, `broker/watchdog/{types,client}.rs` | Only watchdog crate can append authority records; broker has no append API; single-writer framing; monotonic sequence/epoch binding; file fsync before response; creation parent fsync; corruption/truncation refusal tests |
| **L03-AC03** | `watchdogd/storage/{epoch,safe_mode}.rs` | Atomic replacement protocol; monotonic allocation/refusal; decision-log cross-check; injected external-floor mismatch; durable safe-mode marker; fail-closed for missing/stale/rolled-back/corrupt/ambiguous/unavailable storage; no external/OOB implementation |
| **L03-AC04** | `watchdogd/rpc/{protocol,server}.rs`, `watchdogd/auth.rs`, `broker/watchdog/client.rs` | Bounded versioned Unix protocol; deny-unknown; peercred authorization; request/response binding; replay/duplicate handling; deadlines; uniform fail-closed errors; forbidden-field rejection matrix |
| **L03-AC05** | `watchdogd/arming.rs`, `watchdogd/invariants.rs` | Arming validation matrix: moved base, wrong epoch, duplicate/conflicting arming, weakened mandatory invariants, expired deadline, ambiguity; additive manifest-only strictness |
| **L03-AC06** | `watchdogd/authority.rs`, `broker/transaction/recovery.rs` (existing safe-mode on watchdog WAL) | Only watchdog selects `BEGIN_COMMIT`/`BEGIN_REVERT`; duplicate/stale/unknown/malformed/moved-base/wrong-epoch/expired-lease/corrupt-log/ambiguous requests cannot cause or override decision; broker WAL with watchdog-owned states still fails closed |
| **L03-AC07** | `watchdogd/lease.rs`, `watchdogd/auth.rs` | Lease renewal/expiry; heartbeat binding; clock regression/replay/wrong-binding tests with deterministic fakes; production OOB signing not claimed |
| **L03-AC08** | `watchdogd/fencing.rs`, `watchdogd/fakes/{process,job}.rs`, `watchdogd/tests/fencing_fixture.rs` | SIGTERM→wait→SIGKILL→group exit→zero jobs ordering; bounded spawned-fixture test; failure injection keeps safe mode; no overlap with revert |
| **L03-AC09** | `watchdogd/tests/l03_failure_matrix.rs` (planned) | Hermetic matrix: corrupt/truncated log; epoch rollback/mismatch; stale/malformed/duplicate/unknown RPC; moved base; wrong epoch; expired lease; stopped heartbeat; process survivor; job survivor; every fsync/rename/readback boundary; unavailable store; restart reconstruction; safe-mode persistence |
| **L03-AC10** | `watchdogd/interfaces.rs`, `watchdogd/fakes/`, PLAN non-goals §1 | All externals injected; tests need no root/NixOS/systemd/Proxmox/credentials; explicit statement: no hostile-root boundary, no Gate 1 exit evidence |
| **L03-AC11** | `plans/AGB-8/{PLAN,RESULT,red-evidence}.md` | PLAN before production; RED tests/fixtures only against unchanged production; focused RED bare/unpiped with causal non-zero output; smallest GREEN; RESULT with exact current-head commands; DCO on every commit |
| **L03-AC12** | PLAN §1 non-goals | Review + static checks prove no L04/L05/live install/OOB/credentials/router changes in this lane |

## 5. Failure and authority matrix (L03-AC09)

| Scenario | Inject | Expected |
|---|---|---|
| Decision log corrupt/truncated tail | Mutate last frame bytes | Safe mode; refuse arming/RPC that needs log |
| Decision log partial write | Drop fsync or truncate mid-frame | Refusal; no response claiming acceptance |
| Epoch store rollback | Replace `high-water.json` with stale value below log max | Safe mode; monotonic refusal |
| External floor below local high-water | Inject `ExternalFloor` fake | Safe mode; no new epoch |
| Epoch/log cross-check mismatch | Epoch file ahead of log binding | Safe mode |
| Stale epoch on RPC | Reuse old epoch after increment | Fail closed; no new authority record |
| Unknown tx_id | Random tx on heartbeat | Fail closed |
| Duplicate arm | Same tx_id twice with conflict | Second refused before double `ARMED` |
| Moved base | Changed base revision on re-arm | Refused before `ARMED` |
| Weakened mandatory invariant | Remove default invariant from set | Refused before `ARMED` |
| Expired arming deadline | Clock fake past deadline | Refused / `BEGIN_REVERT` path only via watchdog evaluation |
| Malformed/oversized frame | Bad length/CRC/version | Uniform error; no durable append |
| Peercred mismatch | Wrong uid on socket | Refused before handler |
| Replay request_id | Duplicate RPC with same binding | Fail closed |
| Heartbeat wrong lease binding | Mismatched `lease_id`/`process_group` | Fail closed |
| Clock regression | Decrease fake clock on renewal | Fail closed |
| Lease expiry with live worker | Process fake survives SIGTERM | Safe mode; no `BEGIN_*` until fenced |
| Job survivor after fence | JobInspector reports non-zero | Safe mode; no recovery decision |
| fsync failure on append | Durability fake fails file fsync | No accepted record; error to caller |
| rename failure on epoch | Durability fake fails rename | Safe mode; epoch not advanced |
| Unavailable store | Open returns EIO | Safe mode at startup |
| Restart reconstruction | Reopen log + epoch after crash injection | Consistent recovery or safe mode; no silent repair |
| Safe-mode persistence | Marker present at startup | All arming/RPC refused until cleared by defined test hook |
| Broker WAL contains `Committed` | Existing L01 test | Broker safe mode (unchanged) |

## 6. Verification commands

Full GREEN gates (bare/unpiped):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
git diff --check 01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c..HEAD
```

Focused L03 hermetic suites (RED and GREEN):

```bash
cargo test -p agentbed-watchdogd
cargo test -p agentbed-broker --test l03_watchdog_client
cargo test -p agentbed-adapter-nix -- protected_broker_state
```

RED checkpoint: execute focused suites against **unchanged** production (`watchdogd` stub); preserve unpiped causal non-zero output in `plans/AGB-8/red-evidence.txt`.

## 7. Rollback and stop conditions

Stop if decision log shares rollback/WAL domain or topology proof accepts fallback directory. Stop if broker can append or select `BEGIN_*`/terminal records. Stop if expired lease can overlap a surviving worker, revert, or candidate job. Stop if L03 claims live mount provisioning, OOB implementation, or Gate 1 exit evidence.

Revert leaves watchdog disarmed and candidate/base unchanged. Hermetic tests use temp stores only.

## 8. Delivery sequence

Branch `agent/agb-8/l03-watchdog-decision-authority`, PR `AGB-8: Watchdog decision authority and durable local protocol`.

| Phase | Content | Stop after |
|---|---|---|
| **PLAN** (this artifact) | `plans/AGB-8/PLAN.md` only | Coordinator readback — **current run** |
| **RED** | Tests, fixtures, `red-evidence.txt` only; production unchanged | Independent RED acceptance |
| **GREEN** | Smallest implementation; `plans/AGB-8/RESULT.md` | Push branch, open PR, `in_review` |
| **Review/merge** | Current-head CI, scenario matrix verification, `agentos-reviewer` | Explicit L-P `merge AGB-8` only |

TDD: PLAN → RED (tests only) → GREEN (implementation) → RESULT. DCO sign-off (`git commit -s`) on every commit. Repairs stay on same issue/branch/worktree/PR; head change invalidates prior gates.

## 9. Gate exit honesty

L03 delivers hermetic proof of watchdog-only durable authority, local RPC boundary, arming/invariants, leases/heartbeats, fencing ordering, and epoch/safe-mode fail-closed behavior. It does **not** close Gate 1. Gate 1 exit requires L04–L08 evidence including live OOB, spare-node chaos matrix, and authorized infrastructure runs — all explicitly out of scope here.
