# Runbooks

A runbook is a versioned workflow that runs exact skill versions in order. It decides the order,
branches, approval points, budget, and the run's final status. It never gives a skill more authority:
every step is its own invocation with that skill's full checks.

## What a runbook can say

| Element | Meaning |
|---|---|
| **Steps** | Each pins a skill version and digest, with its reviewed binding set and inputs |
| **Inputs** | A fixed value, a run input, or a field produced by an earlier step |
| **Dependencies** | `depends_on`: steps that must finish first (cycles are refused) |
| **Branches** | `when`: run only if an earlier step succeeded, failed, or was blocked |
| **Approval points** | `approval: true`: a person other than the run's starter approves the exact step |
| **Retries** | `max_retries` (at most 2), only for a step that had no effect |
| **Failure policy** | `stop` (default) or `continue` for independent steps |
| **Completion** | The steps that must succeed; default every step without a branch |
| **Budget** | The most operations the run may use, retries included |
| **Target** | The environment and repository the skills act in |

## Commands

```
crane runbook create --file triage.json          # or: --from-template linear --skills A@1,B@2 --bindings a.json,b.json
crane runbook validate triage@1                  # structure, pinned skills, operations, isolation
crane runbook test triage@1                      # what a run must refuse; nothing is executed
crane runbook plan triage@1 --input in.json      # each step's plan, cost, approvals; no effects
crane runbook activate triage@1 --review-ref R   # a person other than the author
crane runbook run triage@1 --input in.json --idempotency-key K1
crane runbook status rbr_...
crane approval request --run rbr_...             # what waits, and its approval reference
crane approval approve rbr_...:restart@DIGEST    # by another person; the run continues
crane runbook resume rbr_... [--from-step STEP]  # after reconciling, or to retry a step without effects
crane runbook cancel rbr_... --reason TEXT
crane runbook revoke triage@1 --reason TEXT      # no new runs; running runs stop at their next step
```

## How a run behaves

- Before each step: the runbook must still be active, its approval point approved, and its
  operations reserved from the budget (a short reservation blocks the step).
- The step is invoked: preflight, the skill's own approval if needed, execution through registered
  operations, verification.
- **Succeeded**: proven. **Failed** or **blocked**: the failure policy decides; branches can take
  over. **Waiting for approval**: the run pauses. **Uncertain**: the run pauses for reconciliation
  (`crane recovery reconcile`), and nothing is retried.
- The budget is settled from the invocation's own record: operations that started are consumed, the
  rest released; an uncertain outcome consumes everything until reconciled.
- The run succeeds only if every required step succeeded with proof.

## Honest limits

Runbooks are stored per machine (`runtime/runbook/`), not yet in the signed shared store. They run
the operations that exist, so today that means read-only Git operations, plus fixture systems in
tests.
