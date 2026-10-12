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

## Getting started without the command line

1. **Foxx: Set Up Crane in This Repository**: connects the clone to GitHub and runs `crane init`.
   In a new repository you become the owner and a `baseline` checkpoint is recorded; in a
   repository a teammate set up, Foxx shows the owner key's fingerprint and asks you to confirm it.
2. **Foxx: Create Policy**: name it and choose its file in `.crane/policies`.
3. Select lines in the editor and run **Foxx: Create Selection Contract** (or right-click): choose
   preserve or target and the policy.
4. **Foxx: Install Agent Hooks** for Claude Code or Codex, then commit the `.crane` folder.

## Commands

| Command | What it does |
| --- | --- |
| Foxx: Set Up Crane in This Repository | Connect the clone (`crane repo`), `crane init` (confirming the owner key when joining), and a `baseline` checkpoint for a new repository |
| Foxx: Create Policy | `crane create policy NAME FILE` |
| Foxx: Create Checkpoint | `crane checkpoint NAME`: record the current commit as trusted |
| Foxx: Move Selection to Policy | `crane add policy NAME marker ID` |
| Foxx: Install Agent Hooks | `crane agent install --profile claude\|codex` |
| Foxx: Create Selection Contract | Preserve or target the selected lines (`crane protect` / `crane target`), choosing the policy, change type, and an optional name |
| Foxx: Preserve Selected Lines / Target Selected Lines | Shortcuts for the two operations |
| Foxx: Validate Governance | `crane validate --json`; findings become diagnostics |
| Foxx: Test Policies | `crane test . --json`; failing and unresolved commands become diagnostics and CodeLens outcomes |
| Foxx: Policy Status | `crane policy NAME status` in the Foxx output |
| Foxx: Attach File as Policy Context | `crane add policy-context file FILE policy NAME` |
| Foxx: Create Zone / Create Flow | Add a **proposed** zone or flow |
| Foxx: Attach to Zone / Attach to Flow | Add a selector (path, file, directory, symbol, selection) or an entry point / include / exclude |
| Foxx: Attach Context Pack | Create or reuse a context pack and attach it to a zone or flow |
| Foxx: Activate Zone or Flow | Turn a proposed entity into an enforced one (an explicit authority change) |
| Foxx: Preview Impact | Zone, flow, selector, and context findings of the current configuration |
| Foxx: Undo Last Governance Change | Restore the previous `governance.yaml` |
| Foxx: Create Proposal Branch | Commit `governance.yaml` on a new branch for review |
| Foxx: Open Dashboard | Open the hosted Foxx dashboard (setting `foxx.dashboardUrl`) |

Every governance edit is shown as a diff first, written only after you confirm, validated by
Crane, and reverted automatically if Crane reports a zone, flow, or selector error. Decorations
and CodeLens never modify source files; only `crane protect` and `crane target` insert anchor
comments.

## Development

```bash
npm install
npm test          # compiles and runs the editor-independent unit tests
```

Press F5 in VS Code with this folder open to run the extension in a development host.

Not implemented yet: extension telemetry events in the Foxx event store (the CLI has no
ingestion command in 0.4), and integration tests in a VS Code host.
