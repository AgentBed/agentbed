# AGB-6 — L02 Nix proposal, test activation, and boot-promotion primitives

**Issue:** AGB-6 · parent AGB-1 · GitHub #12  
**Workflow:** `workflow:guarded`  
**Baseline:** `5c7ec772a48ce82208bc11173283d2283bf18e6d`  
**Branch:** `agent/agb-6/l02-nix-proposal-primitives`

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L02-AC01** | `adapters/nix/probe.rs`, `adapters/nix/adapter.rs`, `broker/src/nix_host_adapter.rs`; `adapters/nix/tests/l02_adapter.rs` (`probe_reports_generation_only_when_verified`, `probe_refuses_when_generation_missing`, `nix_adapter_host_surface_matches_probe`). |
| **L02-AC02** | `adapters/nix/protected.rs`, `broker/src/nix_host_adapter.rs`, `broker/src/transaction/engine.rs`; `adapters/nix/tests/l02_adapter.rs` (`protected_path_matrix_rejects_class_f_before_staging`), `broker/tests/l02_nix_adapter.rs` (`nix_propose_rejects_protected_paths_without_wal_side_effects`). |
| **L02-AC03** | `adapters/nix/{capture,propose}.rs`, `broker/src/nix_host_adapter.rs`, `broker/src/transaction/engine.rs`; `adapters/nix/tests/l02_adapter.rs` (`propose_captures_immutable_candidate_and_replays_identically`), `broker/tests/l02_nix_adapter.rs` (`nix_propose_stages_candidate_with_nix_test_plan`, `nix_propose_idempotent_replay_survives_restart`, `conflicting_nix_capture_fails_closed`). |
| **L02-AC04** | `adapters/nix/command_runner.rs`; `adapters/nix/tests/l02_adapter.rs` (`fake_runner_never_invokes_live_nixos_rebuild`). |
| **L02-AC05** | `adapters/nix/promotion/{build,test_activation}.rs`; `adapters/nix/tests/l02_adapter.rs` (`promotion_build_and_test_bind_to_capture`, `promotion_failures_are_explicit_at_each_boundary`). |
| **L02-AC06** | `adapters/nix/promotion/{pin,profile}.rs`; `adapters/nix/tests/l02_adapter.rs` (`promotion_pin_profile_boot_flush_readback_happy_path`). |
| **L02-AC07** | `adapters/nix/promotion/{boot,flush,readback}.rs`; `adapters/nix/tests/l02_adapter.rs` (`promotion_pin_profile_boot_flush_readback_happy_path`, `readback_detects_profile_boot_mismatch`). |
| **L02-AC08** | `adapters/nix/promotion/`, `broker/tests/l02_nix_adapter.rs`; `adapters/nix/tests/l02_adapter.rs` (`promotion_failures_are_explicit_at_each_boundary`, `promotion_module_has_no_forbidden_switch_commands`, `tx_test_still_transitions_for_nix_proposal`). |
| **L02-AC09** | `plans/AGB-6/{PLAN,red-evidence,RESULT}.md`; verification commands below — all PASS on GREEN head. |
| **L02-AC10** | PLAN non-goals; hermetic `FakeCommandRunner` only; no live `nixos-rebuild`/profile/boot/systemd execution paths in production defaults. |

## RED→GREEN evidence (L02-AC09)

- PLAN: `747b612d702080bfd8836aa1129f5f126be24942`
- RED (tests-only): `fd314db58f032e14443bed9046a917668b8ea827` — see `plans/AGB-6/red-evidence.txt`
- GREEN: production implementation commit on this branch (see PR head)

## Verification commands (bare, unpiped)

```text
cargo fmt --all -- --check                              PASS (exit 0)
cargo clippy --workspace --all-targets -- -D warnings   PASS (exit 0)
cargo build --workspace --all-targets                   PASS (exit 0)
cargo test --workspace                                PASS (exit 0)
cargo test -p agentbed-adapter-nix --test l02_adapter PASS (exit 0, 11 tests)
cargo test -p agentbed-broker --test l02_nix_adapter  PASS (exit 0, 6 tests)
```

## Changed paths (summary)

- `adapters/nix/src/{adapter,capture,command_runner,probe,propose,protected,promotion/}` — Nix adapter probe, protected-path rejection, propose capture, promotion primitives
- `adapters/nix/Cargo.toml`, `adapters/nix/src/lib.rs`
- `broker/src/{adapter,nix_host_adapter,dispatch,lib,transaction/engine}.rs` — `HostAdapter::propose_config`, `EngineError::ProposeRejected`, Nix integration
- `broker/Cargo.toml`, `Cargo.lock`
- `plans/AGB-6/RESULT.md`

## Residual gaps (explicit)

- No real NixOS VM, power-loss, watchdog/OOB recovery, or live host mutation evidence — deferred to later authorized lanes (L02-AC10).
- `tx.rollback` and L03+ commit/recovery orchestration remain out of scope.
- Gate 1 remains open; this lane delivers L02 primitives only.
