# AGB-6 — L02 Nix proposal, test activation, and boot-promotion primitives

**Issue:** AGB-6 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `5c7ec772a48ce82208bc11173283d2283bf18e6d`
**Branch:** `agent/agb-6/l02-nix-proposal-primitives`

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L02-AC01** | `adapters/nix/probe.rs`, `adapters/nix/adapter.rs`, `broker/src/nix_host_adapter.rs`; `adapters/nix/tests/l02_adapter.rs` (`probe_reports_generation_only_when_verified`, `probe_refuses_when_generation_missing`, `nix_adapter_host_surface_matches_probe`). |
| **L02-AC02** | `adapters/nix/protected.rs`, `broker/src/nix_host_adapter.rs`; `adapters/nix/tests/l02_adapter.rs` (`protected_path_matrix_rejects_class_f_before_staging`), `adapters/nix/tests/l02_review_repair.rs`, `adapters/nix/tests/l02_review_repair_2.rs` (structural nested kernel/bootloader), `broker/tests/l02_review_repair.rs`, `broker/tests/l02_review_repair_2.rs`. |
| **L02-AC03** | `adapters/nix/{capture,propose}.rs`, `broker/src/nix_host_adapter.rs`, `broker/src/transaction/engine.rs`; `adapters/nix/tests/l02_adapter.rs`, `broker/tests/l02_nix_adapter.rs`, `broker/tests/l02_review_repair.rs` (internal WAL closure), `broker/tests/l02_review_repair_2.rs` (schema + WAL). |
| **L02-AC04** | `adapters/nix/command_runner.rs`; `adapters/nix/tests/l02_adapter.rs` (`fake_runner_never_invokes_live_nixos_rebuild`), `adapters/nix/tests/l02_review_repair_2.rs` (env/stdin/timeout policies). |
| **L02-AC05** | `adapters/nix/promotion/{build,test_activation}.rs`, `adapters/nix/capture.rs`; `adapters/nix/tests/l02_adapter.rs`, `adapters/nix/tests/l02_review_repair.rs`, `adapters/nix/tests/l02_review_repair_2.rs` (durable exactly-once activation). |
| **L02-AC06** | `adapters/nix/promotion/{pin,profile}.rs`; `adapters/nix/tests/l02_adapter.rs` (`promotion_pin_profile_boot_flush_readback_happy_path`), `adapters/nix/tests/l02_review_repair.rs` (`pin_closure_must_match_captured_candidate`). |
| **L02-AC07** | `adapters/nix/promotion/{boot,flush,readback}.rs`; `adapters/nix/tests/l02_adapter.rs`, `adapters/nix/tests/l02_review_repair.rs`, `adapters/nix/tests/l02_review_repair_2.rs` (explicit profile/boot flush boundaries). |
| **L02-AC08** | `adapters/nix/promotion/`, `broker/tests/l02_nix_adapter.rs`; `adapters/nix/tests/l02_adapter.rs` (`promotion_failures_are_explicit_at_each_boundary`, `promotion_module_has_no_forbidden_switch_commands`). |
| **L02-AC09** | `plans/AGB-6/{PLAN,red-evidence,review-red-evidence,review-2-red-evidence,RESULT}.md`; verification commands below — all PASS on repair-2 GREEN head. |
| **L02-AC10** | PLAN non-goals; hermetic `FakeCommandRunner` only; no live `nixos-rebuild`/profile/boot/systemd execution paths in production defaults. |

## RED→GREEN evidence (L02-AC09)

- PLAN: `747b612d702080bfd8836aa1129f5f126be24942`
- RED (tests-only): `fd314db58f032e14443bed9046a917668b8ea827` — `plans/AGB-6/red-evidence.txt`
- GREEN (initial): `f2cd4f428b342e2bace368400c9923899b9dcb49`
- Review-1 RED: `7dc83f4e4ccae5277b381b5f2c9319ed8a7da7e8` — `plans/AGB-6/review-red-evidence.txt`
- Review-1 GREEN: `d4b6e88476645b677d7bffb3fc86d06b42f91ab2`
- Review-2 RED: `2a206043797178c710500cb1bd263aa197888d08` — `plans/AGB-6/review-2-red-evidence.txt`
- Review-2 GREEN: repair-2 commit on this branch (see PR head)

## Review repair #5018822751 — addressed findings

| Severity | Finding | Repair |
|---|---|---|
| CRITICAL | Structural Class-F rejection bypass | `protected.rs` semantic normalization for nested `boot = { kernelPackages / loader … }` forms |
| IMPORTANT | Race-prone non-durable activation | `capture.rs` durable reservation/finalization; `test_activation.rs` reserves before probe, finalizes terminally after `nixos-rebuild test` invocation |
| IMPORTANT | Public `candidate_closure` schema leak | removed from `ConfigProposeResult`; kept in internal `WalConfigProposePayload` only |
| IMPORTANT | Command-runner / promotion boundaries | `CommandSpec` env/stdin/timeout policies; `CommandError::{Timeout,Interrupted}`; build output binding; `sync_profile_boot_boundaries` |
| MINOR | Trailing whitespace / `git diff --check` | cleaned `plans/AGB-6/PLAN.md` |

## Verification commands (bare, unpiped)

```text
cargo fmt --all -- --check                              PASS (exit 0)
cargo clippy --workspace --all-targets -- -D warnings   PASS (exit 0)
cargo build --workspace --all-targets                   PASS (exit 0)
cargo test --workspace                                PASS (exit 0)
cargo test -p agentbed-adapter-nix --test l02_review_repair_2 PASS (exit 0, 10 tests)
cargo test -p agentbed-broker --test l02_review_repair_2 PASS (exit 0, 3 tests)
git diff --check 5c7ec772a48ce82208bc11173283d2283bf18e6d..HEAD PASS (exit 0)
```

## Residual gaps (explicit)

- No real NixOS VM, power-loss, watchdog/OOB recovery, or live host mutation evidence — deferred to later authorized lanes (L02-AC10).
- `tx.rollback` and L03+ commit/recovery orchestration remain out of scope.
- Gate 1 remains open; this lane delivers L02 primitives only.
