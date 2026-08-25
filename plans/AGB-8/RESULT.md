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
| RED | `58ec7da309e435f456b800e14703d4e8536fb24a` | `plans/AGB-8/red-evidence.txt` + L03 RED tests/fixtures |
| GREEN (implementation) | `999a63f11850ed8077aa2e314be9ed1d15959c34` | initial production sources |
| GREEN (clippy repair) | `8072b68c3b02b64415a6e8330c2ba58e6806b1c8` | workspace `-D warnings` compliance |
| GREEN (initial RESULT) | `522c6bcc67a8406e89f7f18cab06367273206314` | initial `RESULT.md` |
| Review RED (scenario verification) | `94301f8b0aeefb0560df87caecba8271ec81ac16` | `plans/AGB-8/review-red-evidence.txt` + `watchdogd/tests/l03_review_repair.rs` |
| Review GREEN (production repair) | `b79ba1bec7d0b2b257a211f6470825b47d21a347` | production fixes for scenario findings F1–F11, F14, F15 |
| GREEN (RESULT, review repair Stage B) | `b46237e`, `5a7c473`, branch HEAD | Stage B `RESULT.md` gate evidence |

Accepted sealed RED tests/fixtures/evidence remain byte-identical to `58ec7da` (`git diff 58ec7da -- <red-files>` empty at final head). Review RED assertions/evidence remain byte-identical to `94301f8`; the only shared-fixture delta is minimal `StreamPeerAuth` seam wiring in `watchdogd/tests/common/{fakes,deps}.rs`.

## Scenario verification review repair — addressed findings

| Finding | Repair |
|---|---|
| **F1** | Decision log append fails closed without `set_len` truncation (`read_model.rs`) |
| **F2** | Runtime safe-mode latch persists `state/safe-mode.json` (`durability_store.rs`, `core.rs`) |
| **F3** | Epoch replacement uses unique `O_EXCL` temps; legacy `.tmp-epoch` ambiguity refused; stale epoch below high-water refused (not clamped) |
| **F4** | Production topology verifier (`topology.rs`, `ProductionTopologyProbe`) |
| **F5** | `ProductionProcessGroupFencer: ProcessGroupFence`; fence wait failure latches safe mode |
| **F6** | Arming refused while any transaction remains armed |
| **F7** | Moved base and expired arming deadline rechecked at decision time |
| **F8** | Accepted-stream `SO_PEERCRED` via `StreamPeerAuth`; socket read/write timeouts; capability binds peer pid/uid/gid |
| **F9** | Arm epoch must meet durable high-water authority, not session-only |
| **F10** | Decision log reader rejects decreasing epoch |
| **F11** | Append uses `O_NOFOLLOW` and post-write `Durability` fsync seam |
| **F14** | Additive manifest checks validated (not ignored) |
| **F15** | Late lease renewal refused; corrupt log at decision enters safe mode |

## Scope delivered

Hermetic L03 watchdog decision authority: durable append-only decision log and epoch high-water store; fail-closed safe-mode and external-floor handling; injected topology/durability/process-group/job/invariant interfaces; framed authenticated local RPC (`SessionBind` → `SessionEstablished` → five production request types); broker narrow client stub (wire DTOs only); Nix protected-resource rejection for `/var/lib/agentbed/broker/state`; production process-group fencer and topology probe; scenario-verification review repair suite (`l03_review_repair.rs`, 19 tests).

Production paths (22 files; sealed RED tests unchanged):

