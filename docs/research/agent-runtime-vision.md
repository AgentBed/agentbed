# Agent runtime vision — "the brain on top of Agentbed"

**Status:** Vision note — **non-normative, deferred**. This is not an ADR and changes nothing in ADR-001.
**Date:** 2026-08-23
**Owner:** L-P
**Positioning decision:** the agent hierarchy described here is a **sibling runtime project**, not part of Agentbed. It is an *optional* runtime for users who do not already run Hermes, OpenClaw, Claude Code or ChatGPT; those users remain the primary audience and connect their own agent over MCP. This project is **not scheduled before the Agentbed MCP layer (Gates 0–3+) is stable.** It exists now only to (a) record the vision while it is fresh and (b) extract the few requirements it imposes on Agentbed's MCP surface, which are cheap to bake in early and expensive to retrofit (§6).

Name: **AgentBed Staff** (decided 2026-08-23, round 2) — first-party family branding: "install AgentBed, add Staff if you don't have your own agent." "The runtime" below refers to AgentBed Staff.

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

## 5. Second-round decisions (2026-08-23, round 2)

The open questions from the first draft are resolved as follows. Items marked *(proposed default)* were delegated to the assistant and stand until the owner vetoes them.

### 5.1 Name — **AgentBed Staff**

First-party family branding. Positioning line: "Install AgentBed; add Staff if you don't already run an agent."

### 5.2 Engine — DSH lead candidate; final call deferred

Four candidates were evaluated (web sweep, 2026-08-23):

| Candidate | Fit | Concerns |
|---|---|---|
| **DeepSeek Harness (DSH)** — MIT, "everything is a plugin" on the Cordis composition framework | **Best architectural fit.** Provider-agnostic model adapters (G6); sessions are an append-only event log — exactly §2.3's state backbone; durable/forkable/resumable sessions match §2.4's wake-rehydrate-exit model; experimental "Agent Teams" (durable roster, task board, mailbox over continuable subagents) mirrors the CoS/queue design | Developer preview, breaking changes expected; sandboxing limited to filesystem (irrelevant here — AgentBed is the sandbox); no published benchmarks |
| **OpenAI Agents SDK** — open-source, production-proven | Handoffs, guardrails, sessions (Redis-backed), tracing, sandbox agents; model-agnostic via LiteLLM | Handoff-centric shape fits tiers 2–3 less naturally than a task-board model; heavier coupling to OpenAI conventions |
| **Vercel AI SDK** | Excellent realtime/streaming front-end plumbing — a candidate for the **comm agent's** voice/UI layer specifically | Single-agent native; its own guidance says to pair with Mastra/LangGraph for multi-agent — not an orchestration engine |
| **Claude Agent SDK** | Most mature runtime; subagents, skills, deepest MCP client support | Claude-models-only as the engine — breaks G6 for a runtime meant for users without their own agent stack |

**Decision:** DSH is the lead candidate and the OpenAI Agents SDK the fallback; the final call is made when Staff is actually scheduled, once DSH's preview status has resolved either way. Until then every Staff contract (task briefs, event-log schema, manifests, queue semantics) is written engine-agnostic. The Vercel AI SDK remains in the picture only as a possible comm-agent front-end.

### 5.3 Interjection policy — urgency-tiered

- **Urgent** (blocking failure, approval about to expire, hard deadline at risk): the comm agent interjects politely even mid-topic.
- **Normal** (task complete, non-blocking question): waits for a conversational pause.
- **Low** (FYI, progress notes): daily digest, or on request.
- **No live conversation:** urgent → Telegram push; everything else queues for the next session and the digest.

### 5.4 Model tiering and escalation ladder *(proposed default)*

| Tier / role | Model class |
|---|---|
| Comm agent | Realtime voice model (cloud, v1) |
| Chief of Staff | Frontier reasoning model with vision |
| Orchestrators | Mid-tier reasoning model |
| Workers | Cheapest model that passes review for that skill; orchestrator may pin a higher class for known-hard skills |
| Reviewers | At least one class above the worker under review, **or** a different model family at the same class |

Escalation ladder on worker failure: retry same model once (fresh context) → escalate one model class once → orchestrator replans the decomposition → escalate to CoS → CoS asks the owner. Each rung logged; each task carries a token/$ cap set by the orchestrator from the project budget, and blowing the cap is itself an escalation, never a silent overrun.

### 5.5 Independent review — risk-based scope, diverse reviewer

