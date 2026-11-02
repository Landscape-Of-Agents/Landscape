# The dashboard

DataSea, the hosted dashboard ([dashboard.agentslandscape.cloud](https://dashboard.agentslandscape.cloud)),
gathers what each part of the organization knows, section by section. Machines upload signed,
redacted reports to the DataSea service; the dashboard reads them. Enforcement never depends on
it: it all happens on each machine.

## Sign in and visibility

The landing page and the section pages are open to anyone. Flows needs a GitHub sign-in. You see the
repositories GitHub lets you see; there is no separate user administration. Each machine is linked
to a person once (`crane agent link`). Once a machine is linked, Flows shows only what linked
machines reported.

## What is on it

| Area | What you see | Status |
|---|---|---|
| **Landing page** | What DataSea is, every section with its subsections, and how changes reach a machine | exists |
| **Engineering** | Its subsections | exists |
| **Engineering › Flows** | A map of every flow (by priority) and draft above the zones, each flow's trace and draft revisions (approve, retrace, rewind), typed context, priority and the routed model, tests with assumptions and mock data (required ones), designated people, subagents, assigned agents, incidents, and Bricks. Every change is a request a person applies on a machine. See [features/flow-knowledge.md](../features/flow-knowledge.md) | exists (as last reported by a member's machine) |
| **Product, Sales, Marketing, Finance, Human Resources, Operations, Legal** | The section and its purpose | sections exist; no subsections yet |

The metric pages (activity, agents and sessions, governance, operate, compliance, reports), the
Bricks and Resident agents pages, Dashboard Agents, credit grants, and resident-agent decisions
were removed. Their data was removed from the service too (schema 14). Bricks and resident agents
are still managed on each machine (`crane brick`, `crane resident`) and in Landscape.

## What the dashboard may change

Only narrow, repository-scoped writes:

- linking a machine to a person;
- confirming the owner key;
- the change requests made in Flows.

GitHub decides each one. A signed flow change needs a code owner of the signed file, or a
repository admin when no CODEOWNERS line covers it. A request waits for a person to apply it on a
machine (`crane brick inbox apply` or one click in Landscape), where Crane checks and signs it. The
dashboard cannot sign rules: signing needs an owner or admin key on a machine.

## What the service keeps

The service keeps only what Flows and the machines need:

- repositories with their verified remotes;
- each member's latest repository status;
- how far each machine's uploads arrived;
- approval requests;
- Brick and flow change requests;
- the organization catalog of Bricks.

Machines still upload events, ledger entries, governance changes, and alerts. The service records
only how far each of these arrived, so nothing is uploaded twice. Released Crane versions still ask
for remote quarantines, credit grants, and resident-agent decisions. The service answers with
empty lists.

## Turning uploads off

`CRANE_INGEST_URL=off` keeps everything local. Flows then shows nothing reported by that machine.
With uploads on, the hook still checks the service before tool calls, and an unreachable service
still means "deny" on those machines (a known availability trade-off).

The events machines record follow the specification in [DASHBOARD.md](../../DASHBOARD.md).
