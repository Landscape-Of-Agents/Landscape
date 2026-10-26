// Bricks: reusable, versioned modules of one fixed functional type (context, identity, scope,
// permission, constraint, precondition, approval, isolation, budget, action, verification,
// provenance, recovery, revocation, monitoring, intelligence) that are attached to Flows and Zones
// (and, later, Runbooks). A Brick is not an authority source: its restricting controls are folded
// into the guard's per-file limits (effect), which can only tighten, and its context is guidance
// delivered before the agent acts (context). Stacks compose exact Brick versions; they are never a
// new type. Activated versions, attachments with their per-target Current Context, and agent
// assignments live in the signed store (.crane/bricks.json); drafts grant nothing (store).
//
//   types    - the closed type taxonomy and what each type really does in this version
//   config   - typed per-type configuration and its validation
//   model    - definitions, versions, Stacks, attachments, assignments
//   store    - the signed store and the unsigned drafts
//   validate - the one validation of Bricks and Stacks
//   compose  - attachment resolution and monotonic composition of controls
//   effect   - folding controls into the guard's limits, and previews of that
//   context  - snapshot assembly, delivery ledger, and refresh tracking
//   runtime  - assignment status and the deterministic monitors
//   report   - the Brick part of the status a machine reports to the dashboard

pub(crate) mod compose;
pub(crate) mod config;
pub(crate) mod context;
pub(crate) mod effect;
pub(crate) mod model;
pub(crate) mod report;
pub(crate) mod runtime;
pub(crate) mod store;
pub(crate) mod types;
pub(crate) mod validate;

#[cfg(test)]
mod tests;
