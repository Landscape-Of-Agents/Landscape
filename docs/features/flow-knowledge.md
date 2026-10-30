# Flows as a knowledge base

A flow in `.crane/governance.yaml` says where a piece of work starts and which zones it runs
through. The flow knowledge base adds what engineers and agents need to work on it safely:

- the call graph Crane traced from its starting point, reviewed and approved by a person;
- typed context: queries, metrics, latency budgets, files, notes, and links;
- a priority that routes the work to the right model tier;
- the people designated on it, linked to Jira and Slack;
- tests written for it, with their assumptions and mock data, some required in every release;
- read-only watchers that agents start to monitor it, within limits only a person changes.

The knowledge is signed in `.crane/flows.json`, which the manifest covers like `bricks.json`.
Agents and the dashboard read it. Only a person on a machine with an admin key changes it.

## Tracing a flow

Give Crane a starting point: a function, a method, an RPC handler, or any symbol written as
`SYMBOL`, `Type.SYMBOL`, or `path:SYMBOL`. Crane follows the calls through the repository with
its lexical call-graph index and saves the result as a draft in `.crane/drafts/flows/ID.json`.

```
crane flow trace --from app/pay.py:CheckoutService.submit --flow checkout --description "Pay for an order"
crane flow draft checkout                 # the call tree, what was not followed, the digest
crane flow retrace checkout --reason "send() retries now"   # a new revision
crane flow rewind checkout --to 1         # review an earlier revision again (nothing is deleted)
crane flow approve checkout --reason "reviewed the call graph"
```

- **Deterministic.** The same code gives the same trace and the same digest. A call that matches
  no definition (external or dynamic) or several (ambiguous) is listed as not followed, never
  guessed.
- **Revisions.** A draft keeps every revision (up to 50). Retrace adds one, and rewind makes an
  earlier one current. Each revision shows what changed since the one before.
- **Approval.** `approve` adds a new flow to `.crane/governance.yaml`, with a context pack written
  from its trace. Comments and the layout of the rest of the file are kept, and the file must read
  back as the old one plus exactly this flow and pack. It also records the trace in the signed
  knowledge, in the same signed generation as `crane governance sign`. If signing fails, the file
  is put back. `--proposed` adds the flow without enforcing it yet.
- **Drift.** A watcher with the `trace_drift` check retraces the approved trace and raises an
  incident when the call graph no longer matches its digest.

On DataSea, approving needs a code owner of `.crane/governance.yaml` (or a repository admin when
CODEOWNERS does not cover it). Every other knowledge change needs a code owner of
`.crane/flows.json`.

## Context, priority, people

```
crane flow context add checkout --kind latency --title "Charge budget" --value 250 --threshold 300
crane flow context add checkout --kind query --title "Failed charges" --value "select ..." --file app/pay.py --lines 40-80
crane flow priority checkout P0 --enforce-model on
crane flow models add --provider claude --model claude-opus-5-5 --tier frontier
crane flow routing P2 balanced,fast
crane flow person add checkout --name Ana --role owner --github ana --jira PAY-12 --slack U0123
crane flow people suggest checkout        # the Git authors of the flow's files
```

**Context kinds:**

| Kind | For |
|---|---|
| query | a query that answers questions about the flow (SQL, logs, metrics) |
| metric | a value with a unit and a threshold |
| latency | a latency budget, in milliseconds |
| file | a file (and lines) to read first |
| note | a short fact |
| link | a dashboard, a runbook, a document |

Context is guidance: it never authorizes anything. Text that looks like a secret is refused, and
withheld from the dashboard.

**Priority and models:** P0 routes to the frontier tier; P1 to frontier, then balanced; P2 to
balanced, then frontier; P3 to fast, then balanced. `crane flow routing` changes a priority's
tiers. Inside a tier, the organization's order decides, and the asking agent's own provider goes
first. With `--enforce-model on`, a change in the flow from a model outside its tiers needs a
person (rule `FLOW_MODEL_ROUTING`). An unknown or unreported model counts as outside. The rule only
tightens.

**People:** roles are author, owner, reviewer, and oncall. On an incident, Crane notifies the
people on call, else the owners, else everyone named:

- in Slack, mentioning a member id (`U…`);
- as a comment on the Jira issue that names them (`PAY-12`).

## Tests

An agent writes a test file, then proposes it with what it assumes and the mock data it uses, in
plain words. A person approves it, and can mark it required:

