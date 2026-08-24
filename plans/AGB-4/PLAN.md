# AGB-4 — L01 Durable transaction core and read surface

**Issue:** AGB-4 · parent AGB-1 · GitHub #12  
**Workflow:** `workflow:guarded`  
**Baseline:** `8a1956d274140cfd8800eb0d1bdd47faef57f2a7` (`origin/main`, verified 2026-08-24)  
**Roadmap gate / exit:** Gate 1 L01 (`plans/AGB-1/PLAN.md` lane L01). Gate 1 remains open after this lane.

## 1. Evidence and scope

Inspected: `plans/AGB-1/PLAN.md` L01 row; ADR-001 rev. 6; `docs/threat-model.md`; `docs/effects.md` §§1–4; `docs/protocol.md` §7; `plans/AGB-2/RESULT.md` (L00 complete); broker dispatch/tools stubs; `proto/src/dto/transaction.rs`; existing broker tests.

L00 froze protocol v2 and wire schemas. Mutating v2 operations currently validate, digest, and policy-check, then return `internal`. This lane adds the serialized D/M transaction engine, broker WAL, recovery/safe mode, idempotency and base-revision checks, `tx.status` wired to durable state, and the first-release `agentbed://events` durable append log with cursor replay.

### Consequential assumptions

1. **No watchdog in L01.** Watchdog decision records (`ARMED`, `PROBATION_PASSED`, `BEGIN_COMMIT`, `BEGIN_REVERT`, `COMMITTED`, `REVERTED`) are not written by the broker. Broker-owned WAL progression stops at `PROBATION`; transitions into watchdog-owned terminal states are refused at the engine boundary until L03.
2. **No real host effects.** `config.propose` stages a durable transaction record and synthetic diff/test plan from the proposed change set; no Nix adapter execution, no `nixos-rebuild`, no profile/boot mutation.
3. **Base revision from adapter.** `HostAdapter` gains `current_base_revision()`; `UnresolvedAdapter` returns a deterministic stub for hermetic tests.
4. **State directory.** `BrokerConfig::state_dir` (default under `/var/lib/agentbed` in production; temp dir in tests) holds WAL, checkpoint, idempotency index, and event log — separate paths, no shared rollback domain with a future watchdog store.
5. **Event cursor format (L01 first release).** Opaque client cursor: base64url(JSON `{"log_id":"<uuid>","seq":<u64>}`). `log_id` is assigned at broker first start and persisted; `seq` is the monotonic event sequence. Replay returns events with `seq > cursor.seq` strictly, never skipping or duplicating.

### Hard non-goals (L01-AC10)

No Nix adapter execution; no real apply/rollback effect; no watchdog decision log/leases/fencing/OOB; no Proxmox/VM mutation; no deployment or activation; no Gate 2 identity/approval/ledger/external effects; no Gate 3 enforcement; no Gate 4 Intent-to-App; no router/reconciler or branch-protection changes; no credentials.

## 2. Architecture

### State ownership

```
Broker WAL (serialized, one lock):
  PROPOSED → TESTING → APPLYING → PROBATION
       │         │          │
       ▼         ▼          ▼
    REJECTED  REJECTED   (refused: watchdog-owned beyond PROBATION)

Watchdog (L03+, not implemented here):
  ARMED → PROBATION_PASSED → BEGIN_COMMIT → COMMITTED
       └──────── BEGIN_REVERT → REVERTED
```

The broker persists a complete transaction record **before** each visible state entry. It never appends watchdog decision records or chooses terminal commit/revert.

### Persistence layout (`{state_dir}/`)

| Path | Purpose |
|---|---|
| `wal/records/` | Per-transition durable records (`{seq}.json.tmp` → rename → fsync) |
| `wal/checkpoint.json` | Latest recovered transaction index |
| `idempotency/` | `(agent_id, op, key)` → original serialized result |
| `events/log.jsonl` | Append-only event log |
| `events/meta.json` | `log_id`, `next_seq` |
| `broker_mode.json` | `normal` \| `safe_mode` + reason code |

