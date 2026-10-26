# The eleven controls

Every skill, runbook, and agent action passes through the same families of checks. You rarely run
them by hand: Crane applies them automatically at each protected action. The commands let you
inspect, check, and test them. Each family has one command per file in `src/cli/<family>/`, and all
answer with the same envelope (`--format json`, stable exit codes: 0 ok, 3 denied or failed or
blocked, 4 approval required, 5 unknown).

| Control | Applied automatically when | Commands |
|---|---|---|
| **Scope** | every agent action and every repository operation | `scope inspect\|resolve\|explain\|diff\|test\|audit` ([scope.md](scope.md)) |
| **Permission** | every operation a skill plans | `permission inspect\|check\|test` |
| **Precondition** | before an invocation runs | `precondition inspect\|validate\|evaluate\|test` |
| **Constraint** | when a plan is built and before each operation | `constraint inspect\|validate\|test` |
| **Approval** | a plan or step that changes state or is gated | `approval request\|status\|approve\|deny\|revoke\|test` |
| **Isolation** | when a skill is validated or planned | `isolation profiles\|inspect\|validate\|test` |
| **Verification** | after execution; success only on proof | `verification inspect\|evaluate\|report\|test` |
| **Provenance** | every record is sealed with a digest | `provenance inspect\|verify\|export\|test` |
| **Budget** | before every runbook step; credits for agent sessions | `budget inspect\|evaluate\|reserve\|settle\|release\|test` |
| **Recovery** | when an effect is uncertain or a step fails | `recovery inspect\|reconcile\|resume\|abort\|test` |
| **Revocation** | before every invocation and every runbook step | `revocation inspect\|revoke\|test` |

## What each one guarantees

- **Permission.** An operation must be registered (an unknown id is denied, never interpreted); with
  a skill, it must implement one of its steps; its resource must be in the namespace its adapter
  declares. Repository resources are decided by the scope engine, so a skill can never do in the
  repository what an agent could not. Business resources (`crm:…`) need their own adapter.
- **Precondition.** A precondition holds only when its claim is proven by evidence, or as a person's
  declaration. An agent's confirmation never counts.
- **Constraint.** Engine ceilings: 64 operations, 10 minutes, 1 MiB of output, two extra retries.
  Policy may narrow them, never raise them. No wildcards; high-risk irreversible effects need an
  approval step.
- **Approval.** Bound to the exact plan or step digest; never by the requester; a denied action can
  never be approved; an unused approval can be revoked; a used one cannot.
- **Isolation.** Profiles say what is confined: `agent-hooks` (repository files partly; shell and
  network not), `typed-adapters` (no commands, typed parameters, no secrets as inputs), `sandbox`
  (not available). A skill needing what no profile enforces is blocked.
- **Verification.** PASS, FAIL, UNKNOWN, BLOCKED, or INCONCLUSIVE. A claim marked verified without
  evidence is not a pass; a required unknown keeps the result from passing.
- **Provenance.** Records, plans, approvals, and transitions are digest-bound; `provenance verify`
  names any broken link. Exports go only to stdout or the runtime folder.
- **Budget.** Reservations are atomic and idempotent; usage is read from the run's own records,
  never from the caller; nothing is settled twice.
- **Recovery.** A timeout after an effect is reconciled, not retried; aborting records unresolved
  effects and never claims a rollback.
- **Revocation.** A revoked skill is blocked here and on every machine that pulls the shared store;
  a revoked runbook stops its runs at their next step.

## The `test` commands

Each family's `test` runs that family's rules against fixtures or your skill, without touching real
systems: approvals, recovery, revocation, and budgets on fixture systems; permission, precondition,
constraint, and verification on copies of your skill; isolation and scope on your repository.
