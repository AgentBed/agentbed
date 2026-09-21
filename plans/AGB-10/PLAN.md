# AGB-10 — Patch rustls supply-chain advisory

**Issue:** AGB-10 · `workflow:guarded`  
**Baseline:** `c6087481e77d786889376a3325f12751bf98c2e7` (`origin/main`, verified 2026-09-21)  
**Advisory:** RUSTSEC-2026-0285 / GHSA-2mjx-qc3c-rqvc — rustls 0.23.43 affected; patched in 0.23.45.

## Scope

Minimal `Cargo.lock` update: `cargo update -p rustls --precise 0.23.45`. No manifest, policy, workflow, or application changes. No blanket `cargo update`. No advisory ignores.

## Acceptance traceability

| AC | Scope | Verification |
|---|---|---|
| **AC-01** | `Cargo.lock` only | `cargo update -p rustls --precise 0.23.45`; lockfile shows rustls ≥ 0.23.45 |
| **AC-02** | Pre-update baseline | `cargo deny --all-features check advisories licenses bans sources` → RED (advisory failure) |
| **AC-03** | Post-update | Same `cargo deny` → GREEN; `cargo tree --locked --all-features -i rustls` proves patched version |
| **AC-04** | Build hygiene | `RUSTFLAGS='-D warnings'` + fmt/clippy/build/test with `--locked`; `git diff --check` |
| **AC-05** | Traceability | This PLAN + `plans/AGB-10/RESULT.md` mapping all ACs to exact commands and outcomes |
| **AC-06** | Delivery | DCO sign-off; one PR `AGB-10: Patch rustls supply-chain advisory` against main; no merge |

## Rollback / stop conditions

Stop if update requires manifest or `deny.toml` changes. Hold PR and repair within patched 0.23.x releases only. Reverting to vulnerable lockfile is not acceptable.

## Verification commands

```bash
cargo deny --all-features check advisories licenses bans sources
cargo update -p rustls --precise 0.23.45
cargo tree --locked --all-features -i rustls
RUSTFLAGS='-D warnings' cargo fmt --all -- --check
RUSTFLAGS='-D warnings' cargo clippy --locked --workspace --all-targets -- -D warnings
RUSTFLAGS='-D warnings' cargo build --locked --workspace --all-targets
RUSTFLAGS='-D warnings' cargo test --locked --workspace
git diff --check
```
