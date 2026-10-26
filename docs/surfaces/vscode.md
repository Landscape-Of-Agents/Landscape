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

## While agents work

When an agent touches governed code, the guidance it receives is highlighted in the editor, so you
see what it was told.

## What it does not do

It does not sign anything on its own (signing runs `crane` with your key), and it does not author
skills or runbooks (that is the CLI today, and the dashboard Skill Builder later).

Full reference: [extensions/foxx/README.md](../../extensions/foxx/README.md).
