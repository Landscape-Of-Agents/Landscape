# The operator dashboard

**<https://dashboard.agentslandscape.cloud/operator/>**

A separate dashboard from DataSea's pages, for settings only the Crane operator may change. It is
served by the DataSea service at `/operator/` (its own page and its own operator-only access,
sharing only DataSea's GitHub sign-in and database), and can also run on its own with
`crane operator serve`. It holds one
setting today: **whether AI agents may run read-only Crane commands inside their own sessions**
(Claude Code, Codex, Copilot CLI, Gemini CLI, or any other agent Crane recognizes).

It is **off** by default, and off is the answer whenever anything is in doubt.

## What the switch does

| Switch | Agents in a session | People |
|---|---|---|
| **Off** (default) | Every `crane` command is refused, by the CLI and by the hooks, as before | Unaffected |
| **On** | One plain, read-only command at a time runs: `crane validate`, `crane test .`, `crane scope inspect\|resolve\|explain\|diff\|test\|audit`, `crane session …`, `crane task …`, `crane flow list\|show\|draft\|incidents`, `crane brick list\|show\|history\|agents\|inbox`, `crane resident list\|show\|status\|…`, and the other read-only commands listed on the dashboard | Unaffected |

Even when it is on:

- **Nothing that changes anything runs for an agent.** Signing, protecting, targeting, checkpoints,
  governance, admins, hooks, secrets, integrations, Brick and flow changes, resident actions, alert
  changes, and `crane operator` itself stay human-only. `commit()` and every mutating command keep
  their own check as well.
- **Only one plain invocation passes the hook.** Pipes, `&&`, `;`, redirections, `$(…)`,
  backticks, quotes, and environment assignments in front are denied, as are scripts that run
  Crane.
- The CLI process the agent starts checks the same thing again. The hook and the CLI share one
  list ([src/trust/agent_commands.rs](../../src/trust/agent_commands.rs)).
- Agents are told at session start which form applies.

## Who can change it

Only you, the operator:

1. **Signing in.** The dashboard lets a GitHub account in only when its login is listed in
   `CRANE_OPERATOR_LOGINS` on the operator service. Anyone else who signs in with GitHub is
   refused, and no session is created. Every `/api` request checks the list again, so removing a
   login revokes access at once.
2. **Changing.** A change needs your session, a JSON body, the `X-Crane-Operator` header (so no
   other site can submit it), and a reason. Every change is kept in the audit with who, when, and
   why.
3. **Machines cannot be fooled.** A machine accepts only answers **signed by the operator key**
   whose public half is compiled into official Crane builds (`CRANE_OPERATOR_PUBLIC_KEY`).
   - **Customers cannot turn it on.** They have no operator key, and pointing
     `CRANE_OPERATOR_URL` at a service of their own yields answers that fail verification.
   - **Customers can always turn it off** on their machine with `CRANE_AGENT_COMMANDS=off`.
   - **An answer expires 15 minutes after it was issued.** An unreachable dashboard therefore
     means off once the cached answer expires. A copied or replayed answer grants nothing beyond
     its own expiry.

## Setup

Inside DataSea, nothing needs to be configured:

- **Sign-in** is DataSea's GitHub OAuth app; signing in from the operator page returns to it.
- **Operators** are `CRANE_OPERATOR_LOGINS` on the DataSea service, `VishuKalier2003` when it is
  not set. To add someone, set it in Render (comma-separated logins).
- **The operator key** is created by the service on its first start and kept in its MongoDB
  (`service_settings`, name `operator_key`). It survives restarts and redeploys. Setting
  `CRANE_OPERATOR_SIGNING_KEY` on the service uses that key instead; machines trust only the key
  their build was made with, so a changed key means "off" until a release trusts the new one.
- **Official builds** trust the key through two repository variables the release workflow
  compiles in: `CRANE_OPERATOR_PUBLIC_KEY` (shown on the dashboard and in every
  `/v1/agent-commands` answer) and `CRANE_OPERATOR_URL` (`https://dashboard.agentslandscape.cloud`,
  also the built-in default).

**Running it on its own instead:** `crane operator serve` with `CRANE_MONGODB_URI`,
`CRANE_OPERATOR_DB` (default `crane_operator`), `CRANE_OPERATOR_LOGINS`, a GitHub OAuth app
(`CRANE_GITHUB_CLIENT_ID`, `CRANE_GITHUB_CLIENT_SECRET`, callback `PUBLIC_URL/auth/callback`), and
optionally `CRANE_OPERATOR_SIGNING_KEY` (`crane operator keygen` makes one).

## Using it

1. Open <https://dashboard.agentslandscape.cloud/operator/> and click **Sign in with GitHub**.
2. The panel shows the switch (**ON** or **OFF**), who changed it last and when, what agents may
   run, the operator public key, and the audit.
3. Type a reason and click **Turn on** or **Turn off**, then confirm.
4. Machines pick it up at their next background sync. That happens after every tool call, so
   within about a minute while agents work. To check one machine now:
   ```bash
   crane operator status --refresh
   ```
   It prints the key this build trusts, the dashboard address, the switch with who set it and how
   long the answer is valid, and "agents may run read-only commands now: yes/no".

## How a machine decides, step by step

| Step | What happens | Result when it fails |
|---|---|---|
| 1 | Background sync runs, even with DataSea uploads off: `GET {operator}/v1/agent-commands` (5 s timeout) | Keeps the cached answer, which expires on its own |
| 2 | Verifies the Ed25519 signature against the build's operator key, the issue time (≤5 min in the future), and the lifetime (≤15 min) | Discards it; the cached answer stays until it expires |
| 3 | Stores it in `~/.crane-trust/operator/agent-commands.json` | — |
| 4 | Session start: if on, the agent is told which commands it may run | Otherwise it is told not to run crane |
| 5 | Pre-tool hook, for a shell command naming crane: allowed only if it is one plain read-only invocation, the person did not set `CRANE_AGENT_COMMANDS=off`, and the cached answer verifies **now** and says on | Denied (`CRANE_COMMAND`), as before |
| 6 | The crane process the agent started: the same check on its arguments | "crane cannot be executed by an AI agent … (reason)" |
| 7 | The command runs and is recorded like any command | — |

## Endpoints

Inside DataSea every route below is under `/operator/`, except `/v1/agent-commands`, which is at the host's root.

| Route | Who | What |
|---|---|---|
| `GET /` | anyone | The page (it shows only sign-in to non-operators) |
| `GET /auth/login`, `/auth/callback`, `/auth/logout` | anyone | GitHub sign-in; only listed logins get a session |
| `GET /api/state` | operator | The switch, the read-only list, the public key, the audit |
| `POST /api/agent-commands` | operator | `{"enabled": true\|false, "reason": "…"}` with `X-Crane-Operator: 1` |
| `GET /v1/agent-commands` | machines | `{policy: {enabled, issued_at, expires_at, changed_by, changed_at}, public_key, signature}` |
| `GET /health` | the host | Database reachable |

## Tested

- **`tests/operator.rs`** runs the real binary and its hooks, with no network:
  - off by default;
  - on only with a fresh operator signature;
  - forged, expired and "off" answers refused;
  - the local off honored;
  - writes, chains and redirections denied;
  - the session context follows the switch.
- **`tests/operator_service.rs`** runs against a live MongoDB and a stand-in GitHub, both inside DataSea and standalone:
  - a non-operator is refused (a DataSea session alone gets 403);
  - the operator key survives a restart;
  - a change needs the session, the header and a reason;
  - machines follow on and off;
  - a build trusting another key refuses the dashboard.