```
crane flow proposals                       # what agents proposed
crane flow proposals approve fpr_… --required
crane flow proposals reject fpr_… --reason "mocks the gateway too loosely"
crane flow test add checkout --file tests/test_pay.py --name "charges once" --description "..." --command "pytest tests/test_pay.py"
crane flow test require checkout charges-once [--off]
crane flow tests run --required            # in CI or a release step: fails while a required test fails
```

`tests run` runs each test's own command, never through a shell, and records the results that the
watchers' `required_tests` check reads. Add `crane flow tests run --required` to your release
workflow so every release checks the required tests.

## Subagents: Monitor and Incident

A subagent works on one flow. It is created by a person (`crane flow subagent add`, or on DataSea)
or by an agent (`crane-request watch`), and it is one of two subtypes, which must be chosen:

- **Monitor** holds the monitoring of the flow. Crane runs its checks without a model:
  - `trace_drift`: the call graph no longer matches the approved trace;
  - `required_tests`: a required test failed its last run, or its file is gone;
  - `denials`: agents were denied five times or more in the flow in the last hour.

  Its actions read observability metrics (Datadog, Grafana), open a Google Meet, trigger the on-call
  status, activate the flow's Incident subagents, and tell the parent session and the flow's people.
- **Incident** works an incident. Its actions read the Jira ticket of the task
  (`parameters.jira_issue`), write RCA notes and flow context (as proposals a person approves),
  prepare pull requests, code changes, and reviews on its designated branch, and tell the flow's
  people.

`crane flow subagent catalog` lists every action with the ceiling it needs and the integration it
uses.

**Places.** A flow holds at most 3 subagents, pending or active, and at most one of them created by
an agent. The signed limits (`crane flow limit`) can only lower this, and an agent session still
starts at most 2 subagents across flows.

**Bounds.** A subagent is constrained to its flow:
- its targets are the flow and the zones it runs through;
- its autonomy ceiling is at most the flow's;
- each of its actions needs no more than that ceiling.

A configuration that asks for more fails with every problem listed, and with how to resolve it:
raise the flow's ceiling (or add the zone to the flow) in `.crane/governance.yaml` first, then
configure the subagent again. Only a person's manual trigger reaches beyond the flow, for that one
run, and it is recorded:

```
crane flow subagent trigger wat_… --reason "incident drill" --beyond zone:ledger
```

**Configuration.** A subagent has actions, standing context, parameters, integrations, a ceiling,
targets, and (Incident) a branch. Change it with a JSON object merged into the current one:
fields you give replace theirs, `null` resets one, and parameters merge key by key.

```
crane flow subagent configure wat_… --config '{"branch": "incident/checkout", "parameters": {"jira_issue": "PAY-12"}}'
```

It has no volatile context of its own: when it runs, the flow's common context is its volatile
context.

**Approval.** Creating a subagent or changing its configuration waits for a person's approval, in
Landscape or with `crane flow subagent approve|reject`. Crane checks the bounds again then.

**Runs.** When a Monitor's check starts firing, Crane raises one incident and tells the flow's
people and the parent session. Monitors with actions then run them, and when one activates Incident
subagents, those run too. Every run is recorded, action by action:
- **Run for real:** Slack messages, reading a Jira issue, and context or RCA proposals.
- **Recorded as prepared:** Datadog, Grafana, Google Meet, on-call paging, and GitHub pull requests.
  They have connection settings (`crane integrate datadog|grafana|google_meet|oncall`), but Crane
  does not call them yet.

**Removal.** When a flow or one of a subagent's zones is removed from governance, the subagent is
terminated. Landscape asks for a confirmation that the removal is permanent, then runs
`crane flow subagent prune`, which every background sync also runs.

`crane flow watch …` still works and manages the same subagents.

## Test scenarios

`crane flow scenarios generate ID` writes test scenarios for a flow by fixed rules (no model), from:
- its starting points: smoke tests, and mocked tests whose mocks come from the approved call graph;
- its zones: an end-to-end integration test;
- its context: latency as performance tests, metrics as regression tests, queries and links as
  integration tests, files as regression tests, notes as contract tests;
- its Bricks: negative tests that the forbidden action is blocked;
- critical zones (contract tests) and a strict ceiling.

A person reviews them, edits URLs, parameters, JSON bodies, and expectations, accepts each one (or
all of them), and saves them to `.crane/scenarios/ID.json` (`crane flow scenarios save ID --file
PATH`). Generating again keeps the saved edits. `crane flow scenarios show ID --text` prints them as
text. In Landscape, click a flow on the graph: the drawer under it shows the scenarios as a
searchable carousel with Save to repository and Download .txt.