- `watchdogd/src/{core,durability_store,error,fencing,interfaces,peercred,read_model,session,topology,lib}.rs`
- `watchdogd/src/rpc/{mod,protocol,server}.rs`
- `broker/src/watchdog/{mod,client}.rs`, `broker/src/lib.rs`, `broker/Cargo.toml`
- `adapters/nix/src/protected.rs`
- `watchdogd/Cargo.toml`, `Cargo.lock`

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L03-AC01** | `watchdogd/src/core.rs`, `watchdogd/src/topology.rs`, `watchdogd/src/interfaces.rs`; `adapters/nix/src/protected.rs`; `watchdogd/tests/l03_failure_matrix.rs` topology matrix; `adapters/nix/tests/l03_protected_broker_state.rs` (6 tests) |
| **L03-AC02** | `watchdogd/src/read_model.rs`, `watchdogd/src/core.rs`; `broker/src/watchdog/client.rs`; matrix AC02 + `broker/tests/l03_watchdog_client.rs`; review F1/F10/F11 |
| **L03-AC03** | `watchdogd/src/core.rs`, `watchdogd/src/durability_store.rs`, `watchdogd/src/read_model.rs`; epoch/safe-mode/fsync/rename/readback matrix; review F2/F3/F9 |
| **L03-AC04** | `watchdogd/src/rpc/{protocol,server}.rs`, `watchdogd/src/session.rs`, `watchdogd/src/peercred.rs`; `broker/src/watchdog/client.rs`; frame codec, stream peercred, session bootstrap, counter/capability binding, socket permissions, unix round-trip; review F8 |
| **L03-AC05** | `watchdogd/src/core.rs` arming validation; matrix AC05; review F6/F7/F14 |
| **L03-AC06** | `watchdogd/src/core.rs` authority selection; broker WAL safe-mode tests; review F15 |
| **L03-AC07** | `watchdogd/src/session.rs`, `watchdogd/src/core.rs` lease/heartbeat; matrix AC07; review F15 late renewal |
| **L03-AC08** | `watchdogd/src/fencing.rs`; `watchdogd/tests/fencing_fixture.rs`; review F5 |
| **L03-AC09** | `watchdogd/tests/l03_failure_matrix.rs` (54 tests) + `watchdogd/tests/l03_review_repair.rs` (19 tests) |
| **L03-AC10** | `watchdogd/src/interfaces.rs` + test fakes under `watchdogd/tests/common/`; PLAN §1 assumption 4 — no hostile-root boundary claimed |
| **L03-AC11** | This `RESULT.md`, sealed RED at `58ec7da`, review RED at `94301f8`, review GREEN at `b79ba1b`, verification commands below (bare/unpiped, exit 0) |
| **L03-AC12** | No L04/L05/live install/OOB/credentials/router changes; hermetic tests only |

## Verification commands (bare, unpiped, final head)

```text
cargo fmt --all -- --check                              PASS (exit 0)
cargo clippy --workspace --all-targets -- -D warnings   PASS (exit 0)
cargo build --workspace --all-targets                   PASS (exit 0)
cargo test --workspace                                PASS (exit 0)
cargo test -p agentbed-watchdogd --test l03_review_repair PASS (exit 0, 19/19)
cargo test -p agentbed-watchdogd --test l03_failure_matrix PASS (exit 0, 54/54)
cargo test -p agentbed-watchdogd --test fencing_fixture   PASS (exit 0, 1/1)
cargo test -p agentbed-broker --test l03_watchdog_client PASS (exit 0, 9/9)
cargo test -p agentbed-adapter-nix --test l03_protected_broker_state PASS (exit 0, 6/6)
git diff --check 01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c..HEAD PASS (exit 0)
```

## Hard non-goals held (L03-AC12)

No L04 commit/recovery orchestration; no actual promotion/revert execution; no L05 OOB implementation; no live watchdog/systemd install/start; no live NixOS/profile/boot/network/firewall/Proxmox mutation; no deployment or activation; no credentials/keys/external epoch store; no Gate 2+; no router/reconciler/repository-setting changes; no merge.

## Residual gaps (explicit)

- No live dedicated-mount provisioning, systemd daemon, NixOS VM, power-loss, spare-node chaos, or OOB mirror evidence — deferred to later authorized lanes (L04–L08).
- Broker transaction engine does not yet wire full watchdog RPC orchestration (L04 scope).
- `watchdogd` production layout consolidates PLAN module names into fewer files (`core`, `read_model`, `rpc`) for smallest GREEN; behavior is covered by RED matrix and review repair suite.
- Gate 1 remains open; this lane delivers L03 hermetic proof only (PLAN §9).
