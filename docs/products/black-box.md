# Black Box: the flight recorder for AI agents (paid)

Aviation became safe by recording every flight and studying near misses without blame. Black Box
does the same for AI agents, because Crane is present at every decision: it records what agents
*tried*, not only what they produced.

## Who it is for

CTOs and VPs of Engineering together with the CISO (buyers), tech leads and platform and security
teams (daily users), and compliance, the CEO, and the board (report readers). Strongest fit:
fintech, healthtech, insurance, infrastructure, public sector, and anyone who has had an agent
incident.

## What it does

| Capability | What you get | Status |
|---|---|---|
| **Flight recorder** | A replayable, chain-verified timeline per agent session: each attempt, the rule that applied, the decision, what really changed, credits, and tokens | record exists; replay viewer to build |
| **Near-miss register** | Every attempt classified: near miss (stopped), incident (caught after the effect), serious incident (bypass), accident (bypass that reached production) | data exists; classifier to build |
| **Barrier analysis** | Which check stopped each near miss, and how often an action passed every check until the after-check caught it | data exists; reports to build |
| **Earned autonomy** | Per zone, evidence that supports widening or narrowing agent limits, as proposals a person signs | to build |
| **Role views** | Developers (private by default), tech leads, security, compliance, CEO and board | dashboard exists; role views to build |
| **Export** | Events to OpenTelemetry and SIEMs; signed compliance and board reports | CSV exists; the rest to build |

## Why only this system can do it

Observability and analytics tools see results. They never see the attempt that was stopped, so they
cannot count near misses or measure which barrier worked. Black Box can, across every agent vendor,
with a chain of custody, and with history that starts the day it is installed.

## What it reads

Decision events (each attempt and its rule), verification events (what really changed), identity
(person, agent, session, task), the credit ledger, governance changes, and the flow call graph. See
[features/agent-hooks.md](../features/agent-hooks.md) and [DASHBOARD.md](../../DASHBOARD.md).

## Honest limits

- Severity is coarse until counterfactual fields and per-check codes are emitted (to build).
- Few rules mean few near misses, which can look like safety; reports must show how much is governed.
- Person-level views must stay opt-in, or developers will turn recording off.
- Cloud-agent sessions contribute only what the pull request shows until the gateway exists.

Pricing and sequencing: [strategy.md](strategy.md#12-pricing-sequencing-and-first-customers).
