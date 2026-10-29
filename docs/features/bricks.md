# Bricks

A **Brick** is a reusable module that solves one problem: reviewed knowledge, who may work in an
area, which files and operations are allowed, when a person decides, how much work a session may do.
You build it once and attach it to the Flows and Zones where it applies. Every Brick has exactly one
of **twelve fixed kinds**, and each kind answers one question no other kind answers. You can make as
many Bricks as you like, but you cannot invent a new kind. To solve a bigger problem, combine Bricks
into a **Stack**, or let one Brick share its configuration with another (below).

A Brick is never a source of authority. When a Brick restricts something, Crane folds the
restriction into the decision it already makes before every agent action, and it can only tighten
that decision. A Brick's context is guidance the agent receives; it never authorizes anything.

| | What it is | Where it lives |
|---|---|---|
| **Brick** | One fixed kind, typed settings, Common Context, intent, owner | drafts: `.crane/drafts/bricks/ID.json` (unsigned, grants nothing); active versions: `.crane/bricks.json` (signed) |
| **Brick Stack** | A named set of exact Brick versions, such as "Production code change" | `.crane/bricks.json` (signed) |
| **Attachment** | A Brick or Stack version pinned to a Flow, Zone, or Runbook, with its own **Current Context** | `.crane/bricks.json` (signed) |
| **Assignment** | An agent (provider or agent id) assigned to Flows and Zones | `.crane/bricks.json` (signed) |
| **Snapshot** | The assembled context agents receive for some targets, with a digest per source | trust directory (runtime) |
| **Credential** | A secret an Environment Brick names; only its name is shared | each machine's vault in the trust directory |

## The twelve kinds

Support levels: **enforced** (the guard decides with it before every agent tool call), **checked**
(evaluated by `crane brick test` and the monitors), **delivered** (agents receive it as context).

| Kind | Purpose | Only this kind | Support |
|---|---|---|---|
| Knowledge | Gives agents the reviewed documents, terms, and references they need | Puts repository documents into an agent's context; informs, never restricts | delivered |
| Identity | Says which providers and models may work in the target at all | Decides by who is calling, whatever they try to do | enforced |
| Access | Says which files agents may read or change, and how each operation is treated | Per-path, per-operation rules: scope, read-only, unreadable, auto/ask/deny | enforced |
| Approval | Names the kinds of change a person must approve, even where access allows them | Recognizes kinds of change (dependency, config, migration, push, install, large diff) | enforced |
| Limits | Caps how much and how long agents work in one session | Counts: tool calls, minutes, files and lines changed, commands, network calls, denials, ceiling | enforced |
| Environment | Defines what the work runs with: variables, credentials, network reach | Governs the surroundings: required and hidden variables, credentials by name, hosts | enforced |
| Precondition | Gates changes on facts that must hold first | Evaluated before every change; a failing gate asks a person or stops | enforced |
| Verification | Proves the outcome after work | Evidence after the fact: proven, contradicted, unknown, not run | checked |
| Action | Lists the side-effecting operations Crane may run, and keeps agents from doing them directly | Operations in other systems: allow-list, daily caps, dry runs, no direct shortcut | enforced |
| Recovery | Decides what happens after a failed, partial, or uncertain outcome | Reacts to failure: retries with reconciliation first, pause after repeated failures | enforced |
| Provenance | Requires every outcome to be traceable | Lineage: where changes came from, not whether they work | checked |
| Watch | Keeps checking the target while agents are idle, and reacts | Runs on its own schedule: signals with reactions and advisory analyses | checked |

### Settings and what really happens

