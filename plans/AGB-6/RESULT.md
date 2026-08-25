# AGB-6 — L02 Nix proposal, test activation, and boot-promotion primitives

**Issue:** AGB-6 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `5c7ec772a48ce82208bc11173283d2283bf18e6d`
**Branch:** `agent/agb-6/l02-nix-proposal-primitives`

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L02-AC01** | `adapters/nix/probe.rs`, `adapters/nix/adapter.rs`, `broker/src/nix_host_adapter.rs`; `adapters/nix/tests/l02_adapter.rs` (`probe_reports_generation_only_when_verified`, `probe_refuses_when_generation_missing`, `nix_adapter_host_surface_matches_probe`). |
| **L02-AC02** | `adapters/nix/protected.rs`, `broker/src/nix_host_adapter.rs`; `adapters/nix/tests/l02_adapter.rs` (`protected_path_matrix_rejects_class_f_before_staging`), `adapters/nix/tests/l02_review_repair.rs`, `adapters/nix/tests/l02_review_repair_2.rs`, `adapters/nix/tests/l02_review_repair_3.rs`, `adapters/nix/tests/l02_review_repair_4.rs`, `broker/tests/l02_review_repair.rs`, `broker/tests/l02_review_repair_2.rs`. |
| **L02-AC03** | `adapters/nix/{capture,propose}.rs`, `broker/src/nix_host_adapter.rs`, `broker/src/transaction/engine.rs`; `adapters/nix/tests/l02_adapter.rs`, `broker/tests/l02_nix_adapter.rs`, `broker/tests/l02_review_repair.rs`, `broker/tests/l02_review_repair_2.rs`. |
| **L02-AC04** | `adapters/nix/command_runner.rs`; `adapters/nix/tests/l02_adapter.rs` (`fake_runner_never_invokes_live_nixos_rebuild`), `adapters/nix/tests/l02_review_repair_2.rs`. |
| **L02-AC05** | `adapters/nix/promotion/{build,test_activation}.rs`, `adapters/nix/capture.rs`; `adapters/nix/tests/l02_adapter.rs`, `adapters/nix/tests/l02_review_repair.rs`, `adapters/nix/tests/l02_review_repair_2.rs`, `adapters/nix/tests/l02_review_repair_3.rs`, `adapters/nix/tests/l02_review_repair_4.rs`, `adapters/nix/tests/l02_review_repair_5.rs`. |
| **L02-AC06** | `adapters/nix/promotion/{pin,profile}.rs`, `adapters/nix/capture.rs`; `adapters/nix/tests/l02_adapter.rs`, `adapters/nix/tests/l02_review_repair.rs`, `adapters/nix/tests/l02_review_repair_3.rs`. |
| **L02-AC07** | `adapters/nix/promotion/{boot,flush,readback}.rs`; `adapters/nix/tests/l02_adapter.rs`, `adapters/nix/tests/l02_review_repair.rs`, `adapters/nix/tests/l02_review_repair_2.rs`, `adapters/nix/tests/l02_review_repair_3.rs`. |
| **L02-AC08** | `adapters/nix/promotion/`, `broker/tests/l02_nix_adapter.rs`; `adapters/nix/tests/l02_adapter.rs` (`promotion_failures_are_explicit_at_each_boundary`, `promotion_module_has_no_forbidden_switch_commands`). |
| **L02-AC09** | `plans/AGB-6/{PLAN,red-evidence,review-red-evidence,review-2-red-evidence,review-3-red-evidence,review-4-red-evidence,review-5-red-evidence,RESULT}.md`; `adapters/nix/tests/l02_review_repair_5.rs` (`result_md_maps_all_l02_acceptance_ids_in_traceability_table`); verification commands below. |
| **L02-AC10** | PLAN non-goals; hermetic `FakeCommandRunner` only; no live `nixos-rebuild`/profile/boot/systemd execution paths in production defaults; residual gaps below. |

## RED→GREEN evidence (L02-AC09)

- PLAN: `747b612d702080bfd8836aa1129f5f126be24942`
- Initial RED: `fd314db58f032e14443bed9046a917668b8ea827` — `plans/AGB-6/red-evidence.txt`
- Initial GREEN: `f2cd4f428b342e2bace368400c9923899b9dcb49`
- Review-1 RED: `7dc83f4e4ccae5277b381b5f2c9319ed8a7da7e8` — `plans/AGB-6/review-red-evidence.txt`
- Review-1 GREEN: `d4b6e88476645b677d7bffb3fc86d06b42f91ab2`
- Review-2 RED: `2a206043797178c710500cb1bd263aa197888d08` — `plans/AGB-6/review-2-red-evidence.txt`
- Review-2 GREEN: `0a763d3b8e866da556ed625bc8cf2e2ae4b6bed8`
- Review-3 RED: `a004dfdcec5a3f917527e43ab71d5a2679c867d5` — `plans/AGB-6/review-3-red-evidence.txt`
- Review-3 GREEN: `e663294bffd5c318e80e410e18efec0dd7a8c7fa`
- Review-4 RED: `9809fda9d2155db49974afa687c1fa8738ec539e` — `plans/AGB-6/review-4-red-evidence.txt`
- Review-4 GREEN: `e582ec2cc17628fbbde71ab0dbf71c399c213423`
- Review-5 RED: `2ef152a62d91ccb8e6987099c612ed5ae8bbfd39` — `plans/AGB-6/review-5-red-evidence.txt`
- Review-5 GREEN: see PR head

## Review repair #5019548471 — addressed findings

| Severity | Finding | Repair |
|---|---|---|
| IMPORTANT | Interrupted-creator activation durability | `capture.rs` fsyncs `root` on every reservation path before probe/test |
| IMPORTANT | Incomplete RESULT traceability | restored full L02-AC01…L02-AC10 table; `l02_review_repair_5.rs` guards completeness |

## Verification commands (bare, unpiped)

```text
cargo fmt --all -- --check                              PASS (exit 0)
cargo clippy --workspace --all-targets -- -D warnings   PASS (exit 0)
cargo build --workspace --all-targets                   PASS (exit 0)
cargo test --workspace                                PASS (exit 0)
cargo test -p agentbed-adapter-nix --test l02_review_repair_5 PASS (exit 0, 2 tests)
git diff --check 5c7ec772a48ce82208bc11173283d2283bf18e6d..HEAD PASS (exit 0)
```

## Residual gaps (explicit)

- No real NixOS VM, power-loss, watchdog/OOB recovery, or live host mutation evidence — deferred to later authorized lanes (L02-AC10).
- `tx.rollback` and L03+ commit/recovery orchestration remain out of scope.
- Gate 1 remains open; this lane delivers L02 primitives only.

## Hard non-goals (L02-AC10)

- No real NixOS/Proxmox mutation, live `nixos-rebuild` execution, deployment, activation, credentials, watchdog/OOB work, L03+ orchestration, chaos/VM evidence, later Gate 1 lanes, Gate 2+, router/reconciler changes, repository settings changes, merge, or push to `main`.
