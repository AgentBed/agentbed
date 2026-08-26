# AGB-8 — L03 Watchdog decision authority and durable local protocol

**Issue:** AGB-8 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c` (`origin/main`, verified 2026-08-25)
**Roadmap gate / exit:** Gate 1 L03 (`plans/AGB-1/PLAN.md` lane L03). Gate 1 remains open after this lane.

## 1. Evidence and scope

Inspected: `plans/AGB-1/PLAN.md` L03 row; ADR-001 rev. 6; `docs/threat-model.md`; `docs/effects.md` §§3a, 4; `docs/protocol.md`; L01 `plans/AGB-4/RESULT.md`; L02 `plans/AGB-6/RESULT.md`; `watchdogd/src/lib.rs` (empty stub); `broker/src/transaction/{state,engine,recovery}.rs`; `broker/src/peercred.rs`; `adapters/nix/src/protected.rs`; `broker/tests/l01_repair_review.rs` (watchdog-owned WAL → safe mode).

L00–L02 merged: v2 wire contract, durable broker WAL/events/idempotency, Nix propose with semantic class-F rejection and hermetic promotion primitives. `watchdogd` is still an intentional empty stub. Broker engine refuses watchdog-owned WAL states at recovery; happy path stops at `Probation`. L02 protected-path matcher names `/var/lib/agentbed/wal/` but production broker state (and WAL) lives at `/var/lib/agentbed/broker/state` — L03 must close that real-path gap without weakening existing aliases.

This lane implements **watchdog-only durable authority** and the narrow authenticated local broker↔watchdog protocol as a **library/protocol/authority contract only** — no daemon binary, entrypoint, lifecycle, cgroup worker-handle minting, service/install/start, or live fencing. It does **not** wire broker transaction orchestration (L04), perform actual promotion/revert execution, provision mounts, or implement OOB. Daemon entrypoint, lifecycle, cgroup ownership, and live fencing remain later-lane work.

### Consequential assumptions

1. **Hermetic only.** Filesystem durability, topology proof, clock, local authentication, entropy, process-group fencing (via injected interfaces only — no live signaling in L03), job inspection, and invariant observations are injected behind narrow interfaces with fakes. Tests require no root, NixOS VM, systemd daemon, Proxmox, credentials, or live host mutation. Real-signal tests on developer/login/shared-runner hosts are explicitly forbidden in L03.
2. **H-04 sealed design.** The exact domains, paths, permissions, durability semantics, and startup refusal rules below are authoritative for L03. No improvisation of alternate mount roots, fallback directories, or cross-filesystem stores.
3. **Single writer.** Only watchdog-owned code appends decision-log records (`ARMED`, `PROBATION_PASSED`, `BEGIN_COMMIT`, `BEGIN_REVERT`, `COMMITTED`, `REVERTED`). Broker crates hold request/response types and RPC client stubs only — no append API, no epoch allocation, no invariant evaluation, no terminal-state selection.
4. **Process independence, not hostile-root boundary.** Permission/ownership checks and transaction self-protection are real; a malicious root can still alter watchdog files. OOB is the backstop (H-02/H-05, L05+). L03 must state this honestly (L03-AC10).
5. **External epoch floor is injected only.** L03 provides the outside-domain epoch-store abstraction and an injectable external-floor interface for hermetic mismatch tests. It does not implement OOB mirror placement, signing, or live rollback detection against an external store.
6. **Session bootstrap is mandatory and non-authority.** `SessionBind`/`SessionEstablished` precede all five production request types; capability is never persisted or logged; reconnect requires exact durable-state match.
7. **Library-only crate.** `watchdogd` remains library-only with no `[[bin]]`, no `main.rs`, and no production constructor for real signaling. Daemon entrypoint, lifecycle, cgroup ownership/worker-handle minting, service/install/start, and live fencing are later-lane work.

### Hard non-goals (L03-AC12)

No L04 commit/recovery orchestration or actual promotion/revert execution; no L05 OOB implementation; no live watchdog/systemd install/start; no live NixOS/profile/boot/network/firewall/Proxmox mutation; no deployment or activation; no credentials/keys/external epoch store; no Gate 2+; no router/reconciler/agent-instruction or repository-setting changes; no push to `main`; no merge.

## 2. Sealed H-04 topology and storage contract

### Exact domains and locations

| Domain | Path | Notes |
|---|---|---|
| Watchdog dedicated store (mount root) | `/var/lib/agentbed/watchdog` | Required dedicated, non-snapshotted mount; excluded from Nix generation switching and rollback snapshot sets |
| Watchdog binary | `/var/lib/agentbed/watchdog/runtime/agentbed-watchdogd` | `root:root`, mode `0555`, regular file |
| Watchdog config | `/var/lib/agentbed/watchdog/config/watchdog.json` | `root:root`, mode `0400`, regular file |
| Decision log | `/var/lib/agentbed/watchdog/decisions/decision.log` | Watchdog-only, append-only; `root:root`, mode `0600`, regular file, link count 1 |
| Epoch high-water | `/var/lib/agentbed/watchdog/epoch/high-water.json` | Outside rollback/WAL domain; `root:root`, mode `0600`, regular file, link count 1 |
| Safe-mode marker | `/var/lib/agentbed/watchdog/state/safe-mode.json` | Durable fail-closed flag; `root:root`, mode `0600`, regular file, link count 1 |
| Volatile RPC socket dir | `/run/agentbed/watchdog/` | `root:root`, mode `0700`; not authoritative after restart |
| Volatile RPC socket | `/run/agentbed/watchdog/watchdog.sock` | `root:root`, mode `0600`; not authoritative after restart |
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

**Fail-closed triggers:** checksum/frame/sequence/hash/epoch inconsistency; partial tail; truncation; missing expected file; ambiguous temp file; local epoch high-water rolled back below maximum epoch observed in the decision log or below the injected external floor; external floor unavailable or ambiguous; failed fsync/rename/readback; unavailable storage. No auto-truncation, best-effort continuation, or default epoch.

**RPC refusal without new authority record:** stale epoch; unknown transaction; duplicate/conflicting request; moved base; expired lease; malformed/oversized frame; bad frame CRC or payload length; deny-unknown JSON; authentication failure; `SessionBind` stale/conflicting binding; capability/counter/request-response binding mismatch; corrupt log; ambiguous state.

### Ownership and update boundary

- Mount root and all watchdog-store subdirectories: `root:root`, mode `0700`.
- Decision log, epoch high-water store, and safe-mode marker: regular files, `root:root`, mode `0600`, link count exactly one.
- Runtime socket directory: `root:root`, mode `0700`; socket: `root:root`, mode `0600`.
- Reject symlinks, non-regular files, unexpected owner/group/mode, link count other than one, hard-link ambiguity, digest/config mismatch.
- If unavailable storage prevents persisting the safe-mode marker when fail-closed entry is required, the in-memory latch remains active, refuses all arming/RPC, and the library runtime must terminate or refuse startup — it must not claim durable safe-mode marking without a persisted marker.
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

Broker may request transitions; it cannot select or append watchdog-owned records. Only the watchdog durably chooses `BEGIN_COMMIT` versus `BEGIN_REVERT`. L04 wires broker engine to these answers; L03 delivers the library protocol and authority contract plus hermetic proof.

### Component layout

```
watchdogd/src/
  topology.rs           — mount/device proof, path normalization, permission checks (L03-AC01)
  storage/
    decision_log.rs     — framed append-only log, single writer (L03-AC02)
    epoch.rs            — high-water allocation/refusal, external-floor hook (L03-AC03)
    safe_mode.rs        — durable marker + in-memory latch when persist unavailable (L03-AC03)
    durability.rs       — shared temp/rename/fsync/readback helpers
  rpc/
    protocol.rs         — sealed wire DTOs, FrameCodec (length/CRC/JSON), SessionBind bootstrap, deny-unknown (L03-AC04)
    server.rs           — Unix listener, SessionBind→SessionEstablished, deadlines, response binding (L03-AC04)
  auth.rs               — SO_PEERCRED + SessionEstablished capability mint/validation + counter binding (L03-AC04, AC07)
  arming.rs             — invariant set validation, immutable capture (L03-AC05)
  authority.rs          — sole BEGIN_* selection from watchdog-owned evaluation (L03-AC06)
  lease.rs              — renewable lease + heartbeat/progress (L03-AC07)
  fencing.rs            — injected `ProcessGroupFence` ordering contract; production unavailability (L03-AC08)
  invariants.rs         — mandatory invariant observation interface (injected; watchdog reads)
  interfaces.rs         — Clock, Entropy, ProcessGroupFence, JobInspector, Topology, ExternalFloor, WorkerGroupTag
  lib.rs                — library surface; no `[[bin]]` or `main.rs` in L03 (L03-AC12)
  fakes/                — hermetic doubles for all injected interfaces (L03-AC10)
  tests/                — crate integration + hermetic fencing-seam tests (L03-AC08, AC09)

