# The operator dashboard

A separate dashboard from DataSea, for settings only the Crane operator may change. It holds one
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

## One-time setup (the operator)

1. **Create the operator key.** On your machine, in a normal terminal:
   ```bash
   crane operator keygen
   ```
   - It prints `CRANE_OPERATOR_SIGNING_KEY=…` (secret) and `CRANE_OPERATOR_PUBLIC_KEY=…`.
   - Store the signing key in a password manager. It is shown only once.
2. **Create a GitHub OAuth app** (GitHub → Settings → Developer settings → OAuth Apps → New):
   - Homepage URL: `https://crane-operator.onrender.com` (or your custom domain);
   - Authorization callback URL: `https://crane-operator.onrender.com/auth/callback`;
   - copy the Client ID and generate a Client secret.
3. **Deploy on Render.** In the Render dashboard, open the Blueprint for this repository and sync
   it: [render.yaml](../../render.yaml) defines the `crane-operator` web service (same Docker image
   as DataSea, command `crane operator serve`). Fill in the secrets it asks for:

   | Variable | Value |
   |---|---|
   | `CRANE_MONGODB_URI` | the same MongoDB connection string DataSea uses (its own database `crane_operator`) |
   | `CRANE_OPERATOR_SIGNING_KEY` | the signing key from step 1 |
   | `CRANE_GITHUB_CLIENT_ID`, `CRANE_GITHUB_CLIENT_SECRET` | the OAuth app from step 2 |
   | `CRANE_OPERATOR_LOGINS` | preset to `VishuKalier2003`; add more logins separated by commas |

   If Render gives the service another address, update the OAuth app's URLs to match.
4. **Make official builds trust the key.** In GitHub → the repository → Settings → Secrets and
   variables → Actions → Variables, add:
   - `CRANE_OPERATOR_PUBLIC_KEY`: the public key from step 1;
   - `CRANE_OPERATOR_URL`: the dashboard address, for example `https://crane-operator.onrender.com`.

   The release workflow compiles both into every binary. Builds without the key never let agents
   run Crane commands.
5. **Release** a new Crane version and install it where you want the switch to apply.

## Using it

1. Open the dashboard and click **Sign in with GitHub**.
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
- **`tests/operator_service.rs`** runs against a live MongoDB and a stand-in GitHub:
  - a non-operator is refused;
  - a change needs the session, the header and a reason;
  - machines follow on and off;
  - a build trusting another key refuses the dashboard.
