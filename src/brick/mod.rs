// Bricks: reusable, versioned modules of one fixed functional kind (knowledge, identity, access,
// approval, limits, environment, precondition, verification, action, recovery, provenance, watch)
// that are attached to Flows and Zones (and, later, Runbooks). Each kind answers one question no
// other kind answers. A Brick is not an authority source: its restricting controls are folded
// into the guard's per-file limits (effect) and decided on every tool call (guard), both of which
// can only tighten, and its context is guidance delivered before the agent acts (context). Stacks
// compose exact Brick versions; they are never a new kind. Activated versions, attachments with
// their per-target Current Context, and agent assignments live in the signed store
// (.crane/bricks.json); drafts grant nothing (store). Former kinds stay readable (legacy).
//
//   types    - the closed type taxonomy and what each type really does in this version
//   config   - typed per-type configuration and its validation
//   legacy   - the former kinds, read as the current ones and rewritten by 'crane brick migrate'
//   model    - definitions, versions, Stacks, attachments, assignments
//   store    - the signed store and the unsigned drafts
//   validate - the one validation of Bricks and Stacks
//   compose  - attachment resolution and monotonic composition of controls
//   effect   - folding controls into the guard's limits, and previews of that
//   guard    - the rules decided on every tool call (identity, access, approval, limits, ...)
//   usage    - what each agent session did per target (caps and failures in a row)
//   context  - snapshot assembly, delivery ledger, and refresh tracking
//   runtime  - checks, assignment status, and the deterministic monitors (watch)
//   report   - the Brick part of the status a machine reports to the dashboard

pub(crate) mod compose;
pub(crate) mod config;
pub(crate) mod context;
pub(crate) mod effect;
pub(crate) mod guard;
pub(crate) mod legacy;
pub(crate) mod model;
pub(crate) mod report;
pub(crate) mod runtime;
pub(crate) mod store;
pub(crate) mod types;
pub(crate) mod usage;
pub(crate) mod validate;

#[cfg(test)]
mod tests;