| Kind | Settings | What really happens |
|---|---|---|
| Knowledge | `files`, `glossary`, `references` | Delivered with the Common Context before the agent acts in the target |
| Identity | `agents`, `models` (patterns), `on_other` (deny or ask) | Every tool call touching the target's files from another provider or model is refused (or held for a person). `crane scope` questions, which have no agent, are not decided by identity |
| Access | `include`, `exclude`, `read_only`, `deny_read`, `write`, `create`, `update`, `delete`, `execute`, `network` (each auto, ask, deny) | Unreadable files refuse reads; read-only and out-of-scope files refuse changes; each operation is allowed, held for a person, or refused. `execute` applies to commands naming the target's files, `network` while the session works in the target |
| Approval | `operations` (write, create, update, delete, execute, network, dependency_change, config_change, migration_change, test_change, git_commit, git_push, package_install, large_change), `paths`, `except`, `min_lines_changed`, `reason`, `approvers`, `min_approvals`, `distinct_approver`, `expires_minutes`, `scope_from` | A matching change needs a person. Commits, pushes, and package installs name no file, so they are held in every target whose Approval Bricks name them. The agent tool's own prompt answers; approvers and quorum apply to resident agents |
| Limits | `max_tool_calls`, `max_runtime_minutes`, `max_files_changed`, `max_lines_changed`, `max_commands`, `max_network_calls`, `max_denials`, `max_retries`, `max_ceiling`, `min_criticality`, `on_exhausted`, `scope_from` | Counted per agent session and target before every call (`runtime/bricks/usage/`); at a cap the call is refused (stop) or held for a person (escalate). The ceiling and criticality fold into the per-file limits |
| Environment | `profile`, `require_env`, `hide_env`, `credentials` (name, for), `network.allow`, `network.deny` | Commands that read a hidden variable (`$NAME`, `printenv`, environment dumps) and calls to unlisted or denied hosts are refused while the session works in the target; a missing required variable holds changes for a person; credentials reach only Crane-run operations |
| Precondition | `checks` (file_exists, file_absent, file_contains, env_set, branch_is, branch_not, git_clean, time_window, command_available, credential_present, signed_state), `on_fail` (ask or deny) | File checks fold into the per-file limits; the others run before every change. While one fails, changes need a person or stop |
| Verification | `checks` (every gate, plus policy_tests_pass, selections_resolved, http_ok, json_value, local_metric, command_succeeds), `checks_from` | Evaluated by `crane brick test` and the monitors. Endpoints and programs run only when a person runs `crane brick test` |
| Action | `operations` (adapter, operation, max_per_day, dry_run_first, approval), `deny_direct` | Resident agents may only plan the steps an Action Brick lists. With `deny_direct`, agent shell commands that do the same operation (git commit/push/tag/merge/rebase/reset, gh pr/release/workflow, package installs, kubectl/terraform apply, docker push) are refused |
| Recovery | `on_failure` (stop, escalate, retry), `max_retries` (≤3), `reconcile_before_retry`, `backoff_seconds`, `max_consecutive_failures`, `on_uncertain` | After that many failed agent tool calls in a row in the target, changes need a person. Retries and reconciliation apply to resident agents and skills |
| Provenance | `require` (signed_state, audit_chain, decisions_recorded, commit_trailer, linked_issue), `trailer`, `issue_pattern`, `commits`, `scope_from` | Verified against the signed manifest, the audit chain, the local decisions, and the recent commits touching the target |
| Watch | `signals` (governance_integrity, selections_resolved, policy_tests, denials, preconditions, verification), `on_fire` (alert, pause, revoke_due, propose), `triggers`, `analyses`, `scope_from` | Deterministic checks on every background sync. When a signal starts firing it alerts through the integrations once; `pause` holds changes in the target for a person while it fires. `ci`, `metrics`, and `external` are reported as coverage gaps |

Settings are checked against the kind's schema. Unknown fields, commands outside verification
checks, paths outside the repository, and context that looks like it contains a secret are refused.

## Sharing configuration between Bricks

Two Bricks attached to the same Flow or Zone can work as one. **`scope_from`** (Approval, Limits,
Provenance, Watch) takes the files from an Access Brick on the same target, so a scope is defined
once: an Access Brick scopes the agent to `src/api/**`, and a Provenance Brick with
`scope_from: api-access` proves the lineage of exactly those files afterwards. **`checks_from`**
(Verification) re-runs a Precondition Brick's gates after the work, so the facts that had to hold
before are proven to hold after. A share that names no such Brick on the same target cannot be
honored and fails closed: changes there need a person until an admin fixes it.

## Credentials

