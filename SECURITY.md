# Security policy

Crane is a security tool: it stands between AI agents and the code they change. A weakness in
Crane can let an agent do what it was told not to, so reports are taken seriously and handled
privately.

## Supported versions

Only the latest release receives security fixes. Upgrade to the newest
[release](https://github.com/Landscape-Of-Agents/Landscape/releases/latest) (Crane CLI and the Landscape
extension together) before reporting.

| Version | Supported |
| --- | --- |
| Latest 0.4.x release | Yes |
| Older releases | No |

## Reporting a vulnerability

**Do not open a public issue for a vulnerability.**

Report it privately through GitHub:
[Security → Report a vulnerability](https://github.com/Landscape-Of-Agents/Landscape/security/advisories/new).
Include:

- what an attacker (or an agent) can do, and what it should not be able to do;
- the Crane and Landscape versions, the operating system, and the agent (Claude Code, Codex, Copilot
  CLI, Gemini CLI) involved;
- the smallest steps, repository layout, or hook payload that reproduces it.

You can expect an acknowledgement within 3 working days and an assessment within 10. Fixes are
released as soon as they are ready, and the advisory credits you unless you prefer otherwise.
Please give us a reasonable time to release a fix before disclosing it.

## What counts

In scope:

- an agent changing preserved code, `.crane/`, or Crane's markers without being denied;
- a forged, copied, or edited selection, policy, registry, or `governance.yaml` (zones, flows, and
  their source markers) that validation accepts;
- a way for an agent to run governance commands, unseal a signing key, or disable the hooks
  unnoticed;
- the hosted service: reading another repository's or another member's data, writing as someone
  else, bypassing GitHub-confirmed approvals, or exhausting its limits;
- secrets (tokens, keys, webhook URLs) leaking into events, uploads, logs, or the dashboard.

Out of scope: an agent with an unrestricted shell running as your own user reading files in your
home directory (see the model below), denial of service against your own machine, and reports from
automated scanners without a working attack.

## Security model

- **The signed registry is the authority.** Selections, policies, checkpoints, configuration,
  `governance.yaml`, and the zone and flow markers it names are covered by a manifest signed with
  Ed25519 over SHA-512 digests. Anything changed outside Crane fails `crane validate`, and agent
  changes stop until a human signs again.
- **Only people change the rules.** No `crane` command runs inside an agent session, and the hooks
  deny any agent command or script that invokes `crane`. Only the owner and the admins they sign can sign rules; teammates trust the
  owner key after comparing its fingerprint, and a GitHub admin can confirm it on the dashboard.
  Agents see the boundary without running Crane: the session context lists the files where active
  zones and flows restrict changes (restrictions only, never the limits of enforcement), and every
  denial names its rule. People inspect, probe, diff, and audit it with `crane scope`.
- **One worktree per session.** A write into another worktree of the same repository is denied
  (`OTHER_WORKTREE_WRITE`): its protected code cannot be checked against the rules this session
  resolved. Work in that worktree with its own session.
- **Skills are signed and withdrawable everywhere.** A published capability's exact definition is
  signed into `.crane/skills.json` under the manifest. Invocation treats that store as authoritative
  for withdrawal (deprecated, revoked, or another digest blocks), and a store that fails
  verification blocks every invocation. The store never makes a version eligible by itself.
- **Keys stay out of the repository.** Signing keys and runtime evidence live in `CRANE_HOME`
  (default `~/.crane-trust`). On Windows signing keys are sealed with the Data Protection API;
  `CRANE_KEY_PROTECTION=prompt` seals them under a passphrase typed on the console, which a process
  running as you (such as an agent's shell) cannot supply. Hooks never unseal a signing key. An
  agent with an unrestricted shell running as your user can still read the rest of that directory:
  run agents in a sandbox or a separate account for a hard boundary.
- **Scripts are read before they run.** Before a shell command runs a script (`python x.py`,
  `node x.js`, `./x.sh`, `npm run NAME`, inline `-c`/`-e` code), Crane reads it: code that names
  Crane's files, changes files holding preserved selections, runs downloaded or decoded code, or is
  encoded PowerShell is denied; reading credential files while using the network, rewriting Git
  history, and deleting directory trees need approval. At stop, files changed outside every
  recorded tool call are reported as unattributed changes.
- **Evidence is tamper-evident.** Events are hash-chained per machine; the hosted service checks
  that every upload continues the chain and raises a critical alert for a rewrite, gap, or fork.
- **The service holds the only database credentials.** Machines sign uploads with their own upload
  key and can only add to and read their own records. Dashboard views read through a handle that
  cannot write; the few dashboard writes are repository-scoped and checked against GitHub.
- **What is not observed.** Network, CPU, memory, and token usage are not observed by hooks and are
  reported as unsupported, never estimated.
