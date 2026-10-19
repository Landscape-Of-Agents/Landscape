// Capability Evolution & Regression Engine (component 7). It closes the capability lifecycle:
//
//   capture -> define -> compile -> validate -> discover -> invoke -> observe outcomes
//     -> detect evolution signals -> propose a revision -> regression checks -> review packet
//     -> publication through Component 2's existing governance (never by this engine)
//
//   outcomes    - invocation records (Component 6's ledger, read only) normalized into
//                 observations: verified outcomes apart from declarations and unknowns, retries
//                 and resubmissions collapsed, DataSea events counted but never as trials
//   patterns    - deterministic detection and qualification of signals (configurable thresholds)
//   drift       - pinned Zone, Flow, policy, capability, and Evidence Pack versions against the
//                 current ones; unknown stays unknown
//   proposals   - minimal, evidence-linked change sets, gated, applied to the parent definition and
//                 stored as a new candidate version through Component 2's propose_version
//   regression  - the exact candidate against its exact parent: the Lab's supported scenarios on
//                 both, structural, claim, coverage, and governance comparisons; behavior the Lab
//                 cannot execute stays inconclusive
//   reports     - the evolution store (signals, analyses, proposals, plans, runs), integrity
//                 checked, separate from the registry and the invocation ledger
//   workflow    - the operations: analyze, drift, propose, regression plan and run, review packet,
//                 abandon, dismiss
//
// The engine may read, analyze, report, prepare a candidate through the registry, and run
// deterministic checks. It never publishes, validates, approves, signs, deprecates, or revokes,
// never edits policy or governance, and never executes what a candidate describes.

pub(crate) mod demo;
pub(crate) mod drift;
pub(crate) mod models;
pub(crate) mod outcomes;
pub(crate) mod patterns;
pub(crate) mod proposals;
pub(crate) mod regression;
pub(crate) mod reports;
pub(crate) mod workflow;

#[cfg(test)]
mod tests;

/** Version of the evolution record layouts */
pub(crate) const EVOLUTION_SCHEMA_VERSION: u32 = 1;

/** Version of the deterministic detectors and qualification rules */
pub(crate) const DETECTOR_VERSION: &str = "1";

/** Version of the regression planner and aggregation rules */
pub(crate) const REGRESSION_VERSION: &str = "1";
