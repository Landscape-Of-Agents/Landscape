# Finance

Finance needs two things from an agent program: what it costs per useful result, and proof that
agents never moved money outside policy.

## What you get

| Need | How | Status |
|---|---|---|
| Cost per verified change (not cost per seat) | token use and credits joined to verified outcomes | data partly exists; the report is to build |
| Agent spend that produced nothing | changes that never verified, with their cost | to build |
| Purchases or refunds that bypassed policy | the gateway's record of actions in billing and spend tools | to build |
| Approval limits that hold | constraints and approval points, decided before the action | exists for skills; finance adapters to build |
| Budgets agents cannot exceed | run budgets reserved atomically and settled from what really happened | exists |

## Skills and runbooks for finance

| Example | Template | What keeps it safe |
|---|---|---|
| "Reconcile last month's agent spend" | read-only-lookup | read only |
| "Issue a refund up to a limit" | business-record-update | a person approves above the limit; the billing system is read back; a refund that timed out is reconciled, never sent twice |

## Controls that matter most

Constraint (amount limits), approval, budget, and recovery (an uncertain payment is never retried
blindly).

## Honest limits

Token use is visible only where the agent tool reports it; Crane does not observe network or model
spend directly. No billing or ERP adapter exists yet.
