# Crane guide

How to install and use Crane, the Landscape extension, and the hosted dashboard. For the vision, see the
[README](../README.md); to work on Crane, see [CONTRIBUTING](../CONTRIBUTING.md); to report a
vulnerability, see [SECURITY](../SECURITY.md).

## Install

Download Crane for Linux, Windows, or macOS from the
[latest release](https://github.com/Landscape-Of-Agents/Landscape/releases/latest), unpack it, and put
`crane` on your `PATH`. Or build it from source:

```bash
cargo install --path .        # requires Rust 1.89+ and Git
crane --version
```

`gh` is needed for `crane github`; `curl` for Slack, Jira, and WhatsApp delivery.

The **Landscape** VS Code extension sets up Crane, creates policies, and marks selections from the
editor: install it from the
[Marketplace](https://marketplace.visualstudio.com/items?itemName=agentscript.foxx)
(`code --install-extension agentscript.foxx`), or from the `.vsix` on each release.

## Quick start

```bash
crane repo --https https://github.com/OWNER/REPO    # connect the clone to its GitHub repository
crane init                                          # create .crane/ and a signing key
crane checkpoint baseline                           # trusted baseline = current HEAD

crane protect app/payments.py start-line 12 end-line 30 context "Fees are audited; keep the rate."
crane target  app/refunds.py  start-line 5 end-line 18 change_type logical_bn context "Refunds must be partial-aware."

crane validate          # integrity, syntax, and markers
crane test .            # run every preserve/target command
crane agent install --profile claude
```

Then open the dashboard at <https://dashboard.agentslandscape.cloud> and sign in with GitHub.

Commit `.crane/` with your code. Policies live in `.crane/policies/*.crane`:

```text
policy payments {
    preserve K7M2P9RX;
    target 4QZ8M2LD change_type logical_bn;
}
```

A policy passes only if every command passes. `change_type` is one of `logical_bn` (business
logic), `logical_cn` (loops or branches), `logical_sn` (structure), or `semantic` (names or
comments); whitespace-only changes never satisfy a target.

## Commands

| Area | Commands |
| --- | --- |
| Basic | `--version`, `init`, `checkpoint [NAME]`, `protect FILE ...`, `target FILE ...`, `--set default policy\|checkpoint NAME`, `parse file FILE`, `validate`, `test .`, `add policy-context file FILE [policy NAME]`, `policy-context NAME --view` |
| Repository | `repo --ssh\|--https\|--gh-cli REMOTE [privileges]`, `repo status`, `repo --view`, `repo del`, `repo rebind OLD_REMOTE`, `github`, `github days DAYS` |
| Policies | `create policy NAME FILENAME`, `add policy NAME marker MARKER`, `policy NAME status` |
| Runtime | `task current [ID]`, `task .`, `session current [ID]`, `session .` |
| Agents | `agent install\|uninstall\|hooks --profile claude\|codex\|copilot\|gemini`, `agent link` |
| Admins | `admin list`, `admin register`, `admin add PUBLIC_KEY [name NAME]`, `admin del PUBLIC_KEY` |
| Zones and flows | `governance sign`, `governance graph [--json]` (zones, flows, and selections as an acyclic graph, with the agents active in each) |
| Work Capture | `capture demo [--json]`, `capture sources [--json]`, `capture investigate [commit SHA] [task TASK_ID] [--json]` |
| Capabilities | `capability demo`, `capability intake ...`, `capability create\|propose FILE`, `capability validate (ID [--dry-run] \| file FILE)`, `capability inspect\|versions\|list`, `capability publish ID version N reason TEXT`, `capability deprecate\|revoke\|reject ...`, `capability stale` |
| Integrations | `integrate jira\|slack\|whatsapp\|github` |

`protect` and `target` require `context TEXT` (guidance agents receive whenever they touch the
selection; it is signed into the record). Options: `policy NAME`, `checkpoint NAME`,
`start-line N`, `end-line N`, `name LABEL`. Run `crane help` for details.

**Only people run Crane.** Every `crane` command refuses to run inside an AI agent session (the
hooks themselves excepted), and the hooks deny any agent shell command or script that invokes
`crane`, including `crane validate` and `crane --version`.

**Moving the repository.** After a transfer or rename (for example to another organization), the
repository identity changes with the remote, so `crane validate` reports the registry as belonging
to another repository. Update the remote (`git remote set-url origin NEW_URL`), then the owner runs
`crane repo rebind OLD_REMOTE` once: Crane proves the rules were signed for the old remote, carries
the trust over, and signs the selections, admin list and manifest again for the new identity.
Commit the result; teammates run the same command after pulling, then `crane repo --https NEW_URL`.

## Teams

The person who runs `crane init` first is the **owner**; `crane init` prints the owner key.

1. **Teammates join.** On their machine: `crane repo --https URL`, then `crane init`. Crane shows
   the owner key's fingerprint and asks whether to trust it; compare it with the owner's
   `crane admin list`. Until the key is trusted, the rules cannot be verified and agent changes are
   blocked. After it is, enforcement works normally. Teammates who are not admins cannot change
   the rules.
2. **More admins.** A teammate sends the key `crane init` printed for them; the owner runs
   `crane admin add KEY name NAME` and commits `.crane`. Admins can then protect, target,
   checkpoint, and change policies from their own machines. Every teammate trusts them through
   the owner's signature. `crane admin del KEY` removes an admin; machines that saw the removal
   reject an older admin list.
3. **Two admins changed the rules on different branches.** Git reports a conflict in `.crane`.
   Keep one side for the whole directory (`git checkout --theirs .crane`), commit, run
   `crane init` to accept that signed state, then re-apply your change with crane.

CI machines can trust the owner without a prompt: set `CRANE_TRUSTED_PUBLIC_KEYS` to the owner key.

**GitHub vouches for the owner key.** The owner runs `crane admin register` (a new repository's
`crane init` offers it) and opens the printed address; a GitHub admin of the repository confirms
the key there. A teammate's `crane init` then shows "confirmed by @admin" next to the fingerprint,
or a warning when the repository's key is not the confirmed one. Also protect `.crane` on GitHub:
add `/.crane/ @OWNER` to `CODEOWNERS` and require code-owner review on the default branch.

**Machines are linked to people.** Each teammate runs `crane agent link` once per machine and opens
the printed address; GitHub must show them access to the repository. Once any machine is linked,
the dashboard shows only linked machines by default, so an upload key nobody linked (anyone can
create one for a public repository) cannot pass as a teammate.

## Zones, flows, and context packs

Optional `.crane/governance.yaml` adds boundaries that only ever restrict:

```yaml
version: 1
zones:
  - id: payment-core
    selectors: { paths: ["app/payments/**"] }
    criticality: critical          # routine | sensitive | critical | restricted
    autonomy_ceiling: assisted     # observe | assisted | delegated | autonomous
flows:
  - id: checkout
    entry_points: ["app/service.py:submit"]   # files reached by its calls are covered
    autonomy_ceiling: delegated
context_packs:
  - id: payment-invariants
    version: 1
    content: ["Payment processing must be idempotent."]
```

After editing the file, an admin runs `crane governance sign` (Landscape does this for you). The file is
covered by the signed manifest, so an edit nobody signed fails `crane validate` and stops agent
changes until it is signed or restored. Criticality adds credit cost per edit (sensitive +1,
critical +3, restricted +5), and restricted code always needs human approval.

Zones and flows can also name exact source blocks with comment markers. Use the file's ordinary
comment syntax and the governance entity's ID:

```python
# @crane:zone:payment-core:start
def capture(order):
    ...
# @crane:zone:payment-core:end
```

```javascript
// @crane:flow:checkout:start
function submit(order) {
  ...
}
// @crane:flow:checkout:end
```

For block-comment languages, wrap the marker text in the language's normal block comment (for
example `<!-- @crane:zone:payment-core:start -->`). Source markers must resolve to a declared zone
or flow, occur once as a matching start/end pair, and be properly nested; malformed, crossed,
duplicate, and orphan markers prevent governance validation/signing. Markers are signed with
`crane governance sign`: the zone and flow blocks present at signing are recorded in the signed
configuration, and a block added, removed, or moved to another file afterwards fails validation and
stops agent changes until an admin signs again. Every flow needs context: at least one context pack
with content or files (`context_packs: [ID]`), or signing is refused. An entity's source block
enforces limits only when a proposed edit intersects the lines between its markers. Existing path,
file, directory, symbol, selection, flow-entry, and flow-include selectors retain their existing
file-level behavior. Marker edits themselves are always denied to agents.

On every tool call that reaches an active zone, flow, or selection, Crane sends its attached
guidance to the agent and displays it in the agent window. Selection notices include the relevant
policy context files; zone and flow notices include their context packs.

## Work Capture and Evidence Packs

Work Capture is the first part of the Capability Compiler: it collects evidence about a work
episode (a task and the change, CI, deployment, and agent activity around it) and packages it as a
bounded, traceable **Evidence Pack**. It reads Landscape, DataSea, and Crane as sources and never
writes to them.

```bash
crane capture demo                 # failed-deployment investigation from fixtures (no credentials)
crane capture sources              # this repository's live sources and their health
crane capture investigate task tsk_... commit HEAD --json
```

`investigate` uses the live connectors (the local Git repository, DataSea's local event store,
`.crane/governance.yaml`, and the AgentScript policies with the current validation report). It
keeps everything under the trust directory's `runtime/capture` folder. GitHub pull requests, CI,
deployments, logs, and metrics have no live connector yet. The demo covers them with deterministic
fixture connectors.

The code lives in `src/capture`, split into four categories plus the broker:

| Module | Responsibility |
| --- | --- |
| `resources` | Resource registry (only compiled-in connector types; credentials as `env:NAME` references; explicit scopes with no wildcards), the connector contract (`discover`, `fetch`, `query`, `get_metadata`, `health_check`, with structured `ok`/`partial`/`unavailable`/`unauthorized`/`rate_limited`/`invalid_query`/`timeout`/`failed` outcomes), scope resolution, and connectors |
| `format` | Raw artifacts (content hash, redaction, `blob:` storage reference), the versioned Evidence Record (`observed_at` separate from `ingested_at`), and five typed parsers that report `parsed`/`partially_parsed`/`failed` |
| `correlate` | Entities, the controlled relation vocabulary (`CONTAINS` ... `SUPERSEDES`, each `observed`/`declared`/`inferred`/`reviewed`), bounded traversal and Context Views, and assertions with lineage |
| `ingest` | Eligibility-first evidence selection, context contributions, work episodes, questions, feedback, outcomes, and the Evidence Pack |
| `broker` | Versioned Evidence Recipes and the Evidence Query Broker |

**Recipes.** A recipe is a versioned, immutable list of steps. Each step names a resource kind, an
operation, an evidence type, and parameters that are literals or exactly one `{variable}` from the
resolved scope. A step runs only when its branch predicate (`exists`, `missing`, `field_equals`,
`scope_known`, `all`, `any`, `not`) holds for the evidence retrieved so far. A recipe also declares
required evidence (optionally conditional), stopping rules, and limits. The built-in
`failed-deployment` v1 retrieves the deployment's logs and metrics only if the deployment failed,
and schema evidence only if a migration ran. If a step needs a scope value that is unknown, it
reports *not queried* and never widens the query. A proposed plan (for example from a model) is
validated as a recipe before anything runs.

**The broker** resolves scope and authorization before any query. A request that explicitly names
a repository, environment, or resource the actor may not access is rejected without contacting any
source. The broker validates each query against the resource, enforces step, record, payload, and
time budgets, then ingests, normalizes, and correlates the results and derives assertions
(`ci_passed`, `tests_passed`, `deployment_succeeded`, and `change_verified` derived from those
three). The audit record (`audit.jsonl`) holds the exact recipe and digest, policy decisions,
branch decisions, every query with its parameters, source snapshot, records, errors, and timing,
the budget used, and the metrics. A replay re-runs the recorded recipe against the current sources
and reports `exact`, `diverged`, or `unverifiable` (when a source cannot identify its snapshot).

**Evidence Pack** fields: `pack_id`, `episode_id`, `investigation`, `status`
(`sufficient`/`incomplete`/`budget_exhausted`/`rejected`), `scope` (with zone and flow versions and
facts learned from evidence), `evidence` (bounded excerpts plus artifact references), `entities`,
`context_view`, `assertions` (supporting and contradicting evidence, derivation, lineage),
`contributions` (`task_local` kept apart from `reusable`), `missing_evidence` (each with a state:
`empty`, `unavailable`, `denied`, `not_queried`, `parse_failed`), `sources`, `retrieval`,
`warnings` (access, freshness, completeness, uncertainty, conflict, budget, truncation), `limits`,
`manifest` (a digest of every included item), `size_bytes`, and `created_at`.

**Trust boundaries.** Missing evidence stays unknown: a missing test report never means
"tests failed", and a missing metric is never zero. When evidence disagrees, the assertion is left
unresolved and both sides are kept. Copies of one artifact count as one piece of support. Agents
may only *propose* context (namespace `agent-proposed`). Accepting a contribution takes a
different human with review permission, so nobody can approve their own. Hypotheses never become
observations. A constraint proposal that relaxes or removes a policy is refused, and an accepted
one still has to go through Crane's signed policy workflow. Source content is treated as data and
is redacted before storage. A record marked secret keeps only its reference.

## Capability Registry

The Capability Registry is the second part of the Capability Compiler. It defines what a reusable
capability is, accepts a traceable handoff from Work Capture, validates and stores immutable
versions, and governs their lifecycle. It does not execute procedures or compile capabilities from
evidence. Drafts are created explicitly (`manual` or `fixture`); `compiler` is reserved for the
future compiler. The code lives in `src/capability` (`model`, `intake`, `validate`, `lifecycle`,
`registry`, `query`, `refs`).

```bash
crane capability demo                                     # deterministic, from fixtures
crane capture investigate --json                          # produces a pack, digest, episode
crane capability intake episode EPI pack PCK digest D intent intent.json risk low profile low-risk-technical
crane capability create capability.json profile low-risk-technical
crane capability validate change-summary --dry-run        # reports only
crane capability validate change-summary                  # records the run, moves the state
crane capability publish change-summary version 1 reason "..." [acknowledge CODE,CODE]
```

**Capability schema (v1).** `capability_id` names the concept. `version` (1, 2, ...) names one
immutable definition. `schema_version` names the representation, and the three are never
interchangeable. A definition has these sections:

| Section | Contents |
| --- | --- |
| Provenance | creation method, author, and the source intakes, episodes, and Evidence Pack digests |
| Applicability | purpose, task classes, environments, preconditions, required state, exclusions, inputs and outputs, assumptions, risk, review level |
| Dependencies | typed references with a required or optional flag and a version constraint (`exact`, `at_least`, `unpinned`) |
| Execution contract | typed steps with ids, kinds, inputs, outputs, preconditions, and claims verified; effects, state transitions, side effects, recovery |
| Verification contract | claims with a status (`declared`, `observed`, `derived`, `verified`, `unresolved`), evidence and assertion references, method, environment; business invariants with their authority |

An empty environment list means *no known environment*, never "everywhere". Wildcards are
rejected. Dependencies are references, so the registry never copies Landscape, DataSea, or Crane
data and never substitutes another version.

**Intake.** The caller supplies only the episode, the pack, the digest it expects, and the
enterprise intent: the objective, acceptance criteria, invariants, and measures, each marked
`explicitly_mapped`, `human_confirmed`, `inferred_unverified`, or `unmapped`. Crane then:
- recomputes the pack's manifest
- resolves every evidence record and assertion for the submitter's own access
- compares zone, flow, and policy versions with the current ones
- records completeness and uncertainty as separate lists: missing evidence and why, unavailable and
  unauthorized sources, parse failures, conflicts, stale revisions, unresolved mappings, and
  unresolved assumptions

A claimed explicit mapping without an authority is treated as inferred. The same request
submitted twice returns the same intake, and reusing a key for different content is a conflict.
An `incomplete` intake is stored for review, but storage is not readiness. There are two
validation profiles:
- `conservative` (default, any risk): requires an authoritative objective and invariants and
  complete evidence.
- `low-risk-technical` (low risk only): requires technical acceptance criteria, and missing
  evidence must be a declared limitation.

**Validation gates.** Gates 1 to 5 run in a validation run; gate 6 runs at publication.

| Gate | Checks |
| --- | --- |
| 1 Structural | schema, ids, lineage, and procedure and reference consistency |
| 2 Provenance | intake readiness under the same profile, pack digests, episodes; claims may cite only evidence from their packs |
| 3 Applicability | explicit scope, environments within the verified scope, assessed risk, pinned and resolvable dependencies, no cycles among required capability dependencies |
| 4 Verification | each claim's status is *recomputed*: a hypothesis, an unknown or conflicted assertion, missing evidence, or an unknown environment leaves it `unresolved`, and technical evidence (CI, tests, code, tool calls) never verifies a business invariant |
| 5 Governance | review level for the risk, no policy changes (those go through Crane's signed workflow), low-risk capabilities must not change state |
| 6 Publication | every gate passed, every warning acknowledged explicitly, validation current (not stale), a human publisher holding the Publisher role who is not the author |

**Lifecycle.**

| From | To |
| --- | --- |
| `candidate` | `needs_evidence`, `validating`, `rejected` |
| `needs_evidence` | `validating`, `rejected` |
| `validating` | `validated`, `needs_evidence`, `candidate`, `rejected` |
| `validated` | `published`, `validating`, `needs_evidence`, `deprecated`, `revoked`, `rejected` |
| `published` | `deprecated`, `revoked` |
| `deprecated` | `revoked` |

Changes are new versions linked to their predecessor. A new version starts as a candidate and
inherits no validation. Deprecated and revoked versions stay in history but are never eligible
for new use. `capability stale` compares each validation's recorded basis (pack digests, zone,
flow, and policy versions, dependency versions) with the current state and marks it stale without
rewriting it, which removes a published version from eligibility until it is revalidated.
Invocation outcomes are recorded beside a version and never change it.

**Roles.** Roles are decided by Crane, not by the caller:
- Anyone running Crane may submit intakes, store candidates, and validate.
- Publishing, rejecting, deprecating, and revoking need this machine's signing key to be the
  repository owner's or a signed admin's.
- Agents can only contribute.

Every refused privileged operation is recorded. The registry is one versioned document
(`runtime/capability/registry.json`). Each change is a locked, reloaded, atomically replaced
transaction, so concurrent writers lose nothing and an interrupted write leaves the previous
document intact.

## Integrations and alerts

- `crane integrate slack|jira|whatsapp|github` stores settings outside the repository and can test
  the connection. Security alerts go to Slack and WhatsApp, and to the Jira issue in
  `CRANE_TASK_ID`.
- `.github/workflows/pr-notify.yml` posts pull-request activity to Slack and comments on the Jira
  issue named in the PR title or branch. Set the repository secrets `SLACK_WEBHOOK_URL`,
  `JIRA_BASE_URL`, `JIRA_EMAIL`, and `JIRA_API_TOKEN`.

## Dashboard

One dashboard for individuals and teams: <https://dashboard.agentslandscape.cloud>. Sign in with
GitHub, then choose an account, a repository, and optionally one team member.

- **Personal account:** the repositories your GitHub account owns.
- **Organization:** the organization's repositories with every teammate's activity, and a member
  filter. Organizations appear when they allow this app (organization settings, third-party
  access).
- **Views:** overview, agents and sessions, the action graph of every session, the event timeline
  with filters, governance (policies, selections, checkpoints, audit trail), the autonomy ledger,
  every metric with how it is known, compliance controls, alerts, members, health, session reports,
  and evidence bundles.
- **Stale sessions:** a session with no hook activity for `CRANE_STALE_SESSION_MINUTES` (120) and
  no signed end is flagged and alerted: the agent was stopped, its hooks removed, or its evidence
  is not arriving.
- **Broken evidence chains:** the service checks that every upload continues the machine's event
  chain where it left off, and records a critical alert for a rewrite, a gap, or a fork.
- **Scoped access:** telemetry and evidence views read MongoDB through a handle that cannot write.
  Dashboard writes are limited to repository-scoped agent/incident actions and GitHub-confirmed
  approvals. Signed machine requests can read only active quarantine rules needed by local hooks;
  they cannot read telemetry or incident records. Dashboard API access requires a signed-in person
  with access to the selected repository. A repository appears only after an upload proves its
  GitHub remote, so nobody can attach their uploads to someone else's repository.

### Dashboard Agents

Dashboard Agents are repository-scoped monitors configured in the Dashboard Agents page. Each rule
has a name and purpose, one or more `all`, `flow`, `zone`, or `selection` assignments, a supported
event metric, a threshold, and a lookback window. Supported metrics are critical events, denials,
prompt/injection signals, bypass attempts, tool failures, and event volume. `active` monitors all
matching evidence; `idle` considers only HIGH and CRITICAL evidence; `inactive` and `sleep` pause
evaluation. The service evaluates enabled agents after successful telemetry uploads and runs a
60-second recovery sweep (also on startup); dashboard page views only read results. A repository
lease prevents duplicate work across service instances, and threshold incidents are persisted
idempotently.

An incident can produce a pending quarantine request for a provider agent ID, a Crane session ID,
or an exact provider tool name. A fresh GitHub authorization check must confirm repository-admin
access before a request becomes active or a quarantine is released. Active quarantines persist until
an admin approves release. Local hooks query the signed ingestion service before every tool call;
matching calls are denied, and a service/policy error also denies the call. With
`CRANE_INGEST_URL=off` the service never sees the machine's agents, so the quarantine check is
skipped and the local rules still apply. Tool quarantines match
the exact provider tool name only within the incident's Crane session.

Incident response/decision context can be proposed for a flow. It contains the incident reason,
event references, and 3A timeline, and is visible to agents through the Dashboard Agents API only
after a fresh GitHub-confirmed repository admin approves it. Dashboard Agent notifications are
delivered by the hosted service (not a developer's local integration): configure Slack and Resend
email credentials in the Render service environment as listed below. Recipients are the configured
email distribution list, which should include the responsible agent creators and repository owners.
Delivery outcomes are recorded in the service event log; missing or failed delivery configuration is
not represented as a successful send.

The Incidents view supports status/severity/text filtering, human notes, mitigation, quarantine
requests, flow-context proposals, and a chronological 3A record that includes linked quarantine and
context approval decisions. Flow-context proposals include the incident evidence and the related
response-action audit trail captured when proposed. The Agent Activity view traces agent
creation/edits, state changes, evaluations, incident actions, and policy actions; detailed activity
records are retained for 30 days.

Signed-in clients can read the agent view with
`GET /api/dashboard-agents?repository=REPOSITORY_ID` and make bounded changes with JSON
`POST /api/dashboard-agents?repository=REPOSITORY_ID`. POST actions include `save-agent`,
`set-state`, `evaluate`, `open-incident`, `annotate-incident`, `resolve-incident`,
`request-quarantine`, `request-quarantine-release`, and `propose-flow-context`. Admin approvals use
the short-lived signed `/auth/agent-action` GitHub OAuth flow and support approval or rejection of
quarantine, release, and flow-context requests. The OAuth token is used once to check repository
permissions and is not stored. The API returns active/proposed quarantines and proposed or approved
flow context along with agent data.

For the hosted service, set these variables under **Render → Environment**. Never commit their
values or paste them into issue/chat messages:

| Variable | Purpose |
| --- | --- |
| `CRANE_DASHBOARD_AGENT_SLACK_WEBHOOK_URL` | Slack incoming webhook URL (`https://hooks.slack.com/services/...`) |
| `CRANE_DASHBOARD_AGENT_RESEND_API_KEY` | Resend API key used to send incident email |
| `CRANE_DASHBOARD_AGENT_EMAIL_FROM` | Verified sender address configured in Resend |
| `CRANE_DASHBOARD_AGENT_EMAIL_TO` | Comma-separated notification recipients; configure the responsible agent creators/repository owners or their team list |

When both notification providers are unset, incident creation still succeeds and the service records
that delivery is not configured. A configured provider failure is recorded as a failed delivery and
does not roll back the incident.

### Filtered dashboard data for agents

Signed-in users and agents acting with the same browser session can request compact, filtered
event context:

```text
GET /api/agent-context?repository=REPOSITORY_ID&since=...&until=...&filter=...
GET /api/events.csv?repository=REPOSITORY_ID&since=...&until=...&filter=...
```

Both routes require the existing `foxx_session` cookie and enforce the viewer's repository and
member permissions. `agent-context` returns minified JSON in the `foxx-agent-context/v1` format:
component metadata, the applied filter (identity values shortened for display), bounded summary
counts, token totals, and at most 500 newest matching events. Timestamps are integer Unix
milliseconds. The request filter uses the dashboard's normal `filter` JSON clauses (AND by default,
same-group OR, and optional NOT); component metadata can be supplied as `section`, `subsection`,
`component`, `kind`, and `context`. `events.csv` returns at most 500 newest matching event rows as
RFC 4180 CSV. These endpoints return filtered data only; they do not provide access to data outside
the selected repository/member scope. Dashboard component “AI” controls open the corresponding
filtered context and API URL for copying or download.

## Telemetry upload (DataSea service)

Crane uploads events, metrics, sessions, tool calls, alerts, and governance history to the Landscape
ingestion service (`https://agentscript-service.onrender.com`), which stores them in MongoDB.
Nothing needs to be set up: uploads happen when a tool call ends, when a session ends, after every
command, and every 60 seconds. Hooks never wait for the network, and the local evidence stays
authoritative when the service is unreachable.

- Tool-call telemetry retains the exact tool name reported by Claude, Codex, Copilot, Gemini, or
  an MCP server instead of collapsing unrecognized names to `OTHER`. Crane does not ask a separate
  model to guess a tool's identity. Token measurements are observed only when a provider supplies a
  readable transcript with usage; unsupported providers are shown as unmeasured, never estimated.
- From a repository clone, a human CODEOWNER can permanently clear its hosted records with
  `crane data clear`. The command requires GitHub CLI authentication (`gh auth login`) and an exact
  typed confirmation. The service independently checks the caller's GitHub identity against a
  direct account entry in the default-branch `CODEOWNERS` file, then deletes that repository's
  records across all members and installations. Team entries alone do not authorize this command.
  The supplied GitHub token is used for that check only and is not stored. Future uploads will
  create new repository records again.

- Each request is signed with the machine's own upload key (`~/.crane-trust/installation.key`,
  created automatically). Its public key identifies the installation, and an installation can only
  add to and read its own records. Every teammate's machine uploads, not only the one that ran
  `crane init`, and the upload key grants no authority over the repository's rules.
