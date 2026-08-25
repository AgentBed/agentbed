# AGB-6 — L02 Nix proposal, test activation, and boot-promotion primitives

**Issue:** AGB-6 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `5c7ec772a48ce82208bc11173283d2283bf18e6d`
**Branch:** `agent/agb-6/l02-nix-proposal-primitives`

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L02-AC02** | `adapters/nix/protected.rs`; `adapters/nix/tests/l02_review_repair_3.rs`, `adapters/nix/tests/l02_review_repair_4.rs` (fully quoted attrpaths, decoys). |
| **L02-AC05** | `adapters/nix/capture.rs`, `adapters/nix/promotion/test_activation.rs`; `adapters/nix/tests/l02_review_repair_4.rs` (root parent fsync before activation). |
| **L02-AC09** | `plans/AGB-6/review-4-red-evidence.txt`, verification commands below — all PASS on review-4 GREEN head. |

(Full L02-AC01…L02-AC10 traceability retained from prior review cycles in branch history.)

## RED→GREEN evidence (L02-AC09)

- Review-4 baseline: `9f882a40e80aeeb8ec6b92969aae03032284642e`
- Review-4 RED: `9809fda9d2155db49974afa687c1fa8738ec539e` — `plans/AGB-6/review-4-red-evidence.txt`
- Review-4 GREEN: see PR head

## Review repair #5019424546 — addressed findings

| Severity | Finding | Repair |
|---|---|---|
| CRITICAL | Fully quoted attrpath Class-F bypass | `protected.rs` canonicalizes leading and chained quoted attrpath components; skips value strings after `=` |
| IMPORTANT | Activations parent durability | `capture.rs` fsyncs `root` when `activations/` is newly created before reservation proceeds |

## Verification commands (bare, unpiped)

```text
cargo fmt --all -- --check                              PASS (exit 0)
cargo clippy --workspace --all-targets -- -D warnings   PASS (exit 0)
cargo build --workspace --all-targets                   PASS (exit 0)
cargo test --workspace                                PASS (exit 0)
cargo test -p agentbed-adapter-nix --test l02_review_repair_4 PASS (exit 0, 8 tests)
git diff --check 5c7ec772a48ce82208bc11173283d2283bf18e6d..HEAD PASS (exit 0)
```

## Residual gaps (explicit)

- No real NixOS VM, power-loss, watchdog/OOB recovery, or live host mutation evidence — deferred to later authorized lanes (L02-AC10).
- `tx.rollback` and L03+ commit/recovery orchestration remain out of scope.
- Gate 1 remains open; this lane delivers L02 primitives only.
