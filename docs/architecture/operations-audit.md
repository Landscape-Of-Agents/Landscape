# Crane Operations: Audit, Architecture Mapping, and Plan

This document records the audit behind the "unified Crane operations, scope enforcement, Skill
Builder, and runbook execution" work (October 2026): what already exists, where the request and the
source documents disagree, the current-to-target mapping, what has been built, and what is waiting
on a decision. Grounding: [PRODUCTS.md](../products/strategy.md), [AGENT_INTELLIGENCE.md](intelligence.md),
[TERMINOLOGY.md](../TERMINOLOGY.md), [SECURITY.md](../../SECURITY.md), and the source tree.

**Decisions taken (October 2026).** C1: skills and runbooks are Crane-governed procedures, still
exported to vendor formats (PRODUCTS.md updated). C2: no Crane command runs in an agent session;
agents receive the resolved boundary in their session context. C5: published skills are signed into
`.crane/skills.json`. C6: the Skill Builder lives on the hosted dashboard as drafts (proposals) that a
signer activates. F1: writes into other worktrees are denied. See [section 8](#8-decisions-implemented).

Status labels: **implemented** (exists and tested), **partial**, **unverified** (exists, not
sufficiently tested), **missing**, **blocked** (needs an enforcement boundary, an integration, or a
decision first).

## Contents

1. [Conflicts between the request and the source documents](#1-conflicts-between-the-request-and-the-source-documents)
2. [Implementation inventory](#2-implementation-inventory)
3. [Current-to-target module mapping](#3-current-to-target-module-mapping)
4. [The operation contract: propose, authorize, verify](#4-the-operation-contract-propose-authorize-verify)
5. [Security findings](#5-security-findings)
6. [Phase 1 delivered: the shared scope engine](#6-phase-1-delivered-the-shared-scope-engine)
7. [Plan and dependencies](#7-plan-and-dependencies)
8. [Decisions implemented](#8-decisions-implemented)

---

## 1. Conflicts between the request and the source documents

| # | Conflict | Source | Consequence | Resolution |
|---|---|---|---|---|
| C1 | The request asks for a skill registry, a Skill Builder, and a runbook runtime. | [PRODUCTS.md §3](../products/strategy.md#3-the-verdict-on-every-earlier-product) repositions the Capability Hub ("a skill registry and runtime compete with vendor ecosystems"); [§8](../products/strategy.md#8-what-we-will-not-build-and-why) lists "a skill registry or agent runtime" under *what we will not build*. | Building it reverses a documented product decision. | **Decision needed.** Either update PRODUCTS.md (skills as Crane-governed procedures, still exportable to vendor formats), or keep skills as the capability pipeline's verified procedures and limit the work to governance of them. |
| C2 | Agent-accessible inspection and planning operations. | [SECURITY.md](../../SECURITY.md#security-model): "No `crane` command runs inside an agent session." Enforced in [cli/mod.rs](../../src/cli/mod.rs) and by the guard (`CRANE_COMMAND`). | Opening the CLI to agents changes the security model. | **Decision needed.** Recommended: no CLI access; give agents a read-only, redacted answer through the hook context (they already receive denial reasons and guidance) or the future tool gateway. |
| C3 | Proposed `src/commands/<domain>/` layout. | Commands live in [cli/](../../src/cli) (parse and print); logic lives in domain modules (`governance/`, `selection/`, `hooks/`, `capability/`, `invocation/`, …). | Moving handlers into eleven directories is churn without a behavior gain, and most domains have no user-facing command. | Keep `src/cli/<area>/` (one file per command, as `cli/governance/` already does) and domain services in their modules. Domain organization is expressed where it matters: every guard rule code maps to one of the eleven domains ([scope::rule](../../src/scope/mod.rs)). |
| C4 | "Permission" as an allow-list of what an agent may do. | Zones and flows only restrict ([zones.rs](../../src/governance/zones.rs)); files no zone covers are open to agents within policy (ceiling `delegated`). Allow-lists exist only for capability invocation (registered operations). | The repository boundary cannot be described as "granted permissions". | Report it honestly: `crane scope inspect` states the default, and gaps are listed. An allow-list for repository work would be a new AgentScript verb ("only these agents"), listed as future work in PRODUCTS.md. |
| C5 | Team-shared skills edited in a UI. | The capability registry lives in each machine's trust directory (`runtime/capability`, [cli/capability.rs](../../src/cli/capability.rs)); publishing needs a local admin key. | A skill published on one machine is not visible on another, and a revocation does not reach other machines. | **Blocked** until skills have a shared, signed store (in `.crane/`, signed through `commit()`, or on the service with GitHub-confirmed approvals). |
| C6 | A Skill Builder for HR, Product, Sales, and Engineering users "in the existing product". | Two surfaces exist: the VS Code extension ([extensions/foxx/](../../extensions/foxx), developers) and the hosted dashboard ([server/dashboard/](../../src/server/dashboard), read-only handle plus narrow GitHub-confirmed writes; it cannot sign). | Non-technical users cannot use the extension; the dashboard cannot activate anything without a signer. | **Decision needed** (surface), then: drafts on the dashboard as proposals (like flow-context proposals), activation by a signer. |
| C7 | Skills acting in CRM, HR, or finance systems. | Connectors are read-only by contract ([AGENT_INTELLIGENCE §16.3](intelligence.md#163-why-it-stays-safe)); acting needs the tool gateway (to build). Only a fixture adapter and a read-only `git.commit` adapter are registered ([invocation/adapter.rs](../../src/invocation/adapter.rs)). | No business action can be executed safely today. | **Blocked** by the gateway and per-system adapters. The contract stays domain-neutral; nothing department-specific is hard-coded. |
| C8 | Vocabulary. | AgentScript says `preserve`, the CLI says `protect`; "skill" is the capability pipeline's capability; "runbook" does not exist. | Renaming would break the CLI and signed records. | Keep names; treat "skill" as UI vocabulary for a capability; `protect` stays. |

---

## 2. Implementation inventory

### The eleven Crane operation domains

| Domain | Status | Where | Notes |
|---|---|---|---|
| Scope | **implemented** (this change) | [scope/](../../src/scope), [cli/scope/](../../src/cli/scope), [hooks/enforce.rs](../../src/hooks/enforce.rs), [governance/zones.rs](../../src/governance/zones.rs) | One engine for the hooks and `crane scope`; widening detected, not yet prevented (F2). |
| Permission | partial | guard (always-denied metadata, ceilings, modes), [capability Grants](../../src/capability/mod.rs), [governance/admins.rs](../../src/governance/admins.rs) | Restriction-only in the repository (C4). |
| Preconditions | partial | capability `Condition`, [invocation/preflight.rs](../../src/invocation/preflight.rs) | Preconditions are *confirmed by the requester*: an agent requester asserts its own preconditions (F8). |
| Constraints | implemented | preserve and target ([governance/verify.rs](../../src/governance/verify.rs)), ceilings and criticality, `PlanLimits` | |
| Approval | partial | invocation approval bound to plan digest, environment, and repository; approver a person other than the requester ([invocation/engine.rs](../../src/invocation/engine.rs)); dashboard GitHub-confirmed approvals | Hook-level `REQUIRE_APPROVAL` is answered by the agent tool's own prompt, which Crane cannot bind to the exact operation (F7). No escalation or safe timeouts. |
| Isolation | **blocked** | none | No sandbox; same-user shells can read the trust directory ([SECURITY.md](../../SECURITY.md)). The lab never executes. |
| Verification | implemented | post-tool verification ([hooks/runtime/tools.rs](../../src/hooks/runtime/tools.rs)), `crane validate` and `crane test`, invocation verification, [capability_lab/](../../src/capability_lab) | |
| Provenance | implemented | event chain ([telemetry/events.rs](../../src/telemetry/events.rs)), audit chain, signed manifest, record digests, scope decision log | |
| Budgets | implemented | autonomy credits ([telemetry/ledger.rs](../../src/telemetry/ledger.rs)), `PlanLimits`, broker budgets | Token budgets are unobserved by design. |
| Recovery | partial | invocation `reconcile` and `abandon`; `TransactionRuntime` (absent, so operations needing it are blocked); repair instructions after violations | No durable runtime. |
| Revocation | partial / unverified | capability `revoke`/`deprecate`, local and remote quarantine, `admin del` | During a run, `operation_check` rechecks role, adapter, schema, and (before externally consequential operations) staleness; per-operation eligibility after a revoke is not tested. |

### Skills, runbooks, surfaces

| Feature | Status | Where | Notes |
|---|---|---|---|
| Typed operation contract | implemented (capabilities only) | `OperationSpec`, `BindingSet`, `EffectClass` ([invocation/](../../src/invocation)) | Not used by the hooks or the scope engine (section 4). |
| Skill schema | mostly implemented | `CapabilityDefinition` ([capability/model.rs](../../src/capability/model.rs)) | Missing as fields: department, explicit approval policy, isolation requirements, budgets; test cases live in lab test plans. |
| Skill lifecycle | implemented | candidate → validated → published → deprecated/revoked; publisher ≠ author; lab; discovery; evolution | Per-machine registry (C5); adapters limited (C7). |
| Runbooks | missing | — | Capability dependencies exist; no orchestration. Blocked on C1. |
| Skill Builder UI | missing | — | Blocked on C1, C5, C6. |
| Runbook Builder UI | missing | — | Blocked on C1, C5, C6. |
| Agent-accessible inspection | missing by design | — | C2. Agents receive guidance and reasons through hooks. |
| Tool gateway (MCP) | missing | — | Component 2 of AGENT_INTELLIGENCE §10. |
| Pull-request check | missing | — | Component 1. `crane scope test` and `crane scope diff` are usable as checks. |
| Business connectors | missing | contract in [capture/resources/connector.rs](../../src/capture/resources/connector.rs) | Read-only by contract. |

---

## 3. Current-to-target module mapping

The request's proposed directories, mapped to where each responsibility really lives. "New command"
is filled only where a real user or runtime operation exists.

| Requested | Lives in | Command | Action |
|---|---|---|---|
| `scope/` | [scope/](../../src/scope) service; [cli/scope/](../../src/cli/scope) one file per command | `crane scope inspect\|resolve\|explain\|diff\|test\|audit` | **built** |
| `permission/` | guard, `Grants`, admins | answered by `scope resolve` | none |
| `precondition/` | capability `Condition`, invocation preflight | part of `capability invoke` | extend (section 4) |
| `constraint/` | governance verify, zones | `validate`, `test .`, `policy NAME status` | none |
| `approval/` | invocation engine, dashboard approvals | `capability invoke approve` | none |
| `isolation/` | — | reported by `scope inspect` and `scope test` | blocked (sandbox) |
| `verification/` | hooks post-tool, governance verify, invocation verify, lab | existing | none |
| `provenance/` | telemetry events, governance audit, capture packs | `capture verify` | none |
| `budget/` | telemetry ledger | `session` | none |
| `recovery/` | invocation reconcile and abandon | `capability invoke reconcile\|abandon` | none |
| `revocation/` | capability lifecycle, quarantines, admins | `capability revoke`, `admin del` | test mid-run revocation |
| `selection/` (`protect`, `target`) | [cli/governance/selections.rs](../../src/cli/governance/selections.rs), [selection/](../../src/selection) | `protect`, `target` | **kept in place**: the handler already only parses and prints; moving it gains nothing and the command names are public |
| `skill/` | capability, compiler, lab, discovery, invocation, evolution | `capability ...`, `compile`, `lab`, `evolution` | pending C1 |
| `runbook/` | — | — | pending C1; would compose the invocation engine, never bypass it |

No compatibility aliases were needed: no command was renamed or moved.

---

## 4. The operation contract: propose, authorize, verify

The three concepts the request separates are already separate in the code, twice:

| | Repository actions (agent tools) | Skill operations (capabilities) |
|---|---|---|
| Propose | `AgentAction` from an adapter ([hooks/action.rs](../../src/hooks/action.rs)) | `InvocationRequest` with a reviewed `BindingSet` |
| Authorize | `Guard::decide` ([hooks/enforce.rs](../../src/hooks/enforce.rs)), now also behind `scope::Boundary::decide` | `Engine::submit`, preflight, approval bound to the plan digest, `operation_check` before each dispatch |
| Verify | post-tool verification against the real file state | bound verification methods; only supported postconditions make `succeeded` |

An agent cannot turn a request into authorization in either path: policies, approvals, and success
are never read from the proposal.

Proposed next step (not built): one authorization engine for both. Extend `OperationSpec` with the
resources it touches (`kind`, `access`). For repository resources, `operation_check` asks
`scope::Boundary::decide` before dispatch, so a skill can never do in the repository what an agent
tool could not. Business-system operations carry their own resource identity and are authorized by
their adapter and their own rules; repository scope never authorizes them. Bind each precondition to
a trusted check (`Condition.check`) and stop accepting requester-confirmed preconditions for
state-changing operations requested by agents.

---

## 5. Security findings

| # | Finding | Severity | Status |
|---|---|---|---|
| F1 | A write into **another worktree** of the same repository passed the guard as "outside the repository"; that worktree's preserved selections were not enforced. | MEDIUM | **fixed**: denied (`OTHER_WORKTREE_WRITE`); probed by `crane scope test` |
| F2 | **Implicit widening**: renaming a governed file out of a zone's path selector removes its restrictions without a signature. | MEDIUM | detected by `crane scope diff` (fails); not prevented at enforcement time |
| F3 | The guard reported each preserved-selection denial **twice** (start and end markers both carry the id). | LOW | **fixed** in [hooks/enforce.rs](../../src/hooks/enforce.rs) |
| F4 | Shell **indirection** (`f=pay; sed -i … app/${f}ments.py`) passes the lexical pre-check. | MEDIUM | documented; post-action verification quarantines the bypass; reproduced as `GAP` |
| F5 | **Ignored files** and **nested repositories / submodules** are not covered by zones or flows. | LOW | reported as gaps |
| F6 | **Network** is not mediated; no **sandbox** confines shells, processes, or credentials. | MEDIUM | documented; reported on every relevant decision |
| F7 | Hook-level **approvals** are answered by the agent tool's prompt and cannot be bound to the exact operation. | MEDIUM | open; invocation approvals are bound |
| F8 | Invocation **preconditions** are confirmed by the requester. | MEDIUM | open (section 4) |
| F9 | The capability registry is **per machine**; a revocation did not reach other machines. | HIGH for team use | **fixed** for withdrawal: the signed shared store blocks withdrawn versions on every machine; sharing a version for invocation elsewhere still needs a local import |
| F10 | The symlink-escape probe needs symlink privileges on Windows. | LOW | reported as `NOT_RUN`; runs on Linux and macOS |

Repository scope is not a sandbox, and nothing in this work claims otherwise.

---

## 6. Phase 1 delivered: the shared scope engine

- **Engine** ([scope/](../../src/scope)): `Boundary` (loaded once), `enforcement()` (integrity and zone
  limits, now also used by the pre-tool hook), resource resolution, recorded and hash-chained
  decisions with validity checks, boundary snapshots and diffs, probes, and the audit. Every decision
  comes from the guard.
- **Commands** ([cli/scope/](../../src/cli/scope)): `inspect`, `resolve`, `explain`, `diff`, `test`,
  `audit`, each with `--json`, all refused to agents by the existing gate.
- **Baseline**: recorded after `governance sign`, `protect`, and `target` (best effort; it never
  undoes a signed change).
- **Guard fix**: one denial per preserved selection (F3).
- **Tests**: unit tests in each scope module (rule catalog completeness, probe text, chain
  tampering, snapshot diff, path normalization, expectations) and [tests/scope.rs](../../tests/scope.rs)
  (decisions equal to the hooks', resource resolution, explain and staleness, hook decisions by tool
  call id, widening by rename, probes, audit, agent refusal).
- **Docs**: [GUIDE.md](../surfaces/cli.md#scope-the-effective-boundary), [TERMINOLOGY.md](../TERMINOLOGY.md), `crane help`.

---

## 7. Plan and dependencies

| Phase | Work | Depends on |
|---|---|---|
| 1 | Audit and shared scope engine | — (**done**) |
| 2 | Decisions C1, C2, C5, C6, F1 | **done** (section 8) |
| 3 | Operation contract: resource requirements on `OperationSpec`, repository operations authorized by the scope engine, trusted preconditions; mid-run revocation test | phase 2 for scope of skills; F8 can start now |
| 4 | Shared, signed skill store (C5) | **done** for publication and withdrawal; import into local registries next |
| 5 | Skill Builder on the hosted dashboard (C6): plain-language draft stored as a proposal → typed definition parsed and validated by trusted code → scope and authority resolution → lab test → review by a publisher → activation; drafts grant nothing | C1, C5, C6 |
| 6 | Runbooks: ordered steps, explicit dependencies, simple branches, bounded retries, approval points, over the invocation engine (each step its own invocation identity; uncertain effects reconciled before any retry; status from verified step outcomes) | C1, phase 3 |
| 7 | Monitoring and regression through evolution; department extension points through adapters and the gateway | phases 5 and 6, tool gateway |

---

## 8. Decisions implemented

| Decision | Change | Code | Tests |
|---|---|---|---|
| C1 skills and runbooks in the product | PRODUCTS.md: Capability Hub verdict revised; Crane section describes governed skills, runbooks, and scope; *will not build* narrowed to a general skill marketplace or agent runtime | [PRODUCTS.md](../products/strategy.md) | — |
| C2 agent channel | The CLI stays closed to agents. At session start agents receive the resolved boundary: files and blocks where active zones and flows restrict changes, with each rule, and how actions are judged. Restrictions only, never the limits of enforcement. | [scope::agent_view](../../src/scope/mod.rs), [hooks/runtime/session.rs](../../src/hooks/runtime/session.rs) | `agent_view_lists_only_restrictions`, `agents_receive_the_boundary_in_their_context` |
| C5 shared skill store | `.crane/skills.json` signed under the manifest (`skills_digest`, omitted when empty so existing manifests keep their heads); written by `capability publish\|deprecate\|revoke` through `commit()`; preflight `shared` check; `crane capability shared` | [capability/shared.rs](../../src/capability/shared.rs), [governance/state.rs](../../src/governance/state.rs), [invocation/preflight.rs](../../src/invocation/preflight.rs), [cli/capability.rs](../../src/cli/capability.rs) | `shared_store_is_authoritative_for_withdrawal`, `manages_a_capability_from_a_local_pack` (publish by a second person, tamper detection, revoke) |
| C6 UI surface | Decided: hosted dashboard, drafts as proposals, activation by a signer. Not built yet (phase 5). | — | — |
| F1 other worktrees | Guard denies writes and deletes resolving into another worktree of the repository | [hooks/enforce.rs](../../src/hooks/enforce.rs), [platform/git.rs](../../src/platform/git.rs) | `writes_into_other_worktrees_are_denied` |


---

## 9. The command catalog: status per command

The catalog (14 families, 72 commands) was implemented as thin command files over shared services,
in `src/cli/<family>/` (one file per command), not `src/commands/` (decision C3: command handlers
already live in `src/cli/`). Every family command returns the shared envelope of
[cli/protocol.rs](../../src/cli/protocol.rs) with stable exit codes (0, 3, 4, 5; 1 for usage errors).

Status: **implemented** (new behavior), **reused** (a thin command over an existing service),
**partial** (works, with a named limit).

| Family | Commands | Status | Service it calls | Limits |
|---|---|---|---|---|
| scope | inspect, resolve, explain, diff, test, audit | implemented | [scope/](../../src/scope/), permission service | catalog forms (`--skill`, `--resource`, `--operation`, `--against`, `--run`, `--since`) added beside the repository forms; `--repo`, `--environment`, `--fixture` are not accepted (no backing behavior) |
| permission | inspect, check, test | implemented | [invocation/permission.rs](../../src/invocation/permission.rs) (new), scope engine | resources resolve only in namespaces an adapter declares (`repo` today) |
| precondition | inspect, validate, evaluate, test | implemented | [capability/preconditions.rs](../../src/capability/preconditions.rs) (new) | invocation preflight still accepts requester-confirmed preconditions (F8 open) |
| constraint | inspect, validate, test | implemented | engine `within_ceiling` (extracted), applicability matcher | numeric limits are the engine's; per-skill signed limits not yet expressible |
| approval | request, status, approve, deny, revoke, test | implemented | invocation engine (`deny`, `revoke_approval` new), runbook orchestrator | approval groups, quorum, expiry, and escalation are not modeled; `request` presents the pending approval created when the action paused |
| isolation | profiles, inspect, validate, test | partial | [invocation/isolation.rs](../../src/invocation/isolation.rs) (new) | no sandbox exists; `sandbox` is reported unavailable and blocks |
| verification | inspect, evaluate, report, test | implemented | claim evaluation, invocation results | domain verifiers beyond Work Capture evidence and adapter checks are to build |
| provenance | inspect, verify, export, test | implemented | invocation and run records, event store | export destinations: stdout and the runtime folder only |
| budget | inspect, evaluate, reserve, settle, release, test | implemented | telemetry ledger (runtime/budget) | run budgets count operations; time and money budgets are not metered |
| recovery | inspect, reconcile, resume, abort, test | reused | invocation engine, runbook orchestrator | compensation workflows are not modeled (abort never claims a rollback) |
| revocation | inspect, revoke, test | reused | capability registry and shared store, runbook store, approvals | no propagation bound beyond "next preflight / next step" |
| selection | protect, target, inspect, validate, repair | reused (protect, target, validate), implemented (inspect, repair) | governance and selection services | repair proposes candidates; applying one is re-protecting (signed), never an automatic rebind |
| skill | create, validate, inspect, test, plan, activate, invoke, retire, revoke | reused + implemented (`create` from templates) | capability registry, lab, invocation engine, shared store | only read-only Git operations are registered, so skills that change code or business systems are blocked |
| runbook | create, validate, inspect, plan, test, run, status, cancel, resume, revoke, **activate** (added) | implemented | [runbook/](../../src/runbook/) (new) | runbooks are stored per machine, not in the signed shared store |

**Addition to the catalog:** `runbook activate`, because the catalog's lifecycle requires a reviewed
activation before a run, and no other command could make a draft runbook active.

**Conflicts kept on the safer side:**
- The catalog allows read-only inspection by agents; no Crane command runs in an agent session
  (decision C2). Agents get the resolved boundary through their session context instead.
- The catalog's `--format text|json` is supported; `--json` keeps working.

**Fixed while implementing:** the orchestrator could proceed with a reservation the ledger had
clamped under concurrent runs; a short reservation now blocks the step.

**Tests:** unit tests for the protocol, runbook model and orchestrator (approval points, skill
approvals, branches, budgets, revocation), templates, preconditions, permission namespaces,
approval references, and budget keys; [tests/families.rs](../../tests/families.rs) through the real
binary (envelope, exit codes, fixture rule tests, a skill and a runbook end to end, selection aliases,
scope forms, the agent gate).