- Crane never holds database credentials. Only the service has the MongoDB connection string.
- Each machine also reports its repository status (policy results, integrity, compliance inputs,
  connection, non-secret integration settings) every two minutes or on `crane agent sync`, for the
  dashboard's governance and compliance views.
- **Limits** protect the shared database: 120 requests a minute per machine, at most 2,000 records
  of each kind per upload, 250,000 stored events and 50 repositories per machine, and no uploads
  while the database is over 450 MB. A refused upload stays on the machine and is retried.
- Set `CRANE_INGEST_URL=off` to keep everything local, or set it to your own service.

Run the service yourself:

```bash
docker build -t crane-ingest .
docker run -p 8080:8080 -e CRANE_MONGODB_URI='mongodb+srv://...' crane-ingest
# or: cargo run --release --features server -- server --listen 0.0.0.0:8080
```

For dashboard sign-in, create a GitHub OAuth app (GitHub, Settings, Developer settings, OAuth
Apps) with the callback URL `https://your-host/auth/callback`, and set `CRANE_GITHUB_CLIENT_ID` and
`CRANE_GITHUB_CLIENT_SECRET` on the service. The public address is detected (or set
`CRANE_PUBLIC_URL`), and the session key is created and kept in the database.

Put it behind HTTPS, then point Crane at it with `CRANE_INGEST_URL=https://your-host`, or build
with `CRANE_DEFAULT_INGEST_URL=https://your-host` to change the default (the release workflow reads
the repository variable `CRANE_INGEST_URL`).

