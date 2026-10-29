# Landscape of Agents: Control Flows, Processes, and Pipelines

Every control flow of the system, from repository setup to learning across work, with the code that
implements it. Each flow answers the same nine questions:

| Label | Question |
|---|---|
| **Problem** | What is it about, and what problem does it solve? |
| **Lifecycle** | The full lifecycle, from trigger to end state |
| **Steps and why** | The steps in order, and why the architecture is this way |
| **Humans in the loop** | Where a person must act, why, and whether it can be reduced safely |
| **Surfaces** | Which of the CLI, the dashboard, and the extension it influences |
| **Input, result, storage** | What goes in, what comes out, where the data and the audit are kept, and why |
| **Reasoning** | Why the pipeline is built as a pipeline at all |
| **Fixes needed** | Bugs or gaps, with criticality |
| **How to fix safely** | The method that fixes them without breaking the flow |

Criticality: **CRITICAL** (security, privacy, data loss), **HIGH** (silently wrong or incomplete),
**MEDIUM** (misleading, or fails at scale), **LOW** (ergonomics, latent, unwired), **NONE**.

Companion documents: [TERMINOLOGY.md](../TERMINOLOGY.md) defines every term used here;
[PRODUCTS.md](../products/strategy.md) describes the products built on these flows. The F-numbers (F00 to F36)
refer to the event scenarios in [DASHBOARD.md](../../DASHBOARD.md).

## Contents

