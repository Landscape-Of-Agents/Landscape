# Bricks

A **Brick** is a reusable module that solves one problem: a piece of reviewed context, a hard limit,
an approval rule, a monitor. You build it once and attach it to the Flows and Zones where it
applies. Every Brick has exactly one of a **fixed set of types**. You can make as many Bricks as you
like, but you cannot invent a new type. To solve a bigger problem, combine Bricks into a **Stack**.

A Brick is never a source of authority. When a Brick restricts something, Crane folds the
restriction into the decision it already makes before every agent action, and it can only tighten
that decision. A Brick's context is guidance the agent receives; it never authorizes anything.

| | What it is | Where it lives |
|---|---|---|
| **Brick** | One fixed type, typed settings, Common Context, intent, owner | drafts: `.crane/drafts/bricks/ID.json` (unsigned, grants nothing); active versions: `.crane/bricks.json` (signed) |
| **Brick Stack** | A named set of exact Brick versions, such as "Production code change" | `.crane/bricks.json` (signed) |
| **Attachment** | A Brick or Stack version pinned to a Flow, Zone, or Runbook, with its own **Current Context** | `.crane/bricks.json` (signed) |
| **Assignment** | An agent (provider or agent id) assigned to Flows and Zones | `.crane/bricks.json` (signed) |
| **Snapshot** | The assembled context agents receive for some targets, with a digest per source | trust directory (runtime) |

## The fixed types and what each does today

Support levels: **enforced** (the guard decides with it before every action), **checked** (evaluated
by `crane brick test` and the monitors), **delivered** (agents receive it as context), **declared**
(validated and recorded; nothing acts on it yet).

| Type | Settings | Support | What really happens |
|---|---|---|---|
| Context | `files` | delivered | Common Context and files are delivered before the agent acts in the target |
| Identity | `agents` | checked | Not admitting the acting agent is reported to it and noted (F37); the guard does not decide per agent yet |
| Scope | `include` | enforced | Files of the target outside the scope become read-only. A scope that matches nothing makes the whole target read-only |
| Permission | `allow` | enforced | Without `write`, the target becomes read-only. `execute` and `network` limits are delivered, not enforced per file |
| Constraint | `max_ceiling`, `min_criticality`, `protected_paths` | enforced | Lowers the ceiling, raises the criticality, makes protected paths read-only |
| Precondition | `checks` (file_exists, file_absent, signed_state) | enforced | While a check fails, every change in the target needs approval |
| Approval | `operations`, `reason` | enforced | Write or delete approval makes the target "assisted". The agent tool's own prompt answers it, so it is not bound to the exact change |
| Isolation | `profile` | enforced | Agent tools run under `agent-hooks`. Requiring another or an unavailable profile (`sandbox`) makes direct changes read-only. No sandbox exists |
| Budget | `max_tool_calls`, `max_retries`, `max_runtime_minutes`, `on_exhausted` | declared | Validated and delivered; only the existing autonomy credits limit agents |
| Action | `adapter`, `operation` | checked | Must name a registered operation (today `git/git.commit`). A Brick never executes anything |
| Verification | `checks` | checked | Proven, contradicted, unknown, or not run (policy tests, selections, files, signed state) |
| Provenance | `require` (signed_state, audit_chain) | checked | Verified against the signed manifest and the audit chain |
| Recovery | `on_failure`, `max_retries` (≤3), `reconcile_before_retry` | declared | Retries require reconciliation first; the skill invocation engine applies it |
| Revocation | `triggers` | checked | Monitors report a revocation as due. Revoking is always a signed command by a person |
| Monitoring | `signals` (source, every_minutes ≥ 5, threshold) | checked | Deterministic checks on every background sync. `ci`, `metrics`, and `external` are reported as coverage gaps |
| Intelligence | `analyses`, `advisory_only` (must be true) | checked | Denial patterns and stale context produce proposals only |

Settings are checked against the type's schema. Unknown fields, commands, scripts, paths outside
the repository, and context that looks like it contains a secret are refused.

## How it fits together

1. **Draft.** `crane brick create`, the VS Code Bricks view, or a dashboard draft file. A draft
   grants nothing.
2. **Validate and test.** `crane brick validate ID` checks the schema. `crane brick test ID --target
   flow:ID` simulates the Brick on a real Flow and lists exactly which files the guard would
   restrict.
3. **Activate.** An admin signs the version into `.crane/bricks.json` (`crane brick activate ID
   --reason TEXT`). Activation always validates again. The author cannot activate their own Brick
   while another admin can review it; a repository with one signer may, and the version records
   that it was self-reviewed. An activated version never changes: editing it starts the next
   version.
4. **Attach.** `crane brick attach ID@N --target flow:ID --context TEXT` pins an exact version to a
   target with that attachment's Current Context. Use `--dry-run` to see the before and after first.
