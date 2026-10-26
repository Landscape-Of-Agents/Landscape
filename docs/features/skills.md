# Skills

A skill is a reusable, versioned procedure for one kind of task, for any department: what it does,
who owns it, its inputs, the steps, which steps change something, what needs approval, how success
is proven, and what to do when it fails. In the code a skill is a **capability**.

A skill never grants itself anything. It asks for operations; Crane decides.

## Life of a skill

```
crane skill create --from-template read-only-lookup --name "Look up a commit" --department engineering
crane skill inspect look-up-a-commit                 # plain-language summary and its contract
crane skill validate look-up-a-commit@1              # the validation gates (never activates)
crane skill test look-up-a-commit@1 --suite plan.json    # the Validation Lab; nothing is executed
crane skill activate look-up-a-commit@1 --review-ref PR-12   # by a person other than the author
crane skill plan look-up-a-commit@1 --bindings b.json --input in.json          # no effects
crane skill invoke look-up-a-commit@1 --bindings b.json --input in.json --idempotency-key K1
crane skill retire look-up-a-commit@1 --reason "replaced by v2"
crane skill revoke look-up-a-commit@1 --reason "unsafe"
```

| Stage | What happens | Who |
|---|---|---|
| **Draft** | Created from a template; grants nothing; cannot run | anyone |
| **Validate** | Structure, sources, where it applies, proof, rules | anyone (records the result) |
| **Test** | Lab checks of the exact version; unproven is "inconclusive", never "passed" | anyone |
| **Activate** | The exact version is published and signed into `.crane/skills.json` | a person other than the author, holding an admin key |
| **Invoke** | Each run re-checks everything first (preflight), may need approval, and succeeds only on proven claims | anyone; approval by another person |
| **Retire / revoke** | No new use, or withdrawn everywhere | an admin |

Templates: `read-only-lookup`, `repository-change`, `business-record-update`.

## What a skill can do

Only **registered operations**: typed actions such as `git/git.commit`, each with checked parameters.
A skill never runs a shell command, a script, or model output. Each step is mapped to an operation by
a reviewed **binding set**. Today only read-only Git operations are registered in the CLI, so skills
that change code or business systems are blocked until their adapters exist, and Crane says so
(`crane permission inspect`, `crane isolation validate`).

## Running safely

- **Approval** binds to the exact plan (`inv_…@PLAN_DIGEST`); a changed plan needs a new approval,
  and the requester never approves their own.
- **Uncertain effects** are reconciled against the real system, never retried blindly.
- **Success** means every required claim is proven by evidence.

## Sharing and withdrawing

Activation, retirement, and revocation are signed into `.crane/skills.json` with the rules. Commit
`.crane`, and every machine that pulls it blocks a withdrawn version. A hand-edited store fails
`crane validate` and blocks every invocation. `crane capability shared` lists it.

## The Skill Builder

Decided, to build: on the dashboard, a plain form asks what the skill should do, which team owns it,
its inputs, which systems it touches, what it may change, what needs approval, how success is
checked, and what to do if it fails. Suggestions are proposals only; trusted code parses and
validates the result, and a signer activates it.

## Checks for one skill

`crane permission inspect|test`, `crane precondition evaluate|test`, `crane constraint test`,
`crane verification evaluate|test`, `crane isolation validate`, `crane scope inspect --skill`. See
[controls.md](controls.md).