Mandatory independent review for anything **user-facing** (reports, drafts, deliverables) and anything **feeding an E-effect** (an email to be sent, a bid-change payload). Internal intermediate artifacts get orchestrator spot-checks only. Two protocol rules *(proposed default)*: the definition-of-done is written into the task brief **before** work starts and the reviewer checks against it, not against taste; the review verdict (pass/fail + findings) is a logged event, and a task is only `complete` with a passing review attached where review is mandatory.

### 5.6 Long-memory recall *(proposed default)*

No embeddings at this scale. The CoS maintains two rendered views: a global `INDEX.md` (one line per project: status, last activity, next milestone — small enough for the comm agent to always hold) and a per-project `SUMMARY.md` (rolling, updated on every significant event). "Tell me about the March client project" = read `SUMMARY.md`, sub-second. Anything deeper is a grep/log query.

### 5.7 Heartbeat cadence *(proposed default)*

CoS supervision sweeps are cron events over the log: **active** projects every 4 h (expect movement daily), **waiting-on-external** daily, **dormant** weekly. Explicit per-task deadlines override the sweep with their own timers. A stale finding wakes the responsible orchestrator first; only unresolved staleness surfaces to the owner.

### 5.8 Multi-user — single-owner v1, identities designed in

v1 serves exactly one owner — no roles, no per-person approval authority. But every log event, queue entry and approval record carries a `principal` field from day one, so multi-user later is a permissions feature, not a schema migration.

## 6. What this imposes on Agentbed *now* (requirements to carry into Gates 1–3 design)

These are the only items with near-term cost; each is a note against existing ADR sections, not new scope. **They are bound to gates as the "Staff-readiness conditions" block in [roadmap.md](../roadmap.md)** (items 1 and 4 → Gate 2; items 2 and 3 → Gates 1–3), which is the normative copy; this list is the rationale:

