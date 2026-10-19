// Procedure extraction: candidate steps from what the episode observably did, in time order.
// Each step comes from one observed record (a tool call, a commit, a CI run, a test report, a
// deployment, a check); its kind is inferred from that record by fixed rules (an edit is a change,
// a test command is a test, an unrecognized command is conservatively a change). Inputs and
// outputs chain the steps as a sequence; side effects, state transitions, and recovery are stated
// only as far as the evidence shows ("unknown" otherwise). Branch notes are recorded in the report,
// labelled with their basis, because the schema's procedure has no branches and none were proven.
// Steps proposed by a model are accepted only when they cite evidence of the pack the requester can
// read, do not touch policy, and fit the capability's risk; accepted ones stay hypotheses.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::reconstruct::{ItemKind, Reconstruction};
use super::verify::VerificationPlan;
use super::{Basis, ProposedStep};
use crate::capability::intake::CapabilityIntake;
use crate::capability::model::{
    ParamKind, Parameter, ProcedureStep, RiskLevel, SideEffect, StateTransition, StepKind,
};
use crate::capability::refs::{RefOutcome, Sources};
use crate::capability::validate::touches_policy;
use crate::capture::format::record::{EvidenceRecord, EvidenceType};
use crate::capture::resources::registry::valid_identifier;

/** The support of one candidate step
 * Fields
    - step_id: String - step
    - basis: Basis - observed (from a record) or hypothesis (a validated proposal)
    - kind_basis: Basis - how its kind is known (inferred from the record by rule)
    - evidence_refs: Vec<String> - records it comes from
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StepBasis {
    pub(crate) step_id: String,
    pub(crate) basis: Basis,
    pub(crate) kind_basis: Basis,
    pub(crate) evidence_refs: Vec<String>,
}

/** A branch condition noted for review (not part of the procedure)
 * Fields
    - condition: String - the condition
    - basis: Basis - how it is known
    - source: String - where it comes from
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BranchNote {
    pub(crate) condition: String,
    pub(crate) basis: Basis,
    pub(crate) source: String,
}

/** The decision on a model-proposed step
 * Fields
    - step_id: String - proposed step
    - producer: String - who proposed it
    - accepted: bool - whether it was included (as a hypothesis)
    - reasons: Vec<String> - why it was rejected, empty when accepted
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProposalDecision {
    pub(crate) step_id: String,
    pub(crate) producer: String,
    pub(crate) accepted: bool,
    pub(crate) reasons: Vec<String>,
}

/** The extracted procedure
 * Fields
    - inputs: Vec<Parameter> - procedure inputs
    - steps: Vec<ProcedureStep> - steps in order
    - bases: Vec<StepBasis> - support of each step
    - effects: Vec<String> - expected effects
    - state_transitions: Vec<StateTransition> - expected state transitions
    - side_effects: Vec<SideEffect> - external side effects
    - recovery: Vec<String> - recovery notes
    - branches: Vec<BranchNote> - branch notes for review
    - proposals: Vec<ProposalDecision> - decisions on proposed steps
    - mutating: bool - whether any step changes state
    - policy_touches: Vec<String> - observed actions that touched Crane policy or governance
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Extraction {
    pub(crate) inputs: Vec<Parameter>,
    pub(crate) steps: Vec<ProcedureStep>,
    pub(crate) bases: Vec<StepBasis>,
    pub(crate) effects: Vec<String>,
    pub(crate) state_transitions: Vec<StateTransition>,
    pub(crate) side_effects: Vec<SideEffect>,
    pub(crate) recovery: Vec<String>,
    pub(crate) branches: Vec<BranchNote>,
    pub(crate) proposals: Vec<ProposalDecision>,
    pub(crate) mutating: bool,
    pub(crate) policy_touches: Vec<String>,
}

/** Build a parameter
 * Input
    - name: &str - name
    - kind: ParamKind - kind
    - description: &str - meaning
 * Output
    - Parameter
*/
fn parameter(name: &str, kind: ParamKind, description: &str) -> Parameter {
    Parameter {
        name: name.into(),
        kind,
        required: true,
        description: description.into(),
    }
}

