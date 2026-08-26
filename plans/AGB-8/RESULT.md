# AGB-8 — L03 Watchdog decision authority and durable local protocol

**Issue:** AGB-8 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c` (`origin/main`, verified 2026-08-25)
**Branch:** `agent/agb-8/l03-watchdog-decision-authority`
**PR:** https://github.com/AgentBed/agentbed/pull/24

## PLAN / RED / GREEN evidence

| Phase | SHA | Artifact |
|---|---|---|
| PLAN (sealed) | `57d17c2364784bdd6609ce6042c60b79bac6be13` | `plans/AGB-8/PLAN.md` |
| RED (initial) | `58ec7da309e435f456b800e14703d4e8536fb24a` | `plans/AGB-8/red-evidence.txt` + L03 RED tests/fixtures |
| GREEN (implementation) | `999a63f11850ed8077aa2e314be9ed1d15959c34` | initial production sources |
| GREEN (clippy repair) | `8072b68c3b02b64415a6e8330c2ba58e6806b1c8` | workspace `-D warnings` compliance |
| GREEN (initial RESULT) | `522c6bcc67a8406e89f7f18cab06367273206314` | initial `RESULT.md` |
| Review RED (scenario verification) | `94301f8b0aeefb0560df87caecba8271ec81ac16` | `plans/AGB-8/review-red-evidence.txt` + `watchdogd/tests/l03_review_repair.rs` |
| Review GREEN (production repair) | `b79ba1bec7d0b2b257a211f6470825b47d21a347` | production fixes for scenario findings F1–F11, F14, F15 |
| PLAN (fencing-safety ratification) | `83821e57aefbddd039d3c31b169a01285909ec12` | amended `plans/AGB-8/PLAN.md` (L-P decision `6179e45d`) |
| RED (fencing-safety) | `5b5d20798eee2eddd5f9e3110365dbd9945419c8` | `plans/AGB-8/fencing-safety-red-evidence.txt` + `watchdogd/tests/fencing_seam.rs`; `fencing_fixture.rs` deleted |
| GREEN (fencing-safety) | *(this head)* | `WorkerGroupTag`, `UnavailableProcessGroupFencer`, sealed 9-step fence ordering; no real signaling |

Accepted initial RED tests/fixtures/evidence remain byte-identical to `58ec7da` except superseded fencing paths. Accepted fencing-safety RED oracle semantics preserved; `fencing_seam.rs` differs from `5b5d207` only by `rustfmt` and one clippy-neutral `is_none_or` guard (no assertion/oracle weakening).

## Critical fencing incident — root cause and safe design

**Root cause:** Prior GREEN introduced `ProductionProcessGroupFencer` with `libc::kill(-pgid, …)` and a spawned `fencing_fixture.rs` test that could signal `kill(-1)` on shared developer/login hosts.

**Ratified safe design (L-P `6179e45d`):**

- Library-only `watchdogd` — no daemon, no live signaling in L03.
- `WorkerGroupTag` (`u32` newtype) on the wire for opaque correlation only; never passed to syscalls.
- `UnavailableProcessGroupFencer` in production: `FenceError::Unavailable`, `group_alive` resolves ambiguity toward still-alive; authority fails closed with no `BEGIN_*`.
- `ProcessGroupFence::signal` has no caller-supplied pgid/target.
- Hermetic ordering via injected fakes: Term → bounded_wait(Term) → AfterTerm consumed → [if alive: Kill → bounded_wait(Kill) → AfterKill absent] → zero jobs → recovery authority.
- Real process termination deferred to a later daemon-owned cgroup-v2 lane.

## Scenario verification review repair — addressed findings

| Finding | Repair |
|---|---|
| **F1** | Decision log append fails closed without `set_len` truncation (`read_model.rs`) |
| **F2** | Runtime safe-mode latch persists `state/safe-mode.json` (`durability_store.rs`, `core.rs`) |
| **F3** | Epoch replacement uses unique `O_EXCL` temps; legacy `.tmp-epoch` ambiguity refused; stale epoch below high-water refused (not clamped) |
| **F4** | Production topology verifier (`topology.rs`, `ProductionTopologyProbe`) |
| **F5** | `UnavailableProcessGroupFencer: ProcessGroupFence`; fence wait failure latches safe mode |
| **F6** | Arming refused while any transaction remains armed |
| **F7** | Moved base and expired arming deadline rechecked at decision time |
| **F8** | Accepted-stream `SO_PEERCRED` via `StreamPeerAuth`; socket read/write timeouts; capability binds peer pid/uid/gid |
| **F9** | Arm epoch must meet durable high-water authority, not session-only |
| **F10** | Decision log reader rejects decreasing epoch |
| **F11** | Append uses `O_NOFOLLOW` and post-write `Durability` fsync seam |
| **F14** | Additive manifest checks validated (not ignored) |
| **F15** | Late lease renewal refused; corrupt log at decision enters safe mode |

## Scope delivered

Hermetic L03 watchdog decision authority: durable append-only decision log and epoch high-water store; fail-closed safe-mode and external-floor handling; injected topology/durability/process-group/job/invariant interfaces; framed authenticated local RPC (`SessionBind` with `worker_group_tag` → `SessionEstablished` → five production request types); broker narrow client stub (wire DTOs only); Nix protected-resource rejection for `/var/lib/agentbed/broker/state`; `UnavailableProcessGroupFencer` (no syscall fencing); topology probe; `fencing_seam.rs` (8 tests); failure matrix (54 tests); review repair suite (19 tests).

Production paths:

- `watchdogd/src/{core,durability_store,error,fencing,interfaces,peercred,read_model,session,topology,worker_group_tag,lib}.rs`
- `watchdogd/src/rpc/{mod,protocol,server}.rs`
- `broker/src/watchdog/{mod,client}.rs`, `broker/src/lib.rs`, `broker/Cargo.toml`
- `adapters/nix/src/protected.rs`
- `watchdogd/Cargo.toml`, `Cargo.lock`

**Deleted:** `watchdogd/tests/fencing_fixture.rs` (dangerous real-signal fixture).

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L03-AC01** | `watchdogd/src/core.rs`, `watchdogd/src/topology.rs`, `watchdogd/src/interfaces.rs`; `adapters/nix/src/protected.rs`; `watchdogd/tests/l03_failure_matrix.rs` topology matrix; `adapters/nix/tests/l03_protected_broker_state.rs` (6 tests) |
| **L03-AC02** | `watchdogd/src/read_model.rs`, `watchdogd/src/core.rs`; `broker/src/watchdog/client.rs`; matrix AC02 + `broker/tests/l03_watchdog_client.rs`; review F1/F10/F11 |
| **L03-AC03** | `watchdogd/src/core.rs`, `watchdogd/src/durability_store.rs`, `watchdogd/src/read_model.rs`; epoch/safe-mode/fsync/rename/readback matrix; review F2/F3/F9 |
| **L03-AC04** | `watchdogd/src/rpc/{protocol,server}.rs`, `watchdogd/src/session.rs`, `watchdogd/src/peercred.rs`, `watchdogd/src/worker_group_tag.rs`; `broker/src/watchdog/client.rs`; frame codec, stream peercred, session bootstrap, counter/capability binding, socket permissions, unix round-trip; review F8 |
| **L03-AC05** | `watchdogd/src/core.rs` arming validation; matrix AC05; review F6/F7/F14 |
| **L03-AC06** | `watchdogd/src/core.rs` authority selection; broker WAL safe-mode tests; review F15 |
| **L03-AC07** | `watchdogd/src/session.rs`, `watchdogd/src/core.rs` lease/heartbeat with `worker_group_tag`; matrix AC07; review F15 late renewal |
| **L03-AC08** | `watchdogd/src/fencing.rs` (`UnavailableProcessGroupFencer`); `watchdogd/tests/fencing_seam.rs` (8/8); matrix AC08 + review F5; no spawned fixture; `watchdogd/src/**` contains no `libc::kill`/`waitpid`/`killpg`/`sigqueue` |
| **L03-AC09** | `watchdogd/tests/l03_failure_matrix.rs` (54 tests) + `watchdogd/tests/l03_review_repair.rs` (19 tests) + `watchdogd/tests/fencing_seam.rs` (8 tests) |
| **L03-AC10** | `watchdogd/src/interfaces.rs` + test fakes under `watchdogd/tests/common/`; PLAN §1 assumption 4 — no hostile-root boundary claimed |
| **L03-AC11** | This `RESULT.md`, fencing-safety RED at `5b5d207`, verification commands below (bare/unpiped, exit 0) |
| **L03-AC12** | No L04/L05/live install/OOB/credentials/router changes; hermetic tests only; no merge |

## Verification commands (bare, unpiped, final head)

```text
cargo test -p agentbed-watchdogd --test fencing_seam              PASS (exit 0, 8/8)
cargo test -p agentbed-watchdogd --test l03_failure_matrix      PASS (exit 0, 54/54)
cargo test -p agentbed-watchdogd --test l03_review_repair       PASS (exit 0, 19/19)
cargo fmt --all -- --check                                      PASS (exit 0)
cargo clippy --workspace --all-targets -- -D warnings             PASS (exit 0)
cargo build --workspace --all-targets                             PASS (exit 0)
cargo test --workspace                                          PASS (exit 0)
cargo test -p agentbed-broker --test l03_watchdog_client        PASS (exit 0, 9/9)
cargo test -p agentbed-adapter-nix --test l03_protected_broker_state PASS (exit 0, 6/6)
git diff --check 01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c..HEAD PASS (exit 0)
```

Source safety (read-only): `watchdogd/tests/fencing_fixture.rs` absent; no `libc::kill`/`waitpid`/`killpg`/`sigqueue` in `watchdogd/src/**`.

## Hard non-goals held (L03-AC12)

No L04 commit/recovery orchestration; no actual promotion/revert execution; no L05 OOB implementation; no live watchdog/systemd install/start; no live NixOS/profile/boot/network/firewall/Proxmox mutation; no deployment or activation; no credentials/keys/external epoch store; no Gate 2+; no router/reconciler/repository-setting changes; no merge.

## Residual gaps (explicit)

- **Real process termination** is not implemented in L03. Production L03 refuses recovery authority when fencing is unavailable; daemon-owned cgroup-v2 fencing is deferred to a later lane.
- No live dedicated-mount provisioning, systemd daemon, NixOS VM, power-loss, spare-node chaos, or OOB mirror evidence — deferred to later authorized lanes (L04–L08).
- Broker transaction engine does not yet wire full watchdog RPC orchestration (L04 scope).
- Gate 1 remains open; this lane delivers L03 hermetic proof only (PLAN §9).
