# Evidence (Work Capture)

Work Capture answers "what is actually known about this piece of work?" from the systems that know:
Git, CI, GitHub, the dashboard's records, Landscape, and Crane. It only reads, and it never fills a
gap with a guess.

## In plain words

1. **You ask a question**, such as "why did this deployment fail?" (a question class) or "what does
   this commit change?".
2. **A checklist says where to look**: a saved checklist such as "check CI, then the commits, then the
   logs". If no checklist exists, Crane explores, and that answer is never called complete.
3. **Sources answer** within their fixed scope and your access.
4. **You get an Evidence Pack**: the evidence, what it proves, and what is missing, sealed with a
   digest so it cannot be changed later.

## Every answer is one of three

| Answer | Meaning |
|---|---|
| **Proven** | evidence supports it |
| **Contradicted** | evidence shows it is false |
| **Unknown** | not established, always with a reason: empty (looked, nothing there), unavailable, denied, not queried, or could not be parsed |

## Commands

```
crane capture sources                     # the sources and their health
crane capture investigate commit SHA      # an Evidence Pack for a commit
crane capture ask question.json           # a question; it goes only to sources you may use
crane capture verify PACK_ID              # check a pack against its digest
crane capture replay QUERY_ID             # run it again; what changed
crane capture feedback PACK_ID sufficient yes|no|unknown
crane capture outcome EPISODE succeeded|failed|...
```

## Where it leads

Packs are what Evidence Receipts are made of, what skills cite as proof, and what Insight learns
from. Feedback that a pack was sufficient teaches the system which explorations deserve a checklist.

Deep dive: [architecture/work-capture.md](../architecture/work-capture.md).
