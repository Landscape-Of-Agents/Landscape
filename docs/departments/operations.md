# Operations and SRE

Operations and support are the first business functions where agents take actions with real
consequences (restarts, refunds, account changes), which is when control and proof matter most.

## What you get

| Need | How | Status |
|---|---|---|
| What changed before this incident, and which agent made it | the change's agent trail and its evidence | partial: record and evidence exist; incident and deployment connectors to build |
| Stop a runaway agent from anywhere | remote quarantine from the dashboard | exists |
| Team monitors that open incidents | Dashboard Agents | exists |
| Runbooks that never repeat a risky action blindly | runbooks over skills: uncertain effects are reconciled first, retries only without effects | exists |
| Agents act on infrastructure only within signed limits | the tool gateway with zone-like rules for environments and services | to build |

## A runbook in operations

"Triage a failed deployment": collect the deployment, its checks, and its logs (read-only skills);
if the outcome is confirmed, a step branches to "open an incident"; a step that would restart a
service has an approval point. Each step is its own invocation, so each one is authorized,
budgeted, and verified on its own. If a step times out after the effect may have happened, the run
pauses for reconciliation (`crane recovery reconcile`) instead of trying again.

```
crane runbook create --file triage.json
crane runbook validate triage@1 && crane runbook test triage@1
crane runbook activate triage@1 --review-ref CHG-142        # by a person other than the author
crane runbook run triage@1 --input inputs.json --idempotency-key INC-881
crane runbook status rbr_...
```

## Controls that matter most

Recovery (reconcile, never blind retries), revocation (stop a runbook mid-run), budget (operations a
run may use), and approval.

## Honest limits

No deployment, incident-tool, or observability connector exists, and no state-changing
infrastructure adapter: today runbooks run read-only operations and fixture systems in tests.