/** Report whether the step rule recognized a record's action; false when a tool call's kind fell
 * back to the conservative default (an unrecognized tool or command is assumed to be a change)
 * Input
    - record: &EvidenceRecord - record
 * Output
    - bool
*/
pub(crate) fn recognized(record: &EvidenceRecord) -> bool {
    if record.evidence_type != EvidenceType::ToolCall {
        return true;
    }
    let tool = record.field("tool").unwrap_or_default();
    let target = record
        .field("command")
        .or_else(|| record.field("resource"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    match tool.as_str() {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Read" | "Grep" | "Glob" | "LS"
        | "WebFetch" => true,
        "Bash" | "Shell" => ["test", "pytest", "cargo test", "npm test", "go test"]
            .iter()
            .any(|word| target.contains(word)),
        _ => false,
    }
}

/** The step a record represents, if any: kind and description
 * Input
    - record: &EvidenceRecord - record
 * Output
    - Option<(StepKind, String)>
*/
pub(crate) fn step_of(record: &EvidenceRecord) -> Option<(StepKind, String)> {
    let field = |name: &str| record.field(name);
    match record.evidence_type {
        EvidenceType::ToolCall => {
            let tool = field("tool").unwrap_or_default();
            let target = field("command")
                .or_else(|| field("resource"))
                .unwrap_or_default();
            let lower = target.to_ascii_lowercase();
            let kind = match tool.as_str() {
                "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => StepKind::Change,
                "Read" | "Grep" | "Glob" | "LS" | "WebFetch" => StepKind::Diagnostic,
                "Bash" | "Shell"
                    if ["test", "pytest", "cargo test", "npm test", "go test"]
                        .iter()
                        .any(|word| lower.contains(word)) =>
                {
                    StepKind::Test
                }
                _ => StepKind::Change,
            };
            let verb = match kind {
                StepKind::Change => "Change",
                StepKind::Test => "Run tests with",
                _ => "Inspect",
            };
            Some((
                kind,
                format!("{verb} {}", if target.is_empty() { tool } else { target }),
            ))
        }
        EvidenceType::CodeChange => Some((StepKind::Change, "Commit the change".into())),
        EvidenceType::PullRequest if field("review_state").as_deref() == Some("approved") => {
            Some((
                StepKind::Approval,
                "Obtain review approval of the pull request".into(),
            ))
        }
        EvidenceType::CiRun => Some((
            StepKind::Test,
            format!(
                "Run CI ({})",
                field("name").unwrap_or_else(|| "checks".into())
            ),
        )),
        EvidenceType::TestReport => Some((
            StepKind::Test,
            format!(
                "Run the {} test suite",
                field("suite").unwrap_or_else(|| "project".into())
            ),
        )),
        EvidenceType::Deployment
            if field("status").is_some_and(|status| status.contains("rolled_back")) =>
        {
            Some((StepKind::Rollback, "Roll back the deployment".into()))
        }
        EvidenceType::Deployment => Some((
            StepKind::Deploy,
            format!(
                "Deploy the change to {}",
                record
                    .scope
                    .environment
                    .clone()
                    .unwrap_or_else(|| "the target environment".into())
            ),
        )),
        EvidenceType::Verification => Some((
            StepKind::Verify,
            format!(
                "Run the {} check",
                field("check").unwrap_or_else(|| "post-change".into())
            ),
        )),
        _ => None,
    }
}

/** The claims a step verifies
 * Input
    - kind: StepKind - step kind
    - record: &EvidenceRecord - its record
    - plan: &VerificationPlan - claims
 * Output
    - Vec<String> claim ids
*/
fn verifies(kind: StepKind, record: &EvidenceRecord, plan: &VerificationPlan) -> Vec<String> {
    let has = |id: &str| plan.claims.iter().any(|claim| claim.claim_id == id);
    let wanted: Vec<String> = match (kind, record.evidence_type) {
        (StepKind::Test, EvidenceType::CiRun) => vec!["ci-passed".into()],
        (StepKind::Test, _) => vec!["tests-passed".into()],
        (StepKind::Deploy, _) => vec!["deployment-succeeded".into()],
        (StepKind::Verify, _) => {
            let invariants = plan
                .claims
                .iter()
                .filter(|claim| {
                    claim.evidence_refs.contains(&record.evidence_id)
                        && claim.invariant_ref.is_some()
                })
                .map(|claim| claim.claim_id.clone())
                .collect::<Vec<_>>();
            if invariants.is_empty() {
                vec!["deployment-succeeded".into()]
            } else {
                invariants
            }
        }
        _ => Vec::new(),
    };
    wanted.into_iter().filter(|id| has(id)).collect()
}

/** Extract the procedure
 * Input
    - reconstruction: &Reconstruction - reconstructed episode
    - intake: &CapabilityIntake - intake
    - sources: &Sources - canonical sources and the requester's access
    - plan: &VerificationPlan - claims the steps verify
    - proposals: &[ProposedStep] - model proposals
    - risk: RiskLevel - capability risk
 * Output
    - Extraction
*/
pub(crate) fn extract(
    reconstruction: &Reconstruction,
    intake: &CapabilityIntake,
    sources: &Sources,
    plan: &VerificationPlan,
    proposals: &[ProposedStep],
    risk: RiskLevel,
) -> Extraction {
    let mut extraction = Extraction::default();
    if intake.work.task_id.is_some() {
        extraction.inputs.push(parameter(
            "task_id",
            ParamKind::Identifier,
            "Task the procedure is performed for",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut records = Vec::new();
    for item in reconstruction.items.iter().filter(|item| {
        item.basis == Basis::Observed
            && matches!(item.kind, ItemKind::Action | ItemKind::Observation)
    }) {
        let Some(id) = item.evidence_refs.first() else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        if let (_, Some(record)) = sources.evidence(id) {
            records.push(record);
        }
    }
    if records
        .iter()
        .any(|record| record.evidence_type == EvidenceType::Deployment)
    {
        extraction.inputs.push(parameter(
            "environment",
            ParamKind::String,
            "Environment to deploy to",
        ));
    }
    let mut previous: Option<String> = None;
    let mut described = BTreeSet::new();
    for record in records {
        if record.evidence_type == EvidenceType::ToolCall {
            let target = record
                .field("command")
                .or_else(|| record.field("resource"))
                .unwrap_or_default();
            if touches_policy(&target) {
                extraction
                    .policy_touches
                    .push(format!("{} ({})", target, record.evidence_id));
            }
        }
        let Some((kind, description)) = step_of(record) else {
            continue;
        };
        if !described.insert((format!("{kind:?}"), description.clone())) {
            if let Some(basis) = extraction.bases.iter_mut().find(|basis| {
                extraction
                    .steps
                    .iter()
                    .any(|step| step.step_id == basis.step_id && step.description == description)
            }) {
                basis.evidence_refs.push(record.evidence_id.clone());
            }
            continue;
        }
        let step_id = format!(
            "s{:02}-{}",
            extraction.steps.len() + 1,
            format!("{kind:?}").to_ascii_lowercase()
        );
        let inputs = match &previous {
            Some(output) => vec![output.clone()],
            None => extraction
                .inputs
                .iter()
                .map(|input| input.name.clone())
                .collect(),
        };
        let output = format!("{step_id}-result");
        let preconditions = match (extraction.steps.is_empty(), kind) {
            (true, _) => vec!["repository-access".to_string()],
            (false, StepKind::Deploy) => vec!["deployment-target-known".to_string()],
            _ => Vec::new(),
        };
        extraction.mutating |= kind.mutating();
        match kind {
            StepKind::Change
                if !extraction
                    .side_effects
                    .iter()
                    .any(|effect| effect.system == "repository") =>
            {
                extraction.effects.push("Repository files change".into());
                extraction.side_effects.push(SideEffect {
                    system: "repository".into(),
                    description: "Changes repository files".into(),
                    reversible: None,
                    risk,
                });
            }
            StepKind::Deploy => {
                let environment = record
                    .scope
                    .environment
                    .clone()
                    .unwrap_or_else(|| "unknown".into());
                extraction
                    .effects
                    .push(format!("A new release runs in {environment}"));
                extraction.state_transitions.push(StateTransition {
                    entity: format!("environment:{environment}"),
                    from: "previous release (unknown)".into(),
                    to: format!(
                        "release of {}",
                        record
                            .refs
                            .commit
                            .clone()
                            .unwrap_or_else(|| "the change".into())
                    ),
                });
                let reversible = (!intake.observed.recovery.is_empty()).then_some(true);
                extraction.side_effects.push(SideEffect {
                    system: format!("environment:{environment}"),
                    description: "Deploys a new release".into(),
                    reversible,
                    risk,
                });
            }
            _ => {}
        }
        extraction.steps.push(ProcedureStep {
            step_id: step_id.clone(),
            kind,
            description,
            inputs,
            outputs: vec![output.clone()],
            preconditions,
            verifies: verifies(kind, record, plan),
        });
        extraction.bases.push(StepBasis {
            step_id,
            basis: Basis::Observed,
            kind_basis: Basis::Inferred,
            evidence_refs: vec![record.evidence_id.clone()],
        });
        previous = Some(output);
    }
    let readable = intake
        .validation
        .references
        .iter()
        .filter(|check| check.kind == "evidence" && check.outcome == RefOutcome::Resolved)
        .map(|check| check.reference.clone())
        .collect::<BTreeSet<_>>();
    for proposal in proposals {
        let step = &proposal.step;
        let mut reasons = Vec::new();
        if !valid_identifier(&step.step_id)
            || extraction
                .steps
                .iter()
                .any(|existing| existing.step_id == step.step_id)
        {
            reasons.push("the step id is invalid or already used".to_string());
        }
        if step.description.trim().is_empty() || touches_policy(&step.description) {
            reasons.push("the description is empty or changes Crane policy or governance".into());
        }
        if proposal.evidence_refs.is_empty() {
            reasons.push("it cites no evidence".into());
        }
        let foreign = proposal
            .evidence_refs
            .iter()
            .filter(|id| !readable.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        if !foreign.is_empty() {
            reasons.push(format!(
                "it cites evidence outside the pack or not readable by the requester: {}",
                foreign.join(", ")
            ));
        }
        if risk == RiskLevel::Low && step.kind.mutating() {
            reasons.push("a mutating step does not fit a low-risk capability".into());
        }
        let accepted = reasons.is_empty();
        if accepted {
            let output = format!("{}-result", step.step_id);
            let inputs = previous
                .clone()
                .map(|output| vec![output])
                .unwrap_or_default();
            extraction.mutating |= step.kind.mutating();
            extraction.steps.push(ProcedureStep {
                inputs,
                outputs: vec![output.clone()],
                preconditions: Vec::new(),
                verifies: Vec::new(),
                ..step.clone()
            });
            extraction.bases.push(StepBasis {
                step_id: step.step_id.clone(),
                basis: Basis::Hypothesis,
                kind_basis: Basis::Hypothesis,
                evidence_refs: proposal.evidence_refs.clone(),
            });
            previous = Some(output);
        }
        extraction.proposals.push(ProposalDecision {
            step_id: step.step_id.clone(),
            producer: proposal.producer.clone(),
            accepted,
            reasons,
        });
    }
    extraction.recovery = if intake.observed.recovery.is_empty() {
        vec!["No rollback or recovery was observed; how to recover is unknown".into()]
    } else {
        intake
            .observed
            .recovery
            .iter()
            .map(|reference| {
                format!(
                    "Observed recovery: {:?} {}",
                    reference.evidence_type, reference.evidence_id
                )
            })
            .collect()
    };
    if let Some(audit) = sources.capture.audit(&intake.work.query_id) {
        for branch in &audit.branches {
            extraction.branches.push(BranchNote { condition: format!("{} ran only because its condition {}", branch.step_id, if branch.held { "held" } else { "did not hold (skipped)" }), basis: Basis::Observed, source: "the evidence investigation's recipe (how evidence was gathered, not the procedure)".into() });
        }
    }
    if !intake.observed.recovery.is_empty() {
        extraction.branches.push(BranchNote {
            condition: "if the deployment fails, roll it back".into(),
            basis: Basis::Inferred,
            source: "observed recovery evidence".into(),
        });
    }
    extraction
}
