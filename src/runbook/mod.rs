// Runbooks: versioned workflows that compose exact skill versions into steps with dependencies,
// simple branches, bounded retries, approval points, a failure policy, completion requirements, and
// an operation budget. A runbook never adds authority: every step is an invocation through the
// invocation engine, authorized and verified on its own.
//
//   model - the schema, ordering, and static validation
//   store - runbook versions and their lifecycle, and run records (runtime/runbook/store.json)
//   run   - the orchestrator: starting, advancing, approvals, cancelling, and resuming runs

pub(crate) mod model;
pub(crate) mod run;
pub(crate) mod store;

#[cfg(test)]
mod tests;

/** Version of the runbook schema */
pub(crate) const RUNBOOK_SCHEMA_VERSION: u32 = 1;
