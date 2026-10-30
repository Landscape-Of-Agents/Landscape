// Flows as a knowledge base. A flow is declared in .crane/governance.yaml (its entry points, the
// zones it runs through, criticality, autonomy ceiling); this module holds what the repository
// knows about it besides that, signed in .crane/flows.json, and what agents may start around it.
//
//   model   - the signed flow knowledge: traces, typed context, priority and model routing, people,
//             tests, watcher limits, the organization's models
//   trace   - deterministic tracing from a starting point into drafts with revisions (retrace,
//             rewind) that a person approves
//   store   - signed changes of the knowledge, and approving a draft into governance.yaml
//   context - what agents receive about a flow, and the model-routing rule (tighten only)
//   watch   - read-only watchers (monitoring subagents), incidents, and agents' proposals
//   subagent - the subtypes (Monitor, Incident), actions, configuration, bounds, approval, and
//             termination of a flow's subagents
//   scenarios - test scenarios written from a flow's knowledge for a person to review
//   agent   - the agent channel: 'crane-request …', answered by the hook, never executed
//   report  - the flow part of the status a machine reports to the dashboard

pub(crate) mod agent;
pub(crate) mod context;
pub(crate) mod model;
pub(crate) mod report;
pub(crate) mod scenarios;
pub(crate) mod store;
pub(crate) mod subagent;
pub(crate) mod trace;
pub(crate) mod watch;

#[cfg(test)]
mod tests;
