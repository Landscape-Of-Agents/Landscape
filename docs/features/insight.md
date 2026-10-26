# Insight: learning from work

Insight looks across everything Work Capture kept and finds what repeats. It turns patterns into
proposals that a person accepts or rejects. It changes nothing by itself.

## What it finds

| Pattern | Example |
|---|---|
| **Evidence gaps** | 62% of changes in `migrations/` had no test report |
| **Unanswered questions** | "Which deploy broke refunds?" asked six times, never answered by a recipe |
| **Trust** | approvals in a zone always granted, with clean outcomes |
| **Outcomes** | work touching `migrations/` failed 4 of 6 times against 2 of 10 overall (seen together, not a cause) |
| **Work** | the same steps done repeatedly: a candidate skill |

## The honesty rules

- At least 3 cases, so no single record can be singled out.
- Every rate shows its numerator, denominator, and unknowns.
- Only what you may see counts.
- Things seen together are never called causes.

## Commands

```
crane insight report              # patterns found now
crane insight propose PATTERN_ID  # turn a pattern into a proposal (a recipe, or context for a zone)
crane insight recipes             # proposals and their status
crane insight review PROPOSAL_ID accept|reject reason TEXT    # owners and admins
```
