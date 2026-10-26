# People and HR

Agents in HR schedule, summarize, and prepare paperwork. The rules here are the strictest in the
system, and some of them are not configurable.

## Lines Crane will not cross

- **No decisions about people.** Agents never rank, score, or decide on candidates or employees, and
  the system does not prepare individual assessments for hiring, promotion, discipline, or pay.
  This follows the EU AI Act (employment uses are high-risk), GDPR Article 22, and works-council
  rules, and it keeps people willing to use the system.
- **No manager views of a named person's activity** without that person's consent. People-related
  numbers are team-level, with at least 3 cases, so no one can be singled out.
- **Every agent action on people data is recorded and reviewable.**

## What you get

| Need | How | Status |
|---|---|---|
| Agents may only touch what a workflow needs | skills that name exact operations on HR records, nothing else | adapters to build |
| A person signs off every consequential step | approval points in runbooks, decided by someone other than the requester | exists |
| A record of what agents did with candidate or employee data | the same tamper-evident record as for code | exists for code; HR adapters to build |
| Which skills the organization lacks (not which person) | capability gaps from where work and reviews pile up | to build |

## Skills and runbooks for HR

| Example | Template | What keeps it safe |
|---|---|---|
| "Schedule interviews for an approved opening" | business-record-update | the calendar and applicant-tracking adapters (to build) act only on that opening; a person approves |
| "Prepare an onboarding checklist" | read-only-lookup | reads the role's checklist; changes nothing |

Never available as a skill: anything that ranks, scores, filters, or recommends people.

## Controls that matter most

Approval, scope (only the records the workflow needs), and provenance (every access recorded).

## Honest limits

No applicant-tracking or HRIS adapter exists; HR agents cannot be governed through Crane yet.
