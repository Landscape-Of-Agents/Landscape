# Install the agent hooks

`crane agent install` adds Crane's hooks to **Claude Code** or **Codex**. After that, Crane checks every tool call against the repository's signed rules, before it runs.

- Calls that touch preserved code, or go beyond a zone's autonomy ceiling, are denied.
- Every decision is recorded, and appears on the Crane dashboard.