5. **Agents work.** Before every tool call, the guard applies the Bricks' limits to the files of
   attached Flows and Zones. The first tool call that touches a target delivers its snapshot;
   later calls get a one-line reference while nothing changed.
6. **Change or withdraw.** `upgrade`, `context set`, `detach`, `deprecate`, and `revoke` are all
   signed. Detaching keeps the history in the audit trail. Revoking a version makes attachments
   pinned to it **fail closed**: changes in the target need approval until an admin detaches or
   upgrades them.

## Context: Flow, Common, and Current

| Context | Belongs to | Changes affect |
|---|---|---|
| Flow or Zone context | the Flow or Zone in `governance.yaml` (context packs) | that Flow or Zone |
| Common Context | the Brick version | every target the version is attached to |
| Current Context | one attachment | only that attachment |

A snapshot always has the same order: first the enforced controls, stated as facts so no context
can be read as relaxing them; then Flow and Zone context (in previews; the hooks already send it
with each tool call); then each Brick's Common Context, once per version; then each attachment's
Current Context. Every section carries its source, reference, digest, and size. Conflicts, such as
one Brick pinned at two versions or a pin Crane cannot honor, are reported, never resolved by a
guess. `crane brick context preview --targets flow:ID` shows exactly what agents receive.

**Caching.** The hooks remember which snapshot digest each agent session received for each set of
targets. An unchanged snapshot is not sent again. Any change to the signed store, governance, a
context file, or a revocation changes the digest, and the full snapshot is sent. A new, resumed,
or compacted session receives everything again. The cache never authorizes anything: every action
is still decided at that moment. Crane counts deliveries and reuses (`crane brick context status`).
Whether a model provider caches prompts is not observable, so no token savings are claimed.

## Agents that stay assigned while idle

`crane brick assign claude --targets flow:checkout,zone:payments` records an assignment.
`crane brick agents` shows its **assignment status** (active, paused, revoked) separately from its
**runtime status** (idle, running, waiting for approval, degraded, error, paused, revoked), derived
from local sessions, events, and monitors. No process runs a model while agents are idle. Monitors
are deterministic checks run by the background sync that hooks and commands already start, and by
`crane brick monitor run`. To run them on a schedule, call that command from cron or Task
Scheduler. There is no Crane daemon. Model tokens are not observed, so inference is shown as
"not measured".

## Commands

```
crane brick types | list | show ID[@N] | history [ID]
crane brick create --type TYPE --name NAME [--intent ...] [--config JSON] [--context TEXT]   (or --file DRAFT.json)
crane brick edit ID [...] | validate ID | test ID[@N] [--target flow:ID]
crane brick activate ID --reason TEXT [--dry-run] | deprecate ID@N | revoke ID[@N] --reason TEXT
crane brick stack create --name NAME --members ID@N,ID@N | stack show ID | stack activate ID | stack revoke ID@N
crane brick attach ID@N|stack:ID@N --target flow:ID [--context TEXT] [--dry-run]
crane brick detach ATTACHMENT | upgrade ATTACHMENT --to ID@N | context set ATTACHMENT --text TEXT
crane brick context preview --targets flow:ID,zone:ID | context status
crane brick assign AGENT --targets flow:ID | unassign AGENT | agents | monitor run | monitor status
```

Every command takes `--format json` and returns the shared envelope. Exit codes: 0 ok or passed;
3 denied, failed, or blocked; 4 approval required; 5 unknown or unresolved. Agents cannot run any
of them.

## Where to see it

- **Landscape for VS Code**: the Bricks view, with create, validate, test, activate, attach,
  Current Context, detach, upgrade, revoke, and preview. Signing changes are previewed with
  `--dry-run` and need your confirmation. See [surfaces/vscode.md](../surfaces/vscode.md).
- **Dashboard**: the Bricks page (Govern), with the library, types, attachments, Stacks, agents,
  monitors, snapshots, signed history, F37 events, and a draft builder. See
  [surfaces/dashboard.md](../surfaces/dashboard.md).
- **Events**: F37 in [DASHBOARD.md](../../DASHBOARD.md). A decision the guard made because of a
  Brick carries `brick:ID@N` next to its Zones and Flows.

## Limits today

| Gap | Status |
|---|---|
| Identity is not decided per agent by the guard | partial: checked, reported to the agent, noted |
| `execute`, `network`, and separate `delete` permissions are not enforced per Flow or Zone | partial: delivered and reported as gaps |
| Approval Bricks use the agent tool's prompt (not bound to the exact change, F7) | partial |
| No sandbox; Isolation Bricks can only make targets stricter | blocked by the missing sandbox |
| Budget and Recovery have no per-Brick meters | declared |
| Runbook attachments are recorded, not applied by the runbook runtime | to build |
| The dashboard cannot sign or validate; its builder produces a draft file a member imports | partial |
| Monitors only run when a sync or command runs (no daemon); ci, metrics, external have no connector | partial |
| Nested Stacks | not supported (one level, on purpose) |
