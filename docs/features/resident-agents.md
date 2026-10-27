# Resident agents

A **resident agent** is an agent assignment that stays responsible for one Flow, Zone, or Runbook
for days or weeks, and **uses no model and no process while it is idle**. It wakes only when a
relevant trigger or timer arrives. It then loads the current context, investigates with an RCA
graph, makes a typed plan, runs only the steps Crane allows at that moment, verifies the outcome,
records everything, and goes back to idle. If it needs a person, it waits for one instead.

What persists is the **assignment record**, not a model session. Nothing keeps a conversation,
worker, or polling prompt open for the assignment's lifetime.

This document is the README for the feature. It covers what was built, how it works, what each
part of the organization gets from it, and what is honestly not done yet. Status words follow
[the docs index](../README.md): **exists** (built and tested), **partial**, and **to build**.

---

## 1. Summary of the change

| Area | What changed | Status |
|---|---|---|
| Runtime | New module [src/resident/](../../src/resident) (about 12,000 lines with its unit tests): model, triggers, RCA, plans, authority, context cache, sealed store, worker, dashboard decisions, report | exists |
| CLI | New family `crane resident` (31 commands, shared envelope, exit codes) in [src/cli/resident/](../../src/cli/resident) | exists |
| Worker | One tick per background sync (hooks and commands already start it) or `crane resident tick`; no daemon | exists |
| Authority | Re-reads the signed state before every effectful step; only acts inside a signed Brick assignment | exists |
| Telemetry | 33 machine F38 events and 2 service events ([DASHBOARD.md](../../DASHBOARD.md#f38-resident-agents)), registered and documented | exists |
| Dashboard | New **Resident agents** page (Govern group): registry, review inbox, activation timeline, idle cost and health, test and simulate, creation wizard; route `/api/residents`, merged across every member's machine | exists |
| Dashboard decisions | Approve, deny, answer, cancel, pause, revoke from the page (GitHub-verified admin), applied by the machine at its next sync with Crane's checks; route `/auth/resident` and the machine routes `resident-decisions` | exists (approvals and answers only where the definition allows them) |
| VS Code | **Resident agents** group in the Bricks view; show, context preview, create draft (with dry-run and confirmation), validate, pause, resume, revoke | exists |
| Trigger sources | Crane events, signed generation changes, Brick monitors, timers, **GitHub Actions runs (pulled)**, and delivered CI or webhook events | exists |
| Action adapters | READ_CHECK, VERIFY, NOTIFY_SLACK (Slack), **CREATE_TICKET (Jira, deduplicated and reconciled by label)**, and the human or internal steps | exists |
| Governance options | Dashboard decisions (opt-in per definition), separation of duties (the activator cannot approve) | exists |
| Not wired | On-call paging, runbook execution, metrics, deployments, inbound Slack or tickets, a model provider, an HTTP webhook listener, a sandbox | to build (shown as unavailable everywhere) |

Files changed outside the new modules are small hook-ins: [src/main.rs](../../src/main.rs) and
[src/cli/mod.rs](../../src/cli/mod.rs) (module and dispatch),
[src/cli/agent.rs](../../src/cli/agent.rs) and [src/telemetry/ingest.rs](../../src/telemetry/ingest.rs)
(one tick per background sync), [src/foxx/status.rs](../../src/foxx/status.rs) (the `residents`
report, and pulling dashboard decisions), [src/telemetry/spec_commands.rs](../../src/telemetry/spec_commands.rs)
(F38 dispatch), [src/integrations/mod.rs](../../src/integrations/mod.rs) (Jira issue create and
lookup), the service files [src/server/ingest.rs](../../src/server/ingest.rs) and
[src/server/store.rs](../../src/server/store.rs) (the machine routes for decisions), and the
dashboard and extension files listed in sections 7 and 8.

---

## 2. Product: what a person can do now

1. **Describe the agent once.** A *definition* says what it is for, which trigger sources it may
   use, which step types it may plan (and which may run without a per-run approval), and its
   limits. Versions are immutable; an admin activates one.
2. **Give it a responsibility.** An *assignment* puts one definition version on one Flow, Zone, or
   Runbook, for a lifetime of 1 to 90 days, with explicit trigger subscriptions such as
   `ci` `ci.failed`. There is no "any event" option.
3. **Test it before it can act.** `crane resident test` (or the dashboard's test event builder)
   shows whether a trigger would match, whether it would be dropped as a duplicate or by the
   cooldown, the context it would receive, the RCA path, the plan, and Crane's decision for every
   step. Nothing runs.
4. **Turn it on.** Activation validates again against the signed rules of that moment and needs an
   admin key. After that the assignment sits **ACTIVE_IDLE** at zero model cost.
5. **Watch it work.** Each trigger becomes one durable *activation* with a full timeline: the
   trigger, the context and its cache status, the candidate causes and their evidence, the plan,
   Crane's decision per step, approvals, actions, verification, and usage.
6. **Answer when it asks.** Approvals, questions, uncertain effects to reconcile, and proposed RCA
   graphs appear in the dashboard's review inbox with the exact command to run. The answer resumes
   the same activation.
7. **Decide from the dashboard.** A repository admin approves, denies, or answers a waiting step,
   cancels an activation, or pauses or revokes an assignment from the page. GitHub confirms the
   role, and the machine applies the decision at its next sync with every check Crane makes there.
   Approvals and answers count only when the definition says `remote_decisions: true`, and only for
   the plan the admin was shown.
8. **Stop it.** Pause, revoke, or let it expire, from the CLI, the dashboard, or VS Code. Revoking
   the signed Brick assignment (`crane brick unassign`) stops it too, before its next step.

### Lifecycles (kept separate on purpose)

```
Assignment:  draft -> validated -> active <-> paused
                                   active | paused -> expired -> active (renew, validated again)
             any -> revoked (final)

Activation:  QUEUED -> CONTEXT_LOADING -> INVESTIGATING -> PLAN_READY -> EXECUTING -> VERIFYING -> outcome
             waiting states: AWAITING_APPROVAL, AWAITING_INPUT, WAITING_FOR_EVENT, RECONCILIATION_REQUIRED
             outcomes: COMPLETED (proven), PARTIALLY_VERIFIED, FAILED, BLOCKED, CANCELLED, UNKNOWN, SIMULATED (tests)
```

The dashboard shows a **derived runtime status** next to the lifecycle: ACTIVE_IDLE, QUEUED,
INVESTIGATING, EXECUTING, AWAITING_APPROVAL, AWAITING_INPUT, WAITING_FOR_EVENT,
RECONCILIATION_REQUIRED, or **DEGRADED**. DEGRADED means Crane would not admit it right now, for
example because its signed Brick assignment was revoked or its Zone is no longer active. It looks
idle but cannot act, so it is shown differently from a healthy idle assignment.

### The words that matter

| Word | Meaning here |
|---|---|
| **COMPLETED** | Every step succeeded **and** every verification passed. Nothing else, including a model's claim, sets it. |
| **UNKNOWN** | It ended without evidence that proves an outcome (no verification step, or only a question or proposal). Never counted as success. |
| **Supported cause** | This activation's checks support the cause. It is not a confirmed cause. |
| **Unavailable** | No adapter can run the step type. It never runs and never counts as done. |
| **Plan-only test** | Decisions and read-only checks only. It is not a sandbox, and nothing labels it one. |

---

## 3. Engineering: how it works

### 3.1 Module map ([src/resident/](../../src/resident))

| File | Responsibility |
|---|---|
| `model.rs` | Definitions (immutable, digest-pinned), assignments (sealed), lifecycle transitions, subscriptions, timers, limits with administrative ceilings |
| `trigger.rs` | The canonical trigger envelope, normalization and rejection, subscription matching, and collectors for the event store, generation, monitors, and timers |
| `rca.rs` | Versioned RCA graphs (nodes, typed relations, basis), deterministic checks, the `Reasoner` trait and the deterministic `GraphReasoner` |
| `plan.rs` | The closed step-type registry (12 types, v1) and typed, digest-bound plans with validation |
| `authority.rs` | The Crane view (signed state, Brick assignments, target limits, Brick controls, connectors) and the per-step decision |
| `context.rs` | Context packs and the dependency-keyed cache (HIT, MISS, INVALIDATED with the changed dependencies) |
| `registry.rs` | Lifecycle changes of definitions, assignments, and RCA graphs |
| `store.rs` | The sealed runtime store `runtime/resident/store.json`: activations, dedup set, cursors, counters |
| `worker.rs` | The activation state machine, the Slack adapter, human waits, ingest, tick, sync, and plan-only tests |
| `report.rs` | The `residents` part of the machine's status report (no context text, no answers) |

### 3.2 Reuse, not duplication

| Need | Reused | Why |
|---|---|---|
| Who may act where | The signed **Brick assignment** in `.crane/bricks.json` (`crane brick assign`) | One authority source. A resident adds lifetime, triggers, and limits that can only narrow it. |
| Signed-state verification | `brick::store::load` and `trust_of` (signature, rollback anchor, digests) | The same check the hooks use |
| Zone and Flow limits | `governance::zones::resolve` plus `brick::compose::controls` | The same ceilings, criticality, and Brick controls the guard enforces |
| Flow, Zone, and Brick context | `brick::context::assemble` | The same snapshot (and digests) agents receive at the hook |
| Sealed records | The runbook store's pattern (digest per record, lock, atomic write) | Tampering is refused on read |
| Events | `platform::notes` turned into F38 by `telemetry::spec_resident` | Same event store, hash chain, identity, and redaction |
| Slack | `integrations::deliver_now` | The configured channel and credentials already in the trust directory |
| Dashboard data | The status report every sync uploads (like the Bricks page) | No new store or write path in DataSea |
| CLI output | `cli::protocol` envelope and exit codes | Landscape and scripts read it unchanged |

There is **no second policy engine, no new signed file, no new event store, and no new
dependency**.

### 3.3 Zero model use while idle

- A tick reads cursors (event sequence, signed generation, last monitor run) and timer slots.
  When nothing arrived, no activation runs and **no reasoner is called**.
- Idle ticks write nothing to the event store (no growth while waiting). They are counted in
  `counters.idle_ticks`, and any model call during an idle tick would show in
  `counters.idle_model_calls`. Tests prove it stays 0 across 312 simulated hours.
- **No model provider is configured in this repository.** Reasoning is the deterministic
  `GraphReasoner`. The `Reasoner` trait is where a provider would plug in: it must report the calls
  it makes, and `limits.max_model_calls` (default 0, ceiling 10) bounds them per activation.
- Tokens are reported as **unmeasured**, never as zero.

### 3.4 Delivery semantics (at least once in, no duplicate effects out)

- **Trigger identity:** `trg_` comes from the source and idempotency key, so a redelivery has the
  same id and is dropped (`seen`, kept 14 days).
- **Activation identity:** `act_` comes from the assignment and trigger, so a trigger creates at
  most one activation per assignment.
- **Before an external call** the step is persisted as `DISPATCHING`. A crash leaves it there, and
  the next worker, after the 5-minute lease expires, marks it **UNCERTAIN** and asks a person to
  reconcile it. It is never retried blindly.
- **Retries** happen only for failures known to have had no effect, within `max_retries`
  (ceiling 2).
- **Loops are impossible by construction:** events the runtime caused (`resident.*` events and
  commands that carry `RESIDENT_*` notes) never become triggers.
- **Concurrency:** every writer (a command that changes the store, the background sync's tick, applying dashboard decisions) holds one lock from reading the store to its last save, so a background sync can never overwrite a command's change. Background work skips its turn when the lock is busy; commands wait for it. Leases make crash recovery explicit.
- **Automatic reconciliation:** an adapter that can read its result back does so before anyone is
  asked. Jira issues carry a label derived from the step's idempotency key. Before creating, and on
  every tick while a step is uncertain, the label is looked up. An issue that exists is the
  outcome, and a second one is never opened.
- **Exactly-once effects are not claimed.** Slack incoming webhooks take no idempotency key, so an
  uncertain Slack call stays uncertain until a person says what happened.
- **Out-of-order events:** a wait for a later event (for example "the next green run") ignores an
  event that happened before the activation's own trigger, even if it arrives later.

### 3.5 Context and cache

The pack is compiled from: the agent mission (definition), Flow and Zone context, Brick Common and
Current Context, and the RCA graph version. The trigger and the latest previous outcome are
recorded on the activation (a reference, not a transcript dump). The cache key is the digest of
definition, assignment content, signed generation, governance digest, Brick store digest, snapshot
digest, and RCA digest and state. The activation records HIT, MISS, or INVALIDATED with the
dependency names that changed. Required context that is missing (an unreadable context file, a pin
Crane cannot honor, an expected Brick not delivered, a missing RCA graph, an unverified signed
state) either **blocks** the activation or **asks** a person, as the definition says. It never
continues silently. **The cache never authorizes**: every step is still decided against the signed
state read at that moment.

### 3.6 RCA graphs and investigation

Nodes are signal, symptom, cause, check, remediation, or verification. Relations are
`observed_with`, `may_cause`, `confirmed_by`, `contradicted_by`, `mitigated_by`, and
`verified_by`. Every node carries a **basis**: observed, inferred, hypothesized, confirmed,
contradicted, or unknown. Validation refuses a "confirmed" node without a stated basis, and wrong
relation kinds.

| Investigation result | Plan |
|---|---|
| Exactly one cause **supported** by a passing check, reviewed graph, remediation exists | The remediation's steps plus VERIFY steps from its verifications |
| Same, but the graph is only **proposed** | One PROPOSE_ACTION_PLAN step that needs a person's approval |
| Several or no supported causes (**ambiguous**) | PROPOSE_ACTION_PLAN with the remaining candidates' remediations, or ASK_USER |
| No signal matches (**insufficient evidence**) | PROPOSE_RCA (a hypothesized graph, never published by itself) and ASK_USER |

A contradicting check beats a confirming one. A check that cannot be evaluated is **UNKNOWN** and
is listed as a gap. Reviewing a graph never changes node bases: hypotheses stay hypotheses until a
person writes a new version.

### 3.7 The step registry (v1)

| Step | Effect | Today |
|---|---|---|
| `READ_CHECK`, `VERIFY` | read-only | available (deterministic checks: trigger attribute, file exists, signed state verifies, Brick monitors OK, recent trigger) |
| `NOTIFY_SLACK` | external | available when Slack is configured; message names its assignment, activation, and trigger |
| `REQUEST_APPROVAL`, `ASK_USER`, `WAIT_FOR_EVENT`, `RECONCILE` | internal | available (durable waits with deadlines; no polling) |
| `PROPOSE_RCA`, `PROPOSE_ACTION_PLAN` | internal | available (proposals for review) |
| `CREATE_TICKET` | external | available when Jira is configured: one issue per step in the configured project, deduplicated and reconciled by its label |
| `PAGE_ONCALL` | external | **unavailable**: no on-call adapter |
| `EXECUTE_PROCEDURE` | external | **unavailable**: residents cannot start runbooks yet |

Plans are validated for unique ids, dependencies on earlier steps only (so they are acyclic),
allowed types, required parameters, bounded text without secrets, valid checks, no nested
proposals, and (when the definition requires it) a VERIFY step whenever there is an external step.

---

## 4. Security and compliance

**Crane stays the only authority.** For every step, immediately before it runs, `authorize` reads
the signed state again and applies these rules. The strictest one wins.

| Condition | Decision |
|---|---|
| Signed state does not verify, or there is no signed Brick store | DENY |
| No **active signed Brick assignment** for the agent on the target | DENY |
| Assignment not active, or past its lifetime | DENY |
| Definition not active, or its digest differs from the pinned one | DENY |
| Target missing or not active | DENY |
| Step type not allowed by the definition | DENY |
| No adapter, or connector not configured | UNAVAILABLE |
| External step on an **observe** target | DENY |
| External step on an **assisted** or **restricted** target, under an approval, read-only, or fail-closed Brick control, on a runbook target, or not marked auto | REQUIRE_APPROVAL |

Who may decide a waiting step:

| Decision | From a machine (CLI or VS Code) | From the dashboard |
|---|---|---|
| Approve or deny | A repository signer (owner or admin key) | A GitHub-verified repository admin, only if the definition has `remote_decisions: true`, and only for the plan digest shown |
| Answer a question | Any person running Crane | Same rule as approvals |
| Cancel, pause, revoke | Any person running Crane | Any GitHub-verified repository admin (it only makes things stricter) |
| Resume, activate, renew, review an RCA graph | A repository signer | Never |
| Reconcile an uncertain effect | Any person running Crane (declared evidence) | Never; adapters that can read back (Jira) reconcile automatically |

With `distinct_approver: true`, the person who activated the assignment cannot approve its steps.

Other guarantees:

- **Dashboard decisions** pass three gates: GitHub confirms the admin role when the decision is
  made, the decision waits for at most one day for its machine, and the machine applies it through
  the same functions and checks as the CLI. Each decision is applied once (by its id). A decision
  about a plan that changed since is refused, and the refusal is reported back to the page.
- **Approvals** bind to the activation, the plan digest, the step's content, and the signed-state
  digest. They expire (`approval_ttl_minutes`), are used once, and come only from a repository
  signer (owner or admin key). A change to the signed rules after approval voids it.
- **Only people run Crane**: every `crane resident` command is refused inside an agent session.
  Making things stricter (pause, revoke, cancel) never needs an admin. Activating, renewing,
  approving, and reviewing RCA graphs do.
- **Triggers are data, never instructions.** They are validated: kind syntax, target keys, at most
  32 attributes of 256 characters, no control characters, no secret-looking values, the right
  repository, not stale (30 days), and not from the future. Delivered files take only `ci` and
  `webhook` sources and refuse unknown fields.
- **Secrets never enter** definitions, plans, triggers, answers, or events. Validation refuses
  them, and the remaining text is redacted.
- **Records are sealed**: an edit to the resident store outside Crane is refused when it is read.
- **What leaves the machine** (the dashboard report and F38 events) is identifiers, states,
  digests, counts, reason codes, and bounded plan parameters. Mission text, context text, and
  answers stay in the trust directory.
- **Bounded cost**: per-assignment cooldown, a daily activation limit (ceiling 200), steps per plan
  (ceiling 20), retries (ceiling 2), step timeouts, wait deadlines, model calls per activation
  (default 0), and at most 25 activations advanced per tick.

For auditors: every lifecycle change records who, when, and why. Every activation keeps its
timeline, every Crane decision with reason codes and the signed generation, and every approval
with approver, digest, expiry, and use. F38 events link to these records by id.

---

## 5. Operations and SRE

- **Where it runs:** the background sync that hooks and commands already start, and
  `crane resident tick`, which an OS scheduler (cron, Task Scheduler) can run to get timers
  without agent activity. There is no daemon to deploy.
- **How CI wakes it, option 1 (pulled):** when the repository is connected to GitHub
  (`crane repo --gh-cli` or another method) and the `gh` CLI is signed in, every tick reads finished
  GitHub Actions runs, at most once every 5 minutes and only while an active assignment subscribes
  to `ci`. Runs become `ci.passed`, `ci.failed`, `ci.cancelled`, and so on, with the attributes
  `workflow`, `branch`, `event`, `run_id`, `attempt`, `sha`, and `conclusion`. They name no target,
  so subscribe with `"scope": "repository"` (and filter with attributes such as
  `{"branch": "main"}`). The first read only records where it is; history is not replayed. Set
  `CRANE_RESIDENT_CI_POLL=off` to turn this off.
- **How CI wakes it, option 2 (delivered):** add a step that runs
  `crane resident trigger --file event.json`, for example:

  ```json
  {"source": "ci", "kind": "ci.failed", "targets": ["flow:checkout"], "attributes": {"job": "tests"}, "idempotency_key": "gh-run-9172-attempt-1"}
  ```

- **Runbook for a stuck assignment:** run `crane resident show ID` to see its status and Crane's
  reasons now. `crane resident activations --assignment ID` and `crane resident activation ACT` show
  the timeline. Then approve, answer, reconcile, or cancel.
- **Health:** `crane resident status` and the dashboard show ticks, idle ticks, **model calls while
  idle**, model calls in activations, triggers received, rejected, and duplicate, ingestion lag,
  queue depth, failed or blocked activations, cache hit, miss, and invalidation, ignored triggers by
  reason, and outcomes by kind.
- **Revocation:** `crane resident revoke`, or `crane brick unassign` for the signed authority.
  Queued and waiting work is cancelled. An external call already started stays uncertain for
  reconciliation; it is not reported as cancelled.

---

## 6. CLI reference (`crane resident`)

```
crane resident catalog
crane resident definition save --file DEFINITION.json [--dry-run]
crane resident definition activate|revoke ID@N --reason TEXT        (activate: admin)
crane resident definition list | show ID[@N]
crane resident create --file ASSIGNMENT.json [--dry-run]
crane resident duplicate ID --as NEW_ID [--target flow:ID|zone:ID]    (a new draft)
crane resident validate ID
crane resident activate|renew ID --reason TEXT [--dry-run]          (admin)
crane resident pause|resume|revoke ID --reason TEXT
crane resident list | show ID | context ID | status
crane resident test ID (--event EVENT.json | --replay ACTIVATION | --replay-event SEQUENCE)   (plan only)
crane resident trigger --file EVENT.json                            (ci or webhook, then a tick)
crane resident tick
crane resident activations [--assignment ID] | activation ACTIVATION
crane resident approve ACTIVATION --step STEP --reason TEXT [--deny] (admin)
crane resident answer ACTIVATION --step STEP --text TEXT
crane resident reconcile ACTIVATION --step STEP --happened yes|no --reason TEXT
crane resident cancel ACTIVATION --reason TEXT
crane resident rca save --file GRAPH.json
crane resident rca review ID@N (--accept | --reject) --reason TEXT  (admin)
crane resident rca list | show ID[@N]
```

Every command takes `--format json`. Exit codes follow the shared envelope: 0 ok or passed, 3
failed, blocked, or denied, 4 approval required, 5 unknown.

---

## 7. DataSea dashboard (Govern → Resident agents)

| Zone | What it shows |
|---|---|
| State | Active and idle, investigating or executing, awaiting approval, degraded, model calls while idle (should stay 0), queue depth |
| Registry | Assignments (with their machine and **Pause… / Revoke…** buttons; status, lifecycle, target, definition, agent, RCA, trigger support per source, Crane's health now, expiry, last event, latest activation, last verification, last failure, model calls); definitions; RCA graphs with node bases |
| Review inbox | Pending approvals (with proposed steps and why Crane asks), questions, uncertain effects, proposed RCA graphs, expired or unhealthy assignments. **Approve…, Deny…, Answer…** buttons decide on the page; the exact command is shown as the alternative |
| Activation timeline | Every activation with filters and a **Cancel…** button while it runs; detail: trigger, context sources and versions, cache, candidate causes with evidence, plan with Crane's decision per step, approvals, usage, and timeline |
| Idle cost and health | The counters from section 5, ignored triggers by reason, outcomes, and F38 events over time |
| Test and simulate | Test reports; a typed test-event builder with an in-browser "would it match" preview; what can run today (step types and trigger sources, supported or not) |
| Create | An eight-step wizard: identity and goal, target (inactive ones marked unavailable), context preview from the reported Brick attachments, real trigger sources only, actions and limits (least privilege, nothing auto-runs by default), RCA behavior, verification, and review with risks and missing integrations |

Files: [src/server/dashboard/govern.js](../../src/server/dashboard/govern.js) (`residentsView`),
[app.js](../../src/server/dashboard/app.js) (navigation and guide),
[api/controls.rs](../../src/server/dashboard/api/controls.rs) and
[api/mod.rs](../../src/server/dashboard/api/mod.rs) (`/api/residents`).

**How a dashboard decision travels:** the button opens a short form (reason; the answer for a
question). GitHub sign-in confirms that you administer the repository, and the service records the
decision for the machine that holds the activation (`resident_decisions`, waiting at most one
day). That machine's next sync fetches it (`GET resident-decisions`) and applies it through
`resident::remote`, with the same functions and checks as the CLI. It then reports `APPLIED` or
`REJECTED` with the reason (`POST resident-decisions`). Events: `RESIDENT_DECISION_RECORDED` or
`RESIDENT_DECISION_REFUSED` on the service, then `RESIDENT_DECISION_APPLIED` from the machine.

**Several members:** every member's machine keeps its own resident store. The page merges the
latest report of every machine. Each row says which machine it is on, which is where a decision is
applied, and counters are summed. A member whose Crane has no resident agents no longer hides the
others.

**What the dashboard still cannot do:** it holds no signing key, so it cannot validate, activate,
resume, or renew an assignment, review an RCA graph, or declare a reconciliation. The wizard and
test builder **download** draft and event files that a member imports. The durable record is each
machine's resident store; DataSea receives reports, events, and pending decisions only.

---

## 8. Landscape for VS Code

In the Bricks view there is a new **Resident agents** group, by target, with the workspace's own
Zones and Flows first. Each assignment shows its status, definition, and a tooltip with its
triggers, Crane's answer now, its latest activation, and model calls. Commands:

- **Show Resident Agent**: a Markdown preview with its latest activations.
- **Preview Resident Context**: the pack sources, versions, sizes, cache status, and anything
  missing.
- **Create Resident Agent Draft**: pick an active Zone or Flow and an active definition, edit the
  JSON, see Crane's `--dry-run` validation, confirm, and create the draft. Activation stays an
  admin's step.
- **Validate Resident Agent**: problems and warnings appear in the output.
- **Pause, Resume, or Revoke Resident Agent**: offers only the changes its lifecycle allows, asks for
  a reason, confirms, and runs Crane (resuming needs what Crane requires).

It reads the same `crane resident` envelopes as the dashboard's data source, so an assignment made
on the dashboard path (draft imported with the CLI) and one made in the editor are the same
record. Landscape never reads the trust directory or decides anything. It stays disabled in
untrusted workspaces, as before. Files: [core/residents.ts](../../extensions/foxx/src/core/residents.ts),
[bricksView.ts](../../extensions/foxx/src/bricksView.ts), and
[package.json](../../extensions/foxx/package.json).

---

## 9. Tests (changed code only; the full suite was not run)

| Command | Covers | Result |
|---|---|---|
| `cargo test --bin crane resident` | 24 resident unit tests, the F38 registration and documentation test, and the CLI status mapping | 26 passed |
| `cargo test --features server --bin crane resident_reports_of_every_machine` | Merging several machines' reports on the dashboard | 1 passed |
| `cargo test --test residents` | The real binary: definition and RCA save, activate, and review; dry-run create; validate (Slack and Jira not configured); activate; agent gate; plan-only test; trigger to PROVEN; duplicate; tick with 0 model calls; revoke to DEGRADED and BLOCKED; rejected delivery; replay of a recorded event; duplicate | 1 passed |
| `cargo test --bin crane telemetry::spec` | Event specification, including the dispatch change | 6 passed |
| `cargo test --bin crane integrations::` | Integration providers (the Jira module gained functions) | 2 passed |
| `cargo test --test bricks` | Bricks end to end (the reused authority path) | 1 passed |
| `cargo test --test cli_basics version_and_help` | Help text | 1 passed |
| `cargo clippy --bin crane --tests` and `--features server --bin crane --tests` | No warning in changed code | clean |
| `rustfmt --check` on changed files | Formatting | clean |
| `npx tsc -p .` and `node --test --test-name-pattern=resident out/test/core.test.js` (extension) | Types; 3 resident tests (now also lifecycle changes and repository-scope CI drafts) | 3 passed |
| `node --check` on `govern.js` and `app.js` | Dashboard scripts parse | ok |

The unit tests use a fake clock, fixtures, a counting fake model, a fake Slack, a fake Jira, and
recorded GitHub answers. No test needs a paid model, a real Slack message, a ticket, a GitHub call,
or credentials. Besides the first round's coverage, this round adds:

- one Jira issue per step, including after a lost answer (found by label on the next tick)
- a person reconciling while Jira cannot answer
- dashboard decisions:
  - refused without `remote_decisions`
  - refused for a changed plan
  - applied for the current plan
  - applied only once
  - pause accepted and resume refused
- separation of duties and the deny path
- out-of-order events not satisfying a wait
- GitHub Actions runs:
  - history recorded, not replayed
  - one trigger per new run
  - reads limited to one per 5 minutes
  - no read without a CI subscription
- duplicating an assignment

**Not run:** the dashboard integration tests (`tests/dashboard.rs`) and the ingestion tests need a
live MongoDB (`CRANE_TEST_MONGODB_URI`). So the `/auth/resident` sign-in flow, the
`resident-decisions` service routes, and the page buttons were checked by type-checking, clippy,
`node --check`, and the unit tests of both ends (the machine's `remote::apply` and the dashboard's
merge), not against a running service or in a browser. The real Jira and GitHub calls were not
made; their request shapes follow the integrations already in the repository.

## 10. Limitations and next steps

| Gap | Effect today | Next step |
|---|---|---|
| No model provider | Reasoning is deterministic RCA traversal; ambiguous cases become questions or proposals | Choose a provider and decide whether repository context may leave the machine; then plug it into `Reasoner`, metered and bounded by `max_model_calls` |
| Runbook execution from a plan | `EXECUTE_PROCEDURE` is unavailable | Start an active runbook version from an approved step, and track its run through reconciliation (the runbook engine lives in the CLI layer today) |
| On-call paging | `PAGE_ONCALL` is unavailable | An on-call adapter (PagerDuty or Opsgenie) with dedup keys |
| No HTTP webhook listener | Webhooks deliver with `crane resident trigger`; CI is also pulled from GitHub Actions | A signed, authenticated listener on the DataSea service |
| Metrics, deployments, inbound Slack or tickets | Sources listed as unsupported | Connectors in `trigger.rs`, one at a time |
| Pulled CI names no target | CI subscriptions use the repository scope with attribute filters | Map a run's changed files to Zones and Flows |
| Slack has no idempotency key | An uncertain Slack call needs a person to reconcile it | Bot-token mode with message lookup for automatic reconciliation |
| Separation of duties compares names | A person known as `@login` on the dashboard and by their Git name on a machine is not recognised as the same | Link identities (the dashboard already links machines to GitHub accounts) |
| Dashboard cannot resume, activate, or review | Those stay signer actions on a machine | Keep, by design: loosening needs a signing key |
| One store per machine | Each member's machine runs the assignments it holds | Pick one runner machine per repository, or move durable state to the service |
| Sandbox | Tests are plan-only | An isolation profile that can really confine execution |

Related: [Bricks](bricks.md) (the signed assignment residents act under),
[Runbooks](runbooks.md), [Zones and flows](zones-and-flows.md), and
[the eleven controls](controls.md).