Durability contract: write temp → fsync file → atomic rename → fsync parent directory. Injectable `DurabilityOps` for failure-matrix tests.

### Safe mode

Corrupt, truncated, unknown-version, ambiguous, or structurally inconsistent WAL/checkpoint/event state → `safe_mode`. Refuse all new D/M work; allow read-only `tx.status` (fail-closed for unknown ids) and event replay where the log tail is intact. No automatic truncation or salvage.

## 3. Acceptance traceability

| AC | Intended paths | Verification |
|---|---|---|
| **L01-AC01** | `broker/src/transaction/{state,engine}.rs` | Transition table unit tests: every broker-allowed/refused edge; assert no public API writes watchdog decision states |
| **L01-AC02** | `broker/src/storage/{durability,wal}.rs` | Persist-before-transition tests; temp/rename/fsync injection; state never visible before durable record |
| **L01-AC03** | `broker/src/storage/wal.rs`, `broker/src/transaction/engine.rs` | Failure at every WAL boundary + restart recovery; corrupt/truncated → safe mode |
| **L01-AC04** | `broker/src/transaction/engine.rs`, `broker/src/adapter.rs` | Idempotent replay returns original result; conflicting reuse refused; moved generation/config_digest/etc_git_commit refuses apply |
| **L01-AC05** | `broker/src/dispatch.rs`, `broker/src/tools/transaction.rs` | `tx.status` reads durable state; unknown tx and safe mode fail-closed without sensitive prose |
| **L01-AC06** | `broker/src/storage/events.rs`, `broker/src/events.rs` | Cursor format; restart survival; strict replay no-loss/no-duplication; reject malformed/stale/beyond-tail cursors |
| **L01-AC07** | `broker/tests/performance_reads.rs` | Deterministic fixture (≥100 txs, ≥500 events); assert all R queries < 1s locally |
| **L01-AC08** | `broker/tests/{transaction_state,wal_durability,transaction_engine,events_log,tx_status_rpc}.rs` | Full matrix + RED→GREEN evidence in `plans/AGB-4/red-evidence.txt` |
| **L01-AC09** | `plans/AGB-4/RESULT.md` | fmt, clippy, build, test — all four commands with actual output |
| **L01-AC10** | PLAN non-goals §1 | Review + tests prove no adapter activation, watchdog writes, or deployment hooks |

## 4. Failure injection matrix

| Boundary | Inject | Expected after restart |
|---|---|---|
| WAL record temp write | I/O error before rename | Prior state unchanged; transition refused |
| WAL record post-write pre-fsync | crash (drop fsync) | Recovery rejects incomplete temp; safe mode if ambiguous |
| WAL rename pre-parent-fsync | crash | Prior checkpoint authoritative |
| Checkpoint replace | corrupt JSON / truncate | safe_mode |
| Event append mid-line | truncate log | safe_mode or refuse tail reads |
| Idempotency index corrupt | corrupt file | safe_mode |
| Recovery mid-scan | partial record dir | safe_mode, no invented progress |

## 5. Verification commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```

Focused during TDD:

```bash
cargo test -p agentbed-broker --test transaction_state
cargo test -p agentbed-broker --test wal_durability
cargo test -p agentbed-broker --test transaction_engine
cargo test -p agentbed-broker --test events_log
cargo test -p agentbed-broker --test tx_status_rpc
cargo test -p agentbed-broker --test performance_reads
```

## 6. Rollback and stop conditions

Stop if any code path lets the broker derive or write watchdog terminal decisions. Stop if corrupt WAL is silently repaired. Revert is safe: no adapter activation or OOB integration in this lane.

## 7. Delivery

One branch `agent/agb-4/l01-durable-transaction-core`, one PR titled `AGB-4: Durable transaction core and read surface`, DCO sign-off on each commit. TDD order: PLAN → tests (RED) → implementation (GREEN) → RESULT.
