# The dashboard

The hosted dashboard ([dashboard.agentslandscape.cloud](https://dashboard.agentslandscape.cloud)) shows
a team's agents in one place. Machines upload signed, redacted records to the DataSea service; the
dashboard reads them. Enforcement never depends on it: it all happens on each machine.

## Sign in and visibility

Sign in with GitHub. You see the repositories GitHub lets you see; there is no separate user
administration. Each machine is linked to a person once (`crane agent link`).

## What is on it

| Area | What you see | Status |
|---|---|---|
| **Activity** | Every decision and effect: who, which agent, which rule, what changed | exists |
| **Agents and sessions** | Each agent's state (active, degraded, quarantined, stale) and its timeline | exists |
| **Govern** | Zones, flows, and call graphs; the audit trail of rule changes; compliance posture | exists |
| **Operate** | Alert rules, credits, Dashboard Agents (team monitors), incidents | exists |
| **Approvals** | Quarantines and their releases, confirmed against GitHub admin rights | exists |
| **Reports** | CSV exports; the agent-context API for tools | exists |
| **Bricks** | The library by type and state, the fixed types and what each does today, attachments and Stacks, assigned agents (assignment and runtime status), monitors and coverage gaps, context snapshots, signed history, F37 events | exists (as last reported by a member's machine) |
| **Brick draft builder** | Plain form: type, intent, Common Context, settings; downloads a draft a member imports with `crane brick create --file` | partial: the dashboard cannot validate or sign |
| **Resident agents** | Assignments with lifecycle and derived status (idle, investigating, awaiting approval, degraded), the review inbox, each activation from trigger to verified outcome with Crane's decision per step, idle cost (model calls while idle) and health, plan-only test reports, F38 events | exists (as last reported by a member's machine) |
| **Resident agent wizard** | Eight steps (goal, target, context, triggers, actions and limits, RCA, verification, review); downloads the definition and assignment drafts, and test events, a member imports | partial: the dashboard cannot validate, activate, or approve |
| **Skill Builder** | Plain forms to draft skills; drafts are proposals a signer activates | to build (decided) |
| **Near misses, replay, role views** | The Black Box views | to build |
| **Receipts** | Proof per pull request | to build |

## What the dashboard may change

Only narrow, repository-scoped writes: linking a machine to a person, confirming the owner key,
granting credits, Dashboard Agent settings and incidents, quarantine requests, and context
proposals. Sensitive ones need a fresh GitHub check that you are a repository admin. It cannot sign
rules: signing needs an owner or admin key on a machine.

## Stopping an agent

A remote quarantine (by agent, session, or tool) makes that agent's hooks deny further actions. With
uploads on, an unreachable service also means "deny", so an outage stops agent work on those
machines (a known availability trade-off).

## Turning uploads off

`CRANE_INGEST_URL=off` keeps everything local. The dashboard then shows nothing for that machine,
and remote quarantines cannot reach it.

Every event the dashboard shows follows the specification in [DASHBOARD.md](../../DASHBOARD.md).
