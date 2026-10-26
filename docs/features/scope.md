# Scope: the effective boundary

`crane scope` shows what agents may do, decided by the same guard the hooks use, so the command line
and the hooks never disagree. Every scope command only reads.

## Commands

| Command | What it answers |
|---|---|
| `crane scope inspect [PATH ...]` | Do the rules verify? Which hooks are installed? What is always denied? Every governed file with its limits, and what is unresolved |
| `crane scope inspect --skill SKILL` | The zones and flows a skill depends on, resolved to files (an unresolved one is never treated as allowed) |
| `crane scope resolve write app/pay.py lines 2-5` | Would this be allowed? The target resolved (through `..`, symlinks, other worktrees), the rules, and what Crane cannot enforce for it. The answer is recorded |
| `crane scope resolve --resource repo:app/pay.py` | One canonical identity for a resource reference |
| `crane scope explain scd_...` or `TOOL_CALL_ID` | Why, with each rule's domain, and whether it still applies (a changed file or re-signed rules make it stale) |
| `crane scope explain --operation git/git.commit --resource repo:app/pay.py` | Whether one operation may act on one resource, strictest rule first |
| `crane scope diff [--record]` | The boundary against the last signed baseline; fails when it widened (for example a protected file renamed out of its zone) |
| `crane scope diff --skill SKILL@2 --against SKILL@1` | The scope a new skill version asks for that the old one did not; an expansion needs review |
| `crane scope test [--skill SKILL]` | Adversarial probes: traversal, absolute paths, letter case, symlinks, marker removal, shell changes, metadata, zones, session states |
| `crane scope audit [--days N] [--skill S] [--run INV]` | The boundary against what agents attempted and what changed |

All take `--json` or `--format json`.

## Reading the answers

- **PASS / FAIL** in `scope test`: the rules hold, or they do not.
- **GAP**: a limit of repository rules, reproduced and reported (a file named through a shell
  variable, the network, files outside the repository). Never counted as enforced.
- **Widened** in `scope diff`: agents may do more than when the rules were signed.

Repository scope is not a sandbox: shells, processes, credentials, the network, and files outside the
repository are not confined, and every relevant answer says so.
