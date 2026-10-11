# Foxx for VS Code

Foxx is the editor side of Crane. It creates selection contracts from your selection, shows where
every selection is and whether it passes, and edits zones, flows, and context packs in
`.crane/governance.yaml`. It talks to Crane only through the Crane CLI (`crane validate --json`,
`crane test . --json`, `crane protect`, `crane target`, ...) and never reads Crane's trust
directory, signing keys, or integration secrets.

## Requirements

- `crane` 0.4 on `PATH` (or set `foxx.cranePath`), in a repository where `crane init` has run.
- A trusted workspace: Foxx is disabled in restricted mode because it runs the Crane CLI.

## Commands

| Command | What it does |
| --- | --- |
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
