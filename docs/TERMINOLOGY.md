# Landscape of Agents: Architecture Terminology Register

Every technical term, component, data model, and state value of the system, with a source
reference. Each entry answers five questions:

| Label | Question |
|---|---|
| **What** | What is it, in a short paragraph? |
| **Why** | Why does it exist, technically and for the product? |
| **Without it** | What would break if it were not there? |
| **Breaking now** | What is currently weak or missing, with a criticality value? |
| **Fix** | What needs to be done? |

**Criticality scale.** **CRITICAL**: security, privacy, or data-loss defect. **HIGH**: a silently wrong
or incomplete answer. **MEDIUM**: visible but misleading, or fails at scale. **LOW**: ergonomics, a latent
risk, or a declared feature not wired up. **NONE**: holds as designed today.

Companion documents: [FLOWS.md](FLOWS.md) (every control flow and pipeline), [PRODUCTS.md](PRODUCTS.md)
(products and customers), [OVERVIEW.md](OVERVIEW.md) (the narrative overview),
[WORK_CAPTURE.md](WORK_CAPTURE.md) (component 1 explained line by line), [GUIDE.md](GUIDE.md) (commands).
Links point into the source tree; line numbers are omitted where code moves often, so the file plus
the named function is the reference.

## Contents

1. [Trust and identity](#1-trust-and-identity)
2. [The governance core](#2-the-governance-core)
3. [Selections and AgentScript](#3-selections-and-agentscript)
4. [Zones, flows, and context (the Landscape)](#4-zones-flows-and-context-the-landscape)
5. [Agents at runtime](#5-agents-at-runtime)
6. [Decision vocabularies](#6-decision-vocabularies)
7. [Telemetry and evidence](#7-telemetry-and-evidence)
8. [The hosted service and dashboard](#8-the-hosted-service-and-dashboard)
9. [The Landscape editor extension](#9-the-landscape-editor-extension)
10. [Integrations](#10-integrations)
11. [Work Capture (component 1)](#11-work-capture-component-1)
12. [The capability pipeline (components 2 to 7)](#12-the-capability-pipeline-components-2-to-7)
13. [Insight (component 8)](#13-insight-component-8)
14. [Index of state values](#14-index-of-state-values)
15. [Index of data models and where they are stored](#15-index-of-data-models-and-where-they-are-stored)

---

## 1. Trust and identity

### Repository identity
Code: [governance/workspace.rs](../src/governance/workspace.rs)
- **What.** A SHA-512 digest of the repository's root commits and its normalized origin remote. SSH
  and HTTPS spellings of the same remote give the same identity.
- **Why.** Tech: every signature is bound to it, so a signed rule set cannot be copied into another
  repository and still verify. Product: a customer's rules mean "this repository", never "any
  repository that copied the folder".
- **Without it.** Signed `.crane/` folders could be transplanted between repositories, and the
  dashboard could not tell two repositories apart.
- **Breaking now.** Nothing known. A transfer or rename changes the identity, which is handled by
  rebind. **NONE.**
- **Fix.** None needed.

### `.crane/` directory
Code: [governance/state.rs](../src/governance/state.rs), [governance/mod.rs](../src/governance/mod.rs)
- **What.** The committed folder holding the rules: policies, the selection registry, checkpoints,
  configuration, `governance.yaml`, context bindings, the audit chain, and the signed manifest.
- **Why.** Tech: the rules travel with the code through Git, are reviewable in pull requests, and are
  treated as untrusted until their signatures verify. Product: no extra server is needed to share
  rules across a team.
- **Without it.** Rules would need a central service to distribute, and enforcement would depend on
  the network.
- **Breaking now.** Two admins changing rules on different branches produce a Git conflict inside
  `.crane/` that must be resolved by keeping one side and re-applying the other. **LOW.**
- **Fix.** A `crane governance merge` command that verifies both signed sides and re-applies the
  smaller change set through `commit()`.

### Trust directory
Code: [trust/mod.rs](../src/trust/mod.rs)
- **What.** `CRANE_HOME` (default `~/.crane-trust`), outside every repository. It holds signing keys,
  pinned trusted keys, rollback anchors, the upload key, integration secrets, alert rules, and the
  runtime evidence stores.
- **Why.** Tech: an agent editing the repository cannot forge keys or evidence. Product: the referee's
  notebook is not on the field.
- **Without it.** Keys and evidence inside the repository could be edited or committed by an agent.
- **Breaking now.** An agent's unrestricted shell running as the same operating-system user can read
  files that user can read. Hooks deny tool calls that *name* the directory, and post-action
  verification catches changes, but confidentiality is not guaranteed. **HIGH** (documented in
  [SECURITY.md](../SECURITY.md)).
- **Fix.** Ship a sandbox profile (container, separate user, or OS sandbox) for agent processes;
  default to `CRANE_KEY_PROTECTION=prompt` so a stolen sealed key cannot be used to sign.

### Signing key and key protection
Code: [trust/keys.rs](../src/trust/keys.rs), [trust/crypto.rs](../src/trust/crypto.rs)
- **What.** An Ed25519 key that signs governance state. At rest it is sealed with the Windows Data
  Protection API, or with a passphrase (Argon2id and XChaCha20-Poly1305) typed on the console when
  `CRANE_KEY_PROTECTION=prompt`. Hooks never unseal it.
- **Why.** Tech: authority is a signature, not a file permission. Product: "only people change the
  rules" is enforceable.
- **Without it.** Anyone who could write `.crane/` could change the rules.
- **Breaking now.** Without the prompt mode on non-Windows systems, a same-user process may be able
  to use the key material. **MEDIUM.**
- **Fix.** Make prompt protection the default outside Windows, or support OS keychains (macOS
  Keychain, Secret Service) for sealing.

### Upload key and installation
Code: [telemetry/ingest.rs](../src/telemetry/ingest.rs), [server/ingest.rs](../src/server/ingest.rs)
- **What.** A separate per-machine Ed25519 key (`installation.key`) used only to sign uploads. Its
  public half is the machine's identity, the **installation**, on the service.
- **Why.** Tech: upload identity is separate from rule authority, so a leaked upload key cannot sign
  rules. Product: a team sees each machine's evidence separately and can attribute it.
- **Without it.** The service could not authenticate uploads without user passwords or tokens.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed. A future key rotation command would be useful (**LOW**).

### Owner, admin, admin statement, and epoch
Code: [governance/admins.rs](../src/governance/admins.rs), [cli/governance/admin.rs](../src/cli/governance/admin.rs)
- **What.** The owner is the key that first ran `crane init`. Admins are other keys the owner lists in
  a signed admin statement. Every change to the list raises an epoch, and machines reject an older
  list.
- **Why.** Tech: a small, signed authority model without a user database. Product: teams have more
  than one person who can change rules, and removed people cannot sneak back.
- **Without it.** Only one person could ever sign, or anyone could.
- **Breaking now.** The authority model is per repository; an organization with many repositories
  maintains admin lists repository by repository. **LOW.**
- **Fix.** An organization-level admin statement pinned once per machine and referenced by each
  repository.

### Manifest and generation
Code: [governance/state.rs](../src/governance/state.rs)
- **What.** A signed document listing the digest of every governance file. Each change produces a new
  manifest with generation plus one.
- **Why.** Tech: one signature covers everything, so modification, deletion, rollback, and fork are
  all detectable. Product: "the rules were tampered with" is a provable statement.
- **Without it.** Each file would need its own signature, and deletions could not be detected.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Rollback anchor
Code: [governance/state.rs](../src/governance/state.rs), [trust/mod.rs](../src/trust/mod.rs)
- **What.** The highest manifest generation this machine has seen, kept in the trust directory.
- **Why.** Tech: an older, validly signed `.crane/` is rejected. Product: nobody can restore yesterday's
  looser rules by checking out an old commit.
- **Without it.** Rollback attacks would pass signature checks.
- **Breaking now.** A fresh machine has no anchor, so its first view accepts any validly signed
  generation. **LOW** (inherent to local-first trust).
- **Fix.** Let the hosted service publish the latest generation it has seen, and warn when a new
  machine starts below it.

### Rebind
Code: [governance/rebind.rs](../src/governance/rebind.rs)
- **What.** `crane repo rebind OLD_REMOTE`: proves a repository moved (transfer or rename) and re-signs
  its rules for the new identity.
- **Why.** Tech: identity changes with the remote; rebind is the controlled path. Product: moving a
  repository to a new organization does not mean recreating every rule.
- **Without it.** A renamed repository would fail verification forever.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Agent environment guard
Code: [trust/mod.rs](../src/trust/mod.rs) (`agent_environment`), [cli/mod.rs](../src/cli/mod.rs) (`dispatch`)
- **What.** A check for environment markers that agent tools set. When one is present, every `crane`
  command except the hook entry points is refused.
- **Why.** Tech: an agent cannot run Crane to change its own rules. Product: the core promise, "agents
  cannot rewrite the rules that govern them".
- **Without it.** An agent could call `crane protect`, `crane governance sign`, or review its own
  contributions.
- **Breaking now.** Detection relies on markers the agent tools set; an agent tool that sets none, or
  a process that clears them, is not detected by this check (the hooks still deny shell commands that
  name `crane`). **MEDIUM.**
- **Fix.** Combine with the hook-side denial (already present) and a parent-process check where the
  operating system allows it.

---

## 2. The governance core

### `commit()`, the single door for rule changes
Code: [governance/state.rs](../src/governance/state.rs)
- **What.** The one function every rule change goes through: verify the current state, apply the
  change, append an audit entry, sign a new manifest, update the rollback anchor.
- **Why.** Tech: one code path means one place to get signing right. Product: every rule change is
  audited the same way.
- **Without it.** Each command would sign differently, and some would skip verification.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Governance audit chain
Code: [governance/audit.rs](../src/governance/audit.rs)
- **What.** `.crane/audit.jsonl`, an append-only, hash-chained trail of governance changes, covered by
  the manifest.
- **Why.** Tech: history cannot be rewritten without breaking the chain. Product: change management
  evidence ("who changed which rule, when") for audits.
- **Without it.** Rule changes would be visible only through Git history, which can be rewritten.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Checkpoint
Code: [governance/checkpoints.rs](../src/governance/checkpoints.rs)
- **What.** A trusted baseline: a recorded Git commit. Checkpoints form an insert-only, hash-chained
  list.
- **Why.** Tech: selections are judged against a known-good baseline. Product: "protect this code as
  it was at release 2.3".
- **Without it.** Nobody could tell whether protected code was already tampered with when it was
  protected.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Rule verification
Code: [governance/verify.rs](../src/governance/verify.rs)
- **What.** The shared check behind `crane validate`, `crane test`, `crane policy NAME status`, and the
  hooks: verify the signed state, check policies against the registry, resolve selections from the
  working tree, judge each command against the real file content.
- **Why.** Tech: one verifier, never a single source of truth. Product: what the editor, the CLI, and
  the agent's hooks report always agrees.
- **Without it.** Each surface could disagree on whether a policy passes.
- **Breaking now.** A full verification can be slow on large repositories, and Work Capture's Crane
  connector cannot interrupt it. **MEDIUM** (at scale).
- **Fix.** Cache verification results keyed by the manifest digest and the working-tree status, and
  pass a deadline into `evaluate`.

### Configuration (`config.json`)
Code: [governance/config.rs](../src/governance/config.rs)
- **What.** Signed defaults: default policy and checkpoint, the admin statement, the autonomy model
  (credits, costs, thresholds), and smoke-test commands.
- **Why.** Tech: tunable behavior that is still signed. Product: each team sets its own risk budget.
- **Without it.** Credit costs and defaults would be hard-coded.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Policy context
Code: [governance/context.rs](../src/governance/context.rs)
- **What.** A text file bound to a policy (with its digest) and given to the agent when a session
  starts. Context is guidance, never authorization.
- **Why.** Tech: guidance is versioned with the rules. Product: agents stop rediscovering constraints,
  which saves tokens.
- **Without it.** Agents would be blocked without knowing why the rule exists.
- **Breaking now.** Context is static text; it cannot yet include learned, reviewed knowledge from
  Insight. **LOW.**
- **Fix.** Let accepted, reusable Work Capture contributions for a zone be appended to the context the
  agent receives, labelled with their provenance.

---

## 3. Selections and AgentScript

### Selection and identifier
Code: [selection/registry.rs](../src/selection/registry.rs)
- **What.** A block of lines a person chose to govern, with an 8-character identifier (a lookup key,
  not an authority) and a signed selection record in `.crane/registry.json`.
- **Why.** Tech: governance targets code precisely, not whole files. Product: "this function must not
  change" is expressible.
- **Without it.** Rules could only cover whole files or paths.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Anchor (marker)
Code: [selection/anchors.rs](../src/selection/anchors.rs)
- **What.** Two comment lines Crane inserts around a selection, in the file's own comment syntax.
  Malformed, duplicated, unmatched, or unregistered markers are reported, never trusted.
- **Why.** Tech: anchors move with the code, so selections survive edits above them. Product: no line
  numbers to maintain.
- **Without it.** Every edit above a selection would break it.
- **Breaking now.** Files whose language has no comment syntax Crane knows cannot be anchored.
  **LOW.**
- **Fix.** Extend the comment-syntax table as languages are requested.

### Selection record and its three digests
Code: [selection/registry.rs](../src/selection/registry.rs), [selection/create.rs](../src/selection/create.rs)
- **What.** The signed record of a selection: the content digest of the selected bytes, the
  origin-binding digest (repository identity, checkpoint, blob, path and range, content), and the
  record digest of its canonical JSON, which is what is signed.
- **Why.** Tech: each digest answers a different question (did it change, where did it come from, was
  the record altered). Product: protection is provable, not a convention.
- **Without it.** A changed block could not be told apart from a moved one.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Location tracker and UNRESOLVED
Code: [selection/tracker.rs](../src/selection/tracker.rs)
- **What.** Finds where each selection is now, through edits and renames, without changing its
  identifier. A selection that cannot be found unambiguously is UNRESOLVED, which fails closed.
- **Why.** Tech: enforcement needs the current location. Product: refactoring does not silently drop
  protection.
- **Without it.** Moving a file would remove its protection.
- **Breaking now.** A very large refactor can leave selections UNRESOLVED and block agent work until a
  person repairs anchors. **LOW** (fails safe).
- **Fix.** A `crane selection repair` assistant that proposes the most likely new location for a
  person to confirm and sign.

### AgentScript policy
Code: [governance/policy.rs](../src/governance/policy.rs)
- **What.** The policy language in `.crane/policies/*.crane`: named policies containing `preserve` and
  `target` commands over selection identifiers. A policy passes only if every command passes.
- **Why.** Tech: a tiny, parseable language is easy to verify. Product: rules a reviewer can read in a
  pull request.
- **Without it.** Rules would live in prompts, which are requests, not rules.
- **Breaking now.** The language has two verbs; it cannot yet express "may change only with a test
  change", "only this agent", or time-bound rules. **LOW** (product gap).
- **Fix.** Add verbs one at a time, each with a verifier and a test, keeping the grammar closed.

### Preserve
Code: [governance/verify.rs](../src/governance/verify.rs)
- **What.** A command requiring the selection to stay exactly as it was at the checkpoint (content
  digest equal).
- **Why.** Tech: an exact, cheap check. Product: audited code (fees, auth) stays untouched.
- **Without it.** There would be no hard "do not touch".
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Target and change types
Code: [governance/verify.rs](../src/governance/verify.rs), [governance/changes.rs](../src/governance/changes.rs)
- **What.** A command requiring the selection to change, with a stated change type: `logical_bn`
  (business logic), `logical_cn` (loops or branches), `logical_sn` (structure), or `semantic` (names
  or comments). Whitespace-only changes never satisfy a target.
- **Why.** Tech: "must change, and this way" is measurable. Product: a task's acceptance criterion
  becomes checkable ("the refund logic was actually rewritten").
- **Without it.** Agents could claim to have done the work with cosmetic edits.
- **Breaking now.** Classification is lexical; a semantically equivalent rewrite can be classified as
  a logic change. **LOW** (Crane measures, it does not judge intent; documented).
- **Fix.** Keep it measurable; optionally add language-aware parsers for the most common languages.

### Change classification
Code: [governance/changes.rs](../src/governance/changes.rs), [governance/diff.rs](../src/governance/diff.rs)
- **What.** Before and after text are lexed into identifiers, literals, operators, and punctuation
  (comments separate) and compared on deterministic features: whitespace, comments, business tokens,
  complexity, and shape. Line diffs use Myers' algorithm.
- **Why.** Tech: deterministic, language-light measurement. Product: explainable verdicts.
- **Without it.** Targets could not be checked.
- **Breaking now.** See Target. **LOW.**
- **Fix.** See Target.

---

## 4. Zones, flows, and context (the Landscape)

### Zone
Code: [governance/zones.rs](../src/governance/zones.rs)
- **What.** A persistent governance boundary in `.crane/governance.yaml` over paths, files,
  directories, symbols, selections, or exact source blocks between `@crane:zone:ID:start/end` markers.
- **Why.** Tech: rules over areas, not only blocks. Product: "payments is critical" in one line.
- **Without it.** Every risky area would need many selections.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Flow and flow discovery
Code: [governance/flows.rs](../src/governance/flows.rs)
- **What.** An execution path: entry points and the functions they call, found by a lexical,
  language-aware call graph whose every edge is marked inferred. A flow can also be an ordered list
  of zones (its stages).
- **Why.** Tech: risk follows calls, not folders. Product: "the refund flow" can be governed as one
  thing.
- **Without it.** Code reached through calls from risky entry points would be ungoverned.
- **Breaking now.** Dynamic calls and unsupported languages are not followed (reported as external,
  ambiguous, or not analyzed). **LOW** (reported honestly).
- **Fix.** Add language analyzers incrementally; let a person declare edges explicitly as
  `declared` beside the inferred ones.

### Context pack
Code: [governance/zones.rs](../src/governance/zones.rs)
- **What.** Reusable, versioned guidance attached to zones and flows. Every flow must have context, or
  signing is refused.
- **Why.** Tech: guidance travels with the boundary. Product: agents receive the right guidance the
  moment they touch the area.
- **Without it.** Boundaries would block without explaining.
- **Breaking now.** Same gap as policy context: learned knowledge is not yet merged in. **LOW.**
- **Fix.** See Policy context.

### Criticality
Code: [governance/zones.rs](../src/governance/zones.rs), [governance/config.rs](../src/governance/config.rs)
- **What.** `routine`, `sensitive`, `critical`, or `restricted`. It adds credit cost per edit, and
  restricted code always needs a human.
- **Why.** Tech: a small ordered scale is easy to combine. Product: risk is visible and priced.
- **Without it.** Every area would cost the same autonomy.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Autonomy ceiling
Code: [governance/zones.rs](../src/governance/zones.rs), [hooks/enforce.rs](../src/hooks/enforce.rs)
- **What.** The most autonomy an agent may have inside an entity: `observe`, `assisted`, `delegated`,
  or `autonomous`.
- **Why.** Tech: a ceiling per area, combined with the session mode. Product: autonomy is granted per
  area and can be widened as trust is earned.
- **Without it.** Autonomy would be all or nothing per session.
- **Breaking now.** `autonomous` behaves no more freely than `delegated` in 0.4. **LOW** (declared but
  not differentiated).
- **Fix.** Define what `autonomous` adds (for example, fewer approvals for routine code backed by
  Insight trust patterns) and test it.

### Overlap rule
Code: [hooks/enforce.rs](../src/hooks/enforce.rs), [governance/zones.rs](../src/governance/zones.rs)
- **What.** Where several zones and flows cover the same code, the strictest ceiling and the highest
  criticality apply.
- **Why.** Tech: restrictions only add up, so composition is safe. Product: restructuring can never
  broaden authority.
- **Without it.** A new zone could accidentally loosen an existing one.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Entity status
Code: [governance/zones.rs](../src/governance/zones.rs)
- **What.** `proposed` or `active`. Only active zones and flows are enforced.
- **Why.** Tech: review before enforcement. Product: the extension can draft zones safely.
- **Without it.** Every draft would immediately block agents.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Governance graph
Code: [governance/graph.rs](../src/governance/graph.rs)
- **What.** Zones, flows with ordered stages, and selections as a directed acyclic graph, with the
  agents recently active in each. An edge that would close a cycle is left out and reported.
- **Why.** Tech: a single structure for the editor and the dashboard. Product: people see their
  governance as a map.
- **Without it.** Governance would be a YAML file only experts read.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

---

## 5. Agents at runtime

### Hook and hook events
Code: [hooks/runtime/mod.rs](../src/hooks/runtime/mod.rs), [hooks/install.rs](../src/hooks/install.rs)
- **What.** A program an agent tool runs at fixed lifecycle points. Crane's entry point is
  `crane agent hook --event EVENT --profile PROFILE`, for `SESSION_START`, `PROMPT_SUBMIT`,
  `PRE_TOOL_USE`, `PERMISSION_REQUEST`, `POST_TOOL_USE`, `STOP`, `SESSION_END`.
- **Why.** Tech: the agent tool calls Crane before and after every action, so enforcement is not
  optional. Product: works inside the tools developers already use.
- **Without it.** Crane could only check after the fact.
- **Breaking now.** Enforcement exists only where the agent tool offers hooks; an agent tool without
  hooks, or with hooks removed, is unguarded (detected only as a stale session). **MEDIUM.**
- **Fix.** Detect missing hook configuration at `crane test .` and on the dashboard; offer a wrapper
  mode for tools without hooks.

### Adapter and provider-neutral action
Code: [hooks/adapter/](../src/hooks/adapter/) (`claude.rs`, `codex.rs`, `copilot.rs`, `gemini.rs`), [hooks/action.rs](../src/hooks/action.rs)
- **What.** An adapter translates one agent tool's payloads into provider-neutral actions and renders
  Crane's answer in that tool's protocol. It makes no decisions.
- **Why.** Tech: one guard for every tool. Product: one rulebook across Claude Code, Codex, Copilot,
  and Gemini.
- **Without it.** Each tool would need its own enforcement logic.
- **Breaking now.** Tool payload formats change with vendor releases. **MEDIUM** (operational).
- **Fix.** Contract tests per adapter with recorded payloads per vendor version; a coverage-gap event
  when a payload field is unknown (already the behavior for unknown tools).

### Guard
Code: [hooks/enforce.rs](../src/hooks/enforce.rs)
- **What.** The provider-neutral decision engine applying integrity, safety state, protected files,
  shell analysis, script pre-check, edit simulation, zone limits, autonomy mode, and credits, in that
  order.
- **Why.** Tech: cheap and decisive checks first, failing closed. Product: the agent is told exactly
  why it was stopped.
- **Without it.** There would be no enforcement.
- **Breaking now.** Nothing known in the ordering. **NONE.**
- **Fix.** None needed.

### Shell command analysis
Code: [hooks/shell.rs](../src/hooks/shell.rs)
- **What.** A lexical analysis of shell commands that denies those naming protected metadata, running
  `crane`, or mutating files holding preserved selections.
- **Why.** Tech: most dangerous commands name their targets. Product: blocks the common `sed -i` style
  bypass before it runs.
- **Without it.** The shell would be an open bypass.
- **Breaking now.** Lexical analysis cannot see through indirection (variables, generated scripts);
  post-action verification catches the effect afterwards and quarantines. **MEDIUM.**
- **Fix.** Keep post-action verification as the backstop; add a sandbox profile for full mediation.

### Script pre-check
Code: [hooks/scripts.rs](../src/hooks/scripts.rs)
- **What.** Before `python x.py`, `node -e`, `bash x.sh`, `npm run` and similar run, Crane reads the
  code and looks for clearly dangerous patterns (naming Crane's files, changing preserved files,
  running decoded code, encoded PowerShell, secret reads with network use, history rewrites, tree
  deletion).
- **Why.** Tech: scripts are the second most common bypass. Product: fewer surprises from "helper
  scripts" agents write.
- **Without it.** Any script would run unchecked until afterwards.
- **Breaking now.** Same lexical limit as shell analysis. **MEDIUM.**
- **Fix.** Same as shell analysis.

### Edit simulation
Code: [hooks/enforce.rs](../src/hooks/enforce.rs)
- **What.** A proposed file edit is applied in memory to the file's current text; it is denied if it
  would change a preserved selection or alter a marker.
- **Why.** Tech: decide from the real state, never the agent's claim. Product: precise denials with
  zero false positives on unrelated lines.
- **Without it.** Edits could only be judged after they happened.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Autonomy mode
Code: [hooks/enforce.rs](../src/hooks/enforce.rs), [hooks/runtime/session.rs](../src/hooks/runtime/session.rs)
- **What.** The session's mode: `observe`, `assisted`, `delegated` (default), or `autonomous`. It can
  only make decisions stricter.
- **Why.** Tech: a session-level dial combined with area ceilings. Product: a cautious session for a
  new hire or an unfamiliar task.
- **Without it.** Every session would run at the same autonomy.
- **Breaking now.** See Autonomy ceiling (`autonomous` not differentiated). **LOW.**
- **Fix.** See Autonomy ceiling.

### Autonomy credits and the credit ledger
Code: [telemetry/ledger.rs](../src/telemetry/ledger.rs), [governance/config.rs](../src/governance/config.rs)
- **What.** A budget of risk-bearing authority per session (default 100 for 8 hours), kept apart from
  tokens and money. Movements (grant, reserve, consume, release, revoke, expire) are an append-only,
  hash-chained ledger.
- **Why.** Tech: risk is metered without granting permission (an empty balance only turns "allow"
  into "ask"). Product: a measurable autonomy budget managers understand.
- **Without it.** An agent could take unlimited risky actions without a human noticing.
- **Breaking now.** Default costs are fixed numbers, not learned from outcomes. **LOW.**
- **Fix.** Let Insight trust patterns propose cost changes for owner review.

### Safety state
Code: [telemetry/model.rs](../src/telemetry/model.rs), [hooks/runtime/tools.rs](../src/hooks/runtime/tools.rs)
- **What.** `ACTIVE`, `DEGRADED` (a violation after a file edit: mutating actions need approval), or
  `QUARANTINED` (a confirmed bypass or repeated attacks: mutating actions denied).
- **Why.** Tech: escalation without a human in the loop for every event. Product: a misbehaving
  session is contained automatically.
- **Without it.** A session that just bypassed a rule could keep going.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Bypass and coverage gap
Code: [hooks/runtime/tools.rs](../src/hooks/runtime/tools.rs), [telemetry/model.rs](../src/telemetry/model.rs)
- **What.** A bypass is a protected resource changed without passing through enforcement (for example,
  by a shell command); it is detected after the effect and quarantines the session. A coverage gap is
  an action that could not be observed or decided, reported as such.
- **Why.** Tech: honest accounting of what enforcement did and did not see. Product: no false "all
  good".
- **Without it.** Bypasses would look like normal edits.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Session, task, agent, and installation identifiers
Code: [telemetry/identity.rs](../src/telemetry/identity.rs)
- **What.** Crane's own deterministic identifiers (`ses_…`, `tsk_…`, `agt_…`, `ins_…`) so every hook
  process of one session maps to the same records; provider identifiers are kept beside them with
  their provenance (`PROVIDER`, `ENVIRONMENT`, `GENERATED`). A tracker key from `CRANE_TASK_ID` is
  kept as the external task id.
- **Why.** Tech: stable keys across separate hook processes. Product: one session view, one task view.
- **Without it.** Evidence from one session would scatter.
- **Breaking now.** Two identity systems (Crane's and the tracker's) must both be matched by
  consumers; Work Capture now does. **NONE.**
- **Fix.** None needed.

### Stale session
Code: [server/database.rs](../src/server/database.rs), [DASHBOARD.md](../DASHBOARD.md)
- **What.** A session with no hook activity for 120 minutes and no signed end, flagged on the service.
- **Why.** Tech: absence of evidence is surfaced. Product: tells a team that an agent was stopped, its
  hooks removed, or its evidence is not arriving.
- **Without it.** A removed hook would look like an idle agent.
- **Breaking now.** Detection is time-based, so a hook removal is seen up to two hours late.
  **LOW.**
- **Fix.** Report a missing hook configuration directly from the repository status upload.

---

## 6. Decision vocabularies

All defined in [telemetry/model.rs](../src/telemetry/model.rs). They are kept separate so a denied
attack is never confused with a breach.

### Decision
- **What.** What Crane allowed: `ALLOW`, `REQUIRE_APPROVAL`, `DENY`, `QUARANTINE`, `ERROR`.
- **Why.** Tech: the guard's output. Product: the answer the agent and the person see.
- **Without it.** No record of intent versus result.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Execution outcome
- **What.** What happened when the action ran: `SUCCESS`, `FAILURE`, `PARTIAL`, `UNKNOWN`.
- **Why.** Tech: an allowed action can still fail. Product: tool failure rates per agent.
- **Without it.** "Allowed" would be read as "succeeded".
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Assessment
- **What.** Whether it complied: `COMPLIANT`, `VIOLATION`, `SUSPICIOUS`, `UNRESOLVED`.
- **Why.** Tech: judged from the real effect. Product: compliance rates.
- **Without it.** Violations detected after the fact would be invisible in metrics.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Enforcement outcome
- **What.** How the control performed: `PREVENTED`, `DETECTED_AFTER_EFFECT`, `BYPASS_CONFIRMED`,
  `COVERAGE_GAP`, `NOT_APPLICABLE`.
- **Why.** Tech: separates control effectiveness from agent behavior. Product: "how good are our
  guardrails" is answerable.
- **Without it.** A prevented attack and a missed one would look alike.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Known-as (provenance of a fact)
- **What.** `OBSERVED`, `INFERRED`, `ESTIMATED`, `UNSUPPORTED`, `REDACTED`, `UNOBSERVED`, `LOST`.
- **Why.** Tech: every fact states how it is known. Product: dashboards never show a guess as a
  measurement.
- **Without it.** Missing data would appear as zero.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

---

## 7. Telemetry and evidence

### Event and envelope
Code: [telemetry/events.rs](../src/telemetry/events.rs), [DASHBOARD.md](../DASHBOARD.md)
- **What.** One recorded fact with a versioned envelope (`event_id`, `event_name`, F-number flow,
  `occurred_at`, `schema_version`, `sequence`, `previous_digest`, `digest`, `origin`) and 13 attribute
  groups.
- **Why.** Tech: one schema for every surface. Product: the dashboard, alerts, and Work Capture all
  read the same facts.
- **Without it.** Each surface would invent its own record.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Event chain
Code: [telemetry/events.rs](../src/telemetry/events.rs), [server/ingest.rs](../src/server/ingest.rs)
- **What.** Each event stores the digest of the previous one on that machine; the service checks that
  every upload continues the chain.
- **Why.** Tech: editing, removing, or reordering events breaks the chain. Product: tamper-evident
  evidence.
- **Without it.** Evidence could be rewritten silently.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Specification projection and F-flows
Code: [telemetry/spec.rs](../src/telemetry/spec.rs), [telemetry/spec_commands.rs](../src/telemetry/spec_commands.rs)
- **What.** Each event is projected into its specification form with all 13 attribute groups when it
  is appended, covered by the event's digest. The 37 scenarios F00 to F36 name every situation that
  produces events.
- **Why.** Tech: the specification view cannot drift from the stored fact. Product: a documented,
  stable event contract for integrations.
- **Without it.** Consumers would parse raw, unstable records.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Redaction
Code: [telemetry/redact.rs](../src/telemetry/redact.rs)
- **What.** Credentials, tokens, private keys, and password assignments become `[REDACTED:kind]`
  before anything is written. Raw prompts and tool payloads are never stored.
- **Why.** Tech: secrets never reach disk or the service. Product: safe to share the dashboard with a
  team.
- **Without it.** Evidence would be a secret leak.
- **Breaking now.** Pattern-based; an unfamiliar secret format can pass. Work Capture raises runtime
  logs and tool calls to Confidential regardless. **MEDIUM.**
- **Fix.** Add entropy-based detection for long random strings, tested with a fuzz corpus.

### Idempotency key
Code: [telemetry/events.rs](../src/telemetry/events.rs), [telemetry/ledger.rs](../src/telemetry/ledger.rs)
- **What.** A key derived from what an event or ledger entry represents; a retried hook cannot record
  twice.
- **Why.** Tech: hooks can be retried by agent tools. Product: counts are not inflated.
- **Without it.** Double counting.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Token usage
Code: [telemetry/usage.rs](../src/telemetry/usage.rs)
- **What.** Model token counts read from the agent's own transcript (input, output, cache read, cache
  creation, model), with a tool call attributed an equal share of its turn.
- **Why.** Tech: measured, never estimated. Product: cost per tool call and per task.
- **Without it.** No cost visibility.
- **Breaking now.** Only Claude Code provides a usable transcript; other tools show "unmeasured".
  **MEDIUM** (product gap).
- **Fix.** Read usage from each vendor's telemetry or API logs as they become available; keep
  "unmeasured" otherwise.

### Eval
Code: [telemetry/evals.rs](../src/telemetry/evals.rs)
- **What.** A number shown on the dashboard, computed only from events; a number with no events
  behind it is `null`, never zero.
- **Why.** Tech: one computation for the dashboard and alert rules. Product: honest metrics.
- **Without it.** Metrics would be computed differently in each view.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Severity
Code: [telemetry/model.rs](../src/telemetry/model.rs)
- **What.** `INFO`, `LOW`, `MEDIUM`, `HIGH`, `CRITICAL`, assigned by four rules (failure streak,
  outcome, threshold, fixed).
- **Why.** Tech: deterministic. Product: triage order.
- **Without it.** Everything would look equally urgent.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Built-in alerts and custom alert rules
Code: [telemetry/alerts.rs](../src/telemetry/alerts.rs), [telemetry/rules.rs](../src/telemetry/rules.rs)
- **What.** Built-in alerts (low credits, bypasses, violations, quarantine, coverage gaps, failed
  verification, suspicious prompts) and person-made rules (any eval, a threshold, a window, filters,
  channels) that fire, remind, and resolve. Delivered to Slack and WhatsApp by a detached `curl`
  reading its URL and body from stdin.
- **Why.** Tech: delivery never changes a decision and never exposes credentials on a command line.
  Product: people hear about incidents where they already work.
- **Without it.** Incidents would be noticed only on the dashboard.
- **Breaking now.** Local rules are evaluated by the background sync; a machine that is off evaluates
  nothing (the service's Dashboard Agents cover the team view). **LOW.**
- **Fix.** None needed beyond documenting which to use.

### Compliance controls
Code: [telemetry/compliance.rs](../src/telemetry/compliance.rs)
- **What.** Controls with an objective, enforcement rule, required evidence, verification method, and
  source F-flows, each evaluated as `SATISFIED`, `PARTIAL`, `NO_EVIDENCE`, or `NOT_APPLICABLE`, with
  mappings to OWASP LLM, NIST AI RMF, SOC 2, ISO/IEC 27001:2022, and GDPR.
- **Why.** Tech: evaluated only from held evidence. Product: audit raw material without extra work.
- **Without it.** Compliance would be a manual spreadsheet.
- **Breaking now.** Mappings show what the evidence supports, not certification, and there is no
  auditor-ready export bundle. **LOW** (product gap).
- **Fix.** A signed compliance export (controls, evidence references, chain verification) per period.

### Sync and upload
Code: [telemetry/sync.rs](../src/telemetry/sync.rs), [telemetry/ingest.rs](../src/telemetry/ingest.rs)
- **What.** A detached `crane agent sync` process asks the service how far each local stream reached,
  then uploads newer records in signed batches. `CRANE_INGEST_URL=off` keeps everything local.
- **Why.** Tech: hooks never wait for the network. Product: enforcement works offline; visibility
  catches up.
- **Without it.** Either no team view, or enforcement blocked on the network.
- **Breaking now.** The built-in ingest URL still points at the Render host while the custom domain
  is set up. **LOW.**
- **Fix.** Switch the default to the custom domain once it is live; keep the old host as a fallback.

### Repository status
Code: [foxx/status.rs](../src/foxx/status.rs)
- **What.** Every two minutes, a summary of policy results, integrity, compliance inputs, connection,
  and non-secret integration settings, uploaded with the sync.
- **Why.** Tech: the dashboard sees repository health without reading the repository. Product: a team
  sees at a glance whose rules are broken.
- **Without it.** The dashboard would show events but not rule health.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

---

## 8. The hosted service and dashboard

### DataSea
Code: [server/mod.rs](../src/server/mod.rs), [server/store.rs](../src/server/store.rs), [server/database.rs](../src/server/database.rs)
- **What.** The evidence store: locally, the event store in the trust directory's runtime folder;
  remotely, the MongoDB collections behind `crane server` (database `foxx`).
- **Why.** Tech: one name for local and hosted evidence. Product: the brand of the observability
  surface.
- **Without it.** No team-wide evidence.
- **Breaking now.** A single hosted deployment (Render plus MongoDB) is the only production topology.
  **MEDIUM** (scale and availability).
- **Fix.** Document and test a self-hosted deployment; add health checks and a second region when
  customers need it.

### Signed ingest request
Code: [server/ingest.rs](../src/server/ingest.rs), [server/http.rs](../src/server/http.rs)
- **What.** Every `/v1` request carries the installation's public key, a timestamp within five
  minutes, and a signature over method, path, timestamp, and body digest. An installation can only
  add to and read its own records (and read active quarantines).
- **Why.** Tech: the public key is the identity; no passwords. Product: machines upload without
  accounts.
- **Without it.** Anyone could upload fake evidence.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Limits and retention
Code: [server/limits.rs](../src/server/limits.rs), [server/database.rs](../src/server/database.rs)
- **What.** 120 requests a minute per machine, 2,000 records per kind per upload, 250,000 stored events
  and 50 repositories per machine, no uploads above 450 MB; events retained 30 days by default
  (`CRANE_MONGODB_RETENTION_DAYS`).
- **Why.** Tech: protects a shared database. Product: predictable cost per customer.
- **Without it.** One noisy machine could exhaust the service.
- **Breaking now.** Thirty-day retention is short for audits that look back a year. **MEDIUM**
  (product gap for regulated customers).
- **Fix.** Retention tiers per plan, with cold storage export.

### Dashboard sign-in and visibility
Code: [server/dashboard/auth.rs](../src/server/dashboard/auth.rs), [server/dashboard/mod.rs](../src/server/dashboard/mod.rs)
- **What.** GitHub OAuth (`read:org`) with an eight-hour signed session cookie and no stored GitHub
  token. A viewer sees a repository only if its verified remote is under the viewer or one of their
  organizations, and only after an upload proved that remote.
- **Why.** Tech: visibility follows GitHub ownership. Product: zero user administration for GitHub
  teams.
- **Without it.** A separate user and permission system would be needed.
- **Breaking now.** Only GitHub is supported; GitLab, Bitbucket, and Azure DevOps teams cannot sign in.
  **MEDIUM** (product gap).
- **Fix.** Add providers behind the same "verified remote owner" rule.

### Read-only dashboard handle
Code: [server/dashboard/data.rs](../src/server/dashboard/data.rs)
- **What.** Evidence views read MongoDB through a handle that cannot insert, update, or delete;
  pipelines that would write are refused.
- **Why.** Tech: viewing can never change evidence. Product: the dashboard is trustworthy as an audit
  view.
- **Without it.** A dashboard bug could corrupt evidence.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Dashboard writes and GitHub-confirmed approvals
Code: [server/dashboard/approve.rs](../src/server/dashboard/approve.rs)
- **What.** The only writes: linking a machine to a person, confirming the owner key, granting
  credits, Dashboard Agent configuration and incidents, quarantine requests, and flow-context
  proposals. Sensitive ones need a fresh GitHub check of repository-admin access.
- **Why.** Tech: narrow, repository-scoped writes. Product: admins act from the browser without
  touching the CLI.
- **Without it.** Every approval would need a terminal.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Dashboard Agent and incident
Code: [server/dashboard/api/operate.rs](../src/server/dashboard/api/operate.rs), [server/dashboard/notifications.rs](../src/server/dashboard/notifications.rs)
- **What.** A repository-scoped monitor (metric, threshold, lookback window, assignments to zones,
  flows, or selections) evaluated after each upload and in a 60-second sweep; crossing the threshold
  opens an incident idempotently. States `active`, `idle`, `inactive`, `sleep`.
- **Why.** Tech: monitoring next to the evidence, with a lease preventing duplicate work. Product:
  team-level alerting without configuring each machine.
- **Without it.** Only per-machine alerts.
- **Breaking now.** Monitors use fixed metrics; they cannot yet watch Work Capture or Insight
  patterns. **LOW.**
- **Fix.** Upload insight report summaries and let a monitor watch pattern counts.

### Remote quarantine
Code: [server/dashboard/approve.rs](../src/server/dashboard/approve.rs), [hooks/enforce.rs](../src/hooks/enforce.rs)
- **What.** An admin-approved rule on the service that makes local hooks deny matching tool calls (by
  agent id, session id, or tool name in a session). Hooks query it before every tool call; a service
  error also denies unless uploads are off.
- **Why.** Tech: a remote kill switch. Product: stop a runaway agent from a phone.
- **Without it.** Stopping a teammate's agent would need access to their machine.
- **Breaking now.** Failing closed on a service error means a service outage stops agent work on
  machines with uploads on. **MEDIUM** (availability trade-off).
- **Fix.** A short, signed, cached "no quarantines" answer with an expiry, so an outage degrades to the
  last known state for a bounded time.

### Agent-context API and data clear
Code: [server/dashboard/api/mod.rs](../src/server/dashboard/api/mod.rs), [cli/data.rs](../src/cli/data.rs)
- **What.** `GET /api/agent-context` returns compact, filtered event context (at most 500 events) in
  `foxx-agent-context/v1`; `/api/events.csv` the same as CSV. `crane data clear` lets a CODEOWNER
  permanently delete a repository's hosted records.
- **Why.** Tech: agents and tools can read evidence under a person's permissions. Product: data
  portability and the right to erase.
- **Without it.** No programmatic access; no way to remove data.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

---

## 9. The Landscape editor extension

### Landscape for VS Code (`foxx`)
Code: [extensions/foxx/src/extension.ts](../extensions/foxx/src/extension.ts), [core/crane.ts](../extensions/foxx/src/core/crane.ts)
- **What.** The editor side of Crane. It talks to Crane only through the CLI and never reads the trust
  directory, keys, or secrets. Disabled in untrusted workspaces.
- **Why.** Tech: one source of truth (the CLI). Product: governance where developers work.
- **Without it.** Governance would be CLI-only, which few people adopt.
- **Breaking now.** VS Code family only (no JetBrains). **LOW** (product gap).
- **Fix.** A JetBrains plugin reusing the same CLI contract.

### Panel, sidebar, and governance graph view
Code: [panel.ts](../extensions/foxx/src/panel.ts), [sidebar.ts](../extensions/foxx/src/sidebar.ts), [graphView.ts](../extensions/foxx/src/graphView.ts), [core/graph.ts](../extensions/foxx/src/core/graph.ts)
- **What.** The panel plans and runs actions (preserve, target, zone, flow, context) with a timeline and
  undo; the sidebar shows status, selections, zones, flows, agents, and findings; the graph view draws
  and links zones and flows, refusing cycles.
- **Why.** Tech: every action shows its explicit plan before running. Product: non-experts can govern
  code.
- **Without it.** Only YAML and terminal.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Governance editing
Code: [core/governance.ts](../extensions/foxx/src/core/governance.ts)
- **What.** Edits `governance.yaml` preserving comments and layout; new entities start `proposed`;
  every edit is previewed as a diff, validated, signed by Crane, and reverted if that fails.
- **Why.** Tech: no partial writes. Product: safe visual editing.
- **Without it.** Hand-edited YAML with frequent mistakes.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Diagnostics, CodeLens, and live agent context
Code: [extension.ts](../extensions/foxx/src/extension.ts), [core/live.ts](../extensions/foxx/src/core/live.ts), [src/foxx/live.rs](../src/foxx/live.rs)
- **What.** Validation findings and failing commands as editor diagnostics; while an agent works in
  governed code, the context it received is highlighted beside that code and in the status bar.
- **Why.** Tech: decorations never modify source. Product: a developer sees what the agent was told.
- **Without it.** The guidance an agent received would be invisible.
- **Breaking now.** Evidence Packs, Insight patterns, and capabilities are not shown in the editor.
  **LOW** (product gap).
- **Fix.** Show Work Capture receipts and Insight zone patterns in the sidebar through
  `crane capture … --json` and `crane insight report --json`.

---

## 10. Integrations

### Integration settings and secrets
Code: [integrations/mod.rs](../src/integrations/mod.rs), [integrations/prompt.rs](../src/integrations/prompt.rs)
- **What.** `crane integrate slack|jira|whatsapp|github` stores non-secret settings in
  `integrations/NAME.json` and credentials in owner-only `secrets/NAME.json` in the trust directory.
- **Why.** Tech: credentials never in the repository, events, logs, or dashboard. Product: alerts in
  the team's tools.
- **Without it.** Secrets would end up in configuration files.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Slack, WhatsApp, Jira, GitHub providers and the PR workflow
Code: [integrations/slack.rs](../src/integrations/slack.rs), [whatsapp.rs](../src/integrations/whatsapp.rs), [jira.rs](../src/integrations/jira.rs), [github.rs](../src/integrations/github.rs), [.github/workflows/pr-notify.yml](../.github/workflows/pr-notify.yml)
- **What.** Security alerts to Slack and WhatsApp and to the Jira issue in `CRANE_TASK_ID`; a workflow
  posts pull-request activity to Slack and comments on the Jira issue.
- **Why.** Product: incidents reach people where they work.
- **Without it.** Dashboard-only awareness.
- **Breaking now.** No Microsoft Teams or PagerDuty. **LOW.**
- **Fix.** Add providers behind the same settings and secrets split.

---

## 11. Work Capture (component 1)

Component 1 has its own chapter, [WORK_CAPTURE.md](WORK_CAPTURE.md), with every element explained and
every defect with its resolution. This section summarizes each term.

### Resource, resource type, connector, and resource registry
Code: [capture/resources/registry.rs](../src/capture/resources/registry.rs), [connector.rs](../src/capture/resources/connector.rs), [local.rs](../src/capture/resources/local.rs), [github.rs](../src/capture/resources/github.rs), [fixture.rs](../src/capture/resources/fixture.rs)
- **What.** A resource is an evidence source (Git, DataSea, Landscape, Crane, GitHub, CI, deployments,
  logs, metrics); its type is its category; a connector is the compiled-in code that reads it, behind
  the read-only contract `discover`, `fetch`, `query`, `get_metadata`, `health_check`. The registry
  validates every resource definition (scope, filters, secrets).
- **Why.** Tech: recipes ask for types, not products; only reviewed code can serve a source. Product:
  evidence from the tools a team already uses.
- **Without it.** Every investigation would be custom code with unchecked permissions.
- **Breaking now.** Deployments, logs, and metrics have no live connector; GitHub has no test-report
  API. **MEDIUM** (product gap).
- **Fix.** Add connectors (GitHub Actions artifacts for test reports, a deployment tool, a log store,
  a metrics store) with the existing contract harness.

### Connector status
Code: [capture/resources/connector.rs](../src/capture/resources/connector.rs)
- **What.** `Ok`, `Partial`, `Unavailable`, `Unauthorized`, `RateLimited`, `InvalidQuery`, `Timeout`,
  `Failed` on every source answer.
- **Why.** Tech: a failure is never an empty success. Product: each gap points to the right person.
- **Without it.** "Source down" would read as "nothing exists".
- **Breaking now.** Nothing known since the fixes. **NONE.**
- **Fix.** None needed.

### Access policy
Code: [capture/mod.rs](../src/capture/mod.rs) (`AccessPolicy`), [capture/ingest/selection.rs](../src/capture/ingest/selection.rs) (`withheld`)
- **What.** The resources, repositories, environments, evidence types, and sensitivity ceiling one
  actor may use, plus review rights; checked at scope resolution, every step, every record on arrival,
  and selection.
- **Why.** Tech: permission is code, not a prompt. Product: a contractor sees only what they may.
- **Without it.** Packs would leak forbidden facts.
- **Breaking now.** Nothing known since the fixes. **NONE.**
- **Fix.** None needed.

### Evidence recipe, RecipeBook, predicate, budget
Code: [capture/broker/recipe.rs](../src/capture/broker/recipe.rs)
- **What.** A recipe is a versioned, immutable plan for one question class (steps with branch
  predicates, required evidence, stopping rules, limits). The RecipeBook holds every version, pinned by
  digest. Predicates (`exists`, `missing`, `field_equals` with an optional subject,
  `any_file_matches`, `scope_known`, `all`, `any`, `not`) decide branches. Budgets cap calls, records,
  bytes, and time.
- **Why.** Tech: reproducible, reviewable retrieval. Product: "what does a deployment-failure
  investigation look at" has one answer.
- **Without it.** Every investigation would retrieve differently.
- **Breaking now.** Five built-in recipes; others come from explorations and learned recipes.
  **LOW.**
- **Fix.** Let Insight keep proposing learned recipes; add an incident recipe.

### Exploration
Code: [capture/broker/run.rs](../src/capture/broker/run.rs) (`explore`), [recipe.rs](../src/capture/broker/recipe.rs) (`exploration`)
- **What.** For a question class no recipe serves, the broker derives a bounded plan from the registry
  and each source's discovery (one query per resource type and evidence type the actor may receive,
  filtered by known scope values). The pack is `exploratory`, never `sufficient`.
- **Why.** Tech: deterministic, audited, replayable like any recipe. Product: a new codebase can answer
  questions from day one.
- **Without it.** Unknown questions would be refused.
- **Breaking now.** Discovery listings are used only for evidence types (incomplete listings keep
  every declared type), so an exploration can query types a source does not hold. **LOW.**
- **Fix.** Let connectors return a per-type count summary in discovery.

### Scope resolution and learned facts
Code: [capture/resources/scope.rs](../src/capture/resources/scope.rs)
- **What.** What the investigation is about (task, episode, repository, branch, commits, pull requests,
  deployments, environment, sessions, zones, flows, time window), which sources are permitted or denied,
  which facts are unknown, and which were learned from evidence (with the evidence that taught them).
- **Why.** Tech: authorization before any query. Product: imprecise questions get narrowed safely.
- **Without it.** Queries could widen to "anything".
- **Breaking now.** With several known commits, steps check the first and warn about the others.
  **LOW.**
- **Fix.** Run commit-based steps once per commit within the budget, with the replay key extended to
  parameters.

### Broker and query audit record
Code: [capture/broker/run.rs](../src/capture/broker/run.rs)
- **What.** The bounded middleman that resolves scope, rejects forbidden requests, runs steps with
  gates, calls connectors with deadlines and paging, ingests, links, learns, derives assertions, and
  seals the pack; every decision is written to the audit record.
- **Why.** Tech: no pass-through proxy. Product: every answer is explainable.
- **Without it.** Agents would query sources directly with their own credentials.
- **Breaking now.** Crane's own verification cannot be interrupted by the deadline. **LOW.**
- **Fix.** See Rule verification.

### Evidence record, raw artifact, sensitivity
Code: [capture/format/record.rs](../src/capture/format/record.rs), [artifact.rs](../src/capture/format/artifact.rs), [parsers.rs](../src/capture/format/parsers.rs)
- **What.** The normalized fact (source, type, payload, event time apart from ingestion time,
  provenance, scope, sensitivity, parse status, references); the raw artifact keeps the original
  hashed, redacted, and classified (`Public < Internal < Confidential < Secret`).
- **Why.** Tech: five payload shapes cover every source. Product: secrets never reach a pack.
- **Without it.** Raw source shapes everywhere; secrets in packs.
- **Breaking now.** No per-connector field map; unknown reference names link to nothing. **LOW.**
- **Fix.** Per-connector field maps in the connector specification.

### Evidence graph, relation vocabulary, relation status
Code: [capture/correlate/model.rs](../src/capture/correlate/model.rs), [graph.rs](../src/capture/correlate/graph.rs), [link.rs](../src/capture/correlate/link.rs)
- **What.** Entities and 19 typed relations with a fixed grammar; each relation is `observed`,
  `declared`, `inferred`, or `reviewed`. Bounded traversals build Context Views.
- **Why.** Tech: nothing linked by time proximity; malformed links refused. Product: "why is this
  related" is answerable.
- **Without it.** Facts would be a pile, not a picture.
- **Breaking now.** No word for "test covers file". **LOW.**
- **Fix.** Add vocabulary with a version bump when a producer needs it.

### Assertion and Truth
Code: [capture/correlate/assertions.rs](../src/capture/correlate/assertions.rs), [capture/mod.rs](../src/capture/mod.rs) (`Truth`)
- **What.** Claims (`ci_passed`, `tests_passed`, `deployment_succeeded`, `change_verified`,
  `policies_hold`, `governed_zone_touched`) with three-valued truth and evidence on both sides; copies
  count once.
- **Why.** Tech: disagreement is kept, never resolved by picking a winner. Product: "unknown" is never
  shown as success.
- **Without it.** Missing tests would read as passing.
- **Breaking now.** `contradicted` and `superseded` stay reserved by design. **NONE.**
- **Fix.** None needed.

### Evidence Pack, manifest, missing-evidence states, warnings
Code: [capture/ingest/pack.rs](../src/capture/ingest/pack.rs), [selection.rs](../src/capture/ingest/selection.rs)
- **What.** The bounded, authorized, sealed answer: evidence excerpts, entities, view, assertions,
  contributions (task-local apart from reusable), missing evidence with a state (`empty`, `unavailable`,
  `denied`, `not_queried`, `parse_failed`), typed warnings, and a digest manifest.
- **Why.** Tech: the input contract of components 2 to 8. Product: an honest, shareable receipt.
- **Without it.** Downstream components would have no trustworthy input.
- **Breaking now.** No renderer posts it as a PR check (the Evidence Receipt). **MEDIUM** (product
  gap).
- **Fix.** A receipt renderer and a GitHub check-run publisher.

### Context contribution and namespace
Code: [capture/ingest/contributions.rs](../src/capture/ingest/contributions.rs)
- **What.** A structured proposal of knowledge for a zone, flow, entity, or episode, in a namespace
  (`agent-proposed`, `observed`, `human-approved`, `system-derived`), with a lifecycle (proposed,
  accepted, rejected, superseded, expired). Only a different authorized human accepts.
- **Why.** Tech: agents never write trusted memory. Product: the organization's knowledge grows,
  reviewed.
- **Without it.** Either no learning or agents rewriting their own context.
- **Breaking now.** Accepted reusable context is not yet fed into the context the hooks give agents.
  **LOW.**
- **Fix.** See Policy context.

### Work episode, outcome, pack feedback, question
Code: [capture/ingest/episode.rs](../src/capture/ingest/episode.rs), [capture/store.rs](../src/capture/store.rs)
- **What.** An episode is a unit of work; its outcome (succeeded, failed, partially verified, unknown,
  abandoned) needs evidence of the episode and keeps a history. Feedback records whether a pack served
  a person. A question is a structured evidence request with a lifecycle.
- **Why.** Tech: "a successful tool call is not a successful task". Product: outcomes and feedback are
  what the system learns from.
- **Without it.** No ground truth to learn from.
- **Breaking now.** Outcomes are recorded by hand. **MEDIUM** (limits Insight).
- **Fix.** Derive a proposed outcome from verified assertions (all required evidence supported) for a
  person to confirm with one command.

### Replay and replay fidelity
Code: [capture/broker/run.rs](../src/capture/broker/run.rs) (`replay`, `compare`)
- **What.** Re-runs a recorded investigation with its exact recipe and reports `exact`, `diverged`, or
  `unverifiable`, in both directions.
- **Why.** Tech: drift detection. Product: "would we conclude the same today?"
- **Without it.** Old packs could not be re-checked.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Capture store
Code: [capture/store.rs](../src/capture/store.rs)
- **What.** JSON-lines collections under `runtime/capture/` (artifacts, blobs, evidence, entities,
  relations, assertions, contributions, episodes, questions, packs, audits, recipe proposals, state),
  merged under a lock on save.
- **Why.** Tech: content ids make merges safe. Product: local-first evidence.
- **Without it.** Nothing persists.
- **Breaking now.** Save reads and merges every collection. **MEDIUM** (at scale).
- **Fix.** Append-only segments and persisted indexes; a database backend for hosted use.

---

## 12. The capability pipeline (components 2 to 7)

### Capability
Code: [capability/model.rs](../src/capability/model.rs)
- **What.** A reusable, versioned, governed *description* of a procedure: provenance, applicability,
  dependencies, an execution contract, and a verification contract. Never executable by itself.
- **Why.** Tech: knowledge separate from execution. Product: "how we do X here" becomes an asset
  agents can be offered.
- **Without it.** Every agent session starts from zero.
- **Breaking now.** Few real capabilities exist, because live episodes with full evidence are rare
  (deployment and test connectors missing). **MEDIUM** (product gap).
- **Fix.** Close the connector gaps; let Insight's work patterns point to candidate episodes.

### Capability intake
Code: [capability/intake.rs](../src/capability/intake.rs), [capability/refs.rs](../src/capability/refs.rs)
- **What.** The traceable handoff from an Evidence Pack: recompute the pack manifest, resolve every
  reference under the submitter's access, compare zone, flow, and policy versions with the current
  ones, and record what is uncertain.
- **Why.** Tech: nothing enters the registry unverified. Product: capabilities are grounded in evidence.
- **Without it.** Capabilities could cite evidence that was never there.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Validation gates and profiles
Code: [capability/validate.rs](../src/capability/validate.rs)
- **What.** Six gates (structural, provenance, applicability, verification, governance, publication)
  under a profile (`conservative` default, `low-risk-technical`). Technical evidence never verifies a
  business invariant.
- **Why.** Tech: each gate is a separate, testable check. Product: a published capability has a
  known bar.
- **Without it.** Publication would be a judgment call.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Capability lifecycle and staleness
Code: [capability/lifecycle.rs](../src/capability/lifecycle.rs), [capability/registry.rs](../src/capability/registry.rs)
- **What.** `candidate`, `needs_evidence`, `validating`, `validated`, `published`, `deprecated`,
  `revoked`, `rejected`; publication needs a Publisher who is not the author. `crane capability stale`
  marks a validation stale when what it rested on changed.
- **Why.** Tech: immutable versions with an append-only history. Product: a capability can be trusted,
  retired, or revoked.
- **Without it.** Outdated procedures would keep being offered.
- **Breaking now.** Staleness is checked on command, not continuously. **LOW.**
- **Fix.** Run the stale check in the background sync and surface it on the dashboard.

### Capability compiler, statement basis, compilation outcomes
Code: [compiler/](../src/compiler/) (`reconstruct.rs`, `extract.rs`, `context.rs`, `verify.rs`, `assemble.rs`)
- **What.** Turns one completed episode into a candidate plus a compilation report. Every statement
  keeps its basis (`observed`, `declared`, `derived`, `inferred`, `hypothesis`, `unknown`). Outcomes:
  `candidate_generated`, `needs_more_evidence`, `insufficient_reusable_signal`, `compilation_failed`,
  `compilation_blocked`.
- **Why.** Tech: chronology is never causation; one episode never proves broad applicability. Product:
  turns real work into a draft procedure automatically.
- **Without it.** Capabilities would be written by hand.
- **Breaking now.** Compiles one episode at a time; recurring work across episodes (an Insight work
  pattern) cannot be compiled jointly. **MEDIUM.**
- **Fix.** A multi-episode compilation that keeps only steps observed in every cited episode and
  widens applicability only to observed contexts.

### Validation Lab and check states
Code: [capability_lab/](../src/capability_lab/) (`preflight.rs`, `conformance.rs`, `scenarios.rs`, `invariants.rs`, `report.rs`, `store.rs`)
- **What.** Deterministic, non-executing checks of one exact candidate: preflight, conformance,
  scenarios from an approved typed test plan, and invariants. Each check is `passed`, `failed`,
  `not_run`, `blocked`, or `inconclusive`. A lab report changes no lifecycle state.
- **Why.** Tech: nothing a candidate describes is executed. Product: confidence before publication.
- **Without it.** Candidates would be published untested.
- **Breaking now.** Behavior the lab cannot execute stays `inconclusive`, so dynamic correctness is not
  proven. **LOW** (by design).
- **Fix.** Optional sandboxed dry runs through invocation adapters, reported separately.

### Discovery and discovery outcomes
Code: [discovery/](../src/discovery/) (`resolve.rs`, `evaluate.rs`, `report.rs`)
- **What.** For a task, finds published capabilities through fixed gates (eligibility, validation
  currency, applicability, scope, dependencies, lab reports), then ranks. Outcomes: `recommended`,
  `no_match`, `insufficient_context`, `blocked`. Unknown is never a match.
- **Why.** Tech: explainable, deterministic matching. Product: an agent is offered only what applies.
- **Without it.** Agents would be offered irrelevant or unsafe procedures.
- **Breaking now.** No semantic or fuzzy matching, so a differently worded task class misses.
  **LOW** (by design; limits recall).
- **Fix.** A model may propose task-class mappings as hypotheses for a person to accept, never as
  matches.

### Invocation: binding set, operation adapter, approval, reconciliation, ledger
Code: [invocation/](../src/invocation/) (`plan.rs`, `adapter.rs`, `preflight.rs`, `engine.rs`, `lifecycle.rs`, `ledger.rs`)
- **What.** Invokes one exact published version through explicitly registered, typed operations chosen
  by a reviewed binding set; rechecks before execution; risky and state-changing operations need
  approval bound to the plan digest; uncertain effects require reconciliation; only verified
  postconditions make `succeeded`. `TransactionRuntime` is the interface to an external durable runtime.
- **Why.** Tech: never runs recorded tool calls or model output. Product: reuse without blind trust.
- **Without it.** Capabilities could not be acted on safely.
- **Breaking now.** Only a read-only `git.commit` adapter is registered. **HIGH** (product gap: nothing
  state-changing can be invoked yet).
- **Fix.** Add typed adapters for common operations (open a pull request, run a CI workflow, create a
  ticket), each with effect classes, reconciliation, and tests.

### Evolution: observations, signals, change sets, regression, review packet
Code: [evolution/](../src/evolution/) (`outcomes.rs`, `patterns.rs`, `drift.rs`, `proposals.rs`, `regression.rs`, `workflow.rs`, `models.rs`)
- **What.** Reads invocation outcomes, classifies them, collapses retries, detects and qualifies
  signals (`repeated_failure`, `verification_gap`, `dependency_drift`, …), proposes minimal typed change
  sets as new candidate versions, runs regression checks, and packages a review packet. Never publishes.
- **Why.** Tech: deterministic qualification rules. Product: capabilities improve with use.
- **Without it.** Capabilities would rot.
- **Breaking now.** Live outcome history is thin; there is no review-request delivery, and
  `submitted_for_review` is not produced. **MEDIUM.**
- **Fix.** Deliver review packets through the dashboard and integrations; produce
  `submitted_for_review` when delivered.

---

## 13. Insight (component 8)

### Insight
Code: [insight/mod.rs](../src/insight/mod.rs), [cli/insight.rs](../src/cli/insight.rs)
- **What.** Finds patterns across everything Work Capture kept and turns them into proposals people
  review. Writes nothing it reads.
- **Why.** Tech: deterministic, versioned miners. Product: the system learns from observed work
  (business and agentic intelligence) without agents rewriting rules.
- **Without it.** Each investigation is forgotten after it answers.
- **Breaking now.** Reads Work Capture's store only (not the registry, invocation ledger, evolution,
  or hosted events). **MEDIUM.**
- **Fix.** Add read models over the other stores, under the same access rules.

### Read model
Code: [insight/read.rs](../src/insight/read.rs)
- **What.** The store as one viewer may see it: packs, audits, and episodes whose repository and
  environment the viewer may access, and evidence passing Work Capture's own `withheld` check.
- **Why.** Tech: aggregate statistics cannot leak hidden records. Product: a contractor's insight is
  their own.
- **Without it.** Rates would leak forbidden environments.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Miner, pattern, counts, minimum support
Code: [insight/miners.rs](../src/insight/miners.rs)
- **What.** Five miners (`evidence-gaps`, `questions`, `trust`, `outcomes`, `work`) produce patterns
  with counts (matching, total, unknown), an optional baseline, cited episodes, packs, and evidence.
  Counts below 3 are suppressed.
- **Why.** Tech: co-occurrence with denominators, never causation. Product: actionable, honest
  intelligence.
- **Without it.** Patterns would be anecdotes.
- **Breaking now.** Thresholds are constants (3 support, 2 judged explorations, 20-point margin).
  **LOW.**
- **Fix.** Make them signed configuration per repository.

### Destination
Code: [insight/mod.rs](../src/insight/mod.rs) (`Destination`), [propose.rs](../src/insight/propose.rs)
- **What.** Where a pattern can lead: zone context, a recipe, a capability (via the compiler), a
  governance decision, or a report.
- **Why.** Tech: every output goes through an existing review step. Product: insight becomes action.
- **Without it.** Patterns would end as text.
- **Breaking now.** Capability and governance destinations are pointed at people, not handed off.
  **LOW.**
- **Fix.** After multi-episode compilation exists, hand work patterns to the compiler as a proposal.

### Recipe proposal and learned recipe
Code: [capture/broker/proposals.rs](../src/capture/broker/proposals.rs), [insight/propose.rs](../src/insight/propose.rs)
- **What.** A complete recipe proposed by the system actor `crane-insight` from explorations people
  judged sufficient; an owner or admin accepts it, which adds a new pinned RecipeBook version
  (`learned-CLASS`).
- **Why.** Tech: learned retrieval passes the same validation and pinning as built-ins. Product: the
  system grows its own playbook.
- **Without it.** Explorations would never become reusable.
- **Breaking now.** Nothing known. **NONE.**
- **Fix.** None needed.

### Insight report
Code: [insight/mod.rs](../src/insight/mod.rs), [insight/store.rs](../src/insight/store.rs)
- **What.** A digested report (viewer, miners version, considered counts, patterns, suppressed count)
  appended to `runtime/insight/reports.jsonl`; a changed report is refused when read.
- **Why.** Tech: reproducible and tamper-evident. Product: trends over time.
- **Without it.** No history of what was learned.
- **Breaking now.** Reports are local; the dashboard does not show them. **LOW.**
- **Fix.** Upload report summaries with the sync.

---

## 14. Index of state values

| Object | Values | Defined in |
|---|---|---|
| Governance state | generation 1, 2, 3, … | [governance/state.rs](../src/governance/state.rs) |
| Selection location | resolved, `UNRESOLVED` | [selection/tracker.rs](../src/selection/tracker.rs) |
| Zone or flow status | `proposed`, `active` | [governance/zones.rs](../src/governance/zones.rs) |
| Criticality | `routine`, `sensitive`, `critical`, `restricted` | [governance/zones.rs](../src/governance/zones.rs) |
| Autonomy ceiling / mode | `observe`, `assisted`, `delegated`, `autonomous` | [governance/zones.rs](../src/governance/zones.rs), [hooks/enforce.rs](../src/hooks/enforce.rs) |
| Change type | `logical_bn`, `logical_cn`, `logical_sn`, `semantic` | [governance/changes.rs](../src/governance/changes.rs) |
| Safety state | `ACTIVE`, `DEGRADED`, `QUARANTINED` | [telemetry/model.rs](../src/telemetry/model.rs) |
| Agent state | `INACTIVE`, `ACTIVE`, `DEGRADED`, `QUARANTINED`, `STALE`, `ENDED`, `ERROR` | [DASHBOARD.md](../DASHBOARD.md) |
| Decision | `ALLOW`, `REQUIRE_APPROVAL`, `DENY`, `QUARANTINE`, `ERROR` | [telemetry/model.rs](../src/telemetry/model.rs) |
| Execution outcome | `SUCCESS`, `FAILURE`, `PARTIAL`, `UNKNOWN` | [telemetry/model.rs](../src/telemetry/model.rs) |
| Assessment | `COMPLIANT`, `VIOLATION`, `SUSPICIOUS`, `UNRESOLVED` | [telemetry/model.rs](../src/telemetry/model.rs) |
| Enforcement outcome | `PREVENTED`, `DETECTED_AFTER_EFFECT`, `BYPASS_CONFIRMED`, `COVERAGE_GAP`, `NOT_APPLICABLE` | [telemetry/model.rs](../src/telemetry/model.rs) |
| Known as | `OBSERVED`, `INFERRED`, `ESTIMATED`, `UNSUPPORTED`, `REDACTED`, `UNOBSERVED`, `LOST` | [telemetry/model.rs](../src/telemetry/model.rs) |
| Severity | `INFO`, `LOW`, `MEDIUM`, `HIGH`, `CRITICAL` | [telemetry/model.rs](../src/telemetry/model.rs) |
| Credit movement | grant, reserve, consume, release, revoke, expire | [telemetry/ledger.rs](../src/telemetry/ledger.rs) |
| Compliance control | `SATISFIED`, `PARTIAL`, `NO_EVIDENCE`, `NOT_APPLICABLE` | [telemetry/compliance.rs](../src/telemetry/compliance.rs) |
| Alert rule | ok, firing, reminding, resolved | [telemetry/rules.rs](../src/telemetry/rules.rs) |
| Remote quarantine | pending, active, release pending, released | [server/dashboard/approve.rs](../src/server/dashboard/approve.rs) |
| Dashboard Agent | `active`, `idle`, `inactive`, `sleep` | [GUIDE.md](GUIDE.md#dashboard-agents) |
| Connector status | `ok`, `partial`, `unavailable`, `unauthorized`, `rate_limited`, `invalid_query`, `timeout`, `failed` | [capture/resources/connector.rs](../src/capture/resources/connector.rs) |
| Step state | `executed`, `skipped`, `not_queried`, `denied`, `failed`, `budget_exhausted` | [capture/broker/run.rs](../src/capture/broker/run.rs) |
| Stop reason | `completed`, `satisfied`, `budget_exhausted`, `rejected` | [capture/broker/run.rs](../src/capture/broker/run.rs) |
| Investigation status | `sufficient`, `incomplete`, `budget_exhausted`, `rejected`, `exploratory` | [capture/ingest/pack.rs](../src/capture/ingest/pack.rs) |
| Source state (missing evidence) | `collected`, `empty`, `unavailable`, `denied`, `not_queried`, `parse_failed` | [capture/ingest/pack.rs](../src/capture/ingest/pack.rs) |
| Parse status | `parsed`, `partially_parsed`, `failed` | [capture/format/record.rs](../src/capture/format/record.rs) |
| Ingest kind | `new`, `duplicate`, `new_revision` | [capture/store.rs](../src/capture/store.rs) |
| Relation status | `observed`, `declared`, `inferred`, `reviewed` | [capture/correlate/model.rs](../src/capture/correlate/model.rs) |
| Truth | `yes`, `no`, `unknown` | [capture/mod.rs](../src/capture/mod.rs) |
| Assertion status | `supported`, `unresolved`, `proposed` (`contradicted`, `superseded` reserved) | [capture/correlate/assertions.rs](../src/capture/correlate/assertions.rs) |
| Replay fidelity | `exact`, `diverged`, `unverifiable` | [capture/broker/run.rs](../src/capture/broker/run.rs) |
| Contribution status | `proposed`, `accepted`, `rejected`, `superseded`, `expired` | [capture/ingest/contributions.rs](../src/capture/ingest/contributions.rs) |
| Namespace | `agent-proposed`, `observed`, `human-approved`, `system-derived` | [capture/ingest/contributions.rs](../src/capture/ingest/contributions.rs) |
| Episode outcome | `succeeded`, `failed`, `partially_verified`, `unknown`, `abandoned` | [capture/ingest/episode.rs](../src/capture/ingest/episode.rs) |
| Question state | `open`, `answered`, `blocked`, `unresolvable` | [capture/ingest/episode.rs](../src/capture/ingest/episode.rs) |
| Recipe proposal | `proposed`, `accepted`, `rejected` | [capture/broker/proposals.rs](../src/capture/broker/proposals.rs) |
| Capability version | `candidate`, `needs_evidence`, `validating`, `validated`, `published`, `deprecated`, `revoked`, `rejected` | [capability/lifecycle.rs](../src/capability/lifecycle.rs) |
| Compilation outcome | `candidate_generated`, `needs_more_evidence`, `insufficient_reusable_signal`, `compilation_failed`, `compilation_blocked` | [compiler/mod.rs](../src/compiler/mod.rs) |
| Statement basis | `observed`, `declared`, `derived`, `inferred`, `hypothesis`, `unknown` | [compiler/mod.rs](../src/compiler/mod.rs) |
| Lab check | `passed`, `failed`, `not_run`, `blocked`, `inconclusive` | [capability_lab/mod.rs](../src/capability_lab/mod.rs) |
| Discovery outcome | `recommended`, `no_match`, `insufficient_context`, `blocked` | [discovery/report.rs](../src/discovery/report.rs) |
| Invocation | `requested`, `authorizing`, `preflighting`, `plan_ready`, `awaiting_approval`, `executing`, `verifying`, `reconciliation_required`, `succeeded`, `failed`, `partially_verified`, `unknown`, `blocked`, `abandoned` | [invocation/lifecycle.rs](../src/invocation/lifecycle.rs) |
| Evolution signal | detected, qualifying, qualified, insufficient, investigating, proposal, no change, dismissed, superseded | [evolution/models.rs](../src/evolution/models.rs) |
| Evolution proposal | draft, candidate_created, regression_pending, regression passed / failed / inconclusive, (`submitted_for_review` reserved), abandoned, superseded | [evolution/models.rs](../src/evolution/models.rs) |
| Insight pattern kind | `evidence_gap`, `question`, `trust`, `outcome`, `work` | [insight/mod.rs](../src/insight/mod.rs) |

---

## 15. Index of data models and where they are stored

| Data model | Defined in | Stored in | Why there |
|---|---|---|---|
| Selection record | [selection/registry.rs](../src/selection/registry.rs) | `.crane/registry.json` | travels with the code, signed |
| Policy | [governance/policy.rs](../src/governance/policy.rs) | `.crane/policies/*.crane` | reviewable in pull requests |
| Checkpoint | [governance/checkpoints.rs](../src/governance/checkpoints.rs) | `.crane/checkpoints.json` | baseline shared by the team |
| Configuration | [governance/config.rs](../src/governance/config.rs) | `.crane/config.json` | signed team defaults |
| Zones, flows, context packs | [governance/zones.rs](../src/governance/zones.rs) | `.crane/governance.yaml` | editable, signed |
| Context binding | [governance/context.rs](../src/governance/context.rs) | `.crane/context.json` | digest-bound guidance |
| Governance audit entry | [governance/audit.rs](../src/governance/audit.rs) | `.crane/audit.jsonl` | hash chain under the manifest |
| Manifest | [governance/state.rs](../src/governance/state.rs) | `.crane/` | one signature over everything |
| Keys, anchors, secrets, alert rules | [trust/keys.rs](../src/trust/keys.rs), [integrations/mod.rs](../src/integrations/mod.rs) | `~/.crane-trust/` | outside agents' reach |
| Event, ledger entry | [telemetry/events.rs](../src/telemetry/events.rs), [ledger.rs](../src/telemetry/ledger.rs) | `~/.crane-trust/…/runtime/` | local authoritative evidence |
| Uploaded records, Dashboard Agents, incidents, approvals | [server/store.rs](../src/server/store.rs) | MongoDB `foxx` | team view, the service holds credentials |
| Evidence records, packs, audits, episodes, contributions, recipe proposals | [capture/store.rs](../src/capture/store.rs) | `runtime/capture/*.jsonl` | local-first, content-addressed |
| Capability versions | [capability/registry.rs](../src/capability/registry.rs) | `runtime/` capability store | immutable versions |
| Compilation reports | [compiler/assemble.rs](../src/compiler/assemble.rs) | `runtime/` compiler store | separate from the candidate |
| Lab reports | [capability_lab/store.rs](../src/capability_lab/store.rs) | `runtime/` lab store | append-only |
| Discovery reports | [discovery/report.rs](../src/discovery/report.rs) | `runtime/` discovery store | append-only |
| Invocation records | [invocation/ledger.rs](../src/invocation/ledger.rs) | `runtime/` invocation ledger | idempotent, locked |
| Evolution records | [evolution/reports.rs](../src/evolution/reports.rs) | `runtime/` evolution store | apart from the registry |
| Insight reports | [insight/store.rs](../src/insight/store.rs) | `runtime/insight/reports.jsonl` | apart from what it reads |
