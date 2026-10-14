# Foxx for VS Code

Foxx is the editor side of Crane. It creates selection contracts from your selection, shows where
every selection is and whether it passes, and edits zones, flows, and context packs in
`.crane/governance.yaml`. It talks to Crane only through the Crane CLI (`crane validate --json`,
`crane test . --json`, `crane protect`, `crane target`, ...) and never reads Crane's trust
directory, signing keys, or integration secrets.

Install: search for **Foxx** in the Extensions view, or
`code --install-extension agentscript.foxx`
([Marketplace](https://marketplace.visualstudio.com/items?itemName=agentscript.foxx)).

## Requirements

- `crane` 0.4.3 or later on `PATH` (or set `foxx.cranePath`); download it from the
  [releases](https://github.com/VishuKalier2003/Agentscript/releases).
- A clone of a GitHub repository, opened as a trusted workspace (Foxx runs the Crane CLI, so it is
  disabled in restricted mode).

## Where to find Foxx

- **The fox icon at the top right of the editor** (or **Ctrl+Alt+F**): select a block of code first,
  then click it. The Foxx panel opens beside the code with a search box: type what you want
  (preserve, target, zone, flow, context, …) and fill in the form. Without a selection, Foxx asks
  you to select a code block first.
- **Right-click a file in the Explorer → Open in Foxx:** the same panel for the whole file.
- **Right-click in the editor → Foxx:** preserve or target the selected lines, attach the file as
  policy context, create a policy, and more.
- **Status bar:** the **Foxx** item (bottom left) shows the repository's state.
- **Command palette (Ctrl+Shift+P):** type "Foxx".

## Getting started without the command line

1. **Foxx: Set Up Crane in This Repository**: connects the clone to GitHub and runs `crane init`.
   In a new repository you become the owner and a `baseline` checkpoint is recorded; in a
   repository a teammate set up, Foxx shows the owner key's fingerprint and asks you to confirm it.
2. **Foxx: Create Policy**: name it and choose its file in `.crane/policies`.
3. Select lines in the editor and click the fox icon at the top right (or Ctrl+Alt+F). In the Foxx
   panel, search for what you want: preserve or target the lines, make them a zone or a flow (with
   criticality, autonomy ceiling, and context for the agent), or add context to an existing one.
4. **Foxx: Install Agent Hooks** for Claude Code, Codex, GitHub Copilot CLI, or Gemini CLI, then
   commit the `.crane` folder.

## Commands

| Command | What it does |
| --- | --- |
| Foxx: Open Foxx | The Foxx panel for the selected lines (or, from the Explorer, a whole file) |
| Foxx: Set Up Crane in This Repository | Connect the clone (`crane repo`), `crane init` (confirming the owner key when joining), and a `baseline` checkpoint for a new repository |
| Foxx: Create Policy | `crane create policy NAME FILE` |
| Foxx: Create Checkpoint | `crane checkpoint NAME`: record the current commit as trusted |
| Foxx: Move Selection to Policy | `crane add policy NAME marker ID` |
| Foxx: Install Agent Hooks | `crane agent install --profile claude\|codex\|copilot\|gemini` |
| Foxx: Validate Governance | `crane validate --json`; findings become diagnostics |
| Foxx: Test Policies | `crane test . --json`; failing and unresolved commands become diagnostics and CodeLens outcomes |
| Foxx: Policy Status | `crane policy NAME status` in the Foxx output |
| Foxx: Undo Last Governance Change | Restore (and sign) the previous `governance.yaml` |

Zone, flow, and context changes made in the panel are validated by Crane and signed with
`crane governance sign`; only the repository's owner and admins can sign, and the change is
reverted if validation or signing fails. An edit to `governance.yaml` that nobody signed fails
verification and stops agent changes until an admin signs it. Decorations and CodeLens never
modify source files; only `crane protect` and `crane target` insert anchor comments.

## Development

```bash
npm install
npm test          # compiles and runs the editor-independent unit tests
```

Press F5 in VS Code with this folder open to run the extension in a development host.

Not implemented yet: extension telemetry events in the Foxx event store (the CLI has no
ingestion command in 0.4), and integration tests in a VS Code host.
