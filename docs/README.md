# Landscape documentation

Start with the part that matches what you need. Every term is explained in one place:
[TERMINOLOGY.md](TERMINOLOGY.md).

## By department: what it does for your team

| Department | Read |
|---|---|
| Engineering | [departments/engineering.md](departments/engineering.md) |
| Product | [departments/product.md](departments/product.md) |
| Sales and customer success | [departments/sales.md](departments/sales.md) |
| People and HR | [departments/hr.md](departments/hr.md) |
| Operations and SRE | [departments/operations.md](departments/operations.md) |
| Finance | [departments/finance.md](departments/finance.md) |
| Security and compliance | [departments/security-compliance.md](departments/security-compliance.md) |
| Leadership | [departments/leadership.md](departments/leadership.md) |

## By product

| Product | What it is | Read |
|---|---|---|
| **Crane** (free) | Signed rules for every AI agent, checked before each action and verified after | [products/crane.md](products/crane.md) |
| **Black Box** (paid) | The flight recorder: every attempt, near miss, and what stopped it | [products/black-box.md](products/black-box.md) |
| **Evidence Receipts** (paid) | Proof for every change: proven, contradicted, unknown and why | [products/evidence-receipts.md](products/evidence-receipts.md) |
| Strategy | Why these three products, the market, pricing, and what we will not build | [products/strategy.md](products/strategy.md) |

## By feature

| Feature | Read |
|---|---|
| Selections: protect and target code | [features/selections.md](features/selections.md) |
| Zones and flows | [features/zones-and-flows.md](features/zones-and-flows.md) |
| Flows as a knowledge base: traced call graphs, context, priority and models, people, tests, watchers | [features/flow-knowledge.md](features/flow-knowledge.md) |
| Agent hooks: deciding every action | [features/agent-hooks.md](features/agent-hooks.md) |
| Scope: the effective boundary | [features/scope.md](features/scope.md) |
| Skills | [features/skills.md](features/skills.md) |
| Runbooks | [features/runbooks.md](features/runbooks.md) |
| Bricks: reusable modules of a fixed type, attached to flows and zones | [features/bricks.md](features/bricks.md) |
| Resident agents: assignments that stay idle (no model) until a trigger wakes them | [features/resident-agents.md](features/resident-agents.md) |
| The eleven controls (permission, approval, budget, ...) | [features/controls.md](features/controls.md) |
| Evidence (Work Capture) | [features/evidence.md](features/evidence.md) |
| Insight: learning from work | [features/insight.md](features/insight.md) |

## By surface

| Surface | Read |
|---|---|
| The `crane` command line (full reference) | [surfaces/cli.md](surfaces/cli.md) |
| The dashboard | [surfaces/dashboard.md](surfaces/dashboard.md) |
| Landscape for VS Code | [surfaces/vscode.md](surfaces/vscode.md) |

## Architecture (for people building Crane)

| Document | What it covers |
|---|---|
| [architecture/overview.md](architecture/overview.md) | The whole system, how each part works inside, and its limits |
| [architecture/intelligence.md](architecture/intelligence.md) | Where the product is going: intelligence for every role, human-in-the-loop design |
| [architecture/flows.md](architecture/flows.md) | Every control flow and pipeline |
| [architecture/work-capture.md](architecture/work-capture.md) | Work Capture explained line by line |
| [architecture/work-capture-fixes.md](architecture/work-capture-fixes.md) | Work Capture's fixed issues |
| [architecture/operations-audit.md](architecture/operations-audit.md) | The audit behind the command families, skills, and runbooks: decisions, findings, status |
| [../DASHBOARD.md](../DASHBOARD.md) | The event specification every record and dashboard chart follows |
| [../SECURITY.md](../SECURITY.md) | The security model and how to report a vulnerability |

## Status words used throughout

- **Exists**: built and tested.
- **Partial**: built for some cases; the rest is named.
- **To build**: planned, not built. Nothing marked "to build" works today.
