# Agent runtime vision — "the brain on top of Agentbed"

**Status:** Vision note — **non-normative, deferred**. This is not an ADR and changes nothing in ADR-001.
**Date:** 2026-08-23
**Owner:** L-P
**Positioning decision:** the agent hierarchy described here is a **sibling runtime project**, not part of Agentbed. It is an *optional* runtime for users who do not already run Hermes, OpenClaw, Claude Code or ChatGPT; those users remain the primary audience and connect their own agent over MCP. This project is **not scheduled before the Agentbed MCP layer (Gates 0–3+) is stable.** It exists now only to (a) record the vision while it is fresh and (b) extract the few requirements it imposes on Agentbed's MCP surface, which are cheap to bake in early and expensive to retrofit (§6).

Working name in this note: **the runtime**. (Naming TBD; must not collide with Agentbed itself.)

---

## 1. The four tiers

| Tier | Role | Never does |
|---|---|---|
| **Communication agent** | The user's realtime conversational interface (2-way voice first). Optimized for latency and conversational quality. Answers status/info questions directly from the ledger; routes work downward; relays questions/decisions upward from the message queue. | Execute work. Hold task context beyond the current conversation — it *knows where everything is*, not *everything*. |
| **Chief of Staff (CoS)** | Intake, clarification, classification. Files every request into the right project; creates projects; delegates to the project's orchestrator; supervises all orchestrators (nothing stuck, nothing stale); escalates decisions to the user via the comm agent. | Execute project work. Talk to the user directly (always through the comm agent / queue). |
| **Orchestrator (one per project)** | Plans and coordinates the project's work: decomposes tasks, picks/spawns sub-agents, balances cost vs quality vs speed, escalates failed sub-agents to bigger models, mandates independent review of completed work, escalates judgment calls to the CoS. | Talk to the user. Cross project boundaries. |
| **Sub-agents (workers)** | Do the work. Each has a defined skill; short-lived by default, permanent when a project warrants it. Created, evaluated and retired by their orchestrator. | Communicate outside their orchestrator. Exceed their manifest. |

## 2. Decided design points (2026-08-23 brainstorm)

1. **Sibling project.** ADR-001's boundary stands: Agentbed = governed hands, the runtime = brain. Every runtime tier that touches a host does so as an Agentbed identity with its own capability manifest.
2. **Tiered routing — the full chain is the exception, not the rule.**
   - *Fast path 0:* status/info questions → comm agent answers directly from the event log/ledger. No delegation.
   - *Fast path 1:* trivial one-off tasks → CoS runs a single short-lived worker. No project, no orchestrator.
   - *Full chain:* real multi-step projects only.
