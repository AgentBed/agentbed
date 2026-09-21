# AGB-10 — RESULT

**Issue:** AGB-10 · `workflow:guarded`  
**Worktree:** `/home/lpbaril/multica_workspaces/workspace-77a551fa4fb4/task-c7657b30781d/workdir/agentbed`  
**Branch:** `agent/cursor-implementer/c7657b30781d`  
**Baseline:** `c6087481e77d786889376a3325f12751bf98c2e7`  
**Toolchain:** Rust 1.98.0 (`rust-toolchain.toml`)  
**Advisory:** RUSTSEC-2026-0285 / GHSA-2mjx-qc3c-rqvc

## Summary

Minimal `Cargo.lock` update: `rustls` 0.23.43 → 0.23.45 via `cargo update -p rustls --precise 0.23.45`. No manifest, policy, workflow, or application changes.

## Acceptance traceability

| AC | Outcome | Evidence |
|---|---|---|
| **AC-01** | PASS | Only `Cargo.lock` changed (2 lines: version + checksum). `rustls` 0.23.45 in lockfile. |
| **AC-02** | PASS (RED at baseline) | Hermes independently reproduced RED at immutable base `c6087481` in `/home/lpbaril/.hermes/cache/scratch/agb10-baseline-red`: `/tmp/agb10-cargo-bin/bin/cargo-deny --log-level error --all-features check advisories licenses bans sources` → exit **1**, `error[vulnerability] RUSTSEC-2026-0285`; advisories FAILED, bans/licenses/sources ok. |
| **AC-03** | PASS (GREEN post-update) | `/tmp/agb10-cargo-bin/bin/cargo-deny --log-level error --all-features check advisories licenses bans sources` → exit **0**, `advisories ok, bans ok, licenses ok, sources ok`. `cargo tree --locked --all-features -i rustls` → `rustls v0.23.45`. |
| **AC-04** | PASS | All commands below exit **0** on Linux with `RUSTFLAGS='-D warnings'`. Tests use injected/fake harnesses only; no live host mutation. |
| **AC-05** | PASS | This RESULT maps all ACs. Gate 1 scope, security policy, and application behavior unchanged. No exploitability audit claimed. |
| **AC-06** | PASS | DCO sign-off commits; one PR against `main`; no merge. |

## Verification commands (this run)

All commands run bare (no pipes/tee/status masking) from worktree root unless noted.

### AC-02 RED (baseline — Hermes reproduction, superseding masked prior run)

```
/tmp/agb10-cargo-bin/bin/cargo-deny --log-level error --all-features check advisories licenses bans sources
# exit 1 — RUSTSEC-2026-0285 on rustls 0.23.43 at base c6087481
```

### AC-03 GREEN (post-update)

```
/tmp/agb10-cargo-bin/bin/cargo-deny --log-level error --all-features check advisories licenses bans sources
# exit 0 — advisories ok, bans ok, licenses ok, sources ok

cargo tree --locked --all-features -i rustls
# rustls v0.23.45 (via jsonschema/reqwest transitive chain)
```

### AC-04 build hygiene

```
RUSTFLAGS='-D warnings' cargo fmt --all -- --check          # exit 0
RUSTFLAGS='-D warnings' cargo clippy --locked --workspace --all-targets -- -D warnings  # exit 0
RUSTFLAGS='-D warnings' cargo build --locked --workspace --all-targets                # exit 0
RUSTFLAGS='-D warnings' cargo test --locked --workspace                              # exit 0 (all workspace tests passed)
git diff --check                                                                      # exit 0
```

## Diff scope

```
Cargo.lock: rustls 0.23.43 → 0.23.45 (version + checksum only)
plans/AGB-10/PLAN.md: committed in prior commit 25940f3
plans/AGB-10/RESULT.md: this file
```

## Residual gaps

None. Independent current-head Codex review and CI pending per AC-06.
