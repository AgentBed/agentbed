# AGB-2 — Broker RPC v2 contract

**Issue:** AGB-2 · parent AGB-1 · GitHub #12  
**Workflow:** `workflow:guarded`  
**Baseline:** `0b3caf6fda511e4e2e579ec1ab5b38d5f706a53f`

## Acceptance traceability

| AC | Evidence |
|---|---|
| **V2-AC01 — v1 frozen** | `docs/protocol.md` §§1–6 unchanged in semantics; v1 `system.info` digest vector `sha256:b407fa81…` still pinned in `broker/tests/jcs_conformance.rs`; all Gate 0 broker/gw tests green (`cargo test --workspace`). |
| **V2-AC02 — explicit v2 boundary** | `proto/src/lib.rs` (`PROTOCOL_VERSION_V1/V2`); `Request::protocol_supported` / `operation_allowed`; `Response` echoes request `v`; unknown `v` → `invalid_request` (`broker/tests/rpc_v2.rs::unknown_protocol_version_is_refused_without_negotiation`). |
| **V2-AC03 — Gate 1 operation contracts** | `docs/protocol.md` §7.3; JSON Schemas under `schemas/tool/`; typed params/results in `proto/src/wire.rs` and `proto/src/dto/transaction.rs`; broker dispatch validates and digests each op (`broker/src/dispatch.rs`). Execution of mutating ops returns `internal` at L00 (contract-only). |
| **V2-AC04 — cross-version digest separation** | `broker/src/digest.rs` (`agentbed.operation.v2\0`); `broker/tests/jcs_conformance.rs::v1_and_v2_domains_never_share_a_digest`; golden vector `tests/fixtures/rpc-v2/digest-system-info-v2.txt`. |
| **V2-AC05 — strict decoding retained** | Existing `proto/src/strict.rs` unchanged; v2 negative vectors in `broker/tests/rpc_v2.rs` (duplicate keys, unknown fields); fuzz smoke updated for v2-invalid case. |
| **V2-AC06 — migration policy** | `docs/protocol.md` §7.4 (dual-version coexistence, independent dispatch, additive vs breaking rules). |
| **V2-AC07 — tests prove boundary** | New `broker/tests/rpc_v2.rs` (round trip, refusals, digest domain); updated `jcs_conformance`, `rpc_fuzz_smoke`, `system_info` pattern matches. RED→GREEN: v2 tests fail on baseline without implementation (verified by design — new files/tests added with v2 code). |
| **V2-AC08 — bounded artifacts** | Normative `docs/protocol.md` §7; minimal `proto/`, `schemas/`, `broker/` dispatch+digest+tools stubs, fixture; this RESULT. |

## Verification commands

```text
cargo fmt --all -- --check          PASS
cargo clippy --workspace --all-targets -- -D warnings   PASS
cargo build --workspace --all-targets                   PASS
cargo test --workspace                                PASS
```

## Changed paths (summary)

- `docs/protocol.md` — §7 v2 contract + migration
- `proto/src/{lib.rs,wire.rs,dto/transaction.rs,dto/mod.rs}`
- `broker/src/{digest.rs,dispatch.rs,tools/}`
- `broker/tests/{rpc_v2.rs,jcs_conformance.rs,rpc_fuzz_smoke.rs,system_info.rs}`
- `gw/src/session.rs` — reject unknown result variants
- `schemas/tool/*.schema.json`, `schemas/src/lib.rs`
- `tests/fixtures/rpc-v2/digest-system-info-v2.txt`
- `plans/AGB-2/RESULT.md`

## Residual gaps (explicit)

- Transaction engine, Nix adapter activation, watchdog/OOB, gateway v2 tool exposure — deferred to AGB-1 lanes L01+.
- Mutating v2 operations validate + digest + policy, then return `internal` until L01 implements execution.
