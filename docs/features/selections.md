# Selections: protect and target code

A selection is a block of lines a person chose to govern. It gets an 8-character id, two marker
comments that move with the code, and a signed record in `.crane/registry.json`.

## Two kinds

| Kind | Rule | Example |
|---|---|---|
| **Protect** (`preserve` in the policy) | The lines must stay exactly as they were at the checkpoint | the fee calculation, an auth check |
| **Target** | The lines must change, in a stated way: `logical_bn` (business logic), `logical_cn` (loops and branches), `logical_sn` (structure), `semantic` (names, comments). Whitespace never counts | the refund logic a task must rewrite |

## Commands

```
crane protect app/fees.py start-line 10 end-line 24 context "Fees are audited; keep the rate."
crane target  app/refunds.py start-line 40 end-line 58 change_type logical_bn context "Rewrite the partial-refund rule."
crane selection inspect K7M2P9RX      # the signed record and where it is now
crane selection validate [K7M2P9RX]   # the same verifier as 'crane validate' and the hooks
crane selection repair K7M2P9RX       # markers lost? where its signed content appears now
crane policy payments status          # every rule of a policy, simulated against the hooks
```

`crane selection protect` and `crane selection target` are the same commands as `crane protect` and
`crane target`. Every selection needs context: it is what the agent is told when it touches the
code.

## What protects it

- A proposed edit is applied in memory; if it would change protected lines or any marker, it is
  refused, even with approval.
- Shell commands that would modify a file holding protected code are refused; the after-check
  catches the rest and quarantines a bypass.
- If Crane cannot find a selection (markers removed, file rewritten), it is **unresolved** and agent
  changes to it are refused until a person repairs it. Repair never rebinds by itself: a person
  protects the code again at its new place.

## The policy language

```text
policy payments {
    preserve K7M2P9RX;
    target   4QZ8M2LD change_type logical_bn;
}
```

A policy passes only if every rule in it passes. Policies live in `.crane/policies/*.crane` and are
reviewed in pull requests like code.
