# Evidence Receipts: independent proof for every change (paid)

As agents produce more pull requests than people can review, the question moves from "is this diff
good?" to "what is proven about this change?". A receipt answers it, from an independent verifier,
for every pull request and deployment.

## What a receipt says

- **Proven**, and by which evidence.
- **Contradicted**, and by what.
- **Unknown, and why**: empty (looked, nothing there), unavailable, denied, not queried, or could
  not be parsed.
- Which zones and policies the change touched, which rules applied, and which near misses happened
  while it was made (from Black Box).
- Integrity: the evidence pack's digest and whether replaying the investigation gives the same
  answer.

## Who it is for

Reviewers and tech leads who approve agent pull requests, release managers, product managers
(feature status), quality and platform teams (what keeps going unproven), and later on-call
engineers.

## What it does

| Capability | Status |
|---|---|
| Evidence Packs: evidence about a change, with a reason for every gap | exists ([features/evidence.md](../features/evidence.md)) |
| Learned checklists: questions people found answered well become reusable | exists ([features/insight.md](../features/insight.md)) |
| Verified procedures: work repeatedly proven becomes a skill | exists ([features/skills.md](../features/skills.md)) |
| Receipt renderer and pull-request check (pass when proven, block on "no", ask a person on "unknown") | to build |
| Feature rollup for product managers (acceptance criteria as `target` rules) | to build |
| Verification debt: recurring gaps ranked by risk and routed to their owner | to build |
| Test-report, deployment, log, and metrics connectors | to build |

## Why it is independent

A CI platform can post checks but cannot see the attempt history; an agent vendor can summarize its
own work, but a receipt written by the party that did the work is not independent. Receipts join
evidence (with reasons for every unknown) to the decision-time record of how the change was made.

## Honest limits

- Without live test and deployment connectors, many receipts say "unknown" for those parts. That is
  honest, and it is shown as the gap to close.
- The evidence store is local files today; hosted, automatic investigations need a hosted store.
- Outcomes of work are recorded by hand today.

Pricing and sequencing: [strategy.md](strategy.md#12-pricing-sequencing-and-first-customers).