3. **State backbone = structured log + Markdown views.** Machine truth is an append-only event log / task store (SQLite or JSONL): tasks, statuses, messages, decisions-pending. The CoS renders human-and-agent-readable `.md` project files (one folder per project: `PROJECT.md`, `TASKS.md`, `DECISIONS.md`, `LOG.md`, deliverables) *from* the log. Agents read `.md` for context; all writes go through the log. This kills the concurrent-writer problem and makes "what's pending across 40 projects?" a query, not a crawl.
4. **Event-driven, not resident.** CoS and orchestrators hold no long-lived process or context. They wake on events (new task, sub-agent completion, question, timer), rehydrate from the project files + log, act, write, exit. "Supervision" = the CoS's scheduled sweep over the log (stale tasks, stuck orchestrators, unanswered questions) plus event wakes. The comm agent is the only latency-critical, warm component.
5. **Autonomy line = effect class + per-project budgets.** R (read) and M (internal data) proceed autonomously. D (system change) and E (irreversible external) require approval — *except* inside an explicit, narrowly scoped per-project pre-authorization (e.g. "ads project may adjust bids ±10 % up to $X/day"). This is exactly effects.md §1 precedence; the runtime adopts Agentbed's taxonomy rather than inventing its own.
6. **Approvals: voice suffices within budgets.** A verbal "yes" through the comm agent authorizes E-effects *inside* pre-authorized scopes. Beyond scope (new recipient domains, spend over cap, account-level changes, anything class D on a host), approval must come through the independent channel (Agentbed's Telegram bot / takeover UI with the exact diff) — per the threat-model rule that an approval relayed by the requesting runtime is friction, not authorization.
7. **Voice v1 = cloud realtime API**, behind an interface that lets a local STT→LLM→TTS pipeline swap in later. Local voice is a goal, not a v1 constraint.
8. **Message queue for the comm agent.** CoS/orchestrator questions, completions and alerts land in a priority queue the comm agent drains at natural conversation boundaries; the user can always interrupt with new topics and the queue holds. Queue entries reference log events, so "what's the status of the ads report?" is answerable at any time without asking anyone downstream.

## 3. Internal dogfooding (pre-release testing)

Chosen internal test: **use the runtime to develop Agentbed itself** — features, bug fixes, updates. Two modes, sharply distinguished:

- **Runtime as dev team on the Agentbed *codebase*** — orchestrator + coding/review sub-agents working a git repo. Low risk (git is the rollback), available as soon as the runtime minimally works, and exercises decomposition, review passes and escalation on real work. ✅ This is the dogfood.
- **Runtime operating a *live* Agentbed host to update Agentbed itself** — this is class F in ADR-001 (Agentbed self-modification, refused in v0) and stays behind its gates. Not a testbed. When the runtime later operates hosts at all, it does so on the NixOS lab VMs through the ordinary transaction engine, like any other MCP client.

Until the runtime exists, Claude Code sessions remain the de facto orchestrator for Agentbed development (status quo).

## 4. Sketch: how a request flows

"Review all my Google Ads client accounts, analyze each campaign, send me a PDF report."

1. Comm agent: confirms scope conversationally (all accounts? period? deadline?), emits `task.created` to the log, keeps talking about whatever the user wants next.
2. CoS (woken by the event): classifies → existing project `clients/ads-reporting/`; appends the task to its `TASKS.md` via the log; wakes the project orchestrator.
3. Orchestrator: plans — one cheap data-pull worker per account (parallel), one analysis worker per account, one synthesis/report worker, one *independent reviewer* on the final PDF. Cost-tiers models per stage; escalates any failing worker to a stronger model once before raising it to the CoS.
4. Effects: data pulls are R; report generation M; nothing E → no approval needed. If a worker proposes pausing a campaign, that's E and outside the reporting project's pre-auth → question into the queue → comm agent raises it at the next conversational beat.
5. Completion event → queue → comm agent: "Your ads report is ready — 12 accounts, two need attention. Want the summary now?"

## 5. Open questions (next brainstorm)

- **Naming** the runtime project.
- **Build vs assemble:** custom engine vs building on the Claude Agent SDK (whose orchestrator/subagent/skills model maps 1:1 onto tiers 3–4), vs Hermes/OpenClaw extension. Leaning SDK-based assembly; decide when the project is scheduled.
- **Interjection policy:** when may the comm agent proactively speak (task done, urgent question) vs wait for a conversational pause vs push to Telegram because no conversation is live?
- **Model tiering table:** which model class per tier/stage, and the concrete escalation ladder (worker fails → retry same → bigger model → CoS).
- **Independent review:** same model family as the worker or deliberately different? What artifact makes a review "passed"?
- **Long-memory recall:** what the comm agent reads to discuss a 3-month-old project in <1 s — per-project `SUMMARY.md` maintained by the CoS? An index? Embeddings are *not* assumed.
- **Heartbeat cadence** for CoS supervision sweeps, and staleness thresholds per project class.
- **Multi-user later?** v1 is single-owner; the queue/approval model assumes one human.

## 6. What this imposes on Agentbed *now* (requirements to carry into Gates 1–3 design)

These are the only items with near-term cost; each is a note against existing ADR sections, not new scope:

1. **Derived / attenuated identities (future-proofing §5 identity).** The runtime will mint many short-lived workers. Today Agentbed knows only statically registered agent tokens, and skill-level narrowing is advisory-only. Roadmap candidate (post-G3): a parent identity may request a short-lived child credential bound to a *narrowed* manifest (subset check enforced by the broker). This is the same mechanism that would make skill narrowing a host boundary — one feature, two payoffs. Design the token store so "token → parent chain → manifest" is representable.
2. **`agentbed://events` must be a durable, cursor-resumable stream**, not fire-and-forget — an event-driven CoS that can crash and rehydrate needs replay-from-cursor. (Any webhook remains a convenience on top.)
3. **Cheap R-class status surface.** `tx.status`, ledger queries and per-agent activity summaries should be fast and cheap enough to be called conversationally (the comm agent's fast path 0).
4. **Approval-channel API shape.** Approvals stay on the independent channel, but the *notification* that an approval is pending should be subscribable by external runtimes, so a comm agent can say "there's a Telegram approval waiting for the diff on X" without being able to approve it. Within-budget E-effects need no new mechanism — that is effects.md scoped pre-authorization as already specified.
