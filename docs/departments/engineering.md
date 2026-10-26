# Engineering

Agents already write and run code at scale, so engineering is where everything works today.

## What you get

| Need | How | Status |
|---|---|---|
| Agents must not touch audited code (fees, auth, migrations) | `crane protect` the lines; edits are refused before they happen | exists |
| A task must really change something (not a cosmetic edit) | `crane target` with a change type; whitespace never counts | exists |
| Risky areas need care | zones and flows with criticality and an autonomy ceiling (observe, assisted, ...) | exists |
| Agents should know the house rules | context on every selection, zone, and flow, given to the agent the moment it touches that code | exists |
| The same rules for Claude Code, Codex, Copilot CLI, Gemini CLI | one set of hooks, one guard | exists |
| Know the rules hold | `crane scope test` (adversarial probes), `crane test .`, `crane validate` | exists |
| Catch rules loosened by a refactor | `crane scope diff` fails when a governed file moves out of its zone | exists |
| Reusable procedures (fix, test, verify) | skills, run only through registered operations | partial: read-only Git operations exist; operations that change code are to build |
| Proof on every pull request | Evidence Receipts as a required check | to build |

## A day with it

1. A tech lead protects the fee calculation and marks `payments/` as a critical zone with an
   `assisted` ceiling (in VS Code or with `crane protect` and `crane governance sign`).
2. An agent tries to "simplify" the fee rule. The edit is simulated, refused, and the agent is told
   why, with the rule's context. It fixes the real bug elsewhere.
3. An agent edits a file in `payments/`: the change waits for a person's approval.
4. Before release, `crane scope test` and `crane test .` run in CI.

## Skills and runbooks for engineers

| Example | Template | What makes it safe |
|---|---|---|
| "Summarize what a commit changed" | read-only-lookup | only the read-only `git/git.commit` operation |
| "Fix a bug and prove it" | repository-change | the change step needs a registered code-change operation (to build); tests and evidence prove success |
| "Triage a failed deployment" (runbook) | skills in order | each step its own invocation; uncertain effects reconciled, never retried blindly |

## Controls that matter most

Scope (`crane scope`), verification (tests and evidence, never the agent's word), and recovery
(reconcile before retry). See [features/controls.md](../features/controls.md).

## Honest limits

- Shell commands are checked as text; indirection (`f=pay; sed -i ... ${f}ments.py`) passes the
  pre-check and is caught afterwards, which quarantines the session.
- Cloud agents are covered at the pull request only once the check exists.

Start: [products/crane.md](../products/crane.md#get-started). Commands: [surfaces/cli.md](../surfaces/cli.md).