An Environment Brick names credentials (`credentials: [{name: jira-billing, for: [jira]}]`); the
name is shared through the signed store, the value never is. A person stores the value on each
machine with `crane secret set jira-billing` (read from standard input or a hidden prompt, never an
argument). On Windows it is sealed with the Data Protection API, elsewhere it is kept owner-only, in
the repository's trust directory, which agents cannot read. Crane-run operations use it: a Jira
integration named in `for` uses that credential as its token, and resident steps on a target whose
Environment Brick names a credential the vault lacks are unavailable until it is set. AI agents'
tool calls never receive credentials.

## Former kinds

Repositories made before the twelve kinds may hold Bricks of the former kinds (context, scope,
permission, constraint, isolation, budget, revocation, monitoring, intelligence). They keep working:
signed versions are never rewritten (their digests stay valid) and are read as the kinds that
replaced them (context as knowledge; scope and permission as access; constraint as limits, plus
access for protected paths; isolation as environment; budget as limits; revocation, monitoring, and
intelligence as watch). New Bricks cannot use them. `crane brick migrate` writes drafts of the
current kinds (the next version of each Brick, plus `ID-access` for a constraint's protected paths)
and lists what an admin then signs: activating each draft and moving the attachments to it.

## How it fits together

1. **Draft.** `crane brick create`, the Bricks panel in VS Code, or a request on DataSea. A draft
   grants nothing.
2. **Validate and test.** `crane brick validate ID` checks the schema. `crane brick test ID --target
   flow:ID` simulates the Brick on a real Flow, lists the files the guard would restrict and the
   rules it decides per call, evaluates gates and verification checks, and checks provenance.
3. **Activate.** An admin signs the version into `.crane/bricks.json` (`crane brick activate ID
   --reason TEXT`). Activation always validates again. The author cannot activate their own Brick
   while another admin can review it; a repository with one signer may, and the version records
   that it was self-reviewed. An activated version never changes: editing it starts the next
   version.
4. **Attach.** `crane brick attach ID@N --target flow:ID --context TEXT` pins an exact version to a
   target with that attachment's Current Context. Use `--dry-run` to see the before and after first.
5. **Agents work.** Before every tool call, the guard applies the Bricks' per-file limits to the
   files of attached Flows and Zones, then decides the per-call rules (identity, access, approval,
   limits, environment, gates, direct operations, recovery, watch pauses). A finding can only make
   the decision stricter, and the decision carries `brick:ID@N`. The first tool call that touches a
   target delivers its snapshot; later calls get a one-line reference while nothing changed.
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
with each tool call); then each Brick's Common Context (with a Knowledge Brick's files, glossary,
and references), once per version; then each attachment's Current Context. Every section carries
its source, reference, digest, and size. Conflicts, such as one Brick pinned at two versions or a
pin Crane cannot honor, are reported, never resolved by a guess. `crane brick context preview
--targets flow:ID` shows exactly what agents receive.

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

## Changing Bricks from DataSea

The Bricks page on DataSea can change everything the CLI can. It never holds a signing key, so it
never changes Bricks itself. Each change is a **request** that a person applies on a machine.

1. **Ask.** On the page, use New Brick, Assign an agent, or New Stack, or a row's actions: Edit,
   Activate, Attach, Deprecate, Revoke; for attachments Context, Upgrade, and Detach; Unassign; and
   Migrate former kinds. Give a reason. The page shows the configuration, Common Context, and Current
   Context the last machine reported, so you edit the real text.
2. **GitHub confirms your role.** You sign in with GitHub, and the service asks GitHub about the
   repository. Drafts (create, edit, migrate, new Stack) need write access (push, maintain, or
   admin). Signed changes (activate, deprecate, revoke, attach, detach, upgrade, context set, assign,
   unassign, Stack activate and revoke) need a repository admin. If GitHub says no, the request is
   refused, and the page says why.
3. **A person applies it.** The request waits up to 14 days. On a machine that uploads for the
   repository, use one click in Landscape ("changes requested on DataSea", then Review and apply), or
   run `crane brick inbox` and `crane brick inbox apply ID...` (or `--all`). Crane checks each
   request again and runs the same `crane brick` command a person would type. The reason records
   who asked, for example `(requested by @ana on DataSea, brq_…)`. Every validation, the
   separation-of-duties rule, and the signature apply as usual. A signed change needs that
   machine's admin key. `crane brick inbox dismiss ID --reason TEXT` declines a request.
