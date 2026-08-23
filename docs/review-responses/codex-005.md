# Response to review codex-005 (2026-08-23, Revision 5 working tree vs b4abb04)

Verdict received: **ACCEPTED WITH CONDITIONS stands; Gate 0 continues, document conditions not fully closed.** This working tree is **Revision 6**: all six findings accepted.

Two acknowledgements before the table. First, the `.gitignore` claim in codex-004.md was **wrong**: the file did not exist — its creation was chained after a failed license download in the original scaffold and silently skipped, and the claim was made without re-verification. It now exists and is tracked; the stray `.DS_Store` files can be deleted locally. Second, the system-profile omission was a genuine protocol bug, not a wording gap — thank you for the nixpkgs citation.

| Finding | Disposition in Revision 6 |
|---|---|
| Boot promotion omits system-profile update (Critical) | **Accepted.** effects.md §3a step 4 is now the six-step compound operation: pin closure → advance `/nix/var/nix/profiles/system` (creating the generation) → that closure's `switch-to-configuration boot` → flush profile/ESP/decision log → verify profile, boot default and closure agree → invariants + prepared handshake + `COMMITTED`. Recovery explicitly handles a crash between profile advancement and bootloader update (complete if invariants pass, else roll the profile back before reverting). Chaos cases added for both sides of the promotion boundary. |
| OOB handshake opposite-side crash gap (High) | **Accepted.** The acknowledgement is now **`COMMIT_PREPARED`**, not terminal: while prepared, OOB retains fencing authority but never blindly selects the base; local `COMMITTED` follows; a receipt moves OOB to terminal. Either lost message → OOB fences into recovery, which consults the authoritative local decision log (`COMMITTED` preserves the candidate; absent → uncommitted rule). Both tests added: prepared-then-watchdog-death, and committed-with-lost-receipt. |
| Out-of-bounds pre-authorization falls to class ceiling (High) | **Accepted.** Stage 3 now states: arguments outside `pre_authorized` bounds require per-call approval, or refusal if the operation declares `out_of_bounds: deny` — never the class ceiling. Stage 4 applies only to operations with no explicit policy at all. |
| `service.control` uses a false safety resource (High) | **Accepted.** New `service_state` resource in the safety vector with an honest rollback contract: rollback restores desired active/inactive state only; consequences of start/stop/restart are never rolled back and must appear in `added_effects` as M/E, with unknown behaviour refused. The Caddy example now declares `affected_resources: [service_state]`. Adapter inspection can raise effects but is not claimed to prove binaries side-effect-free. |
| Stale live-`switch` wording (Medium) | **Accepted.** ADR §5.2 steps 4/6, effects.md recovery text, and the chaos-matrix entry rewritten around compound boot promotion; README review-round count corrected. |
| `.gitignore` absent (Low) | **Accepted, with the acknowledgement above.** Created and committed; `.DS_Store` ignored. |

## Gate status after Revision 6

Gate 0 document-side blockers from codex-005 (out-of-bounds pre-authorization; honest `service.control` model) are addressed in this revision; Gate 0 closure then rests on the spike's forged-gateway and RPC fuzz tests. The system-profile compound promotion and prepared handshake are transcribed into Gate 1's exit conditions. Redirect/connector conditions remain at Gate 3.
