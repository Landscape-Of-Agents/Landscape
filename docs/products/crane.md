# Crane: signed rules for every AI agent (free)

Crane is one rulebook for every AI agent an organization uses. People sign the rules; Crane checks
each agent action against them before it happens, looks at what really changed afterwards, and
keeps a tamper-evident record. It is free and open source (Apache 2.0).

## Who it is for

Every team using AI agents on code or systems that matter, and platform teams rolling agents out to
many teams. It is worth most when more than one agent tool is in use, because the same rules apply
to all of them.

## What it does

| Capability | Status | Read |
|---|---|---|
| Protect or target exact lines of code, signed | exists | [features/selections.md](../features/selections.md) |
| Zones and flows with risk levels and autonomy limits | exists | [features/zones-and-flows.md](../features/zones-and-flows.md) |
| Decide every action of Claude Code, Codex, Copilot CLI, and Gemini CLI before it runs, and verify after | exists | [features/agent-hooks.md](../features/agent-hooks.md) |
| See, test, diff, and audit the effective boundary | exists | [features/scope.md](../features/scope.md) |
| Skills: reusable procedures that run only through checked, typed operations | exists (Git read operations; state-changing adapters to build) | [features/skills.md](../features/skills.md) |
| Runbooks: skills in order, with approvals, branches, budgets | exists | [features/runbooks.md](../features/runbooks.md) |
| The eleven controls as commands (permission, approval, budget, revocation, ...) | exists | [features/controls.md](../features/controls.md) |
| Shared, signed skill store: a revocation reaches every machine | exists | [features/skills.md](../features/skills.md#sharing-and-withdrawing) |
| Authoring in VS Code | exists | [surfaces/vscode.md](../surfaces/vscode.md) |
| A pull-request check (covers cloud agents Crane never saw) | to build | [architecture/intelligence.md](../architecture/intelligence.md) |
| A tool gateway (MCP) so tool calls pass the same checks | to build | [architecture/intelligence.md](../architecture/intelligence.md) |
| A sandbox that confines shells, network, and credentials | to build | [features/controls.md](../features/controls.md) |
| A Skill Builder on the dashboard (plain forms, drafts a signer activates) | to build (decided) | [features/skills.md](../features/skills.md#the-skill-builder) |

## The promises it keeps

- **Only people change the rules.** Rules live in `.crane/`, signed by an owner or admin. Agents
  cannot write there, cannot run `crane`, and cannot change their own hook settings.
- **Decisions come from the real files.** A proposed edit is applied in memory first; what the agent
  says it will do does not count.
- **Restrictions only add up.** Where rules overlap, the strictest one wins. Restructuring code
  never loosens anything, and `crane scope diff` catches a protected file renamed out of its zone.
- **Unknown is never "fine".** If Crane cannot decide, the answer is no; if it cannot see, it says
  so (a coverage gap).
- **Allowed is not done.** A skill succeeds only when evidence proves its claims.

## Honest limits

- Crane decides at the hooks of agent tools on your machine. Cloud agents are covered only once the
  pull-request check and the gateway exist.
- Shell commands are checked as text before they run; the real effect is judged afterwards. Network
  traffic and files outside the repository are not confined. Repository scope is not a sandbox.
- A shell running as you can read the trust directory. Use a sandbox or a separate account for a
  hard boundary ([SECURITY.md](../../SECURITY.md)).

## Get started

1. Install the `crane` binary and, optionally, Landscape for VS Code.
2. In the repository: `crane repo --https URL`, `crane init`, `crane checkpoint baseline`.
3. Protect the first critical code: `crane protect app/fees.py start-line 10 end-line 24 context "Fees are audited."`
4. Connect your agent tools: `crane agent install --profile claude` (or codex, copilot, gemini).
5. Check the boundary holds: `crane scope test`.

Full command reference: [surfaces/cli.md](../surfaces/cli.md).
