# Intent-to-App design contract

**Status:** Accepted product/design decision · 2026-08-24 · implementation deferred by the gated roadmap.

This document defines how a non-technical request becomes an AgentBed App without making the user specify MCP, manifests, containers, network policy, backup commands, or export internals. The durable security/runtime invariants remain normative in [ADR-001](adr/ADR-001-agentbed-architecture.md) §6.4; sequencing remains normative in the [roadmap](roadmap.md). This document owns the conversational intake and builder handoff.

## 1. Terms and boundary

- **App** is the user-facing product term: a durable local tool built or wrapped for the owner.
- **Plugin** is the internal `kind: plugin` manifest and runtime term.
- **Designer** is an LLM-backed agent in Hermes, OpenClaw, AgentBed Staff, or another external runtime. It turns plain language into a structured App Brief.
- **Builder** implements or wraps the App outside AgentBed's privileged broker.
- **AgentBed** validates the proposed manifest, computes and enforces capabilities/effects, installs or updates the App transactionally, and owns lifecycle proof.

AgentBed does not need a built-in model. The LLM may propose a product and capabilities; it is never the authority that grants them. Generation, review, and build run outside the broker and without host credentials.

## 2. User promise

A valid starting request is intentionally non-technical:

> Build me a time tracker to log the hours I'm spending on each project.

The user is not expected to mention storage, network access, MCP tool names, JSONL, health checks, migration, rollback, isolation, or backup. Those are platform obligations inferred or injected after the product goal is understood.

The default response should propose a small usable App rather than begin a long interview:

> **Here's what I'll build**
>
> A private time tracker where you can create projects, start and stop a timer, correct entries, see weekly totals, and export your records. It stays on this computer, works without an account, and does not connect to other services.
>
> **Build it** · **Customize**

## 3. Pipeline

```text
plain-language request
    -> conversational Designer
    -> versioned App Brief
    -> plain-language product preview
    -> Builder in an isolated workspace
    -> deterministic tests + independent review
    -> capability proposal
    -> AgentBed manifest/effect compilation
    -> exact capability + artifact approval
    -> governed install/update of that exact artifact
```

The App Brief, not an expanded prompt or chat transcript, is the durable source of truth. Builder prompts are generated artifacts. The brief is inspectable, versioned, and updated when the owner changes the App.

Every App install and update in Gate 4, including every generated App update, has an explicit `requires_approval` operation policy and uses AgentBed's independent approval channel; it is never pre-authorized by the conversational **Build it** action. The operation includes the App Brief digest and immutable artifact/image digest in its canonical arguments. AgentBed compiles the proposed manifest, computes its digest and exact effect set, and renders the final plain-language capability summary. Approval is bound through the existing transaction/operation contract to the canonical operation digest, manifest digest and effect set, thereby also binding those argument digests. A changed build, brief, manifest or capability set invalidates the approval; semantic validity alone never authorizes installation. Automatic signed-store patch policy remains future work outside Gate 4.

## 4. Clarification policy

Ask only when the answer materially changes the durable data model, primary workflow, authority, or definition of success.

| Uncertainty | Behaviour |
|---|---|
| Safe and easily reversible | Choose a sensible default and disclose it in the preview. |
| Product-defining but reversible | Include the assumption and offer **Customize**. |
| Expensive to migrate later | Ask one plain-language question. |
| Exposes data or raises authority | Require an explicit choice before build/install. |

Interaction rules:

1. Ask one question per turn, with 2–4 concrete choices and the recommended choice first.
2. For one intake or owner-triggered **Customize** cycle, ask no more than three product questions before returning a preview; ask nothing further unless the owner explicitly starts another cycle. Always allow “build the simple version.”
3. Ask about user outcomes and workflows, never implementation technology.
4. Do not ask for choices the platform can supply safely and deterministically.
5. Keep technical detail behind optional **Advanced** disclosure.

For the time tracker, reversible defaults are: one owner, local-only data, project names created in the App, timer plus manual entry, editable records, weekly totals, no integrations, and no external actions.

Questions become necessary when the owner asks for team sharing, clients/invoicing, importing existing projects, reading folders automatically, external integrations, sensitive records, public access, automated messages, purchases, or destructive external operations.

## 5. App Brief

The first design spike will seal a versioned schema. At minimum, the brief must represent:

- goal and explicit non-goals;
- intended user or users;
- entities and durable data relationships;
- primary workflows;
- views and reports;
- input methods;
- data sources and integrations;
- sharing/exposure;
- automation and external actions;
- retention and user-facing export;
- accepted assumptions and unresolved decisions;
- concrete acceptance examples.

Illustrative time-tracker brief:

```yaml
app_brief_version: 1
app_id: time-tracker
goal: Track time spent on projects
non_goals:
  - client invoicing
  - team sharing
  - external integrations
users: [owner]
entities:
  project:
    fields: [name, archived]
    relationships: ["has_many:time_entry"]
  time_entry:
    fields: [project_id, started_at, stopped_at, duration, note]
    relationships: ["belongs_to:project"]
workflows:
  - create and archive projects
  - start and stop a timer
  - add and correct time entries
  - view weekly totals by project
views: [today, projects, weekly_summary]
input_methods: [timer, manual_entry]
data_sources: [local_input]
sharing: private
integrations: []
automation: []
retention:
  records: until_owner_deletes
  deletion: explicit_in_app_action
user_exports: [csv]
assumptions:
  - timer and manual entry are both available
unresolved_decisions: []
acceptance_examples:
  - stopping a running timer records its elapsed time
  - editing an entry updates every affected total
  - weekly totals equal the sum of entries in that week
```

