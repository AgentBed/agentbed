<!-- Keep PRs small and bound to one issue. -->

## What & why

Closes #<!-- issue -->.

## Normative documents

<!-- Which documents this touches, or "none". Remember: where code and
     document disagree, the document wins — if this PR changes behavior a
     document specifies, it must change the document too. -->

## Gate

<!-- Which roadmap gate / exit condition this serves, or "housekeeping". -->

## Checklist

- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` pass locally
- [ ] New behavior has a test
- [ ] Commits are signed off (`git commit -s`, DCO)