## Environment variables

All optional. Crane does not read `.env` files; set them in your shell or CI. Credentials for
Slack, Jira, WhatsApp, and GitHub are set with `crane integrate NAME` instead.

| Variable | Purpose | Default |
| --- | --- | --- |
| `CRANE_HOME` | Trust directory (keys, evidence, secrets); use an absolute path | `~/.crane-trust` |
| `CRANE_TRUSTED_PUBLIC_KEYS` | Trusted owner keys (hex, comma separated), for CI without the `crane init` prompt | none |
| `CRANE_GITHUB_HOSTS` | GitHub Enterprise hosts accepted by `crane repo` | `github.com` only |
| `CRANE_GH` | Path of the GitHub CLI | `gh` |
| `CRANE_TASK_ID`, `CRANE_TASK_NAME` | Link agent sessions to a tracker task (Jira alerts use the ID) | none |
| `CRANE_SESSION_ID` | Session id when the agent reports none | generated |
| `CRANE_ENVIRONMENT` | Environment label on events | `local` (`ci` when `CI` is set) |
| `CRANE_ALERTS_DRY_RUN` | `1` records alerts without delivering them | `0` |
| `CRANE_INGEST_URL` | DataSea ingestion service; `off` keeps telemetry local | the release's service |
| `CRANE_SYNC_SECONDS` | Periodic upload interval | `60` |
| `CRANE_KEY_PROTECTION` | Signing keys at rest: `prompt` (a passphrase typed on the console for every signing command), `plain` | Windows Data Protection on Windows, plain (owner-only file) elsewhere |
| `CRANE_STALE_SESSION_MINUTES` | `crane server`: minutes without hook activity before an unfinished session is stale | `120` |
| `CRANE_MONGODB_URI`, `CRANE_MONGODB_DB`, `CRANE_MONGODB_EVENT_RETENTION_DAYS` | `crane server` only (never on developer machines) | required, `foxx`, `30` |
| `CRANE_GITHUB_CLIENT_ID`, `CRANE_GITHUB_CLIENT_SECRET` | `crane server`: dashboard sign-in (GitHub OAuth app) | sign-in off |
| `CRANE_PUBLIC_URL` | `crane server`: public address, when it cannot be detected | `RENDER_EXTERNAL_URL` or the request host |
| `CRANE_SERVER_REQUESTS_PER_MINUTE`, `CRANE_SERVER_ADDRESS_REQUESTS_PER_MINUTE` | `crane server`: rate per machine and per client address | `120`, `600` |
| `CRANE_SERVER_MAX_BATCH_RECORDS`, `CRANE_SERVER_MAX_EVENTS_PER_INSTALLATION`, `CRANE_SERVER_MAX_REPOSITORIES_PER_INSTALLATION`, `CRANE_SERVER_MAX_STORAGE_MB` | `crane server`: upload limits | `2000`, `250000`, `50`, `450` |
| `CRANE_ALLOW_LOCAL_REMOTE`, `CRANE_TEST_MONGODB_URI` | Offline mirrors and the live ingestion test suite | unset |
