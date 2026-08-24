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
| No `agentbed://events` MCP resource | Broker `events.replay` + gateway `resources/list` / `resources/read` |

## Repair round 2 (review #5010707523 @ `d105e21`)

| Finding | Fix |
|---|---|
| Cross-agent transaction re-attribution | `ensure_owner` on every transition; WAL persists original `agent_id`/`manifest_digest` |
| Moved-base refusal not durable | `tx.apply` records `Rejected` WAL + event before returning `BaseRevisionMoved` |
| Lexical WAL sort false safe mode | Numeric filename ordering in `WalStore::load_records` |
| Silent `events.replay` v2 extension | `docs/protocol.md` revision 2 documents operation + cursor format |
| Raw JSON event cursors | `EventCursor` encodes/decodes base64url per sealed plan |

Repair tests: `broker/tests/l01_repair_review.rs` (13), `gw/tests/events_resource.rs` (1). See `plans/AGB-4/red-evidence.txt` for RED→GREEN trace.

## Repair round 3 (review #5010933840 @ `db27b7a`)

| Finding | Fix |
|---|---|
| Transaction ID collision on restart | `ulid::Ulid` generation; ambiguous duplicate-`Proposed` WAL recovery enters safe mode |
| PID-derived event log identity | Persisted UUID `log_id`; strict nonempty UUID cursor validation |
| Moved-base apply not idempotently replayable | `replay_apply` + idempotency record on moved-base `Rejected` refusal |

Repair tests: `broker/tests/l01_repair_review.rs` (19), `gw/tests/events_resource.rs` (1).

## Repair round 4 (native review #5011209070 @ `89b848e`)

| Finding | Fix |
| --- | --- |
| Recovery accepts watchdog-owned / impossible WAL chains and immutable-field drift | `broker/src/transaction/recovery.rs` validates `record_version`, watchdog states, `broker_may_enter`, and identity/base-revision immutability before rebuilding `txs` |
| Unsupported explicit `record_version` ignored | `WalRecord.record_version` (default 1); reject `!= 1` during recovery |
| Idempotency index failure after WAL visibility allows duplicate retry | Reorder `config.propose` / `transition` to WAL → idempotency → event; WAL replay on same-key retry; complete partial bindings on replay |
| Idempotency write/rename fault injection | Filesystem chmod / rename-blocker tests in `l01_repair_review.rs` |

Repair tests: `broker/tests/l01_repair_review.rs` (32).

## Repair round 5 (native review #5011747127 @ `236f75e` RED checkpoint)

| Finding | Fix |
| --- | --- |
| Immutable WAL payload drift (`effect_set`, `diff`, `affected_resources`, `approval_ref`) | `broker/src/transaction/recovery.rs` rejects cross-record drift before rebuilding `txs`; invalid chains enter safe mode |
| Conflicting `config.propose` after idempotency-index write/rename fault (immediate) | `broker/src/transaction/engine.rs` WAL idempotency lookup returns `IdempotencyConflict` on fingerprint mismatch even when the on-disk idempotency file is missing |
| Moved-base `tx.apply` refusal after idempotency fault: replay must return `BaseRevisionMoved` without duplicate WAL/event | `broker/src/transaction/engine.rs` soft-reverts appended WAL on idempotency failure, completes partial refusals via in-place WAL rewrite + idempotency repair; `broker/src/storage/wal.rs` `rewrite_transition` |
| `tx.apply` moved-base idempotency rebuild on restart | `broker/src/storage/idempotency.rs` indexes `Rejected` apply refusals from WAL `result_json` during `merge_from_wal` |

RED matrix @ `236f75e` (unpiped, `--test-threads=1`): **10 FAIL / 34 PASS** (cells 1–4 payload drift, 5/7 conflicting propose immediate, 9–12 moved-base idempotency ordering).

GREEN @ 41728986; lint-hardened @ 6e2b517:

```text
cargo test -p agentbed-broker --test l01_repair_review -- --test-threads=1   # 44 passed
cargo fmt --all -- --check                                                   # PASS
cargo clippy --workspace --all-targets -- -D warnings                        # PASS (sole post-GREEN change: `l01_repair_review.rs:1117` indexing → checked `.first().expect("load")`; non-semantic)
cargo build --workspace --all-targets                                        # PASS
cargo test --workspace                                                       # PASS
```

Production files changed @ 41728986: `broker/src/transaction/{recovery,engine}.rs`, `broker/src/storage/{wal,idempotency}.rs`. Post-GREEN test-only lint hardening: `broker/tests/l01_repair_review.rs` (`7da8f021b1e59067773c5d6ef75ed9213487474453e1a9e25a16b4da6fe30ff0` → `b201c12801b3887255c0de384c8dbbbd8e3ad5c5a7b037133103f1d2a3eaa853`).

Test hash (`broker/tests/l01_repair_review.rs`): `b201c12801b3887255c0de384c8dbbbd8e3ad5c5a7b037133103f1d2a3eaa853`.
