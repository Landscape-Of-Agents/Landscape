# Zones and flows

Selections govern exact lines. Zones and flows govern areas, in `.crane/governance.yaml`, signed with
`crane governance sign`.

| | What it covers | Example |
|---|---|---|
| **Zone** | Paths, files, folders, functions, selections, or blocks between `@crane:zone:ID:start/end` markers | `payments/` is critical; agents ask first |
| **Flow** | An entry point and every function it calls (found by a call graph), or an ordered list of zones | the refund flow, from the API handler to the ledger |
| **Context pack** | Guidance attached to zones and flows; every flow needs one | "Refund jobs queued before a deploy may lack an idempotency key." |

## Limits

| Setting | Values | Effect on agents |
|---|---|---|
| **Criticality** | routine, sensitive, critical, restricted | adds credit cost; restricted always needs a person |
| **Autonomy ceiling** | observe, assisted, delegated, autonomous | observe: no changes; assisted: every change needs approval |
| **Status** | proposed, active, deprecated | only active ones are enforced |

Where several cover the same code, the strictest ceiling and the highest criticality apply. Rules
only restrict, so a new zone can never loosen another.

## Example

```yaml
version: 1
zones:
  - id: payments
    selectors:
      directories: [payments]
    criticality: critical
    autonomy_ceiling: assisted
    context_packs: [money]
flows:
  - id: refund-flow
    entry_points: [refund_handler]
    autonomy_ceiling: assisted
    context_packs: [money]
context_packs:
  - id: money
    version: 1
    content: ["Amounts are integers in cents. Never round."]
```

Then `crane governance sign`. Editing the file without signing fails `crane validate` and stops agent
changes until an admin signs again.

## Watch for

- A selector that matches nothing (renamed or deleted path) covers nothing and grants nothing; it is
  reported as stale.
- Zones select by path, so renaming a governed file out of a zone loosens it. `crane scope diff`
  reports this and fails until someone signs or accepts it.
- `crane governance graph` shows zones, flows, and selections as a map.
