# Security and compliance

Security teams often block agent rollout because agents are unknown processes with developer rights.
Crane makes them known: every action decided against signed rules, every effect checked, every
record tamper-evident.

## What you get

| Need | How | Status |
|---|---|---|
| Agents cannot change the rules or their own hooks | `.crane/`, the trust directory, and hook settings are denied to agents, even with approval | exists |
| Attacks on the controls are stopped and counted | bypass attempts quarantine the session; each denial carries a rule code | exists |
| Proof the boundary holds, on the real repository | `crane scope test`: traversal, symlink, case, marker, shell, and session probes | exists |
| Rules cannot loosen unnoticed | signed manifest, `crane scope diff`, `crane validate` | exists |
| Revocation that reaches every machine | the signed shared skill store | exists |
| Tamper-evident records | hash-chained events, ledgers, run and invocation records (`crane provenance verify`) | exists |
| Evidence per control (OWASP LLM, NIST AI RMF, SOC 2, ISO 27001 and 42001, EU AI Act) | compliance controls from evidence; signed exports | controls exist; exports to build |
| SIEM export | events to your SIEM | to build |

## Know exactly what is and is not enforced

```
crane isolation profiles          # what each execution path confines, and what it does not
crane isolation test --profile agent-hooks
crane scope inspect               # the boundary, hooks installed, "sandbox: none"
crane scope audit --days 30       # attempts, bypasses, coverage gaps, ungoverned changes
```

Repository scope is not a sandbox. Network traffic, processes, and files outside the repository are
not confined, and a shell running as the user can read the trust directory. These are reported as
gaps on every relevant answer. See [SECURITY.md](../../SECURITY.md).

## Controls that matter most

Isolation, provenance, revocation, and scope. See [features/controls.md](../features/controls.md).
