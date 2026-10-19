// The Capability Compiler (component 3): turns the evidence of one completed work episode into a
// capability candidate in the existing Capability Model schema, plus a separate compilation
// report. It is a thin, deterministic layer over the two earlier components and owns no store of
// evidence or capabilities:
//
//   intake       - Component 2's submit_intake (integrity, authorization, references, idempotency)
//   reconstruct  - actions, decisions, observations, interventions, outcomes, unresolved effects
//   extract      - candidate steps, inputs, outputs, side effects, recovery, branch notes; model
//                  proposals are accepted only after validation and stay labelled as hypotheses
//   context      - environments, prerequisites, exclusions, and versioned dependencies
//   verify       - claims mapped to evidence, and the gaps (missing, contradicted, untested)
//   assemble     - the definition, a dry run of Component 2's gates, and the hand-off as a candidate
//
// Every statement keeps its basis (observed, declared, derived, inferred, hypothesis, unknown).
// Event order is only chronology, never causation, and one episode never proves applicability
// beyond what it observed. The compiler cannot validate for the record, publish, execute, or
// change policy: it can only store a candidate through Component 2, as any contributor can.

pub(crate) mod assemble;
pub(crate) mod context;
pub(crate) mod demo;
pub(crate) mod extract;
pub(crate) mod reconstruct;
pub(crate) mod verify;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use crate::capability::intake::IntakeRequest;
use crate::capability::model::ProcedureStep;

/** Version of the compiler's rules, recorded in every report and candidate */
pub(crate) const COMPILER_VERSION: &str = "1";

/** Version of the compilation report layout */
pub(crate) const REPORT_SCHEMA_VERSION: u32 = 1;

/** How a statement in a reconstruction, candidate, or report is known
 * Variants
    - Observed - a source recorded it (an evidence record)
    - Declared - an authorized person or the request declared it (intent, outcome, intervention)
    - Derived - a deterministic rule computed it from identified inputs (an assertion)
    - Inferred - the compiler generalized it (a step kind from a tool, a precondition)
    - Hypothesis - an agent or model proposed it; unverified
    - Unknown - not established; recorded so it is not mistaken for absence
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Basis {
    Observed,
    Declared,
    Derived,
    Inferred,
    Hypothesis,
    Unknown,
}

/** The outcome of a compilation
 * Variants
    - CandidateGenerated - a candidate was stored and every required claim maps to evidence
    - NeedsMoreEvidence - a candidate was stored, but blocking gaps remain
    - InsufficientReusableSignal - the episode does not show a reusable, successful procedure
    - CompilationFailed - the request or the assembled candidate is invalid
    - CompilationBlocked - authorization, integrity, references, or policy prevent compilation
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompilationOutcome {
    CandidateGenerated,
    NeedsMoreEvidence,
    InsufficientReusableSignal,
    CompilationFailed,
    CompilationBlocked,
}

/** A step proposed by a model or agent; accepted only after validation
 * Fields
    - step: ProcedureStep - proposed step
    - rationale: String - why it is proposed
    - evidence_refs: Vec<String> - evidence it is based on (must belong to the pack)
    - producer: String - model or agent that proposed it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProposedStep {
    pub(crate) step: ProcedureStep,
    pub(crate) rationale: String,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) producer: String,
}

/** A compilation request
 * Fields
    - intake: IntakeRequest - the work episode, Evidence Pack and digest, and enterprise intent
      (handed to Component 2 unchanged)
    - capability_id: String - capability to create or revise
    - name: String - human-readable name
    - purpose: String - intended purpose of the capability
    - task_class: String - task class it is meant for
    - proposals: Vec<ProposedStep> - model-assisted step proposals, validated before acceptance
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompileRequest {
    pub(crate) intake: IntakeRequest,
    pub(crate) capability_id: String,
    pub(crate) name: String,
    pub(crate) purpose: String,
    pub(crate) task_class: String,
    #[serde(default)]
    pub(crate) proposals: Vec<ProposedStep>,
}
