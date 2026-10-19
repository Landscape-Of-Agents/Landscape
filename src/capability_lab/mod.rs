// The Capability Validation Lab (component 4): deterministic, non-executing checks of one exact
// capability candidate version against its compilation report, its cited evidence, an approved
// test plan, and the current governance versions, producing a versioned, integrity-protected lab
// report. It is organized as:
//
//   preflight    - the exact version and digest, its state, the matching compilation report,
//                  and the report's blocking gaps (carried, never bypassed)
//   conformance  - every compiled step re-derived from its cited observed record; every claim's
//                  basis (observed, declared, derived, unknown) checked against its sources
//   scenarios    - an approved, typed test plan: applicability, stale-dependency, missing-evidence,
//                  and negative-case scenarios, evaluated against the contracts (never executed)
//   invariants   - claims recomputed from evidence (Component 2's rules), business invariants,
//                  contradictions, dependency staleness, and the single-episode generalization
//   report       - the lab report, its limitations, and its digest
//   store        - append-only report storage and the report-reference interface
//
// Nothing a candidate describes is executed and no external side effect happens: checks read
// Work Capture records, the registry, and the current Landscape and Crane versions. Component 2
// keeps validation, lifecycle, and publication authority: a lab report changes no lifecycle state
// and no eligibility; it is a referenceable record (Component 2 has no interface that consumes
// it, and this component does not add one to Component 2). Passing checks prove only the
// conditions they tested.

pub(crate) mod conformance;
pub(crate) mod demo;
pub(crate) mod invariants;
pub(crate) mod preflight;
pub(crate) mod report;
pub(crate) mod scenarios;
pub(crate) mod store;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compiler::Basis;

/** Version of the lab's rules */
pub(crate) const LAB_VERSION: &str = "1";

/** Version of the lab report layout */
pub(crate) const LAB_REPORT_SCHEMA_VERSION: u32 = 1;

/** The status of one check
 * Variants
    - Passed - the tested condition holds
    - Failed - the tested condition does not hold (a counterexample is attached when there is one)
    - NotRun - the check does not apply or was not requested
    - Blocked - a prerequisite is missing (evidence, access, a blocking gap)
    - Inconclusive - the check ran but cannot establish the condition
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckStatus {
    Passed,
    Failed,
    NotRun,
    Blocked,
    Inconclusive,
}

/** Which part of the lab a check belongs to
 * Variants
    - Preflight - candidate identity, state, and compilation report
    - Conformance - steps and claims against their sources
    - Scenario - test-plan scenarios
    - Invariant - claims, invariants, contradictions, dependencies
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckCategory {
    Preflight,
    Conformance,
    Scenario,
    Invariant,
}

/** One check result
 * Fields
    - check_id: String - stable identifier ("conformance:s01-change")
    - category: CheckCategory - category
    - status: CheckStatus - status
    - detail: String - what was checked and what was found
    - basis: Option<Basis> - the subject's recorded basis, kept as compiled
    - evidence_refs: Vec<String> - evidence, assertions, or reports it relied on
    - counterexample: Option<Value> - what contradicts the condition, when it failed
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LabCheck {
    pub(crate) check_id: String,
    pub(crate) category: CheckCategory,
    pub(crate) status: CheckStatus,
    pub(crate) detail: String,
    pub(crate) basis: Option<Basis>,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) counterexample: Option<Value>,
}

impl LabCheck {
    /** Build a check
     * Input
        - check_id: String - identifier
        - category: CheckCategory - category
        - status: CheckStatus - status
        - detail: impl Into<String> - explanation
     * Output
        - LabCheck
    */
    pub(crate) fn new(
        check_id: String,
        category: CheckCategory,
        status: CheckStatus,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            check_id,
            category,
            status,
            detail: detail.into(),
            basis: None,
            evidence_refs: Vec::new(),
            counterexample: None,
        }
    }

    /** Attach evidence references
     * Input
        - refs: Vec<String> - references
     * Output
        - LabCheck
    */
    pub(crate) fn with_refs(mut self, refs: Vec<String>) -> Self {
        self.evidence_refs = refs;
        self
    }

    /** Attach the subject's basis
     * Input
        - basis: Basis - basis
     * Output
        - LabCheck
    */
    pub(crate) fn with_basis(mut self, basis: Basis) -> Self {
        self.basis = Some(basis);
        self
    }

    /** Attach a counterexample
     * Input
        - counterexample: Value - counterexample
     * Output
        - LabCheck
    */
    pub(crate) fn with_counterexample(mut self, counterexample: Value) -> Self {
        self.counterexample = Some(counterexample);
        self
    }
}