4. **The result comes back.** The page's Requests zone shows each request as applied, failed (with
   Crane's reason), dismissed, refused, withdrawn, or expired. The next status report shows the new
   state. Whoever made a waiting request can withdraw it.

What stays on the machine:

- Requests are never applied by a sync or a hook, so an agent cannot apply one.
- Text and configurations that look like they hold a secret are refused on DataSea and refused
  again on the machine.
- Reported text that looks like a secret is withheld: the page shows that it exists, not what it
  says.
- A text longer than 16 KB is cut and not edited on the page.
- A draft made from DataSea is authored by the person who applies it. With two or more signers,
  another admin activates it.

## Commands

```
crane brick types | list | show ID[@N] | history [ID]
crane brick create --type TYPE --name NAME [--intent ...] [--config JSON] [--context TEXT]   (or --file DRAFT.json)
crane brick edit ID [...] | validate ID | test ID[@N] [--target flow:ID] | migrate [--dry-run]
crane brick activate ID --reason TEXT [--dry-run] | deprecate ID@N | revoke ID[@N] --reason TEXT
crane brick stack create --name NAME --members ID@N,ID@N | stack show ID | stack activate ID | stack revoke ID@N
crane brick attach ID@N|stack:ID@N --target flow:ID [--context TEXT] [--dry-run]
crane brick detach ATTACHMENT | upgrade ATTACHMENT --to ID@N | context set ATTACHMENT --text TEXT
crane brick context preview --targets flow:ID,zone:ID | context status
crane brick assign AGENT --targets flow:ID | unassign AGENT | agents | monitor run | monitor status
crane brick inbox | inbox apply ID...|--all | inbox dismiss ID...|--all --reason TEXT
crane secret set NAME | list | remove NAME
```

Every command takes `--format json` and returns the shared envelope. Exit codes: 0 ok or passed;
3 denied, failed, or blocked; 4 approval required; 5 unknown or unresolved. Agents cannot run any
of them.

## Where to see it

- **Landscape for VS Code**: the Bricks view and panel (graph with what each Zone and Flow
  receives, a guide per kind, the configuration pop-up of each Brick, create, attach, contexts,
  migrate, and the changes requested on DataSea). Signing changes are previewed with `--dry-run`
  and need your confirmation. See
  [surfaces/vscode.md](../surfaces/vscode.md).
- **Dashboard**: the Bricks page (Govern), with the library (configuration and Common Context per
  Brick), kinds, attachments with their Current Context, Stacks, agents, monitors, snapshots, signed
  history, F37 events, the requests made there, and actions for every change (see above). See
  [surfaces/dashboard.md](../surfaces/dashboard.md).
- **Events**: F37 in [DASHBOARD.md](../../DASHBOARD.md). A decision the guard made because of a
  Brick carries `brick:ID@N` next to its Zones and Flows, and its rule (`BRICK_IDENTITY`,
  `BRICK_DENY_READ`, `BRICK_ACCESS_MODE`, `BRICK_APPROVAL`, `BRICK_LIMIT`, `BRICK_NETWORK`,
  `BRICK_HIDDEN_ENV`, `BRICK_ENV_MISSING`, `BRICK_PRECONDITION`, `BRICK_ACTION_DIRECT`,
  `BRICK_RECOVERY_PAUSE`, `BRICK_WATCH_PAUSE`).

## Limits today

| Gap | Status |
|---|---|
| Approval uses the agent tool's prompt (not bound to the exact change, F7); approvers and quorum apply to resident agents only | partial |
| A command that names no file is attributed to a target only for commits, pushes, installs, deployments, network, and hidden variables | partial |
| A new file belongs to the targets of the files in its folder | partial |
| No sandbox; an Environment profile other than agent-hooks only makes targets stricter | blocked by the missing sandbox |
| Credentials reach integrations by name (`for`), not per target; agents never receive them | by design |
| Runbook attachments are recorded, not applied by the runbook runtime | to build |
| The dashboard does not sign or validate: its changes are requests a person applies on a machine | by design |
| Requests from the dashboard have no dry-run preview; Crane's checks run when they are applied | partial |
| Monitors only run when a sync or command runs (no daemon); ci, metrics, external have no connector | partial |
| Nested Stacks | not supported (one level, on purpose) |