**Governance**
1. [Repository setup and initialization](#1-repository-setup-and-initialization)
2. [A teammate joins and trusts the owner key](#2-a-teammate-joins-and-trusts-the-owner-key)
3. [A rule change through commit()](#3-a-rule-change-through-commit)
4. [Creating a selection (protect and target)](#4-creating-a-selection-protect-and-target)
5. [Verifying the rules (validate, test, policy status)](#5-verifying-the-rules)
6. [Defining and signing zones and flows](#6-defining-and-signing-zones-and-flows)
7. [Admin management and repository rebind](#7-admin-management-and-repository-rebind)

**Runtime enforcement**
8. [Installing agent hooks](#8-installing-agent-hooks)
9. [Session start](#9-session-start)
10. [The decision before a tool call](#10-the-decision-before-a-tool-call)
11. [The verification after a tool call](#11-the-verification-after-a-tool-call)
12. [Stop and session end](#12-stop-and-session-end)
13. [Autonomy credits](#13-autonomy-credits)

**Evidence and visibility**
14. [Recording an event](#14-recording-an-event)
15. [Alerts and custom rules](#15-alerts-and-custom-rules)
16. [Background sync and upload](#16-background-sync-and-upload)
17. [Service ingest](#17-service-ingest)
18. [Dashboard sign-in and viewing](#18-dashboard-sign-in-and-viewing)
19. [Dashboard Agents, incidents, and remote quarantine](#19-dashboard-agents-incidents-and-remote-quarantine)
20. [Extension operations](#20-extension-operations)

**Learning (the Capability Compiler and Insight)**
21. [A Work Capture investigation](#21-a-work-capture-investigation)
22. [Exploration of a question no checklist serves](#22-exploration-of-a-question-no-checklist-serves)
23. [Replay](#23-replay)
24. [Contributions, outcomes, feedback, and questions](#24-contributions-outcomes-feedback-and-questions)
25. [Capability intake, validation, and publication](#25-capability-intake-validation-and-publication)
26. [Compilation](#26-compilation)
27. [The Validation Lab](#27-the-validation-lab)
28. [Discovery](#28-discovery)
29. [Invocation](#29-invocation)
30. [Evolution](#30-evolution)
31. [The Insight learning loop](#31-the-insight-learning-loop)

**Delivery**
32. [CI, release, and deployment](#32-ci-release-and-deployment)

---

## 1. Repository setup and initialization
Code: [cli/repo.rs](../../src/cli/repo.rs), [github/mod.rs](../../src/github/mod.rs), [cli/governance/init.rs](../../src/cli/governance/init.rs), [governance/state.rs](../../src/governance/state.rs). Events: F01.

- **Problem.** A repository needs a root of trust before any rule means anything: who signs, which
  repository the signatures are bound to, and what the trusted baseline is.
- **Lifecycle.** `crane repo --https|--ssh|--gh-cli REMOTE` connects the clone (and grants read
  privileges) → `crane init` computes the repository identity, creates the signing key in the trust
  directory, creates `.crane/`, signs the first manifest (generation 1), sets the rollback anchor →
  `crane checkpoint NAME` records the first trusted commit.
- **Steps and why.** Identity first, because every later signature binds to it. The key lives outside
  the repository, so an agent editing files cannot read or forge it. The first manifest is signed
  through the same `commit()` as every later change, so there is no special bootstrap path to get
  wrong.
- **Humans in the loop.** A person runs every step; this is the act of taking ownership and cannot be
  delegated to an agent (the agent guard refuses it). It can be shortened (the extension's setup
  walks through it) but not removed.
- **Surfaces.** CLI; the extension's setup ([core/setup.ts](../../extensions/foxx/src/core/setup.ts)); the
  dashboard sees the repository after the first upload proves its remote.
- **Input, result, storage.** Input: a remote and a working tree. Result: `.crane/` (committed) and
  keys plus anchor in `~/.crane-trust/` (never committed). The first governance audit entry is in
  `.crane/audit.jsonl`; an F01 event is in the runtime event store.
- **Reasoning.** Trust is established once, locally, and everything else is derived from it.
- **Fixes needed.** Key protection is not passphrase-based by default outside Windows (**MEDIUM**).
- **How to fix safely.** Default `CRANE_KEY_PROTECTION=prompt` for new keys only; existing keys keep
  working, and `crane init` offers to reseal.

## 2. A teammate joins and trusts the owner key
Code: [cli/governance/init.rs](../../src/cli/governance/init.rs), [cli/governance/admin.rs](../../src/cli/governance/admin.rs), [server/dashboard/approve.rs](../../src/server/dashboard/approve.rs). Events: F02.

- **Problem.** A clone contains signed rules, but a new machine cannot know whose signature to trust.
- **Lifecycle.** Teammate runs `crane init` → Crane shows the owner key fingerprint → the teammate
  compares it (with `crane admin list` on the owner's machine, or the dashboard's "confirmed by @admin"
  after `crane admin register`) → confirms → the key is pinned and rules verify. Until then, agent
  changes are blocked. CI trusts the owner through `CRANE_TRUSTED_PUBLIC_KEYS`.
- **Steps and why.** Trust on first use with an out-of-band check: no central authority is required,
  and GitHub can vouch when present.
- **Humans in the loop.** The teammate confirms the fingerprint. It can be mitigated with the GitHub
  confirmation (already present): the dashboard shows a GitHub admin vouched for the key.
- **Surfaces.** CLI, extension (setup walk-through), dashboard (owner-key confirmation).
- **Input, result, storage.** Input: the clone. Result: a pinned key in the trust directory. Audit: F02
  events locally; the confirmation request on the service.
- **Reasoning.** Pinning per machine keeps enforcement offline.
- **Fixes needed.** None known (**NONE**).
- **How to fix safely.** —

## 3. A rule change through commit()
Code: [governance/state.rs](../../src/governance/state.rs), [governance/audit.rs](../../src/governance/audit.rs).

- **Problem.** Every rule change must be authorized, recorded, and tamper-evident.
- **Lifecycle.** Verify current state → apply the change → append an audit entry → sign a new manifest
  (generation + 1) → update the rollback anchor. On failure nothing is written.
- **Steps and why.** Verifying first means nothing is ever signed on top of tampered rules. One function
  means one signing path.
- **Humans in the loop.** A person runs the command that calls `commit()` and holds the key (with
  prompt protection, types the passphrase). Cannot be removed without breaking "only people change the
  rules".
- **Surfaces.** CLI and extension (every signed change goes through it).
- **Input, result, storage.** Input: a change. Result: a new signed manifest in `.crane/`, an audit
  entry in `.crane/audit.jsonl`, the anchor in the trust directory.
- **Reasoning.** A single door is the cheapest way to guarantee consistent signing.
- **Fixes needed.** Concurrent changes on two branches conflict in Git and need manual resolution
  (**LOW**).
- **How to fix safely.** A `crane governance merge` that verifies both signed sides, keeps one, and
  re-applies the other's change set through `commit()`; nothing changes for single-branch teams.

## 4. Creating a selection (protect and target)
Code: [selection/create.rs](../../src/selection/create.rs), [selection/anchors.rs](../../src/selection/anchors.rs), [governance/diff.rs](../../src/governance/diff.rs).

- **Problem.** People need to say "this block must not change" or "this block must be rewritten this
  way", precisely and durably.
- **Lifecycle.** `crane protect|target FILE start-line N end-line M context TEXT …` → resolve the range
  against the checkpoint (every line unchanged since then) → generate an identifier → insert two anchor
  comments → append `preserve`/`target` to the policy → build and sign the selection record through
  `commit()` → on failure restore the files.
- **Steps and why.** Line numbers are used once, then anchors carry the location. Requiring the lines
  to be unchanged since the checkpoint prevents protecting already-tampered code.
- **Humans in the loop.** A person chooses what to protect; the extension makes this a selection and a
  click. Insight could *propose* selections (for example, code repeatedly involved in failures), but a
  person must still create them.
- **Surfaces.** CLI, extension panel.
- **Input, result, storage.** Input: file, range, context. Result: anchors in the source file, a policy
  line, a signed record in `.crane/registry.json`, an audit entry.
- **Reasoning.** Durable precision without maintenance.
- **Fixes needed.** Large refactors can leave selections UNRESOLVED (fails safe) (**LOW**).
- **How to fix safely.** A repair assistant that proposes the new location and signs it only on a
  person's confirmation.

## 5. Verifying the rules
Code: [governance/verify.rs](../../src/governance/verify.rs), [cli/governance/checks.rs](../../src/cli/governance/checks.rs).

- **Problem.** Every surface needs the same answer to "do the rules hold right now?".
- **Lifecycle.** Verify the signed state → check policies against the registry → resolve every
  selection from the working tree → judge each command from file content against the baseline →
  aggregate per policy. `crane test .` adds smoke tests with time limits and an enforcement self-test.
- **Steps and why.** Never trust one source: signatures, registry, anchors, and content are checked
  against each other. Unresolved fails closed.
- **Humans in the loop.** None to run it; a person fixes failures. Fully automatic.
- **Surfaces.** CLI (`validate`, `test`, `policy NAME status`), extension diagnostics and sidebar,
  hooks (every decision), repository status upload (dashboard), Work Capture's Crane connector.
- **Input, result, storage.** Input: `.crane/` and the working tree. Result: a validation report (JSON
  with `--json`). Not stored except as events and as Work Capture evidence.
- **Reasoning.** One verifier keeps CLI, editor, hooks, and dashboard consistent.
- **Fixes needed.** Can be slow on large repositories and is not interruptible (**MEDIUM** at scale).
- **How to fix safely.** Cache results keyed by the manifest digest plus a working-tree status digest;
  the cache is invalidated by any change, so results stay identical.

## 6. Defining and signing zones and flows
Code: [governance/zones.rs](../../src/governance/zones.rs), [governance/flows.rs](../../src/governance/flows.rs), [governance/graph.rs](../../src/governance/graph.rs), [cli/governance/zones.rs](../../src/cli/governance/zones.rs), [core/governance.ts](../../extensions/foxx/src/core/governance.ts).

- **Problem.** Risk belongs to areas and execution paths, not only to blocks.
- **Lifecycle.** An admin edits `.crane/governance.yaml` (or draws in the extension; new entities start
  `proposed`) → `crane governance sign` validates markers, discovers flows, requires context for every
  flow, records marker blocks, and signs → entities set `active` are enforced. Unsigned edits fail
  verification and stop agent changes.
- **Steps and why.** Unsigned edits fail closed, so nobody can loosen a zone unnoticed. Flow discovery
  marks every edge inferred and never fabricates edges.
- **Humans in the loop.** An admin defines and signs. Insight can propose context for zones; the
  boundaries themselves stay human decisions.
- **Surfaces.** CLI, extension (graph view, governance editing), dashboard (governance view).
- **Input, result, storage.** Input: YAML edits. Result: signed `governance.yaml` under the manifest,
  an audit entry. The graph is derived on demand.
- **Reasoning.** Restrictions only add up, so composition is safe.
- **Fixes needed.** Dynamic calls and unsupported languages are not followed (**LOW**, reported).
- **How to fix safely.** Allow explicitly `declared` edges beside inferred ones; add analyzers per
  language without changing existing edges.

## 7. Admin management and repository rebind
Code: [governance/admins.rs](../../src/governance/admins.rs), [governance/rebind.rs](../../src/governance/rebind.rs).

- **Problem.** Teams change; repositories move.
- **Lifecycle.** `crane admin add|del PUBLIC_KEY` signs a new admin statement with a higher epoch;
  machines reject older lists. `crane repo rebind OLD_REMOTE` proves the move and re-signs for the new
  identity.
- **Steps and why.** Epochs prevent a removed admin from restoring a list that names them.
- **Humans in the loop.** The owner or an admin. Necessary.
- **Surfaces.** CLI; dashboard shows admins.
- **Input, result, storage.** Signed statement in `.crane/config.json` under the manifest; audit entry.
- **Reasoning.** A small signed authority model instead of a user database.
- **Fixes needed.** Per-repository admin lists for multi-repository organizations (**LOW**).
- **How to fix safely.** An organization statement referenced by repositories; repositories without
  it behave as today.

## 8. Installing agent hooks
Code: [hooks/install.rs](../../src/hooks/install.rs), [cli/agent.rs](../../src/cli/agent.rs).

- **Problem.** Enforcement must run inside the agent tool, on every action.
- **Lifecycle.** `crane agent install --profile claude|codex|copilot|gemini` writes the tool's hook
  configuration to call `crane agent hook --event … --profile …`; `uninstall` removes it (and cannot be
  run by an agent).
- **Steps and why.** Hooks are the only interception point the tools offer; the hook settings are
  protected files the guard refuses to let agents edit.
- **Humans in the loop.** A person installs once per machine and tool. Could be automated by
  `crane init` offering installation.
- **Surfaces.** CLI; the extension can trigger it.
- **Input, result, storage.** The tool's settings file; an F13 command event.
- **Reasoning.** Enforcement that does not depend on the agent's cooperation.
- **Fixes needed.** A machine without hooks is unguarded and only shows up as missing evidence
  (**MEDIUM**).
- **How to fix safely.** Report hook installation status in the repository status upload and on
  `crane test .`; no change to enforcement.

## 9. Session start
Code: [hooks/runtime/session.rs](../../src/hooks/runtime/session.rs). Events: F03.

- **Problem.** A session must start from a verified state with a budget and the right guidance.
- **Lifecycle.** `SESSION_START` hook → verify rules → attest the starting state → issue identifiers →
  grant credits (default 100, 8 hours) → return policy and zone context to the agent → record F03.
- **Steps and why.** Verification at start means a broken rule set is noticed before the first tool
  call. Context at start reduces wasted tokens.
- **Humans in the loop.** None.
- **Surfaces.** Agent tool, extension (live context), dashboard (session view).
- **Input, result, storage.** Input: the hook payload. Result: session record, ledger grant, F03 event
  in the runtime store.
- **Reasoning.** Everything later in the session references this attested start.
- **Fixes needed.** Context does not yet include reviewed, learned context (**LOW**).
- **How to fix safely.** Append accepted reusable contributions for governed zones, labelled as
  reviewed context; unchanged when there are none.

## 10. The decision before a tool call
Code: [hooks/enforce.rs](../../src/hooks/enforce.rs), [hooks/shell.rs](../../src/hooks/shell.rs), [hooks/scripts.rs](../../src/hooks/scripts.rs), [hooks/adapter/](../../src/hooks/adapter). Events: F05.

- **Problem.** Decide, from the real state, whether a proposed action may run.
- **Lifecycle.** `PRE_TOOL_USE` → adapter builds a provider-neutral action → guard checks, in order:
  integrity, safety state (and remote quarantines), protected files, shell analysis, script pre-check,
  edit simulation, zone and flow limits, autonomy mode, credits → answer allow, ask, or deny with a
  reason and context → decision event; credits reserved.
- **Steps and why.** Cheap, decisive checks first; any error denies (`HOOK_ERROR`). Edits are simulated
  on the file's current text, never judged from the agent's description. Actions whose effects cannot
  be known in advance are allowed so the after-check can judge them.
- **Humans in the loop.** "Ask a human" (assisted ceilings, restricted code, degraded sessions, empty
  credits). Can be reduced safely by Insight trust patterns proposing a higher ceiling for zones with a
  clean record, decided by the owner, never automatically.
- **Surfaces.** Agent tool (the answer), extension (live context), dashboard (decisions, denials).
- **Input, result, storage.** Input: the hook payload and the working tree. Result: a decision. Stored
  as an F05 event and a ledger reservation in the runtime store, uploaded later.
- **Reasoning.** Enforcement must be before the effect where possible, and honest where not.
- **Fixes needed.** Lexical shell and script analysis cannot see through indirection (**MEDIUM**; the
  after-check catches effects). Remote quarantine check fails closed on a service outage (**MEDIUM**).
- **How to fix safely.** Keep post-action verification as the backstop and add an optional sandbox
  profile. Cache a signed "no quarantine" answer with an expiry so outages degrade to the last known
  state for a bounded time.

## 11. The verification after a tool call
Code: [hooks/runtime/tools.rs](../../src/hooks/runtime/tools.rs). Events: F06.

- **Problem.** Know what actually changed, regardless of what the agent claims.
- **Lifecycle.** `POST_TOOL_USE` → pair with the decision (or record a coverage gap) → consume or
  release credits → attribute changed files (working-tree diff) → re-check protected blocks → a file
  edit violation degrades the session and revokes credits; a shell or other-tool change to protected
  code is a confirmed bypass and quarantines the session → tell the agent to repair.
- **Steps and why.** The after-check is what makes allowing unknowable actions safe.
- **Humans in the loop.** A person repairs or restores after a violation; releasing a quarantine needs
  an admin. Necessary for containment.
- **Surfaces.** Agent tool, dashboard (violations, bypasses, quarantines), alerts.
- **Input, result, storage.** F06 events, ledger entries, safety state in the runtime store.
- **Reasoning.** Decide from the real state, before and after.
- **Fixes needed.** None known (**NONE**).
- **How to fix safely.** —

## 12. Stop and session end
Code: [hooks/runtime/session.rs](../../src/hooks/runtime/session.rs). Events: F07, F08, F10.

- **Problem.** Close the session's books and prove its end state.
- **Lifecycle.** `STOP` reconciles every change of the session against the rules → `SESSION_END`
  attests the final state and expires unused credits → a session with no activity for 120 minutes and
  no signed end is flagged stale on the service.
- **Steps and why.** Reconciliation catches anything individual verifications missed.
- **Humans in the loop.** None.
- **Surfaces.** Dashboard (sessions, stale sessions).
- **Input, result, storage.** F07/F08 events and ledger expiry locally; F10 stale flag on the service.
- **Reasoning.** A session's evidence must have a defined end.
- **Fixes needed.** Stale detection is late by design (**LOW**).
- **How to fix safely.** Report missing hooks directly (see flow 8).

## 13. Autonomy credits
Code: [telemetry/ledger.rs](../../src/telemetry/ledger.rs), [governance/config.rs](../../src/governance/config.rs), [server/dashboard/approve.rs](../../src/server/dashboard/approve.rs). Events: F27.

- **Problem.** Meter risk-bearing authority separately from tokens and money.
- **Lifecycle.** Grant at session start → reserve on an allowed action → consume when it ran, release
  when it failed → revoke on violation → expire at end; dashboard approvals can grant more.
- **Steps and why.** An empty balance never denies, it only asks a human, so credits cannot cause
  silent failures.
- **Humans in the loop.** Approving actions when credits run out; granting credits on the dashboard.
  Can be reduced by tuning costs per zone from trust patterns (owner decision).
- **Surfaces.** Agent tool, dashboard (Operate, credits).
- **Input, result, storage.** Hash-chained ledger in the runtime store, uploaded.
- **Reasoning.** A budget managers understand.
- **Fixes needed.** Costs are fixed defaults (**LOW**).
- **How to fix safely.** Insight proposes cost changes as configuration diffs signed by an admin.

## 14. Recording an event
Code: [telemetry/events.rs](../../src/telemetry/events.rs), [telemetry/spec.rs](../../src/telemetry/spec.rs), [telemetry/redact.rs](../../src/telemetry/redact.rs).

- **Problem.** Every decision must become tamper-evident, privacy-safe evidence.
- **Lifecycle.** Build the event → redact → project into the specification form → deduplicate by
  idempotency key → chain to the previous digest → append.
- **Steps and why.** Redaction before writing means secrets never touch disk; projection at append
  time means the spec view is covered by the digest.
- **Humans in the loop.** None.
- **Surfaces.** Everything downstream: dashboard, alerts, Work Capture's DataSea connector.
- **Input, result, storage.** `runtime/events.jsonl` in the trust directory.
- **Reasoning.** One evidence format for every consumer.
- **Fixes needed.** Pattern-based redaction misses unfamiliar secret formats (**MEDIUM**).
- **How to fix safely.** Add entropy-based detection as an additional pass; existing redactions are
  unchanged.

## 15. Alerts and custom rules
Code: [telemetry/alerts.rs](../../src/telemetry/alerts.rs), [telemetry/rules.rs](../../src/telemetry/rules.rs), [cli/alert.rs](../../src/cli/alert.rs). Events: F14.

- **Problem.** People must hear about incidents without watching a dashboard.
- **Lifecycle.** Built-in alerts trigger from events with deduplication and cooldown; custom rules
  (`crane alert add`) are evaluated by the background sync: ok → firing → reminding → resolved.
  Delivery by detached `curl` reading from stdin.
- **Steps and why.** Delivery never changes a decision and never exposes credentials.
- **Humans in the loop.** People define rules and respond. Could be seeded with recommended rules per
  zone criticality.
- **Surfaces.** CLI, Slack, WhatsApp, Jira; dashboard shows alerts.
- **Input, result, storage.** Rules in the trust directory; alert events in the runtime store.
- **Reasoning.** Alerts belong next to the evidence that triggers them.
- **Fixes needed.** Machine-local evaluation stops when the machine is off (**LOW**; Dashboard Agents
  cover the team view).
- **How to fix safely.** Document which to use; optionally mirror local rules as Dashboard Agents.

## 16. Background sync and upload
Code: [telemetry/sync.rs](../../src/telemetry/sync.rs), [telemetry/ingest.rs](../../src/telemetry/ingest.rs), [foxx/status.rs](../../src/foxx/status.rs). Events: F09.

- **Problem.** Team visibility without slowing or weakening enforcement.
- **Lifecycle.** A detached `crane agent sync` asks the service how far each stream reached → sends
  newer records in signed batches (events, ledger, governance changes, alerts, sessions, tasks,
  selection records, manifest, repository status every two minutes) → retries refused uploads.
- **Steps and why.** Detached, so hooks never wait for the network.
- **Humans in the loop.** None (`crane agent link` once to attribute a machine to a person).
- **Surfaces.** Dashboard.
- **Input, result, storage.** Local stores → MongoDB; sync state in the trust directory.
- **Reasoning.** Local evidence is authoritative; the service is for visibility.
- **Fixes needed.** Default ingest URL still on the Render host (**LOW**). Work Capture packs and
  Insight reports are not uploaded (**LOW**).
- **How to fix safely.** Switch the default once the domain is live with the old host as fallback; add
  new record kinds as new streams so old clients are unaffected.

## 17. Service ingest
Code: [server/ingest.rs](../../src/server/ingest.rs), [server/limits.rs](../../src/server/limits.rs), [server/store.rs](../../src/server/store.rs), [server/database.rs](../../src/server/database.rs).

- **Problem.** Accept evidence from many machines without trusting any of them more than itself.
- **Lifecycle.** Verify signature, timestamp, and limits → verify the upload continues the machine's
  event chain (critical alert on rewrite, gap, or fork) → store tagged with the installation → evaluate
  Dashboard Agents → expire records after retention.
- **Steps and why.** The public key is the identity; chain continuity makes tampering visible across the
  network boundary.
- **Humans in the loop.** None.
- **Surfaces.** Dashboard.
- **Input, result, storage.** MongoDB `foxx` collections; service events.
- **Reasoning.** A minimal-trust collector.
- **Fixes needed.** Thirty-day retention is short for annual audits (**MEDIUM**); one deployment
  topology (**MEDIUM**).
- **How to fix safely.** Retention tiers with export to cold storage; a documented self-host profile
  using the same binary.

## 18. Dashboard sign-in and viewing
Code: [server/dashboard/auth.rs](../../src/server/dashboard/auth.rs), [server/dashboard/data.rs](../../src/server/dashboard/data.rs), [server/dashboard/api/](../../src/server/dashboard/api). Events: F00.

- **Problem.** Show a team its evidence, with GitHub-native permissions and no write path.
- **Lifecycle.** GitHub OAuth (`read:org`) → signed eight-hour cookie → visible repositories are those
  whose verified remote owner is the viewer or their organization → views read through a read-only
  handle, merging members' records after verifying each chain.
- **Steps and why.** Visibility follows GitHub ownership; viewing can never change evidence.
- **Humans in the loop.** Sign-in. None beyond.
- **Surfaces.** Dashboard; `GET /api/agent-context` and CSV for tools.
- **Input, result, storage.** Reads MongoDB; writes only the session cookie.
- **Reasoning.** Zero user administration for GitHub teams.
- **Fixes needed.** GitHub only (**MEDIUM** product gap).
- **How to fix safely.** Add providers behind the same "verified remote owner" rule.

## 19. Dashboard Agents, incidents, and remote quarantine
Code: [server/dashboard/api/operate.rs](../../src/server/dashboard/api/operate.rs), [server/dashboard/approve.rs](../../src/server/dashboard/approve.rs), [server/dashboard/notifications.rs](../../src/server/dashboard/notifications.rs), [hooks/enforce.rs](../../src/hooks/enforce.rs).

- **Problem.** Team-level monitoring with a remote stop.
- **Lifecycle.** A person configures a monitor → evaluated after each upload and in a 60-second sweep
  (with a repository lease) → threshold crossed opens an incident idempotently → a person annotates,
  mitigates, requests a quarantine, or proposes flow context → a fresh GitHub admin check activates a
  quarantine or makes context visible → hooks deny matching tool calls → release follows the same check.
- **Steps and why.** Fresh GitHub checks on the dangerous actions; the lease prevents duplicates.
- **Humans in the loop.** Configuring monitors; activating and releasing quarantines. A quarantine
  should stay human; monitors could be suggested from Insight patterns.
- **Surfaces.** Dashboard, hooks, Slack and email notifications.
- **Input, result, storage.** Monitors, incidents, and approvals in MongoDB; local hooks query
  quarantines.
- **Reasoning.** A kill switch that does not need the agent's machine.
- **Fixes needed.** Outage behavior (see flow 10) (**MEDIUM**). Monitors cannot watch Work Capture or
  Insight patterns (**LOW**).
- **How to fix safely.** Cached signed answers; upload insight summaries as a new stream and add
  monitor metrics over them.

## 20. Extension operations
Code: [extensions/foxx/src/](../../extensions/foxx/src) (`panel.ts`, `sidebar.ts`, `graphView.ts`, `core/*.ts`). Events: F31.

- **Problem.** Governance where developers work, without a second source of truth.
- **Lifecycle.** The extension runs the CLI (`crane validate --json`, `crane test . --json`, `crane
  protect`, …), shows an explicit plan before every action, runs it with a timeline, offers undo, and
  shows live agent context from [src/foxx/live.rs](../../src/foxx/live.rs).
- **Steps and why.** Only the CLI, never keys or secrets; disabled in untrusted workspaces because it
  runs the CLI.
- **Humans in the loop.** Every action is a person's click; that is the point.
- **Surfaces.** Extension; it changes what the CLI writes.
- **Input, result, storage.** Whatever the CLI writes; F31 events.
- **Reasoning.** The CLI is the single source of truth.
- **Fixes needed.** No Work Capture, capability, or Insight views (**LOW**); VS Code family only
  (**LOW**).
- **How to fix safely.** New read-only views over `--json` outputs; no new write paths.

## 21. A Work Capture investigation
Code: [capture/broker/run.rs](../../src/capture/broker/run.rs), [capture/broker/recipe.rs](../../src/capture/broker/recipe.rs), [capture/store.rs](../../src/capture/store.rs), [cli/capture.rs](../../src/cli/capture.rs). Full chapter: [WORK_CAPTURE.md](work-capture.md).

- **Problem.** Answer "what happened with this work, what is proven, what is not, and why" with an
  honest, bounded, authorized pack.
- **Lifecycle.** Choose a checklist (plan, explicit, class) → compute the budget → resolve scope and
  authorization → reject explicit forbidden requests → open the episode → resolve zone and flow versions
  → run each step (budget, branch, type, environment, resources by priority, call with deadline and
  paging, per-record authorization, ingest, link, learn) → stop early when satisfied (mandatory steps
  still run) → link governance changes → derive assertions → compute missing evidence with reasons →
  select, build the view, warn, seal → store pack and audit.
- **Steps and why.** Authorization before retrieval and on every record; checklists decide retrieval, not
  the asker; gaps carry reasons so each routes to the right person.
- **Humans in the loop.** Choosing the question (class, task, commit) and acting on gaps. Can be reduced
  by triggering investigations automatically on pull requests and deployments.
- **Surfaces.** CLI (`crane capture …`); feeds the capability pipeline and Insight; an F13 command
  event carries the summary to the dashboard.
- **Input, result, storage.** Input: an `EvidenceQueryRequest` and an `AccessPolicy`. Result: an
  Evidence Pack and a query audit record in `runtime/capture/*.jsonl` (outside the repository, so
  agents cannot forge evidence).
- **Reasoning.** Retrieval as a governed pipeline makes every answer reproducible and explainable.
- **Fixes needed.** Several known commits: steps check the first and warn (**LOW**). No live
  deployment, log, or metric connectors (**MEDIUM**). Store save reads and merges everything
  (**MEDIUM** at scale).
- **How to fix safely.** Per-commit runs behind a checklist flag in a new checklist version; new connectors
  through the contract harness; append-only store segments with the existing merge semantics.

## 22. Exploration of a question no checklist serves
Code: [capture/broker/run.rs](../../src/capture/broker/run.rs) (`explore`), [capture/broker/recipe.rs](../../src/capture/broker/recipe.rs) (`exploration`).

- **Problem.** A new codebase has no checklists for most questions; refusing them means the system never
  learns.
- **Lifecycle.** No checklist for the class (and no checklist or plan named) → resolve scope → for each
  permitted, querying source: discover what it holds → keep the evidence types the actor may receive
  (dropping types a complete listing lacks) → one query step per resource type and evidence type,
  filtered by known scope values → validate as a checklist (`explore-…`) → run like any checklist → pack
  `exploratory` with an Uncertainty warning. No permitted source: an audited refusal.
- **Steps and why.** Derived from the registry, not from the words of the question, so it is
  deterministic and auditable. It requires nothing, so it never claims sufficiency.
- **Humans in the loop.** Judging the result (`crane capture feedback`). That judgement is the learning
  signal; it should stay human.
- **Surfaces.** CLI; Insight.
- **Input, result, storage.** As flow 21, plus an `exploration` record in the audit (sources, notes).
- **Reasoning.** Cold start without guessing.
- **Fixes needed.** Incomplete discovery listings keep every declared type (**LOW**).
- **How to fix safely.** Optional per-type counts in discovery; connectors that do not provide them
  behave as today.

## 23. Replay
Code: [capture/broker/run.rs](../../src/capture/broker/run.rs) (`replay`, `compare`).

- **Problem.** Would we conclude the same today?
- **Lifecycle.** Load the original audit → refuse if the checklist digest changed → re-run with current
  permissions in the original's episode → compare steps in both directions (state, snapshot, evidence
  sets) → `exact`, `diverged`, or `unverifiable` stored on the replay's audit.
- **Steps and why.** The exact checklist, not the current version, so a difference means the world
  changed.
- **Humans in the loop.** Deciding what a divergence means.
- **Surfaces.** CLI (`crane capture replay`).
- **Input, result, storage.** A new pack and audit with a comparison, in the capture store.
- **Reasoning.** Drift detection for evidence.
- **Fixes needed.** None known (**NONE**).
- **How to fix safely.** —

## 24. Contributions, outcomes, feedback, and questions
Code: [capture/ingest/contributions.rs](../../src/capture/ingest/contributions.rs), [capture/ingest/episode.rs](../../src/capture/ingest/episode.rs), [capture/store.rs](../../src/capture/store.rs).

- **Problem.** People (and agents, only as proposals) need to add knowledge, record outcomes, judge
  packs, and ask for evidence, without anyone rewriting trusted memory.
- **Lifecycle.**
  - Contribution: propose (namespace by actor kind, evidence required, policy weakening refused,
    target must be observed) → review by a different authorized human → accepted (`human-approved`) or
    rejected → superseded or expired.
  - Outcome: record with evidence of the episode → changing another's outcome needs review rights →
    the old outcome kept in history.
  - Feedback: a person records sufficient yes/no/unknown, missing types, and a note; kept beside earlier
    feedback.
  - Question: ask (open) → blocked, reopened, unresolvable, or answered by a kept pack.
- **Steps and why.** Validation before any graph change; review rights from the machine's owner or
  admin key, never from a claim.
- **Humans in the loop.** Reviewing contributions, recording outcomes, giving feedback. Outcomes can be
  partly automated: propose an outcome from verified assertions for a person to confirm.
- **Surfaces.** CLI (`contribute`, `review`, `outcome`, `feedback`, `ask`, `question`, `confirm`).
- **Input, result, storage.** Contributions, episodes, questions in the capture store.
- **Reasoning.** Learning gated by people.
- **Fixes needed.** Outcomes recorded by hand (**MEDIUM**, limits Insight). Accepted reusable context
  not yet given to agents at session start (**LOW**).
- **How to fix safely.** A proposed-outcome command that only suggests; session-start context appends
  reviewed context only.

## 25. Capability intake, validation, and publication
Code: [capability/intake.rs](../../src/capability/intake.rs), [capability/validate.rs](../../src/capability/validate.rs), [capability/lifecycle.rs](../../src/capability/lifecycle.rs), [capability/registry.rs](../../src/capability/registry.rs), [cli/capability.rs](../../src/cli/capability.rs).

- **Problem.** Turn evidence into a governed, versioned procedure only when it meets a known bar.
- **Lifecycle.** `crane capability intake episode … pack … digest …` recomputes the pack manifest,
  resolves references under the submitter's access, compares governance versions → a candidate is
  created or proposed → six gates validate under a profile → a Publisher who is not the author
  publishes → deprecate or revoke later; `capability stale` removes stale validations from
  eligibility.
- **Steps and why.** Technical evidence never verifies a business invariant; publication is a separate
  human act.
- **Humans in the loop.** Validation decisions and publication. Must stay human for publication.
- **Surfaces.** CLI; discovery and invocation consume the registry.
- **Input, result, storage.** A pack reference; capability versions with append-only history in the
  runtime capability store.
- **Reasoning.** A registry is only useful if what is in it can be trusted.
- **Fixes needed.** Staleness checked on command only (**LOW**).
- **How to fix safely.** Run the stale check in the background sync; it only marks, never deletes.

## 26. Compilation
Code: [compiler/](../../src/compiler), [cli/compile.rs](../../src/cli/compile.rs).

- **Problem.** Writing capabilities by hand does not scale; real work should become a draft.
- **Lifecycle.** `crane compile episode … pack … digest …` → intake → reconstruct actions, decisions,
  observations, outcomes → extract candidate steps, inputs, outputs, side effects (model proposals only
  as labelled hypotheses) → context (environments, prerequisites) → verify claims against evidence →
  assemble the definition, dry-run the gates, store as a candidate with a compilation report.
- **Steps and why.** Every statement keeps its basis; chronology is never causation.
- **Humans in the loop.** Naming the capability and its purpose; later validation and publication.
- **Surfaces.** CLI.
- **Input, result, storage.** An episode and pack; a candidate (registry) and a compilation report
  (compiler store).
- **Reasoning.** Separates "what happened" from "what is reusable".
- **Fixes needed.** One episode at a time (**MEDIUM**): recurring work (an Insight work pattern) cannot
  be compiled jointly.
- **How to fix safely.** A new multi-episode mode keeping only steps observed in every cited episode;
  the single-episode mode is unchanged.

## 27. The Validation Lab
Code: [capability_lab/](../../src/capability_lab), [cli/lab.rs](../../src/cli/lab.rs).

- **Problem.** Confidence in a candidate before anyone publishes it, without executing it.
- **Lifecycle.** `crane lab run ID [version N] plan FILE` → preflight (digest, state, report, blocking
  gaps) → conformance (each step re-derived from its record) → scenarios from an approved plan →
  invariants → a digested lab report.
- **Steps and why.** Non-executing, deterministic; a lab report changes no lifecycle state.
- **Humans in the loop.** Approving the test plan; reading results.
- **Surfaces.** CLI; discovery reads lab reports.
- **Input, result, storage.** A candidate and a test plan; an append-only lab report store.
- **Reasoning.** Separation of checking from deciding.
- **Fixes needed.** Dynamic behavior stays inconclusive (**LOW**, by design).
- **How to fix safely.** Optional sandboxed dry runs reported in a separate section.

## 28. Discovery
Code: [discovery/](../../src/discovery), [cli/discover.rs](../../src/cli/discover.rs).

- **Problem.** Offer an agent only the capabilities that apply to its task, with reasons.
- **Lifecycle.** Resolve the task's context (optionally from a pack, integrity-checked, under the
  requester's access) → candidates from registry queries → gates in order → rank survivors → a digested
  discovery report.
- **Steps and why.** Unknown is never a match; no fuzzy matching; every exclusion is explained.
- **Humans in the loop.** None to run; a person or agent reads the recommendation.
- **Surfaces.** CLI; could feed session-start context.
- **Input, result, storage.** A task class and context; an append-only discovery report store.
- **Reasoning.** Safety before relevance.
- **Fixes needed.** Recall limited by exact task classes (**LOW**). Not yet offered to agents at session
  start (**LOW**).
- **How to fix safely.** Model-proposed class mappings as hypotheses for a person to accept; append
  recommended capabilities to session context as references, not instructions.

## 29. Invocation
Code: [invocation/](../../src/invocation), [cli/invoke.rs](../../src/cli/invoke.rs).

- **Problem.** Act on a published capability without running arbitrary commands or model output.
- **Lifecycle.** Plan from the published definition and a reviewed binding set → preflight rechecks
  everything now → approval by another person bound to the plan digest (for risky or state-changing
  operations) → execute registered, typed operations → verify postconditions with independent evidence
  → reconcile uncertain effects → `succeeded` only when required postconditions are verified → the
  outcome is handed to the registry.
- **Steps and why.** Fail closed whenever safety cannot be established.
- **Humans in the loop.** Approving plans; reconciling uncertain effects. Low-risk read-only operations
  already skip approval.
- **Surfaces.** CLI.
- **Input, result, storage.** A capability version, bindings, inputs; a locked, atomically written
  invocation ledger.
- **Reasoning.** Reuse without blind trust.
- **Fixes needed.** Only a read-only `git.commit` adapter exists (**HIGH** product gap).
- **How to fix safely.** Add typed adapters one at a time, each with effect classes, idempotency, and
  reconciliation tests; existing plans are unaffected.

## 30. Evolution
Code: [evolution/](../../src/evolution), [cli/evolution.rs](../../src/cli/evolution.rs).

- **Problem.** Capabilities must improve, or be narrowed, as outcomes accumulate.
- **Lifecycle.** Read invocation outcomes → classify and collapse retries → partition by context →
  detect and qualify signals → propose a minimal typed change set as a new candidate version →
  regression plan and run against the parent → review packet (never delivered automatically, never
  published).
- **Steps and why.** Deterministic qualification (3 or 5 trials, 75% failures, no contradictions);
  allowed changes only narrow or document.
- **Humans in the loop.** A Validator decides; a Publisher publishes. Must stay human.
- **Surfaces.** CLI.
- **Input, result, storage.** Invocation ledger (read only); evolution store.
- **Reasoning.** Learning from use, gated by people.
- **Fixes needed.** Thin live outcome history; no review delivery; `submitted_for_review` not produced
  (**MEDIUM**).
- **How to fix safely.** Deliver review packets through the dashboard and integrations and record the
  delivery as `submitted_for_review`; nothing else changes.

## 31. The Insight learning loop
Code: [insight/](../../src/insight), [cli/insight.rs](../../src/cli/insight.rs), [capture/broker/proposals.rs](../../src/capture/broker/proposals.rs).

- **Problem.** The system answered questions but never learned from them.
- **Lifecycle.** `crane insight report` builds the read model under the viewer's access → five miners
  produce patterns (counts below 3 suppressed) → the report is digested and kept → `crane insight
  propose PATTERN_ID` turns a pattern into a recipe proposal (from explorations judged sufficient) or
  system-derived zone context → `crane insight review` (owner or admin) accepts a checklist, publishing a
  new pinned version → the next question of that class uses it; context goes through contribution
  review.
- **Steps and why.** Every output enters an existing review step; patterns are co-occurrences with
  denominators; the read model reuses Work Capture's eligibility check, so aggregates cannot leak.
- **Humans in the loop.** Judging explorations, accepting checklists and context, compiling capability
  candidates, deciding autonomy. Safe mitigation: schedule reports automatically and notify reviewers;
  never auto-accept.
- **Surfaces.** CLI; changes what Work Capture retrieves (after acceptance) and what packs contain.
- **Input, result, storage.** Capture store (read only); reports in `runtime/insight/reports.jsonl`;
  checklist proposals in `runtime/capture/recipe_proposals.jsonl`; contributions in the contribution
  ledger.
- **Reasoning.** Agentic and business intelligence without agents rewriting rules.
- **Fixes needed.** Reads only the capture store (**MEDIUM**); outcome patterns need hand-recorded
  outcomes (**MEDIUM**); reports not on the dashboard (**LOW**).
- **How to fix safely.** New read models over other stores under the same access rules; proposed
  outcomes (flow 24); upload summaries as a new stream.

## 32. CI, release, and deployment
Code: [.github/workflows/ci.yml](../../.github/workflows/ci.yml), [release.yml](../../.github/workflows/release.yml), [Dockerfile](../../Dockerfile), [render.yaml](../../render.yaml).

- **Problem.** Ship a security tool without regressions.
- **Lifecycle.** CI: formatting, Clippy as errors (with and without `server`), tests with uploads off,
  service tests against MongoDB, smoke test, extension tests and packaging → tag `vX.Y.Z` builds Linux,
  Windows, macOS binaries and publishes the extension when changed → the service deploys as a
  container on Render.
- **Steps and why.** Warnings as errors; end-to-end tests drive the real binary.
- **Humans in the loop.** Tagging a release. Should stay human for a security product.
- **Surfaces.** All.
- **Input, result, storage.** GitHub releases, Marketplace and Open VSX, the Render service.
- **Reasoning.** Repeatable releases.
- **Fixes needed.** None known (**NONE**).
- **How to fix safely.** —
