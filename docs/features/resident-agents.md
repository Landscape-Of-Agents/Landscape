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
| Runtime | New module [src/resident/](../../src/resident) (about 10,600 lines with its unit tests): model, triggers, RCA, plans, authority, context cache, sealed store, worker, report | exists |
| CLI | New family `crane resident` (30 commands, shared envelope, exit codes) in [src/cli/resident/](../../src/cli/resident) | exists |
| Worker | One tick per background sync (hooks and commands already start it) or `crane resident tick`; no daemon | exists |
| Authority | Re-reads the signed state before every effectful step; only acts inside a signed Brick assignment | exists |
| Telemetry | 32 typed F38 events ([DASHBOARD.md](../../DASHBOARD.md#f38-resident-agents)), registered and documented | exists |
| Dashboard | New **Resident agents** page (Govern group): registry, review inbox, activation timeline, idle cost and health, test and simulate, creation wizard; route `/api/residents` | exists (as last reported by a member's machine) |
| VS Code | **Resident agents** group in the Bricks view; show, context preview, create draft (with dry-run and confirmation), validate | exists |
| Trigger sources | Crane events, signed generation changes, Brick monitors, timers, and delivered CI or webhook events | exists |
| Action adapters | READ_CHECK, VERIFY, NOTIFY_SLACK (real Slack integration), and the human or internal steps | exists |
| Not wired | Ticket creation, on-call paging, runbook execution, metrics, deployments, inbound Slack or tickets, a model provider, an HTTP webhook listener, a sandbox | to build (shown as unavailable everywhere) |

Files changed outside the new modules are small hook-ins: [src/main.rs](../../src/main.rs) and
[src/cli/mod.rs](../../src/cli/mod.rs) (module and dispatch),
[src/cli/agent.rs](../../src/cli/agent.rs) and [src/telemetry/ingest.rs](../../src/telemetry/ingest.rs)
(one tick per background sync), [src/foxx/status.rs](../../src/foxx/status.rs) (the `residents`
report), [src/telemetry/spec_commands.rs](../../src/telemetry/spec_commands.rs) (F38 dispatch), and
the dashboard and extension files listed in sections 7 and 8.

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
7. **Stop it.** Pause, revoke, or let it expire. Revoking the signed Brick assignment
   (`crane brick unassign`) stops it too, before its next step.

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
- **Concurrency:** one worker at a time (a worker lock). Leases make crash recovery explicit.
- **Exactly-once effects are not claimed.** Slack incoming webhooks take no idempotency key, so an
  uncertain Slack call stays uncertain until a person says what happened.

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
| `CREATE_TICKET` | external | **unavailable**: the Jira integration only comments on existing issues |
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

Other guarantees:

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
- **How CI wakes it:** add a step that runs `crane resident trigger --file event.json`, for example:

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
crane resident validate ID
crane resident activate|renew ID --reason TEXT [--dry-run]          (admin)
crane resident pause|resume|revoke ID --reason TEXT
crane resident list | show ID | context ID | status
crane resident test ID (--event EVENT.json | --replay ACTIVATION)   (plan only)
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
| Registry | Assignments (status, lifecycle, target, definition, agent, RCA, trigger support per source, Crane's health now, expiry, last event, latest activation, last verification, last failure, model calls); definitions; RCA graphs with node bases |
| Review inbox | Pending approvals (with proposed steps and why Crane asks), questions, uncertain effects, proposed RCA graphs, expired or unhealthy assignments, each with the exact command |
| Activation timeline | Every activation with filters; detail: trigger, context sources and versions, cache, candidate causes with evidence, plan with Crane's decision per step, approvals, usage, and timeline |
| Idle cost and health | The counters from section 5, ignored triggers by reason, outcomes, and F38 events over time |
| Test and simulate | Test reports; a typed test-event builder with an in-browser "would it match" preview; what can run today (step types and trigger sources, supported or not) |
| Create | An eight-step wizard: identity and goal, target (inactive ones marked unavailable), context preview from the reported Brick attachments, real trigger sources only, actions and limits (least privilege, nothing auto-runs by default), RCA behavior, verification, and review with risks and missing integrations |

Files: [src/server/dashboard/govern.js](../../src/server/dashboard/govern.js) (`residentsView`),
[app.js](../../src/server/dashboard/app.js) (navigation and guide),
[api/controls.rs](../../src/server/dashboard/api/controls.rs) and
[api/mod.rs](../../src/server/dashboard/api/mod.rs) (`/api/residents`).

**What the dashboard cannot do (by design, as for Bricks):** it holds no signing key, so it cannot
validate, activate, approve, or execute anything. The wizard and test builder **download** draft
and event files that a member imports. Approvals and answers are given with the command shown. The
page shows the state **as last reported by a member's machine**. The durable record is that
machine's resident store; DataSea receives the report and the F38 events.

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
| `cargo test --bin crane resident` | 17 resident unit tests, the F38 registration and documentation test, and the CLI status mapping | 19 passed |
| `cargo test --test residents` | The real binary: definition and RCA save, activate, and review; dry-run create; validate; activate; agent gate; plan-only test; trigger to PROVEN; duplicate; tick with 0 model calls; revoke to DEGRADED and BLOCKED; rejected delivery | 1 passed |
| `cargo test --bin crane telemetry::spec` | Event specification, including the dispatch change | 6 passed |
| `cargo test --test bricks` | Bricks end to end (the reused authority path) | 1 passed |
| `cargo test --test cli_basics version_and_help` | Help text | 1 passed |
| `cargo clippy --bin crane --tests` and `--features server --bin crane` | No warning in changed code | clean |
| `rustfmt --check` on changed files | Formatting | clean |
| `npx tsc -p .` and `node --test --test-name-pattern=resident out/test/core.test.js` (extension) | Types; 3 resident tests | 3 passed |
| `node --check` on `govern.js` and `app.js` | Dashboard scripts parse | ok |

The unit tests use a fake clock, fixtures, a counting fake model, and a fake Slack adapter. No test
needs a paid model, a real Slack message, a ticket, or credentials. They cover: validation of
definitions and assignments; lifecycle and expiry; zero model calls over 13 idle days, then exactly
one in the activation; trigger normalization and rejection; deduplication, filters, scope,
cooldown, and the daily limit; the round trip to PROVEN and back to ACTIVE_IDLE; crash recovery
(lease expiry, dispatching to uncertain, no repeat, reconciliation); a reopened store resuming an
approval; tamper detection; cache hit, miss, and invalidation, and the cache never authorizing;
revocation between planning and execution; observe, assisted, restricted, fail-closed, and
unavailable decisions; approval binding, expiry, single use, and signer-only; RCA basis (proposed,
contradicted, unknown, insufficient); plan validation; UNKNOWN without verification; unavailable
steps; retry only without effect; uncertain effects not retried; dry runs without side effects;
questions and event waits resuming the same activation; deadlines; missing context asking a person;
timers; self-caused events; and report status and redaction.

**Not run:** the dashboard integration tests (`tests/dashboard.rs`) need a live MongoDB
(`CRANE_TEST_MONGODB_URI`) and skip without it, so the `/api/residents` route and the page were
checked by type-checking, `node --check`, and the report unit tests, not in a browser. The
extension's webview tests and the rest of the repository's suites were not run, by instruction.

---

## 10. Limitations and next steps

| Gap | Effect today | Next step |
|---|---|---|
| No model provider | Reasoning is deterministic RCA traversal; ambiguous cases become questions or proposals | Plug a provider into `Reasoner`, metered and bounded by `max_model_calls` |
| Dashboard cannot write | Creation, approvals, and answers go through the CLI with the shown command | A service-side draft and decision inbox that a machine pulls and applies (as suggested for Bricks) |
| No HTTP webhook listener | CI and webhooks deliver with `crane resident trigger` | A signed, authenticated listener on the DataSea service |
| Ticketing, paging, runbooks | Steps listed as unavailable | Jira issue creation with dedup keys; an on-call adapter; runbook start from an approved step |
| Metrics, deployments, inbound Slack or tickets | Sources listed as unsupported | Connectors in `trigger.rs`, one at a time, each with tests |
| Slack has no idempotency key | An uncertain Slack call needs a person to reconcile it | Bot-token mode with message lookup for automatic reconciliation |
| Approvals are one person's | Any signer can approve, and self-approval is allowed (and recorded) | Approval groups and separation of duties |
| One machine's store | Each member's machine has its own resident store; the dashboard shows the latest report | Choose one runner machine per repository, or move durable state to the service |
| Sandbox | Tests are plan-only | An isolation profile that can really confine execution |

Related: [Bricks](bricks.md) (the signed assignment residents act under),
[Runbooks](runbooks.md), [Zones and flows](zones-and-flows.md), and
[the eleven controls](controls.md).
