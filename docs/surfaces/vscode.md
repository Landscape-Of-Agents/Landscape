# Landscape for VS Code

The editor side of Crane: protect or target code from a selection, draw zones and flows, and see
the context agents receive next to the code. It works only through the `crane` command line and never
reads the trust directory, keys, or secrets.

Install: search for **Landscape** in the Extensions view, or
`code --install-extension agentscript.foxx`. It needs `crane` 0.4.3 or later on `PATH` and a
trusted workspace.

## Everyday use

1. **Set up**: "Landscape: Set Up Crane in This Repository" connects the clone and runs `crane init`
   (in a repository a teammate set up, it shows the owner key's fingerprint to confirm).
2. **Govern code**: select lines, click the fox icon (or Ctrl+Alt+F), then search for what you want:
   preserve, target, zone, flow, or context.
3. **See the map**: "Landscape: Show Governance Graph" draws zones, flows, and selections; drag one
   zone onto another to link them into a flow.
4. **Check**: "Validate Governance" and "Test Policies" turn findings into editor diagnostics and
   CodeLens results.
5. **Connect agents**: "Install Agent Hooks" for Claude Code, Codex, Copilot CLI, or Gemini CLI, then
   commit `.crane`.

## Bricks and controls

- **Bricks view** (Landscape sidebar): drafts and active versions by type, Stacks, attachments per
  Flow or Zone with their Current Context, assigned agents with their runtime status, and whether
  the signed store verifies. Problems Crane reports, such as a draft that does not validate or a pin
  that fails closed, appear in Problems.
- **Create, edit, validate, test** a Brick of one of the fixed types. Common Context and
  configuration are written in an editor. `test` can simulate the Brick on a chosen Flow or Zone.
- **Activate, attach, detach, upgrade, edit Current Context, revoke**: each runs Crane with
  `--dry-run` first and shows what the guard will restrict before and after and the snapshot agents
  will receive. Nothing is signed until you confirm, and Crane signs with your admin key.
- **Preview effective context** for any Flows and Zones: every source with its digest, then the
  exact text agents receive.
- **Run a Crane Control**: the command families (scope, permission, precondition, constraint,
  approval, isolation, verification, provenance, budget, recovery, revocation, selection, skill,
  runbook, brick) from a searchable list. It asks for the arguments each command requires, asks
  for confirmation before commands that change state, and shows Crane's result.

- **Resident agents** (in the Bricks view): assignments by Zone or Flow with their status and
  Crane's answer now; show one with its latest activations, preview the context pack it would
  receive, create a draft (Crane's `--dry-run` validation first, then confirmation), validate
  it, and pause, resume, or revoke it after confirming. Activation stays an admin's step. See [resident agents](../features/resident-agents.md).

These need a Crane with `crane brick` (and `crane resident` for resident agents); with an older
Crane the view says so.

## While agents work

When an agent touches governed code, the guidance it receives is highlighted in the editor, so you
see what it was told, including the Brick snapshot of the Flows and Zones it is in.

## What it does not do

It does not sign anything on its own (signing runs `crane` with your key), and it does not author
skills or runbooks (that is the CLI today, and the dashboard Skill Builder later).

Full reference: [extensions/foxx/README.md](../../extensions/foxx/README.md).
