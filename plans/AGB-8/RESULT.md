# AGB-8 — L03 Watchdog decision authority and durable local protocol

**Issue:** AGB-8 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c` (`origin/main`, verified 2026-08-25)
**Branch:** `agent/agb-8/l03-watchdog-decision-authority`

## PLAN / RED / GREEN evidence

| Phase | SHA | Artifact |
|---|---|---|
| PLAN (sealed) | `57d17c2364784bdd6609ce6042c60b79bac6be13` | `plans/AGB-8/PLAN.md` |
| RED | `58ec7da309e435f456b800e14703d4e8536fb24a` | `plans/AGB-8/red-evidence.txt` + L03 RED tests/fixtures |
| GREEN (implementation) | `999a63f11850ed8077aa2e314be9ed1d15959c34` | production sources listed below |
| GREEN (clippy repair) | `8072b68c3b02b64415a6e8330c2ba58e6806b1c8` | workspace `-D warnings` compliance without editing sealed RED tests |

Accepted RED tests/fixtures/evidence remain byte-identical to `58ec7da` (`git diff 58ec7da -- <red-files>` empty at final head).

## Scope delivered

Hermetic L03 watchdog decision authority: durable append-only decision log and epoch high-water store; fail-closed safe-mode and external-floor handling; injected topology/durability/process-group/job/invariant interfaces; framed authenticated local RPC (`SessionBind` → `SessionEstablished` → five production request types); broker narrow client stub (wire DTOs only); Nix protected-resource rejection for `/var/lib/agentbed/broker/state`; production process-group fencer exercised by bounded spawned fixture.

Production paths (17 files, no RED test edits):

- `watchdogd/src/{core,error,fencing,interfaces,read_model,session,lib}.rs`
- `watchdogd/src/rpc/{mod,protocol,server}.rs`
- `broker/src/watchdog/{mod,client}.rs`, `broker/src/lib.rs`, `broker/Cargo.toml`
- `adapters/nix/src/protected.rs`
- `watchdogd/Cargo.toml`, `Cargo.lock`

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L03-AC01** | `watchdogd/src/core.rs` (topology startup refusal), `watchdogd/src/interfaces.rs`; `adapters/nix/src/protected.rs`; `watchdogd/tests/l03_failure_matrix.rs` topology matrix; `adapters/nix/tests/l03_protected_broker_state.rs` (6 tests) |
| **L03-AC02** | `watchdogd/src/read_model.rs`, `watchdogd/src/core.rs` (single-writer append); `broker/src/watchdog/client.rs` (no append/choice API); matrix AC02 tests + `broker/tests/l03_watchdog_client.rs` static/source checks |
| **L03-AC03** | `watchdogd/src/core.rs`, `watchdogd/src/read_model.rs`; epoch/safe-mode/fsync/rename/readback matrix; external-floor ambiguity/unavailability; epoch/log cross-check on reopen |
| **L03-AC04** | `watchdogd/src/rpc/{protocol,server}.rs`, `watchdogd/src/session.rs`; `broker/src/watchdog/client.rs`; frame codec, peercred, session bootstrap, counter/capability binding, socket permissions, unix round-trip tests |
| **L03-AC05** | `watchdogd/src/core.rs` arming validation; moved base, wrong epoch, duplicate arm, weakened invariants, expired deadline matrix tests |
| **L03-AC06** | `watchdogd/src/core.rs` authority selection; broker WAL safe-mode tests in `broker/tests/l03_watchdog_client.rs` |
| **L03-AC07** | `watchdogd/src/session.rs`, `watchdogd/src/core.rs` lease/heartbeat; clock regression and binding mismatch matrix tests |
| **L03-AC08** | `watchdogd/src/fencing.rs`; `watchdogd/tests/fencing_fixture.rs` (spawned SIGTERM→SIGKILL ordering) |
| **L03-AC09** | `watchdogd/tests/l03_failure_matrix.rs` (54-test hermetic matrix) |
| **L03-AC10** | `watchdogd/src/interfaces.rs` + test fakes under `watchdogd/tests/common/`; PLAN §1 assumption 4 — no hostile-root boundary claimed |
| **L03-AC11** | This `RESULT.md`, sealed RED at `58ec7da`, GREEN commits above, verification commands below (bare/unpiped, exit 0) |
| **L03-AC12** | No L04/L05/live install/OOB/credentials/router changes; hermetic tests only |

## Verification commands (bare, unpiped, final head)

```text
cargo fmt --all -- --check                              PASS (exit 0)
cargo clippy --workspace --all-targets -- -D warnings   PASS (exit 0)
cargo build --workspace --all-targets                   PASS (exit 0)
cargo test --workspace                                PASS (exit 0)
git diff --check 58ec7da309e435f456b800e14703d4e8536fb24a..HEAD PASS (exit 0)
cargo test -p agentbed-watchdogd --test l03_failure_matrix PASS (exit 0, 54/54)
cargo test -p agentbed-watchdogd --test fencing_fixture   PASS (exit 0, 1/1)
cargo test -p agentbed-broker --test l03_watchdog_client PASS (exit 0, 9/9)
cargo test -p agentbed-adapter-nix --test l03_protected_broker_state PASS (exit 0, 6/6)
```

## Hard non-goals held (L03-AC12)

No L04 commit/recovery orchestration; no actual promotion/revert execution; no L05 OOB implementation; no live watchdog/systemd install/start; no live NixOS/profile/boot/network/firewall/Proxmox mutation; no deployment or activation; no credentials/keys/external epoch store; no Gate 2+; no router/reconciler/repository-setting changes; no merge.

## Residual gaps (explicit)

- No live dedicated-mount provisioning, systemd daemon, NixOS VM, power-loss, spare-node chaos, or OOB mirror evidence — deferred to later authorized lanes (L04–L08).
- Broker transaction engine does not yet wire full watchdog RPC orchestration (L04 scope).
- `watchdogd` production layout consolidates PLAN module names into fewer files (`core`, `read_model`, `rpc`) for smallest GREEN; behavior is covered by RED matrix.
- Gate 1 remains open; this lane delivers L03 hermetic proof only (PLAN §9).