broker/src/
  watchdog/
    client.rs           — RPC client stub using only `watchdogd::rpc::protocol` DTOs; no decision-record constructor/append API (L03-AC02, AC04)
  transaction/engine.rs — optional early refusal if watchdog client reports safe mode (minimal; full wire in L04)

adapters/nix/src/
  protected.rs          — add `/var/lib/agentbed/broker/state` and alias normalization (L03-AC01)
```

Wire DTOs live exclusively in `watchdogd/src/rpc/protocol.rs`. The broker client imports those request/response types only; it receives no decision-record constructor, append API, or authority-selection hook. Test helpers that bypass wire authentication or clear safe mode are non-wire, test-only, and never exposed on the production RPC surface.

### Narrow authenticated local RPC (broker ↔ watchdog)

- Transport: Unix socket at `/run/agentbed/watchdog/watchdog.sock` (volatile; tests use temp paths). Socket directory `root:root` mode `0700`; socket `root:root` mode `0600`.
- **Frame codec (normal server/client transport primitive):** every wire message uses `encode_frame` / `decode_frame` (or equivalent `FrameCodec`) — not a test helper. On the wire: `u32` big-endian payload length (maximum applies to payload length only), `u32` CRC32 of the payload, then UTF-8 JSON payload. The JSON envelope includes a protocol version field and is deserialized with strict deny-unknown semantics. Typed `encode_request` / `decode_request` and response equivalents build on this codec; RED may construct valid adverse frames by mutating encoded frame bytes (length/CRC/payload) without any test-only production API.
- **Local process authentication** (not OOB signing, not a hostile-root boundary):
  1. Server derives peer identity from `SO_PEERCRED` and verifies it matches the exact configured broker peer `{uid, gid}` from `watchdog.json`. Wrong peer is refused before any handler.
  2. **Session bootstrap (non-authority control exchange, mandatory before the five production request types):** after peer verification, the client sends a bounded strict `SessionBind` control envelope carrying **only** `{host_id, tx_id, epoch, lease_id, worker_group_tag, client_nonce}`. `worker_group_tag` is an opaque `WorkerGroupTag` newtype over `u32` — correlation/binding data only, never a signal target. `TryFrom<u32>` rejects `0`, `1`, and values above `i32::MAX`; negative JSON values fail decoding as `MalformedRequest`. The watchdog validates that binding against durable/current state (new transaction or exact reconnect), mints an entropy-backed 256-bit capability bound to peer `{pid, uid, gid}` plus those fields, and returns `SessionEstablished {capability, server_nonce, counter=0}`. This control exchange cannot carry health/invariant/decision/terminal/path/signal inputs and **cannot append authority records**. It is **not** a sixth authority request, **not** `Disarm`, and **not** OOB.
  3. **Post-bootstrap binding:** every one of the five production request envelopes and every response carries and validates the exact capability, request ID, host/tx/epoch binding, and strictly monotonic counter. Old capabilities die on reconnect or RPC server restart. A reconnect may establish a new capability only when `SessionBind` exactly matches reconstructed durable state; unknown/stale/conflicting bindings fail closed with **no new authority record**.
  4. Capability is connection/session-lifetime only: never persisted, never logged, never provisioned to callers as a durable credential.
- **Production authority request types (broker → watchdog, post-bootstrap only):** exactly five — `Arm`, `ReportHealth`, `RequestLeaseRenewal`, `Heartbeat`, `RequestDecision`. No sixth authority type, no `Disarm`, no OOB handshake.
- **No production `Disarm` or admin hook.** No caller-visible RPC may disarm, clear safe mode, or choose terminal authority. Test helpers that simulate disarm or safe-mode clear are non-wire, test-only, and not part of the production protocol.
- **Watchdog-owned evaluation:**
  - `ReportHealth` — advisory progress only; carries no invariant result, decision, terminal state, epoch choice, path, or signal target. Broker-supplied health hints cannot override watchdog-owned invariant, lease, or fencing evidence.
  - `RequestDecision` — trigger only; carries no health result, invariant result, decision, terminal state, epoch choice, path, or signal target. The watchdog independently reads/evaluates mandatory invariants (via injected `invariants` interface), its own clock, and lease state before durably choosing `BEGIN_COMMIT` or `BEGIN_REVERT`.
- Forbidden caller inputs on all post-bootstrap requests: log record bodies; epoch choice; invariant evaluation results presented as authority; terminal state; filesystem paths; signal targets; `BEGIN_*` selection; any field that would let the broker override watchdog-owned invariant/lease/fencing evidence. `WorkerGroupTag` is never converted to a syscall target — the `ProcessGroupFence` trait takes no caller-supplied target; a fencer's target (when implemented in a later lane) is fixed at construction by whoever owns the worker.
- Duplicate/conflicting requests are strictly rejected with **no new authority record**.
- Response binding: each response carries `{request_id, host_id, tx_id, epoch}` plus capability/counter binding; replays/duplicates fail closed.
- Uniform errors: no partial durable side effects on refusal.

### Arming payload

Validate and durably record `{host_id, tx_id, epoch, immutable base, deadline, mandatory invariant set, additive manifest checks}`:

- Reject moved base, wrong epoch, unknown/duplicate/conflicting arming, weakened/removed mandatory invariants, expired deadlines, ambiguity before `ARMED`.
- Manifest checks additive only (can increase strictness, never relax mandatory set).

### Lease and heartbeat

Bounded renewable lease bound to `{host_id, tx_id, epoch, lease_id, worker_group_tag, deadline}`:

- Authenticated local heartbeat/progress messages gated by post-bootstrap `SessionEstablished` capability + strictly monotonic counter binding; deterministic clock, entropy, and authenticator fakes.
- Replays, wrong binding, clock regression, late renewal, liveness ambiguity → fail closed.
- Local process authentication only — not OOB signing, not a hostile-root boundary. Production OOB signing remains blocked on H-02.

### Fencing sequence (on lease expiry)

L03 delivers the **ordering/non-overlap contract** through injected hermetic `ProcessGroupFence` and `JobInspector` interfaces plus fail-closed production unavailability. L03 provides **no live process-group signaling** — `watchdogd` is a library-only crate with no daemon; production fencing is `UnavailableProcessGroupFencer` (returns `FenceError::Unavailable`; `group_alive` resolves ambiguity toward "still alive"). On lease expiry where real fencing is unavailable or ambiguous, the watchdog persists safe mode and refuses authority; it must **never** append or return `BEGIN_*` while a worker may survive.

Hermetic ordering contract (enforced via recording fakes, no real signals):

1. `signal(Term)` on injected fencer.
2. `bounded_wait(Term)` on injected fencer.
3. Observe `group_alive(AfterTerm)` — result **consumed**, not discarded.
4. If no longer alive after Term → **skip Kill** and proceed to step 8 (job inventory).
5. Only if still alive after AfterTerm → `signal(Kill)` on injected fencer.
6. `bounded_wait(Kill)` on injected fencer.
7. Observe `group_alive(AfterKill)` and require absent; any ambiguity or still-alive → safe mode/refusal.
8. `candidate_job_count` zero (injected `JobInspector`).
9. Only then inspect durable state and evaluate/emit recovery decision response.

Any signal, wait, liveness, or job-inventory ambiguity or failure → safe mode/refusal with **no** `BEGIN_*` authority append or response. The recording fake in `fencing_seam.rs` must prove **both branches**: Term-success skips Kill (steps 1–4 → 8–9); Term-survivor requires Kill → bounded_wait(Kill) → confirmed AfterKill absence (steps 1–9).

Failure at any step → safe mode / refusal. Expired lease must never overlap a surviving commit worker or revert (L03-AC08, AC09) — satisfied by construction when production fencing is unavailable and authority is refused.

**Deferred to later daemon lane:** real termination proof using watchdog-minted cgroup-v2 worker handles and `cgroup.kill`. That lane owns the daemon, cgroup delegation, and systemd integration (H-03/L05+).

### Later-lane fencing invariants (normative, binding on future signaling)

Recorded now for the lane that reintroduces live fencing:

- The signal target is **never caller-selected**; the target set may only **shrink**; ambiguity never widens it or grants authority.
- **`saturating_neg` is forbidden** on any value that might reach a syscall (it turns `1` into session broadcast and `i32::MIN` into `i32::MAX`). Use `checked_neg()` only on values already proven `>= 2` and already proven owned.
- `ESRCH` on an unproven target means nothing — never treat as success. `ESRCH` on a proven-owned target means target absent.
- `EPERM` → `FenceError::Ambiguous` → safe mode. Not "alive, escalate."
- `waitpid` `ECHILD` is not proof of exit; exit proof must come from pidfd/cgroup handle.
- Any other errno (`EINVAL`, …) → safe mode. No retry, no broadening.
- Bounded wait expires → `FenceError::Incomplete` → safe mode. Never extend deadline, widen target, or escalate signal scope.
- Group liveness unknown → treat as alive **and** refuse; "alive" must never trigger a broader signal.

## 4. Acceptance traceability

| AC | Intended paths | Verification |
|---|---|---|
| **L03-AC01** | `watchdogd/topology.rs`, `adapters/nix/protected.rs`, `broker/tests/l03_topology.rs` (planned) | Hermetic topology fakes: exact mount point, distinct device/mount ID, no fallback, startup refusal matrix; broker/state WAL path protected alongside `/wal/` alias; permissions/ownership/symlink rejection; no live mount/install |
| **L03-AC02** | `watchdogd/storage/decision_log.rs`, `broker/src/watchdog/client.rs` | Only watchdog crate can append authority records; broker client has no decision-record constructor/append API; single-writer framing; monotonic sequence/epoch binding; file fsync before response; creation parent fsync; corruption/truncation refusal tests |
| **L03-AC03** | `watchdogd/storage/{epoch,safe_mode}.rs` | Atomic replacement protocol; monotonic allocation/refusal; decision-log cross-check; injected external-floor mismatch; durable safe-mode marker with in-memory latch when persist unavailable; fail-closed for missing/stale/rolled-back/corrupt/ambiguous/unavailable storage; no external/OOB implementation |
| **L03-AC04** | `watchdogd/src/rpc/{protocol,server}.rs`, `watchdogd/auth.rs`, `broker/src/watchdog/client.rs` | Bounded Unix protocol with sealed `FrameCodec` (`u32` BE length + `u32` CRC32 + versioned UTF-8 JSON, deny-unknown, payload-length max); SO_PEERCRED refusal; mandatory `SessionBind`→`SessionEstablished` bootstrap before the five authority requests; capability + request ID + host/tx/epoch + monotonic counter on every post-bootstrap request/response; stale reconnect/conflicting binding refusal; bad length/CRC/version; syntactically valid JSON unknown field through normal codec; real socket flow `connect → bootstrap → authenticated Arm → bound response`; no `Disarm`/OOB on wire; no persisted/logged capability |
| **L03-AC05** | `watchdogd/arming.rs`, `watchdogd/invariants.rs` | Arming validation matrix: moved base, wrong epoch, duplicate/conflicting arming (no new authority record), weakened mandatory invariants, expired deadline, ambiguity; additive manifest-only strictness |
| **L03-AC06** | `watchdogd/authority.rs`, `broker/transaction/recovery.rs` (existing safe-mode on watchdog WAL) | Only watchdog selects `BEGIN_COMMIT`/`BEGIN_REVERT` from its own invariant/clock/lease evaluation; broker-supplied observations cannot override watchdog-owned evidence; duplicate/stale/unknown/malformed/moved-base/wrong-epoch/expired-lease/corrupt-log/ambiguous requests cannot cause or override decision; broker WAL with watchdog-owned states still fails closed |
| **L03-AC07** | `watchdogd/lease.rs`, `watchdogd/auth.rs` | Lease renewal/expiry; heartbeat capability+counter binding; clock regression/replay/wrong-binding tests with deterministic fakes; local process auth only; production OOB signing not claimed |
| **L03-AC08** | `watchdogd/fencing.rs`, `watchdogd/interfaces.rs`, `watchdogd/fakes/{process,job}.rs`, `watchdogd/tests/fencing_seam.rs` | Hermetic recording-fake ordering matrix: Term → bounded_wait(Term) → AfterTerm (consumed) → [if alive: Kill → bounded_wait(Kill) → AfterKill absent] → zero jobs → recovery decision; Term-success branch skips Kill; Term-survivor branch requires Kill→wait→confirmed absence; `UnavailableProcessGroupFencer` → safe mode with no authority record; source-level proof that `watchdogd/src/**` contains no `libc::kill`, `libc::waitpid`, `libc::killpg`, or `libc::sigqueue` and `fencing.rs` contains no `unsafe`; no overlap with revert. Real termination proof deferred to later daemon lane (watchdog-minted cgroup-v2 handles, `cgroup.kill`). **No spawned fixture; no real-signal tests on developer/login/shared-runner hosts.** |
| **L03-AC09** | `watchdogd/tests/l03_failure_matrix.rs` (planned) | Hermetic matrix: corrupt/truncated log; epoch rollback/mismatch; stale/malformed/duplicate/unknown RPC; `SessionBind` stale reconnect; capability/counter/binding mismatch; bad frame length/CRC/version; deny-unknown JSON via frame codec; moved base; wrong epoch; expired lease; stopped heartbeat; process survivor; job survivor; every fsync/rename/readback boundary; unavailable store; restart reconstruction; safe-mode persistence |
| **L03-AC10** | `watchdogd/interfaces.rs`, `watchdogd/fakes/`, PLAN non-goals §1 | All externals injected; tests need no root/NixOS/systemd/Proxmox/credentials; explicit statement: no hostile-root boundary, no Gate 1 exit evidence |
| **L03-AC11** | `plans/AGB-8/{PLAN,RESULT,red-evidence}.md` | PLAN before production; RED tests/fixtures only against unchanged production; focused RED bare/unpiped with causal non-zero output; smallest GREEN; RESULT with exact current-head commands; DCO on every commit |
| **L03-AC12** | PLAN §1 non-goals | Review + static checks prove no L04/L05/live install/OOB/credentials/router changes in this lane |

## 5. Failure and authority matrix (L03-AC09)

| Scenario | Inject | Expected |
|---|---|---|
| Decision log corrupt/truncated tail | Mutate last frame bytes | Safe mode; refuse arming/RPC that needs log |
| Decision log partial write | Drop fsync or truncate mid-frame | Refusal; no response claiming acceptance |
| Epoch store rollback | Replace `high-water.json` with stale value below log max | Safe mode; monotonic refusal |
| Local epoch high-water below injected external floor | Inject `ExternalFloor` fake with floor above local high-water | Safe mode; no new epoch; no OOB implementation claimed |
| External floor unavailable or ambiguous | `ExternalFloor` fake returns unavailable/ambiguous | Safe mode; refuse arming and epoch allocation; no invented floor |
| Epoch/log cross-check mismatch | Epoch file ahead of log binding | Safe mode |
| Stale epoch on RPC | Reuse old epoch after increment | Fail closed; no new authority record |
| Unknown tx_id | Random tx on heartbeat | Fail closed |
| Duplicate arm | Same tx_id twice with conflict | Second refused; no new authority record |
| Duplicate/conflicting RPC | Replay or conflicting binding on any request type | Fail closed; no new authority record |
| Moved base | Changed base revision on re-arm | Refused before `ARMED` |
| Weakened mandatory invariant | Remove default invariant from set | Refused before `ARMED` |
| Expired arming deadline | Clock fake past deadline | Refused / `BEGIN_REVERT` path only via watchdog evaluation |
| Malformed/oversized frame | Bad payload length, CRC mismatch, or protocol version | Uniform error via `decode_frame`; no durable append |
| Deny-unknown JSON field | Syntactically valid frame with unknown JSON field re-encoded through normal `FrameCodec` | `DenyUnknown`; no handler side effects |
| Peercred mismatch | Wrong `{uid,gid}` on socket | Refused before `SessionBind` or any handler |
| SessionBind stale/conflicting binding | Reconnect with binding that does not match reconstructed durable state | Fail closed; no `SessionEstablished`; no authority record |
| SessionBind success | Valid peer + binding against durable state | `SessionEstablished {capability, server_nonce, counter=0}`; no authority record |
| Post-bootstrap capability mismatch | Wrong/stale capability on any of the five request types | Fail closed; no authority record |
| Post-bootstrap counter replay | Duplicate counter with same capability | Fail closed; no authority record |
| Response binding mismatch | Response capability/counter/request ID does not match request | Fail closed on decode |
| Unix socket authenticated flow | `connect → SessionBind → SessionEstablished → Arm → bound response` | Full codec path; `ARMED` only after valid bootstrap + bound post-bootstrap exchange |
| Stale epoch on RPC | Reuse old epoch after increment | Fail closed; no new authority record |
| Heartbeat wrong lease binding | Mismatched `lease_id`/`worker_group_tag` | Fail closed |
| Clock regression | Decrease fake clock on renewal | Fail closed |
| Lease expiry with live worker | `UnavailableProcessGroupFencer` or recording fake simulating survivor | Safe mode; no `BEGIN_*`; no authority record |
| Fence Term-success (worker gone) | Recording fake: AfterTerm absent | Skip Kill; proceed to job inventory; no `BEGIN_*` until steps 8–9 complete |
| Fence Term-survivor | Recording fake: AfterTerm alive, then Kill→wait→AfterKill absent | Full Kill branch required; no `BEGIN_*` until steps 8–9 complete |
| Job survivor after fence | JobInspector reports non-zero | Safe mode; no recovery decision |
| WorkerGroupTag decode rejection | Wire values `0`, `1`, negative, or `> i32::MAX` | Uniform `MalformedRequest`; no binding |
| Fencing unavailable at expiry | Production `UnavailableProcessGroupFencer` | Safe mode; refuse authority; worker may survive but no `BEGIN_*` |
| fsync failure on append | Durability fake fails file fsync | No accepted record; error to caller |
| rename failure on epoch | Durability fake fails rename | Safe mode; epoch not advanced |
| Unavailable store | Open returns EIO | Safe mode at startup |
| Restart reconstruction | Reopen log + epoch after crash injection | Consistent recovery or safe mode; no silent repair |
| Safe-mode persistence | Marker present at startup | All arming/RPC refused; no wire RPC clears safe mode |
| Safe-mode persist unavailable | Durability fake fails safe-mode write at fail-closed entry | In-memory latch; refuse/terminate; no claim of durable marking |
| Broker WAL contains `Committed` | Existing L01 test | Broker safe mode (unchanged) |

## 6. Verification commands

### Command safety classification

**Forbidden outside an isolated private PID namespace/container at any head at or before `789ec353e6cdac98b8946b93dd538b2bd32a75b9`:**

- `cargo test -p agentbed-watchdogd --test fencing_fixture`
- `cargo test --workspace` (compiles and runs `fencing_fixture`)
- `cargo test -p agentbed-watchdogd` (same)

These commands are unsafe on developer/login/shared-runner hosts because `fencing_fixture.rs` issues real `kill`/`waitpid` syscalls. **After the RED commit deletes `fencing_fixture.rs`, ordinary hermetic test commands become safe.**

Any future real-signal proof (not L03) requires: `#[ignore]`, env gate `AGENTBED_ALLOW_REAL_SIGNALS=1`, and runtime assertion that `getpid() == 1` (PID 1 of a private PID namespace) which aborts before spawning if false. Never on developer, login, or CI-runner sessions.

**Safe at every head:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `git diff --check 01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c..HEAD`.

### Full GREEN gates (bare/unpiped)

After fencing-safety RED (fixture deleted):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
git diff --check 01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c..HEAD
```

Focused L03 hermetic suites (RED and GREEN):

```bash
cargo test -p agentbed-watchdogd --test fencing_seam
cargo test -p agentbed-watchdogd --test l03_failure_matrix
cargo test -p agentbed-watchdogd --test l03_review_repair
cargo test -p agentbed-broker --test l03_watchdog_client
cargo test -p agentbed-adapter-nix --test l03_protected_broker_state
```

RED checkpoint (fencing-safety repair): tests/fixtures/evidence only against unchanged production; delete `fencing_fixture.rs`; add `fencing_seam.rs`; preserve unpiped causal non-zero output in evidence. RED must fail causally on (a) missing `WorkerGroupTag`, (b) source-scan absence assertion while `libc::kill` still exists in `fencing.rs`. `fencing_seam.rs` must discriminate both ordering branches: Term-success skips Kill; Term-survivor requires Kill → bounded_wait(Kill) → confirmed AfterKill absence.

### Named RED tests and evidence expectations (L03-AC04 subset)

RED must name discriminating tests (in `watchdogd/tests/l03_red.rs`, `broker/tests/l03_watchdog_client.rs`, and evidence) covering at minimum:

| Test theme | Expected RED behavior |
|---|---|
| `l03_ac04_peercred_refused_before_session_bind` | Wrong peer refused at `SO_PEERCRED`; no `SessionEstablished` |
| `l03_ac04_session_bind_establishes_capability` | Valid `SessionBind` → `SessionEstablished {counter=0}`; no authority record |
| `l03_ac04_session_bind_stale_reconnect_refused` | Reconnect with binding mismatch vs durable state fails closed |
| `l03_ac04_frame_codec_bad_length_crc_version` | `decode_frame` rejects bad payload length, CRC mismatch, unknown version |
| `l03_ac04_deny_unknown_json_field_via_frame_codec` | Valid length/CRC frame with unknown JSON field → `DenyUnknown` |
| `l03_ac04_capability_counter_binding_refused` | Post-bootstrap request with wrong capability/counter/request binding fails closed |
| `l03_ac04_response_binding_mismatch_refused` | `decode_response` rejects capability/counter/request mismatch |
| `l03_ac04_unix_socket_connect_bootstrap_arm_round_trip` | Real `UnixStream::connect` → `SessionBind` → `SessionEstablished` → authenticated `Arm` → bound response via normal `encode_frame`/`decode_frame` |
| `l03_ac04_request_kinds_round_trip_through_codec` | Each of the five authority request types encodes/decodes through normal typed + frame codec surfaces |
| `l03_ac04_protocol_source_excludes_disarm_oob` | Static source check: no `Disarm`, no `OobHandshake` |

Tests construct adverse frames by mutating `encode_frame` output in test-local helpers only; production exposes `encode_frame`/`decode_frame` as normal transport primitives, not test-only conveniences.

## 7. Rollback and stop conditions

Stop if decision log shares rollback/WAL domain or topology proof accepts fallback directory. Stop if broker can append or select `BEGIN_*`/terminal records. Stop if expired lease can overlap a surviving worker, revert, or candidate job **with an authority record appended**. Stop if L03 claims live mount provisioning, live process-group signaling, OOB implementation, or Gate 1 exit evidence. Stop if real-signal tests run on developer/login/shared-runner hosts.

Revert leaves watchdog disarmed and candidate/base unchanged. Hermetic tests use temp stores only.

## 8. Delivery sequence

Branch `agent/agb-8/l03-watchdog-decision-authority`, PR `AGB-8: Watchdog decision authority and durable local protocol`.

| Phase | Content | Stop after |
|---|---|---|
| **PLAN** (this artifact) | `plans/AGB-8/PLAN.md` only | Coordinator readback |
| **RED** (fencing safety) | Delete `fencing_fixture.rs`; add `fencing_seam.rs`; tests/fixtures/evidence only; production unchanged | Independent RED acceptance |
| **GREEN** (fencing safety) | `WorkerGroupTag`, `UnavailableProcessGroupFencer`, remove syscall code; smallest implementation | Coordinator readback |
| **RESULT** | Update `plans/AGB-8/RESULT.md` with fencing-safety repair evidence | Push branch, `in_review` |
| **Review/merge** | Current-head CI, scenario matrix verification, `agentos-reviewer` | Explicit L-P `merge AGB-8` only |

TDD: PLAN → RED (tests only) → GREEN (implementation) → RESULT. DCO sign-off (`git commit -s`) on every commit. Repairs stay on same issue/branch/worktree/PR; head change invalidates prior gates.

## 9. Gate exit honesty

L03 delivers hermetic proof of watchdog-only durable authority, local RPC boundary, arming/invariants, leases/heartbeats, fencing ordering contract (via injected fakes + fail-closed production unavailability), and epoch/safe-mode fail-closed behavior — all as a **library/protocol/authority contract** with no daemon binary or live fencing. It does **not** provide live process-group signaling, create a hostile-root boundary, or close Gate 1. Gate 1 exit requires L04–L08 evidence including live OOB, spare-node chaos matrix, real cgroup-v2 fencing, daemon entrypoint/lifecycle, and authorized infrastructure runs — all explicitly out of scope here.