## How agents ask: `crane-request`

Agents may run no Crane command. Instead, an agent runs `crane-request …` as a shell command. The
hook answers it and stops the command: nothing runs, and it counts as neither a denial nor a
bypass. The request must be one command. Pipes, chaining, redirection, and substitution make it
malformed.

```
crane-request flows
crane-request flow checkout                 # its context, people, tests, routed model, incidents
crane-request route --flow checkout
crane-request trace --from app/pay.py:submit --flow checkout
crane-request test --flow checkout --file tests/test_pay.py --name "charges once" --description "..." --assumptions "the gateway answers; amounts in cents" --mock "order 42 for 1000 cents"
crane-request context --flow checkout --kind metric --title "Error rate" --value 0.2
crane-request watch --flow checkout --type monitor --purpose "watch the charge path"
crane-request incident --flow checkout --summary "charges doubled after the deploy"
```

With each tool call that touches a flow, agents also receive a short form of its knowledge:

- the priority and routed model (and whether their own model is allowed);
- the designated people;
- up to eight context items;
- the required tests.

## On DataSea: the Flows section

The Flows section (Build → Flows) reads the repository's flow knowledge, not events, so it has no
time range or event filters. It has three parts:

1. **The map,** in columns from left to right:
   - flows by priority, then drafts;
   - each stage of zones;
   - zones in no flow.

   Each flow's path is drawn from the flow through its zones. A node shows only its name, a
   priority stripe, a red dot for incidents, and an agent badge when a subagent or an assigned agent
   monitors it. Drag to pan, scroll to zoom, or use the controls: zoom in and out, fit, reset to
   100%, zoom to the selection, clear. Selecting a node fills it with the accent color, tints the
   zones and lines along its path, fades the rest, and zooms to them. Escape or a click on the
   background clears the selection.
2. **The inspector,** under the map. Filter chips choose what it shows; clicking an item opens it
   in the smaller detail pane beside the list.
   - **With nothing selected:** model routing, limits, proposals, incidents, and requests.
   - **For a flow:** the chips are Overview (priority and model), Zones, Context, Tests, Agents
     (subagents and assigned agents), Trace, People, Proposals, and Bricks.
     - **Trace** shows the whole flow, from its first call to its last: the call stack (function
       names, indented by depth) on the left and the waterfall (one bar per call, spanning the calls
       under it) on the right. Click a call for where it is, its callers, and its calls. It is the
       static call order, not timing.
     - Approval keeps the approved call graph in `.crane/traces/ID.json`. A flow approved before
       graphs were kept shows its functions again once it is retraced and approved.
     - **People** shows whether Slack, Jira, GitHub, and WhatsApp are connected on the reporting
       machine, and whether each person can be reached through them.
   - **For a zone:** Overview, the flows through it, assigned agents, and Bricks.
3. **Subagents.** Each Monitor or Incident subagent is a small tile, filtered by all, active,
   waiting, firing, or the selected flow. Its info button opens:
   - its properties, actions (and whether each one's integration is connected), parameters, and
     context;
   - Crane's governance: its bounds, places, what it inherited, approval, and manual triggers;
   - its latest runs;
   - a JSON configuration editor. The JSON is merged and checked against the flow's bounds (a pop-up
     lists every problem and how to resolve it) before the change is requested.

   With a flow selected, **Add subagent** asks for the required subtype. Landscape approves every
   creation and change.

Every change on DataSea is a request. A person applies it on a machine, with
`crane brick inbox apply` or one click in Landscape. Signed changes need the code owners of the
signed file. Drafts and watchers need write access.

## Limits today

| Gap | Status |
|---|---|
| Tracing is lexical (names, not types): dynamic dispatch and calls through interfaces are listed as not followed | by design |
| A subagent is Crane-run with no model; Crane cannot tell a Claude Code subagent apart from its parent session, so subagents an agent spawns itself share the parent's rules | partial |
| Datadog, Grafana, Google Meet, on-call paging, and GitHub pull requests have connection settings, but subagent actions that use them are recorded as prepared, not run | to build |
| Watchers, incidents, and test results are per machine (the dashboard shows the reporting machine's) | partial |
| Metric and latency context is stated, not measured; no connector reads live values yet | to build |
| Landscape's graph shows a flow's test scenarios and subagents (click a flow); the rest of the flow knowledge is on DataSea | partial |
