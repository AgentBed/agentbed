# AGB-2 — Broker RPC v2 contract

**Issue:** AGB-2 · parent AGB-1 · GitHub #12  
**Workflow:** `workflow:guarded`  
**Baseline:** `0b3caf6fda511e4e2e579ec1ab5b38d5f706a53f`
**Merged:** PR #17 on `main` — Gate 1 L00 complete; Gate 1 remains open for L01+.

## Acceptance traceability

| AC | Evidence |
|---|---|
| **V2-AC01 — v1 frozen** | `docs/protocol.md` §§1–6 unchanged in semantics; v1 `system.info` digest vector `sha256:b407fa81…` pinned in `broker/tests/jcs_conformance.rs::the_operation_digest_matches_its_frozen_vector`; all Gate 0 broker/gw tests green. |
| **V2-AC02 — explicit v2 boundary** | `proto/src/lib.rs` (`PROTOCOL_VERSION_V1/V2`); `Request::protocol_supported` / `operation_allowed`; `Response` echoes request `v`; unknown `v` → `invalid_request` (`broker/tests/rpc_v2.rs::unknown_protocol_version_is_refused_without_negotiation`). |
| **V2-AC03 — Gate 1 operation contracts** | `docs/protocol.md` §7.3; JSON Schemas under `schemas/tool/`; examples in `schemas/examples/tool.*`; typed params/results in `proto/src/wire.rs` and `proto/src/dto/transaction.rs`; broker dispatch validates and digests each op. Mutating ops return `internal` at L00. |
| **V2-AC04 — cross-version digest separation** | `broker/src/digest.rs` (`agentbed.operation.v2\0`); `broker/tests/jcs_conformance.rs::v1_and_v2_domains_never_share_a_digest`; consumed fixture `tests/fixtures/rpc-v2/digest-system-info-v2.txt` via `the_v2_digest_fixture_is_consumed_byte_for_byte`. |
| **V2-AC05 — strict decoding retained** | `proto/src/strict.rs` unchanged; v2 negative vectors in `broker/tests/rpc_v2.rs`; wire fixture `tests/fixtures/rpc-v2/request-system-info-v2.json` parsed in `the_v2_wire_fixture_parses_and_is_supported`. |
| **V2-AC06 — migration policy** | `docs/protocol.md` §7.4. |
| **V2-AC07 — tests prove boundary** | RED→GREEN evidence in `plans/AGB-2/red-evidence.txt` (baseline compile failure with tests-only); GREEN: `broker/tests/rpc_v2.rs` (10 tests), `broker/tests/jcs_conformance.rs`, `schemas/tests/examples_validate.rs::v2_*`. |
| **V2-AC08 — bounded artifacts** | Normative `docs/protocol.md` §7; minimal `proto/`, `schemas/`, `broker/` stubs, fixtures, this RESULT. |

## RED→GREEN evidence (V2-AC07)

See `plans/AGB-2/red-evidence.txt` for executed commands and compiler output.

**RED** (baseline `0b3caf6` + `broker/tests/rpc_v2.rs` only): `cargo test -p agentbed-broker --test rpc_v2` → exit 101, `E0432` (`PROTOCOL_VERSION_V2` missing) and `E0061` (`OperationDigest::of` arity).

**GREEN** (PR head): `cargo test -p agentbed-broker --test rpc_v2` → 10 passed; full workspace suite PASS.

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
- `gw/src/session.rs`
- `schemas/tool/*.schema.json`, `schemas/examples/tool.*`, `schemas/src/lib.rs`, `schemas/tests/examples_validate.rs`
- `tests/fixtures/rpc-v2/{digest-system-info-v2.txt,request-system-info-v2.json}`
- `plans/AGB-2/{RESULT.md,red-evidence.txt}`

## Residual gaps (explicit)

- Transaction engine, Nix adapter activation, watchdog/OOB, gateway v2 tool exposure — deferred to AGB-1 lanes L01+.
- Mutating v2 operations validate + digest + policy, then return `internal` until L01 implements execution.
