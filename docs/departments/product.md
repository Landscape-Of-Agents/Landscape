# Product

Product managers rarely run agents themselves; they need to know whether agent work is really done.

## What you get

| Need | How | Status |
|---|---|---|
| "Done" should mean proven, not "the agent said so" | every change's evidence: proven, contradicted, unknown and why | engine exists; receipts on pull requests to build |
| Acceptance criteria checked from the code | an acceptance criterion becomes a `target` rule: the code must change, in a stated way | exists for code; feature rollup to build |
| A feature's status across many changes | feature rollup per tracker item (Jira, Linear) | to build |
| What customers say about an area, next to what changed | support and feedback connectors | to build |

## Skills and runbooks for product teams

Agents that draft specifications, analyses, or release notes work through skills like any other:

| Example | Template | Notes |
|---|---|---|
| "Collect the evidence that a feature shipped" | read-only-lookup | reads trackers and pull requests once their connectors exist |
| "Update the release record after sign-off" | business-record-update | the update waits for a person's approval; the record is read back to prove it changed |

Creating a skill needs no command line once the dashboard Skill Builder exists (to build): describe
what it should do, who owns it, what it reads and changes, what needs approval, and how success is
checked. Until then, `crane skill create --from-template ...` drafts one.

## Controls that matter most

Verification (a feature is done only when its claims are proven) and approval (a person signs off
what leaves the team). See [features/controls.md](../features/controls.md).

## Honest limits

Tracker connectors and the feature rollup are not built. Today a product manager sees proof per
change through engineering's evidence, not per feature.
