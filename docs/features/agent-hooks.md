# Agent hooks: deciding every action

Agent tools call Crane before and after every action through hooks:
`crane agent install --profile claude` (or `codex`, `copilot`, `gemini`).

## Before an action

The guard decides from the real files, in this order, each step only able to make the answer
stricter:

1. **Integrity**: the signed rules must verify, or every change is refused.
2. **Safety state**: a quarantined session may only read; a degraded one needs approval.
3. **Protected files**: `.crane/`, the trust directory, and hook settings are refused, even with approval.
4. **Shell check**: commands naming protected files, running `crane`, or modifying protected code are refused.
5. **Script check**: scripts are read first (decoded or downloaded code, history rewrites, secrets with network use).
6. **Edit simulation**: the edit is applied in memory; touching protected lines or markers is refused.
7. **Zone limits**: observe ceilings refuse, assisted ceilings and restricted code ask a person.
8. **Autonomy mode**: observe refuses, assisted asks.
9. **Credits**: when the session's budget of risky actions runs out, actions need a person.

Every answer names its rule (`PRESERVED_SELECTION`, `ZONE_ASSISTED`, ...) and gives the agent the
context of the code it touched, so it can fix the cause instead of asking again.

## After an action

The after-check looks at what really changed on disk. A violation degrades the session; protected
code changed without passing the guard (a bypass) quarantines it. Files changed outside every
recorded action are reported at the end of the session.

## What agents are told

At session start: the rules, the policies and their selections, the context packs, the resolved list
of restricted files and their limits, and how actions are judged. Agents never run `crane`; this is
their read-only view of the boundary.

## Explain a decision later

```
crane scope explain TOOL_CALL_ID      # the rules, their domains, and whether it still applies
crane scope audit --days 7            # decisions by rule, bypasses, coverage gaps, ungoverned changes
```

## Honest limits

- The shell and script checks read text. Indirection gets past the pre-check; the after-check is the
  backstop.
- An agent tool without hooks, or with hooks removed, is unguarded; it shows up as a stale session.
- Writes into another worktree of the same repository are refused; files elsewhere outside the
  repository are not governed.