1. **Derived / attenuated identities (future-proofing §5 identity).** The runtime will mint many short-lived workers. Today Agentbed knows only statically registered agent tokens, and skill-level narrowing is advisory-only. Roadmap candidate (post-G3): a parent identity may request a short-lived child credential bound to a *narrowed* manifest (subset check enforced by the broker). This is the same mechanism that would make skill narrowing a host boundary — one feature, two payoffs. Design the token store so "token → parent chain → manifest" is representable.
2. **`agentbed://events` must be a durable, cursor-resumable stream**, not fire-and-forget — an event-driven CoS that can crash and rehydrate needs replay-from-cursor. (Any webhook remains a convenience on top.)
3. **Cheap R-class status surface.** `tx.status`, ledger queries and per-agent activity summaries should be fast and cheap enough to be called conversationally (the comm agent's fast path 0).
4. **Approval-channel API shape.** Approvals stay on the independent channel, but the *notification* that an approval is pending should be subscribable by external runtimes, so a comm agent can say "there's a Telegram approval waiting for the diff on X" without being able to approve it. Within-budget E-effects need no new mechanism — that is effects.md scoped pre-authorization as already specified.

## 7. Third-round decisions (2026-08-23, round 3)

Four topics the resolved rounds had not yet covered.

### 7.1 Standing preferences — one file, two planes

The owner's standing instructions live in a single human-readable `PREFERENCES.md`, editable by hand or by voice ("from now on, never email clients directly — always draft"). The CoS compiles it into per-tier prompt snippets on every wake, so every agent carries the current preferences without holding them in long-lived context. **Rules with teeth are mirrored into the enforcement plane:** spending caps, allowed recipients, and effect ceilings also become Agentbed manifest constraints / scoped pre-authorizations, enforced by the broker. The same two-plane split as the architecture itself: soft preferences (tone, format, style) are prompt-plane and best-effort; hard rules are policy-plane and survive a forgetful model. A preference whose violation would be an unapproved E-effect must exist in the policy plane or it is not considered a hard rule.

### 7.2 Prompt-injection posture — provenance labels + quarantine

Every log event carries a provenance field: `owner` | `agent:<id>` | `external:<source>`. External-derived text (web pages, inbound email, documents, API responses) travels through the system as **quoted data, never as instructions**: tier prompts are constructed so external content is delimited and cannot address the agent, and no tier treats instructions found inside external content as coming from the owner or another tier. Two hard rules on top: an E-effect payload **derived from external content** always gets independent review, even inside a pre-authorized budget (a malicious page must not be able to ride a standing bid-adjustment authorization); and provenance survives summarization — a summary of external content is still `external`. This composes with Agentbed's own connector-side defenses (typed RPC, response projection); Staff's provenance labels cover the semantic layer Agentbed cannot see. No ingest-screening model in v1; revisit after dogfooding if provenance alone proves insufficient.

### 7.3 Scheduling — the CoS owns all recurring work

Recurring task definitions ("every Monday 08:00, the ads report") are entries in the log, fired by the same timer infrastructure as the §5.7 heartbeat sweeps. The comm agent can create, modify and cancel them by voice, and "what's scheduled?" is one query. No per-project scheduler state, no OS-level cron outside the log: if it recurs, the CoS can see it.

### 7.4 Skills lifecycle — orchestrator-authored, independently reviewed

When a task type recurs successfully, the owning orchestrator drafts a skill file from the winning approach (goal G4: recurring work becomes skills). Before a skill enters the shared library it passes the same independent review protocol as deliverables (§5.5) — a skill is a deliverable whose consumer is a future worker. The library is git-versioned, so a bad skill update is a revert, not an incident; each skill records which tasks used it, so a quality regression is traceable to the skill version. The owner is **notified, not consulted** on skill changes (they land in the daily digest); vetoing one is a revert away. Skills remain runtime-format-compatible with Hermes/OpenClaw conventions where practical (ADR §6.3), so a Staff skill can migrate if the user later adopts an external runtime.

## 8. Fourth-round decisions (2026-08-23, round 4)

### 8.1 Budgets — soft per project, hard per month

Task-level token/$ caps (§5.4) remain the orchestrator's tool. Above them: **per-project budgets are advisory** — crossing one raises a normal-priority queue item ("ads-reporting is at 130 % of its monthly budget") but work continues; the **global monthly cap is a hard stop** — once hit, only urgent-priority tasks may run and everything else queues until the owner raises the cap by voice or edits it. Spend is queryable at any grain ("what did the ads project cost this month?") via fast path 0, and the daily digest includes yesterday's spend by project. Every model invocation writes its cost to the log at emission time, so cost accounting is a query over the same backbone as everything else — no separate metering system.

### 8.2 Failure UX — triage-first, plain language

When the §5.4 ladder exhausts, the CoS does not forward the failure — it forwards a **diagnosis with options**: what broke, since when, what was already tried, and 2–3 concrete choices ("Google Ads API has rejected auth since 09:00 — I can retry tonight, skip those 3 accounts and deliver a partial report, or you can re-login from your phone"). Urgency routing follows §5.3: failures blocking a deadline interject; the rest batch into the digest. Raw errors, stack traces and retry noise stay in the log for inspection; the voice channel carries only decisions worth making. A failure with an obvious safe default ("retry tonight") states it and proceeds with it if the owner doesn't respond by the decision's deadline — the queue item records the default taken.

### 8.3 Onboarding — a conversational interview

First run is a guided conversation with the comm agent: who you are and what you do; which services to connect (each answer spawns the corresponding connector auth flow — credentials go to Agentbed connectors, never through the conversation); your ground rules, seeding `PREFERENCES.md` (§7.1) with hard rules compiled to the policy plane immediately; a starter monthly budget (§8.1). It ends by proposing the first project from something the owner actually needs done that week — onboarding completes by *doing*, not by configuring. All of it writes ordinary config underneath (preferences file, manifests, budget entries), so a technical owner can inspect or edit the files directly; the interview is a front-end, not a format.

### 8.4 Staff self-update — owner-triggered, Agentbed-transacted

Staff never updates itself. Releases are pinned versions the **owner** applies through Agentbed's transaction engine — test, apply, probation, automatic rollback — exactly as ADR-001 already requires for Agentbed's own self-update (class F for agents, owner-applied through the engine). Staff may *notify* that an update exists (a queue item with the changelog) but holds no capability to install it; a Staff manifest that requested write access to Staff's own installation would be rejected at compile time. The probation health check for a Staff update includes the comm agent answering a status query — the one component whose silent failure mutes the whole interface is the one the watchdog explicitly probes.

## 9. Remaining parked items

Everything else is decided. Parked by design, revisited when Staff is scheduled: the final engine call (§5.2 — DSH vs OpenAI Agents SDK, pending DSH's maturity); the proposed defaults in §5.4/5.6/5.7 and §8 (standing unless the owner vetoes); local voice replacing the cloud realtime API (§2.7); multi-user (§5.8).
