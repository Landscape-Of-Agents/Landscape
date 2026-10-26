# Sales and customer success

Sales agents mostly draft today (emails, quotes, proposals). The value grows as they gain the
ability to act, because every action then needs limits, a record, and proof.

## What you get

| Need | How | Status |
|---|---|---|
| Answer customer security questionnaires about AI use with evidence | the record of every agent action and human approval on code, exported | record exists; questionnaire export to build |
| Agents may not promise what policy forbids (discounts above a limit, roadmap dates) | signed business rules, checked at the tool gateway before an email or quote leaves | to build |
| Proof that a sent quote matched policy | a receipt per business action | to build |
| What our agent promised this account | the gateway's record of CRM actions | to build |

## Skills and runbooks for sales

| Example | Template | What keeps it safe |
|---|---|---|
| "Look up an account's open deals" | read-only-lookup | read only; needs a CRM operation adapter (to build) |
| "Send a renewal quote within policy" | business-record-update | a person approves the exact quote; the CRM is read back to confirm what was sent |

A skill for a CRM uses the CRM's own resource names (`crm:account-17`). Repository rules never
authorize a CRM action: an adapter must declare the `crm` namespace, and until one is registered
every such action is refused (`UNSUPPORTED_RESOURCE`).

## Controls that matter most

Approval (a person signs off anything that leaves the company or above a limit), constraint
(discount and term limits), and provenance (who sent what, under which rules).

## Honest limits

No CRM, CPQ, or email adapter exists yet, and the tool gateway is not built. Today sales benefits
from engineering's evidence (questionnaires), not from governing sales agents.
