// Resident agents: an agent assignment that stays responsible for one Flow, Zone, or Runbook for
// days or weeks while using no model and no process when idle. The persistent object is the
// assignment record, never a live model session: an activation starts only when a relevant event or
// timer arrives, runs as a bounded, persisted record, and the assignment returns to idle.
//
// Authority is never in this module. A resident can only act inside an active, signed Brick
// assignment (.crane/bricks.json, 'crane brick assign') covering its target; the signed state, that
// assignment, the target's Zone or Flow limits, and the Brick controls are re-read before every
// effectful step (authority). Definitions, RCA graphs, context, and agent output are guidance and
// cannot widen anything.
//
//   model     - definitions, assignments, lifecycle, limits, subscriptions
//   trigger   - the canonical trigger envelope, normalization, matching, and the real sources
//   rca       - versioned RCA graphs, deterministic checks, and the investigation (reasoner)
//   plan      - the closed action-step registry and typed, digest-bound plans
//   authority - the Crane view an action is decided against, re-read before each step
//   context   - versioned context packs and their dependency-keyed cache
//   registry  - lifecycle changes of definitions, assignments, and RCA graphs
//   store     - the sealed runtime store (runtime/resident/store.json)
//   worker    - activation lifecycle, adapters, human waits, the tick the background sync runs
//   report    - the resident part of the status a machine reports to the dashboard

pub(crate) mod authority;
pub(crate) mod context;
pub(crate) mod model;
pub(crate) mod plan;
pub(crate) mod rca;
pub(crate) mod registry;
pub(crate) mod report;
pub(crate) mod store;
pub(crate) mod trigger;
pub(crate) mod worker;

#[cfg(test)]
mod tests;

/** Version of the resident schema (definitions, assignments, activations, triggers) */
pub(crate) const SCHEMA_VERSION: u32 = 1;

/** A serialized enum value as its word ("active", "NOTIFY_SLACK"), empty for anything else
 * Input
    - value: &impl serde::Serialize - value
 * Output
    - String
*/
pub(crate) fn word(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}
