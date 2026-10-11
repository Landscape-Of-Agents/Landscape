# Crane

Crane enforces **AgentScript** contracts on AI coding agents. You mark lines of code an agent must
**preserve** or must **change** (target); Crane checks every agent action at the hook boundary,
verifies what actually changed on disk, and records tamper-evident security evidence that the
**Foxx** dashboard turns into metrics, action graphs, and reports.

- Works with any language that has comments: selections are marked with anchor comments such as
  `# @crane:selection:K7M2P9RX:start` that move with the code.
- A signed registry (Ed25519, SHA-512) is the authority, so forged, copied, or edited markers and
  registry files are detected.
- Claude Code and Codex hooks deny edits to protected code, `.crane/`, and the markers before they
  run, and detect shell bypasses after they run.
- Hash-chained events, an autonomy-credit ledger, and metrics that say how each value is known.

## Install

```bash
cargo install --path .        # requires Rust 1.89+ and Git
crane --version
```

`gh` is needed for `crane github`; `curl` for Slack, Jira, and WhatsApp delivery.

## Quick start

```bash
crane repo --https https://github.com/OWNER/REPO    # connect the clone to its GitHub repository
crane init                                          # create .crane/ and a signing key
crane checkpoint baseline                           # trusted baseline = current HEAD

crane protect app/payments.py start-line 12 end-line 30
crane target  app/refunds.py  start-line 5 end-line 18 change_type logical_bn

crane validate          # integrity, syntax, and markers
crane test .            # run every preserve/target command
crane agent install --profile claude
```

Then open the dashboard at <https://agentscript-service.onrender.com> and sign in with GitHub.

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
| Repository | `repo --ssh\|--https\|--gh-cli REMOTE [privileges]`, `repo status`, `repo --view`, `repo del`, `github`, `github days DAYS` |
| Policies | `create policy NAME FILENAME`, `add policy NAME marker MARKER`, `policy NAME status` |
| Runtime | `task current [ID]`, `task .`, `session current [ID]`, `session .` |
| Agents | `agent install\|uninstall\|hooks --profile claude\|codex` |
| Admins | `admin list`, `admin add PUBLIC_KEY [name NAME]`, `admin del PUBLIC_KEY` |
| Integrations | `integrate jira\|slack\|whatsapp\|github` |

`protect` and `target` options: `policy NAME`, `checkpoint NAME`, `start-line N`, `end-line N`,
`name LABEL`. Run `crane help` for details. Governance commands refuse to run inside an AI agent
session, and the hooks deny them.

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

## Integrations and alerts

- `crane integrate slack|jira|whatsapp|github` stores settings outside the repository and can test
  the connection. Security alerts go to Slack and WhatsApp, and to the Jira issue in
  `CRANE_TASK_ID`.
- `.github/workflows/pr-notify.yml` posts pull-request activity to Slack and comments on the Jira
  issue named in the PR title or branch. Set the repository secrets `SLACK_WEBHOOK_URL`,
  `JIRA_BASE_URL`, `JIRA_EMAIL`, and `JIRA_API_TOKEN`.

## Dashboard

One dashboard for individuals and teams: <https://agentscript-service.onrender.com>. Sign in with
GitHub, then choose an account, a repository, and optionally one team member.

- **Personal account:** the repositories your GitHub account owns.
- **Organization:** the organization's repositories with every teammate's activity, and a member
  filter. Organizations appear when they allow this app (organization settings, third-party
  access).
- **Views:** overview, agents and sessions, the action graph of every session, the event timeline
  with filters, governance (policies, selections, checkpoints, audit trail), the autonomy ledger,
  every metric with how it is known, compliance controls, alerts, members, health, session reports,
  and evidence bundles.
- **Read-only:** the dashboard reads MongoDB through a handle that cannot write, and accepts only
  GET requests. Machines and agents can only upload: no upload route returns stored records, and
  the dashboard answers only signed-in people. A repository appears only after an upload proves its
  GitHub remote, so nobody can attach their uploads to someone else's repository.

## Telemetry upload (Foxx service)

Crane uploads events, metrics, sessions, tool calls, alerts, and governance history to the Foxx
ingestion service (`https://agentscript-service.onrender.com`), which stores them in MongoDB.
Nothing needs to be set up: uploads happen when a tool call ends, when a session ends, after every
command, and every 60 seconds. Hooks never wait for the network, and the local evidence stays
authoritative when the service is unreachable.

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
| `CRANE_INGEST_URL` | Foxx ingestion service; `off` keeps telemetry local | the release's service |
| `CRANE_SYNC_SECONDS` | Periodic upload interval | `60` |
| `CRANE_MONGODB_URI`, `CRANE_MONGODB_DB`, `CRANE_MONGODB_EVENT_RETENTION_DAYS` | `crane server` only (never on developer machines) | required, `foxx`, `30` |
| `CRANE_GITHUB_CLIENT_ID`, `CRANE_GITHUB_CLIENT_SECRET` | `crane server`: dashboard sign-in (GitHub OAuth app) | sign-in off |
| `CRANE_PUBLIC_URL` | `crane server`: public address, when it cannot be detected | `RENDER_EXTERNAL_URL` or the request host |
| `CRANE_SERVER_REQUESTS_PER_MINUTE`, `CRANE_SERVER_ADDRESS_REQUESTS_PER_MINUTE` | `crane server`: rate per machine and per client address | `120`, `600` |
| `CRANE_SERVER_MAX_BATCH_RECORDS`, `CRANE_SERVER_MAX_EVENTS_PER_INSTALLATION`, `CRANE_SERVER_MAX_REPOSITORIES_PER_INSTALLATION`, `CRANE_SERVER_MAX_STORAGE_MB` | `crane server`: upload limits | `2000`, `250000`, `50`, `450` |
| `CRANE_ALLOW_LOCAL_REMOTE`, `CRANE_TEST_MONGODB_URI` | Offline mirrors and the live ingestion test suite | unset |

## Security notes

Signing keys and runtime evidence live in `CRANE_HOME` (default `~/.crane-trust`), never in the
repository. An agent with an unrestricted shell running as your user can still read that directory;
run agents in a sandbox or separate account for a hard boundary. Network, CPU, memory, and token
usage are not observed by hooks and are reported as unsupported.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --features server -- -D warnings
cargo test --features server   # set CRANE_TEST_MONGODB_URI to include the MongoDB and dashboard tests
bash scripts/smoke_test.sh
```

The VS Code extension lives in [extensions/foxx](extensions/foxx).

## License

Apache-2.0
