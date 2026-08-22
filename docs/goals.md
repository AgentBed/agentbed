# Goals and user stories

These guide scope decisions. A feature that serves none of them is out of scope. A user story is written from the owner's point of view ("L-P", a solo operator running Hermes Agent on a home server with several specialised bots).

## G1 — Any agent gets a governed computer

Any agent — a Hermes bot on another machine, Claude Code, ChatGPT, a local model — can connect to an Agentbed host over MCP and receive a computer it can fully operate: shell, files, services, packages, a browser and a desktop. What it may touch is declared in a manifest and enforced by the host, not by the agent's good behaviour.

*Story:* "Connect my external Claude Code session to the Agentbed MCP and let it set up a monitoring stack on that machine. It may install packages and manage services; it may not reach the internet except package mirrors, and it may not read my home directory."

## G2 — Everything an agent does is attributable, reviewable, reversible

Every change carries the agent's identity, the manifest version, a diff and an outcome in an append-only ledger. Any change can be rolled back; the host reports honestly how strong that rollback is (generation, snapshot, or none).

*Story:* "Show me everything the LinkedIn bot changed this week, and undo the config change it made on Tuesday."

## G3 — Disposable computers per bot, shared logins when I want them

Each bot can have its own desktop with a browser, persistent profile, snapshot/restore, and a takeover screen I can reach from my laptop or phone in under five seconds. A shared-desktop mode reproduces the Grok Bot model: I log in to HubSpot, LinkedIn and Google Ads once, every bot can use those sessions on its own screen.

*Story:* "Give the ads bot a fresh desktop, let me log it into Google Ads from my phone, then snapshot it so I never have to log in again."

## G4 — Recurring work becomes skills; needed tools become plugins

Any automatable task can be captured as a reusable skill (owned by the agent runtime). Any tool or app the user needs can be built or wrapped as a durable plugin: isolated, reproducible from its manifest, with its own data snapshots, migrations, export and an MCP interface so agents use it as tools rather than clicking through its UI.

*Stories:* "I need to track time on this project — build me a time tracker." · "I have no CRM for these leads; build a local one I fully control, or install a good open-source one and connect it."

## G5 — Operate from anywhere, by voice or message

The owner operates the machine from voice or messaging apps (Telegram, WhatsApp, SMS, a voice mode). This is delivered **by the agent runtimes**, not by Agentbed; Agentbed's job is to work out of the box under Hermes and OpenClaw so these stories simply work.

*Story:* "Navigate to my LinkedIn, summarise today's top 10 posts for my interests, and send me the draft on Telegram."

## G6 — Bring your own models, local or cloud

The owner chooses models freely: cloud APIs, Claude or ChatGPT subscriptions, or local models (Ollama, llama.cpp) when hardware allows. Agentbed is model-agnostic; it never holds provider keys in plaintext and can route through a local proxy so per-bot keys and budgets live in one place.

## G7 — Self-improvement that is safe

Agents may improve the machine they run on (packages, services, config, plugins) through the transaction engine. Self-improvement loops and "dreaming" live in the agent runtimes; Agentbed's contribution is to make self-modification safe: test before apply, probation, automatic rollback, and a ledger.

## G8 — Installs on the Linux you already run

One command on Ubuntu, Fedora or NixOS. The layer updates independently of the OS. Users keep their distro and their update habits.

## G9 — A community library that cannot hurt you

Skills and plugins should eventually be shareable through a store. Because plugins run as services with data access, the store launches only with signed packages, pinned versions, manifest summaries shown at install time, and review gates for anything requesting egress or secrets. (OpenClaw's skill-marketplace incidents are the cautionary tale.)

## Non-goals

- A new Linux distribution, installer or ISO.
- A chat/voice gateway, memory system or agent loop (Hermes/OpenClaw do this).
- Windows/macOS as *hosts* (they may be *guests* via Cua in a later phase).
- A hardened boundary against a determined adversary in the first release; the manifests stop honest mistakes and model misbehaviour, not nation-states.
