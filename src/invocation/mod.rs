// Capability Invocation & Outcome Engine (component 6): invokes one exact, published capability
// version through explicitly registered, typed operations, enforces execution-time constraints,
// verifies the actual outcome with independently obtained evidence, reconciles uncertain effects,
// and keeps an auditable, integrity-protected invocation record.
//
//   lifecycle  - invocation states and the validated transition table (separate from operation
//                status and external-effect status)
//   adapter    - the adapter registry: typed operation schemas, effect classes, dispatch,
//                reconciliation, health; deterministic fixture adapters, one read-only adapter
//                over the existing Git connector, and the narrow transaction-runtime interface
//   plan       - the versioned invocation plan built from the published definition and a reviewed
//                binding set (step -> registered operation); recorded tool calls are never run
//   preflight  - execution-time checks, all re-evaluated now (discovery is advisory only)
//   engine     - authorization, approval binding, per-operation enforcement, dispatch, retries,
//                reconciliation, verification, outcome hand-off, and events
//   ledger     - versioned, locked, atomically written invocation ledger with idempotency
//
// It never changes a capability definition, lifecycle state, validation, publication, or policy,
// never runs arbitrary commands, and fails closed whenever it cannot establish that an operation is
// permitted and safe. The only registry write is Component 2's supported invocation hand-off
// (record_invocation), which by design never alters a definition or its lifecycle.

pub(crate) mod adapter;
pub(crate) mod demo;
pub(crate) mod engine;
pub(crate) mod isolation;
pub(crate) mod ledger;
pub(crate) mod lifecycle;
pub(crate) mod permission;
pub(crate) mod plan;
pub(crate) mod preflight;

#[cfg(test)]
mod tests;

/** Version of the invocation engine's rules */
pub(crate) const ENGINE_VERSION: &str = "1";

/** Version of the invocation record layout */
pub(crate) const RECORD_SCHEMA_VERSION: u32 = 1;

/** Version of the invocation plan layout */
pub(crate) const PLAN_VERSION: u32 = 1;
