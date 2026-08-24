# Goals and user stories

**Revision 6 (2026-08-24)** — kept in step with ADR-001; review history in `review-responses/`.

These guide scope decisions. A feature that serves none of them is out of scope. Primary persona: "L-P", a solo operator running Hermes Agent on a home server with several specialised bots, on a private tailnet. Trust model and success criteria per gate: see `threat-model.md` and `roadmap.md`. "Fully operate" always means *within a manifest* — full capability is a grant, not a default.

## G1 — Any agent gets a governed computer

Any agent — a Hermes bot on another machine, Claude Code, ChatGPT, a local model — can connect to an Agentbed host over MCP and receive a computer it can fully operate: shell, files, services, packages, a browser and a desktop. What it may touch is declared in a manifest and enforced by the host, not by the agent's good behaviour.

*Story:* "Connect my external Claude Code session to the Agentbed MCP and let it set up a monitoring stack on that machine. It may install packages and manage services; it may not reach the internet except package mirrors, and it may not read my home directory."

## G2 — Everything an agent does is attributable, reviewable, and reversible *per its effect set*

Every change carries the agent's identity (a credential bound to a manifest), the manifest digest, a diff, its computed effect set and an outcome in a hash-chained ledger anchored off-host. Declarative host changes roll back automatically; data mutations restore from tested snapshots; **external effects (sent emails, SaaS mutations, browser or desktop input wherever the desktop has external egress) are irreversible and therefore gated on per-transaction approval or an explicit, narrowly scoped pre-authorization in the manifest** (effects.md §1). The host reports rollback strength per resource, honestly. See `effects.md`.

*Story:* "Show me everything the LinkedIn bot changed this week, and undo the config change it made on Tuesday."

## G3 — Disposable computers per bot, shared logins when I want them

Each bot can have its own desktop with a browser, persistent profile, snapshot/restore, and a takeover screen I can reach from my laptop or phone in under five seconds. A shared-desktop mode reproduces the Grok Bot model: I log in to HubSpot, LinkedIn and Google Ads once, every bot can use those sessions on its own screen.

*Story:* "Give the ads bot a fresh desktop, let me log it into Google Ads from my phone, then snapshot it so I never have to log in again."

Shared desktops merge their agents into **one trust domain by design**; attribution inside one is best-effort and isolation claims are void. Isolated per-agent desktops are the default.

## G4 — Recurring work becomes skills; needed tools become Apps

Any automatable task can be captured as a reusable skill (owned by the agent runtime). Any tool the user needs can be built or wrapped as a durable **App**—the user-facing term for an internal `kind: plugin` runtime: isolated, reproducible from its manifest, with its own data snapshots, migrations, export and an MCP interface so agents can use it as tools rather than clicking through its UI. The user describes the outcome in ordinary language; an external agent runtime may clarify and produce a versioned App Brief, but AgentBed independently validates and enforces the resulting capabilities. See [Intent-to-App design](intent-to-app.md).

*Stories:* "I need to track time on this project — build me a time tracker." · "I have no CRM for these leads; build a local one I fully control, or install a good open-source one and connect it."

## G5 — Operate from anywhere, by voice or message *(dependency on agent runtimes, not an Agentbed deliverable)*

The owner operates the machine from voice or messaging apps (Telegram, WhatsApp, SMS, a voice mode). This is delivered **by the agent runtimes**, not by Agentbed; Agentbed's job is to work out of the box under Hermes and OpenClaw so these stories simply work.

*Story:* "Navigate to my LinkedIn, summarise today's top 10 posts for my interests, and send me the draft on Telegram."

## G6 — Bring your own models, local or cloud

The owner chooses models freely: cloud APIs, Claude or ChatGPT subscriptions, or local models (Ollama, llama.cpp) when hardware allows. Agentbed is model-agnostic; it never holds provider keys in plaintext and can route through a local proxy so per-bot keys and budgets live in one place.

## G7 — Self-improvement that is safe

Agents may improve the machine they run on (packages, services, config, Apps) through the transaction engine. App changes remain internal plugin transactions. Self-improvement loops and "dreaming" live in the agent runtimes; Agentbed's contribution is to make self-modification safe: test before apply, probation, automatic rollback, and a ledger.

## G8 — Installs on the Linux you already run

One command on Ubuntu, Fedora or NixOS. The layer updates independently of the OS. Users keep their distro and their update habits.

## G9 — A community library that cannot hurt you *(later ambition; not scheduled before the manifest format stabilizes)*

Skills and Apps should eventually be shareable through a store. Because App packages run as plugin services with data access, the store launches only with signed packages, pinned versions, manifest summaries shown at install time, and review gates for anything requesting egress or secrets. (OpenClaw's skill-marketplace incidents are the cautionary tale.)

## Non-goals

- A new Linux distribution, installer or ISO.
- A chat/voice gateway, memory system or agent loop (Hermes/OpenClaw do this).
- Windows/macOS as *hosts* (they may be *guests* via Cua behind a later gate).
- A hardened boundary against a determined adversary in the first release; the manifests stop honest mistakes and model misbehaviour, not nation-states.
