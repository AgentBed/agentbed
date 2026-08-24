# AGB-4 — L01 Durable transaction core and read surface

**Issue:** AGB-4 · parent AGB-1 · GitHub #12  
**Workflow:** `workflow:guarded`  
**Baseline:** `8a1956d274140cfd8800eb0d1bdd47faef57f2a7`  
**Branch:** `agent/agb-4/l01-durable-transaction-core`

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L01-AC01** | `broker/src/transaction/{state,engine}.rs`; `broker/tests/transaction_state.rs` (broker-owned transition table; watchdog states refused at engine boundary). |
| **L01-AC02** | `broker/src/storage/{durability,wal}.rs`; `broker/tests/wal_durability.rs` (persist-before-transition; temp/rename/fsync injection). |
| **L01-AC03** | `broker/src/storage/wal.rs`, `broker/src/transaction/engine.rs`; `broker/tests/wal_durability.rs`, `broker/tests/transaction_engine.rs` (recovery at every WAL boundary; corrupt checkpoint → safe mode). |
| **L01-AC04** | `broker/src/transaction/engine.rs`, `broker/src/adapter.rs`; `broker/tests/transaction_engine.rs` (idempotent replay, conflict refusal, moved base revision refusal). |
| **L01-AC05** | `broker/src/dispatch.rs`; `broker/tests/tx_status_rpc.rs`, `broker/tests/rpc_v2.rs` (unknown tx → `denied`; durable state after propose). |
| **L01-AC06** | `broker/src/events.rs`; `broker/tests/events_log.rs` (append log, JSON cursor, strict replay). |
| **L01-AC07** | `broker/tests/performance_reads.rs` (100 txs + 500 events fixture; R-class reads + replay < 1s). |
| **L01-AC08** | `plans/AGB-4/red-evidence.txt` (RED compile failure → GREEN six L01 binaries + workspace PASS). |
| **L01-AC09** | Verification commands below — all four PASS on PR head. |
| **L01-AC10** | PLAN non-goals; `tx.rollback` remains `internal`; no Nix adapter execution, watchdog writes, or deployment hooks. |

## RED→GREEN evidence (L01-AC08)

See `plans/AGB-4/red-evidence.txt`.

## Verification commands

```text
cargo fmt --all -- --check          PASS
cargo clippy --workspace --all-targets -- -D warnings   PASS
cargo build --workspace --all-targets                   PASS
cargo test --workspace                                PASS
```

## Changed paths (summary)

- `broker/src/{events,storage,transaction}/` — WAL, durability ops, transaction engine, event log
- `broker/src/{adapter,config,dispatch,lib}.rs` — base revision, state_dir, v2 execution wiring
- `broker/tests/{transaction_state,wal_durability,transaction_engine,events_log,tx_status_rpc,performance_reads,rpc_v2}.rs`
- `broker/tests/manifests/agent.proposer.yaml` — D-class fixture for RPC propose tests
- `broker/tests/support/mod.rs`, `broker/src/main.rs`, `gw/tests/end_to_end.rs` — state_dir plumbing
- `plans/AGB-4/{PLAN,RESULT,red-evidence}.md`

## Residual gaps (explicit)

- `tx.rollback` still returns `internal` (deferred).
- Watchdog decision states and real Nix effects deferred to L03+ / later lanes.
- Gate 1 remains open (L01 is one lane, not full gate closure).

## Repair (review #5010391942 @ `daaaae0`)

| Finding | Fix |
|---|---|
| Idempotency in-memory only | Durable `IdempotencyStore` under `{state_dir}/idempotency/` + WAL `idem_fingerprint` rebuild |
| `tx.apply` ignores idempotency key | Engine honors key on apply; conflicting reuse fails closed after restart |
| WAL/event corruption not entering safe mode | `WalRecovery`, orphan-tmp detection, checkpoint consistency, `EventLog::validate_integrity` |
| D/M not serialized | Single `dm_lock` for config propose / tx apply paths |
| Event append errors discarded | Fail-closed: rollback last WAL transition + safe mode on append failure |
| No `agentbed://events` MCP resource | Broker `events.replay` (R-class) + gateway `resources/list` / `resources/read` |

Repair tests: `broker/tests/l01_repair_review.rs` (9), `gw/tests/events_resource.rs` (1). See `plans/AGB-4/red-evidence.txt` for RED→GREEN trace.
