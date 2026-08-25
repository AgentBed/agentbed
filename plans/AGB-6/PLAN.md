# AGB-6 — L02 Nix proposal, test activation, and boot-promotion primitives

**Issue:** AGB-6 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `5c7ec772a48ce82208bc11173283d2283bf18e6d` (`origin/main`, verified 2026-08-24)
**Roadmap gate / exit:** Gate 1 L02 (`plans/AGB-1/PLAN.md` lane L02). Gate 1 remains open after this lane.

## 1. Evidence and scope

Inspected: `plans/AGB-1/PLAN.md` L02 row; ADR-001 rev. 6; `docs/threat-model.md`; `docs/effects.md` §§2–4; `docs/protocol.md`; L01 `plans/AGB-4/RESULT.md`; `broker/src/{adapter,transaction/engine}.rs`; `adapters/nix/src/lib.rs` stub.

L01 merged: durable WAL, idempotency, events, synthetic `config.propose` via `UnresolvedAdapter`. This lane replaces synthetic proposal **only** on the resolved Nix adapter path, adds semantic class-F protected-path rejection before any WAL side effect, and delivers hermetic Nix adapter primitives for probe, propose, build, one test activation, closure pin, profile advance, boot configuration, flush, and agreement readback.

### Consequential assumptions

1. **Hermetic only.** All `nixos-rebuild`, profile, bootloader, mount, sync, and systemd commands execute through an injected `CommandRunner`; tests use `FakeCommandRunner` with exact executable/argv assertions. No live host mutation in this lane.
2. **Unresolved preserved.** When `adapter.info().resolved` is false, `config.propose` keeps L01 synthetic diff/test_plan (`unresolved` / `noop-test`).
3. **Protected-path rejection is broker-visible.** `agentbed_adapter_nix::protected` runs before WAL append; rejection returns `EngineError::ProposeRejected` with no WAL/event/idempotency side effect.
4. **Immutable capture.** Candidate closure, base revision, diff, and test plan are captured at first successful propose and persisted in WAL `result_json`; replay does not re-probe.
5. **COMMITTING primitives are adapter-only.** `promotion` module exposes build/test/pin/profile/boot/flush/readback; no `nixos-rebuild switch`, no second `nixos-rebuild test`, no `switch-to-configuration switch`, no systemd job submission.

### Hard non-goals (L02-AC10)

No real NixOS/Proxmox mutation; no deployment or activation; no watchdog daemon/decision authority/leases/fencing; no OOB work; no L03+ commit/recovery orchestration; no chaos harness or VM evidence; no router/reconciler or branch-protection changes; no merge or push to `main`.

## 2. Architecture

### Component layout

```
adapters/nix/
  command_runner.rs   — injected runner + FakeCommandRunner (L02-AC04)
  protected.rs        — semantic class-F path/content rejection (L02-AC02)
  probe.rs            — safety vector + base revision probe (L02-AC01)
  capture.rs          — immutable base/candidate records (L02-AC03)
  propose.rs          — deterministic diff + test plan (L02-AC03)
  promotion/          — build, test, pin, profile, boot, flush, readback (L02-AC05–07)
  adapter.rs          — NixAdapter HostAdapter + propose integration

broker/
  adapter.rs          — HostAdapter::propose_config extension
  transaction/engine.rs — protected check + Nix propose path
  tests/l02_nix_adapter.rs — integration matrix (L02-AC08)
```

### Protected resources (class F at propose)

Semantic rejection (not label-only) for changes touching: watchdog package/closure/binary/config, watchdog unit files, watchdog state, broker WAL, precommitted rollback path, OOB protocol store, AgentBed self-protection paths, kernel (`boot.kernelPackages`), bootloader (`boot.loader`), storage layout (`fileSystems` on protected mounts), firewall management plane (`networking.firewall`). Covers normalized paths, `..` traversal, alias paths, duplicate/conflicting changes, and Nix expression content that indirectly selects protected components.

### Promotion command boundary (hermetic)

| Primitive | Command shape | Forbidden |
|---|---|---|
| build | `nixos-rebuild build` + captured flake | `switch`, second `test` |
| test (once) | `nixos-rebuild test` bound to captured candidate | preserves base boot default |
| pin | `nix-store --realise` closure | live profile mutation |
| profile advance | `nix-env -p /nix/var/nix/profiles/system --set` pinned closure | mismatch refusal |
| boot config | `<closure>/bin/switch-to-configuration boot` | `switch` variant |
| flush | `sync` on profile + boot ESP paths | — |
| readback | read profile link, boot default, closure hash | stale/partial mismatch |

## 3. Acceptance traceability

| AC | Intended paths | Verification |
|---|---|---|
| **L02-AC01** | `adapters/nix/probe.rs`, `adapter.rs` | Fake-runner tests: exact argv; generation-backed safety only when verified; honest `none` for bootloader/kernel; missing/malformed probe → refusal |
| **L02-AC02** | `adapters/nix/protected.rs`, `broker/src/transaction/engine.rs` | Protected path matrix; WAL/event count unchanged on rejection; traversal/alias/content cases |
| **L02-AC03** | `adapters/nix/{capture,propose}.rs`, `engine.rs` | Deterministic diff; immutable capture; idempotent WAL replay; conflicting capture fails closed |
| **L02-AC04** | `adapters/nix/command_runner.rs` | Fake runner records every invocation; suite cannot invoke live nixos-rebuild/profile/boot/systemd |
| **L02-AC05** | `adapters/nix/promotion/build.rs`, `test_activation.rs` | Exact build/test commands; base boot preserved; non-zero exit fails closed |
| **L02-AC06** | `adapters/nix/promotion/{pin,profile}.rs` | Pin + profile advance ordering; mismatch refusal; idempotent readback |
| **L02-AC07** | `adapters/nix/promotion/{boot,flush,readback}.rs` | `switch-to-configuration boot` only; flush failure; agreement mismatch cases |
| **L02-AC08** | `adapters/nix/tests/`, `broker/tests/l02_nix_adapter.rs` | Crash at every promotion boundary; static scan: no forbidden commands in promotion module |
| **L02-AC09** | `plans/AGB-6/{PLAN,RESULT,red-evidence}.md` | fmt, clippy, build, test — unpiped outcomes |
| **L02-AC10** | PLAN non-goals §1 | No live mutation hooks in production default paths |

## 4. Failure injection matrix (L02-AC08)

| Boundary | Inject | Expected |
|---|---|---|
| build | non-zero exit | explicit error, no candidate claim |
| test | malformed output | fail closed |
| pin | closure mismatch | refuse profile advance |
| profile advance | partial movement | readback mismatch |
| boot config | `switch-to-configuration` wrong mode | rejected at API |
| flush | sync failure | explicit error, no invented success |
| readback | stale profile vs boot | `AgreementMismatch` |

## 5. Verification commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```

Focused during TDD:

```bash
cargo test -p agentbed-adapter-nix
cargo test -p agentbed-broker --test l02_nix_adapter
```

## 6. Rollback and stop conditions

Stop if watchdog/self/OOB paths can be proposed without rejection. Stop if adapter reports generation safety without verified probe. Revert is safe: no live host integration.

## 7. Delivery

Branch `agent/agb-6/l02-nix-proposal-primitives`, PR `AGB-6: Nix proposal, test activation, and boot-promotion primitives`. TDD: PLAN → tests (RED) → implementation (GREEN) → RESULT. DCO sign-off on each commit.
