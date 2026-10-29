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

## Watchers: monitoring subagents

A watcher is a monitoring subagent of one flow. An agent starts it, or a person with
`crane flow watch add`. It takes on its parent's properties:

- the parent session, provider, and model;
- the flow as its only target;
- the flow's Bricks, autonomy ceiling, and criticality.

It is read-only by construction: Crane runs its checks without a model and writes nothing but its
own results. The checks:

- `trace_drift`: the call graph no longer matches the approved trace;
- `required_tests`: a required test failed its last run, or its file is gone;
- `denials`: agents were denied five times or more in the flow in the last hour.

When a check starts firing, Crane raises one incident. It notifies the flow's people and leaves a
notice that the parent session receives with its next tool call. Watchers run with every
background sync, or with `crane flow watch run`.

**Limits.** By default, an agent session starts at most 2 watchers and a flow has at most 3.
These are signed limits, and only a person changes them: `crane flow limit --session N`,
`crane flow limit --flow N`, `crane flow limit ID --watchers N`. A flow can lower the repository's
limit but not raise it.

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
crane-request watch --flow checkout --purpose "watch the charge path"
crane-request incident --flow checkout --summary "charges doubled after the deploy"
```

With each tool call that touches a flow, agents also receive a short form of its knowledge:

- the priority and routed model (and whether their own model is allowed);
- the designated people;
- up to eight context items;
- the required tests.

## On DataSea: the Flows section

The Flows section (Build → Flows) has three parts:

1. **The map.** All flows, grouped by priority, and drafts waiting for review, above the zones,
   grouped by criticality. A node shows only its name and a few signals: priority, drafts,
   watchers (◉), and incidents (red dot). Lines from a flow to its zones appear only for the
   selection, so the map stays readable with many flows.
2. **Search.** It finds flows, zones, people, and symbols. `/` focuses the search, and Enter
   selects the first match.
3. **The lookout,** docked under the map. With nothing selected, it shows model routing, limits,
   proposals, incidents, and requests.
   - **For a flow:** tabs for Overview (priority and model), Trace (the call tree, revisions,
     approve, retrace, rewind), Context, Tests (with proposals), People & agents (designated people,
     watchers and their parents, the limit, assigned agents, incidents), and Bricks. The For agents
     lens shows the text an agent receives and the requests it may make.
   - **For a zone:** tabs for Overview, the flows through it, and Bricks.

Every change on DataSea is a request. A person applies it on a machine, with
`crane brick inbox apply` or one click in Landscape. Signed changes need the code owners of the
signed file. Drafts and watchers need write access.

## Limits today

| Gap | Status |
|---|---|
| Tracing is lexical (names, not types): dynamic dispatch and calls through interfaces are listed as not followed | by design |
| A watcher is Crane-run and read-only; Crane cannot tell a Claude Code subagent apart from its parent session, so subagents an agent spawns itself share the parent's rules | partial |
| Watchers, incidents, and test results are per machine (the dashboard shows the reporting machine's) | partial |
| Metric and latency context is stated, not measured; no connector reads live values yet | to build |
| Landscape for VS Code shows flow requests in its inbox; its graph does not show flow knowledge yet | to build |
