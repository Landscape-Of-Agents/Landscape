# Landscape for VS Code

Landscape is the editor side of Crane. It creates selection contracts from your selection, shows where
every selection is and whether it passes, and edits zones, flows, and context packs in
`.crane/governance.yaml`. It talks to Crane only through the Crane CLI (`crane validate --json`,
`crane test . --json`, `crane protect`, `crane target`, ...) and never reads Crane's trust
directory, signing keys, or integration secrets.

Install: search for **Landscape** in the Extensions view, or
`code --install-extension agentscript.foxx`
([Marketplace](https://marketplace.visualstudio.com/items?itemName=agentscript.foxx)).

## Requirements

- `crane` 0.4.3 or later on `PATH` (or set `foxx.cranePath`); download it from the
  [releases](https://github.com/Landscape-Of-Agents/Landscape/releases).
- A clone of a GitHub repository, opened as a trusted workspace (Landscape runs the Crane CLI, so it is
  disabled in restricted mode).

## Where to find Landscape

- **The fox icon at the top right of the editor** (or **Ctrl+Alt+F**): select a block of code first,
  then click it. The Landscape panel opens beside the code with a search box: type what you want
  (preserve, target, zone, flow, context, …) and fill in the form. Without a selection, Landscape asks
  you to select a code block first.
- **Right-click a file in the Explorer → Open in Landscape:** the same panel for the whole file.
- **Right-click in the editor → Landscape:** preserve or target the selected lines, attach the file as
  policy context, create a policy, and more.
- **Landscape sidebar → Bricks:** the Bricks list, with the orange **Open Bricks Panel** button under it.
  The panel opens beside the editor and shows the Zones and Flows with the Bricks attached to each. You can create a Brick, attach it to a Zone or Flow, and edit its
  common context (shared by every attachment) or the volatile context of one attachment. A guide
  explains each of the twelve kinds, and the **i** button next to a Brick opens its configuration.
- **The Resident Agents icon in the activity bar:** opens the Resident Agents panel. It has the same
  graph, with the agents working on each Zone and Flow. You can create an agent and set all its
  properties. Saved agents are listed under "Not working yet" until you put one to work on a Zone or
  Flow, for 30 to 90 days per period, renewable.
- **Governance graph:** drag a zone onto another to link them into a flow. Hover a zone or flow
  and press **×** (or Delete) to remove it; a removed flow keeps its zones.
- **Status bar:** the **Landscape** item (bottom left) shows the repository's state.
- **Command palette (Ctrl+Shift+P):** type "Landscape".

## Getting started without the command line

1. **Landscape: Set Up Crane in This Repository**: connects the clone to GitHub and runs `crane init`.
   In a new repository you become the owner and a `baseline` checkpoint is recorded; in a
   repository a teammate set up, Landscape shows the owner key's fingerprint and asks you to confirm it.
2. **Landscape: Create Policy**: name it and choose its file in `.crane/policies`.
3. Select lines in the editor and click the fox icon at the top right (or Ctrl+Alt+F). In the Landscape
   panel, search for what you want: preserve or target the lines, make them a zone or a flow (with
   criticality, autonomy ceiling, and context for the agent), or add context to an existing one.
4. **Landscape: Install Agent Hooks** for Claude Code, Codex, GitHub Copilot CLI, or Gemini CLI, then
   commit the `.crane` folder.

## Commands

| Command | What it does |
| --- | --- |
| Landscape: Open Landscape | The Landscape panel for the selected lines (or, from the Explorer, a whole file) |
| Landscape: Set Up Crane in This Repository | Connect the clone (`crane repo`), `crane init` (confirming the owner key when joining), and a `baseline` checkpoint for a new repository |
| Landscape: Create Policy | `crane create policy NAME FILE` |
| Landscape: Create Checkpoint | `crane checkpoint NAME`: record the current commit as trusted |
| Landscape: Move Selection to Policy | `crane add policy NAME marker ID` |
| Landscape: Install Agent Hooks | `crane agent install --profile claude\|codex\|copilot\|gemini` |
| Landscape: Validate Governance | `crane validate --json`; findings become diagnostics |
| Landscape: Test Policies | `crane test . --json`; failing and unresolved commands become diagnostics and CodeLens outcomes |
| Landscape: Policy Status | `crane policy NAME status` in the Landscape output |
| Landscape: Show Governance Graph | Zones, flows, and selections on a canvas: drag zones around, drag from a zone's right handle onto another zone to link them into a flow, click a link and press Delete to remove it, and filter what is shown |
| Landscape: Connect Zones in the Graph | Open the graph ready for linking (no names to type) |
| Landscape: Undo Last Governance Change | Restore (and sign) the previous `governance.yaml` |
| Landscape: Create Brick | A draft of one of the fixed Brick types (`crane brick create --file`); it grants nothing until an admin activates it |
| Landscape: Validate / Test Brick | `crane brick validate`; `crane brick test`, optionally simulated on a chosen Flow or Zone |
| Landscape: Activate Brick (sign) | Previews with `--dry-run`, then `crane brick activate` after you confirm |
| Landscape: Attach Brick to a Zone or Flow | Choose a version and a target, write the attachment's Current Context, see what the guard will restrict and what agents will receive, then sign |
| Landscape: Edit Current Context / Detach / Pin to Another Version / Revoke | The matching `crane brick` command, previewed first where it signs |
| Landscape: Preview Effective Context | `crane brick context preview`: every source with its digest, then the exact text agents receive |
| Landscape: Run a Crane Control | The command families (scope, permission, precondition, constraint, approval, isolation, verification, provenance, budget, recovery, revocation, selection, skill, runbook, brick) with their required arguments; confirmation before state changes |

Zone, flow, and context changes made in the panel are validated by Crane and signed with
`crane governance sign`; only the repository's owner and admins can sign, and the change is
reverted if validation or signing fails. An edit to `governance.yaml` that nobody signed fails
verification and stops agent changes until an admin signs it. Decorations and CodeLens never
modify source files; only `crane protect` and `crane target` insert anchor comments.

Linking zones on the graph follows one rule: a flow is one ordered path, and the order of all
flows together stays acyclic. Linking two free zones creates a flow (enforced when both zones
were); linking from the last zone of a flow, or to its first, extends it; linking the end of one
flow to the start of another merges them. A link to itself, from the middle of a flow, or one that
would close a cycle is refused while you drag, and checked again by Crane before it is signed.
Removing a link splits the flow; a zone left alone becomes a zone again. Flows inherit the most
restrictive criticality and autonomy ceiling of their zones.

While an agent works in a selection, zone, or flow that has context, Landscape highlights that context
next to the code (a tinted background and a label), at the top of the Overview, and in the status
bar, so you see what the agent was told. Turn it off with `foxx.agentContext`.

## Development

```bash
npm install
npm test          # compiles and runs the editor-independent unit tests
```

Press F5 in VS Code with this folder open to run the extension in a development host.

Not implemented yet: extension telemetry events in the Landscape event store (the CLI has no
ingestion command in 0.4), and integration tests in a VS Code host.
