# Security policy

Agentbed's purpose is to let AI agents operate a machine under real enforcement.
A vulnerability in Agentbed is therefore not a side issue — it is a defect in the
product's core claim. We want to hear about it.

## Reporting a vulnerability

**Please do not open a public issue for anything exploitable.**

Report privately via GitHub's private vulnerability reporting:
<https://github.com/AgentBed/agentbed/security/advisories/new>

Include what you would put in a `[review]` issue (see `docs/REVIEW.md`): the
affected component or document section, a concrete attack scenario, and — if you
have one — the fix you would make.

You will get an acknowledgment within **72 hours** and an assessment within
**14 days**. We will credit you in the advisory unless you ask otherwise.

## What counts

- Anything that lets an agent act outside its manifest: sandbox escape,
  policy-ladder bypass, credential extraction or cross-identity reuse,
  approval replay, ledger tampering, watchdog defeat.
- Flaws in the *design* that make such an escape possible once the relevant
  gate ships. If the flaw is in a normative document but not exploitable in
  shipped code, a public `[review]` issue is fine and preferred — that is the
  project's normal review channel.

## Supported versions

Pre-alpha (Gate 0). There are no releases and **no supported versions yet**;
fixes land on `main` only. This section will change at the first tagged
release (NixOS-only alpha, Gate 3).