The brief contains product intent. A separate capability proposal maps that intent onto the shared manifest language. The runtime may suggest the proposal, but AgentBed's compiler performs shape and semantic validation and defaults missing authority to deny.

## 6. Platform-injected App contract

Every generated or wrapped App receives platform requirements the user should not have to request:

- dedicated plugin identity, storage, runtime directory, and resource quota;
- reproducible build and pinned image digest;
- health check;
- data migration and standard machine-readable export;
- backup, restore, and retention hooks;
- declared UI and MCP surfaces where applicable;
- default-deny filesystem, network, secret, system, and external-effect capabilities;
- transactionally governed installation and updates;
- independent review and acceptance tests.

Domain workflows may derive agent-callable MCP operations. For the reference App, “start a timer,” “stop it,” and “show a report” naturally derive `time.start`, `time.stop`, and `time.report`; the owner never has to name MCP.

The user-facing export may be CSV while the plugin contract also requires JSONL for portable machine recovery. Surviving host rollback, migration correctness, sibling isolation, and exercised restore remain engineering acceptance criteria rather than user prompts.

## 7. Capability changes

The initial time tracker needs dedicated local storage and private UI/MCP exposure, but no external network, secrets, or external effects.

A later request such as “add a note to each time entry” changes the App Brief and database migration without widening authority. It may proceed through the ordinary review and update transaction.

A request such as “import my Google Calendar meetings” changes the capability envelope. The Designer must explain the new access in plain language, offer a narrow read-only choice, and produce a new capability proposal. OAuth credentials stay in an AgentBed connector; the App never receives raw tokens.

A request such as “email each client a weekly report” adds an external effect and must specify recipients, schedule, content, limits, and approval/pre-authorization policy.

**Functional changes may be conversational. Capability widening is always explicit and governed.** Generated code cannot edit its own manifest or acquire authority from instructions found in external data.

## 8. Build and review policy

The Builder should prefer, in order:

1. composing an existing proven AgentBed App;
2. wrapping a mature open-source application behind the plugin contract;
3. generating the smallest App from a blessed template.

The blessed template owns security-sensitive and repetitive plumbing: project layout, local authentication, database initialization, migrations, health, export, backup hooks, MCP adapter, tests, container definition, and manifest skeleton. The LLM primarily supplies domain entities, workflows, views, validation, reports, and acceptance tests.

Generated output is not installed directly. A fresh-context reviewer outside the Builder session receives read-only access to the sealed App Brief, capability proposal, exact artifact/source and raw test outputs; it cannot write the Builder workspace and does not rely on Builder self-reports. It checks the implementation against the App Brief, unnecessary features and capabilities, data/export correctness, migration/restore behaviour, and adversarial boundaries. AgentBed then independently compiles and validates the manifest before the governed transaction.

The Gate 4 reference contract suite is the shared oracle for both the hand-built and generated Apps. It includes App Brief schema validation; brief-to-test traceability; workflow/MCP contract tests; fixture-based totals and edit correctness; migration from the previous fixture; export parse and round-trip into an empty instance; backup/restore comparison; host-rollback survival; health; reproducible build/image binding; and sibling-isolation probes. Gate 4 records this suite and its evidence before generator work begins.

## 9. Gated sequencing

### Through Gate 3

No generator or conversational App Designer is implemented. Gate 1 remains unchanged. Only the already-recorded Staff-readiness constraints—derived identity representability, durable cursor-resumable `agentbed://events`, cheap status, and subscribable approval-pending notifications—are preserved.

### Before Gate 4 implementation

Run a documentation-only design spike that seals:

- App Brief v1 schema;
- clarification/default policy;
- Designer/Builder/AgentBed handoff;
- capability-proposal format;
- one mocked time-tracker conversation and expected brief.

This spike adds no Gate 1–3 runtime exit criteria.

### Gate 4

Build the time tracker conventionally as the known-good reference App. Prove the plugin runtime, dedicated trust domain, migration, export, MCP surface, backup/restore, sibling isolation, and survival across host rollback before asking an LLM to reproduce it.

### After Gate 4

Run the sentence-to-App generator spike using the exact request in §2. It passes only if:

1. the canonical request asks no required questions: the Designer discloses the §4 defaults and offers **Customize**; any customization path asks no technical questions and at most three product questions before returning a revised preview;
2. the owner can approve or customize a plain-language preview;
3. the resulting brief validates against App Brief v1, has no unresolved required field, and maps every acceptance example to an executable test ID;
4. the generated App passes the recorded Gate 4 reference contract suite without a weaker oracle or skipped requirement;
5. the initial manifest requests no network, secrets, or external effects;
6. a conversational field addition preserves existing data;
7. a Calendar request produces a manifest/effect diff for the narrow connector scope and cannot install without a fresh exact-artifact approval;
8. portability is proved from an empty test host: rebuild from the exported source/build recipe and pinned inputs, import the exported data, restore a backup independently, then compare the canonical fixture and acceptance outputs.
