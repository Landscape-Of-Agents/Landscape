// Regression planning and evaluation: what can be defensibly established about this exact
// candidate compared with its exact parent. Nothing either definition describes is executed.
//
// The plan is a Lab test plan made only of the Lab's supported scenario kinds (it is parsed with
// the Lab's own parser, so an unknown kind is rejected):
//
//   verified-context-N   applicability: every context where the parent verifiably succeeded must
//                        still apply (an exclusion that removes it is a regression)
//   unknown-environment  applicability: unknown never matches
//   environment-N        applicability: each declared environment decides as the declared change
//                        set intends
//   pin-N                stale dependency: every pinned zone, flow, and policy resolves now
//   evidence-N           missing evidence: withholding a verified claim's evidence type leaves the
//                        claim unresolved (kept only when that holds for the parent)
//   negative-N           negative case: the parent's negative cases are still declared
//   failure-N            negative case: the failures the proposal records are declared
//
// A run first checks preflight (identities and digests, candidate created through the proposal,
// lifecycle states, current context observable and the candidate's pins current, evidence
// readable, the plan supported); a failed preflight is blocked and no check runs. It then runs the
// Lab on the parent and the candidate with the same plan, compares their non-scenario Lab checks
// (a comparison establishes non-regression relative to the parent, not the tested property), and
// adds structural, claim, coverage, governance, and behavioral checks. The aggregate is
// conservative: failed if any required check failed, inconclusive if any required check is not
// passed, passed only when every required check passed. Behavior is decidable only when the
// procedure is identical; a changed procedure is inconclusive because the Lab cannot execute it.
// A Lab check that is not passed for both versions keeps its status and does not decide the
// aggregate (it was established for neither).

use std::collections::BTreeSet;

use serde_json::json;

use super::models::{
    digest_of, AnalysisReport, CandidateLink, ChangeKind, Dimension, OutcomeClass, Proposal,
    RegressionCheck, RegressionPlan, RegressionRun, RunState, Sealed, Signal, Transition,
};
use super::proposals::{apply_content, content, weakenings};
use super::{EVOLUTION_SCHEMA_VERSION, REGRESSION_VERSION};
use crate::capability::lifecycle::LifecycleState;
use crate::capability::model::{CapabilityDefinition, ClaimStatus, DependencyKind};
use crate::capability::query::{
    dependencies, ApplicabilityMatcher, ApplicabilityQuery, ExactMatcher,
};
use crate::capability::refs::{RefOutcome, Sources};
use crate::capability::registry::{CapabilityRegistry, RegistryState};
use crate::capability::validate::{effective_claim, RunOutcome};
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode, Role};
use crate::capability_lab::preflight::Target;
use crate::capability_lab::report::{run_lab, LabReport};
use crate::capability_lab::scenarios::{self, Scenario, ScenarioCheck, TestPlan};
use crate::capability_lab::{CheckCategory, CheckStatus};
use crate::capture::{stable_id, Actor, Truth};
use crate::compiler::assemble::CompilationReport;

/** The changes the gate allowed
 * Input
    - proposal: &Proposal - proposal
 * Output
    - Vec<super::models::Change>
*/
pub(crate) fn allowed_changes(proposal: &Proposal) -> Vec<super::models::Change> {
    proposal
        .changes
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            proposal
                .gate
                .iter()
                .any(|decision| decision.index == *index && decision.allowed)
        })
        .map(|(_, change)| change.clone())
        .collect()
}

/** Build an applicability scenario
 * Input
    - id: String - scenario id
    - description: String - description
    - task_class: Option<String> - task class
    - environment: Option<String> - environment
    - preconditions: Vec<String> - satisfied preconditions
    - expect: Truth - expected decision
 * Output
    - Scenario
*/
fn applicability(
    id: String,
    description: String,
    task_class: Option<String>,
    environment: Option<String>,
    preconditions: Vec<String>,
    expect: Truth,
) -> Scenario {
    Scenario {
        scenario_id: id,
        description,
        check: ScenarioCheck::Applicability {
            task_class,
            environment,
            preconditions,
            expect,
        },
    }
}

/** Build the regression plan of a proposal with a candidate
 * Input
    - state: &RegistryState - registry
    - sources: &Sources - canonical sources and the actor's access
    - proposal: &Proposal - proposal (with a candidate)
    - analysis: Option<&AnalysisReport> - the analysis of the signal (for verified contexts)
    - now: u64 - Unix milliseconds
 * Output
    - CapabilityResult<RegressionPlan>
    - Error when the proposal has no candidate or the versions do not exist
*/
pub(crate) fn build_plan(
    state: &RegistryState,
    sources: &Sources,
    proposal: &Proposal,
    analysis: Option<&AnalysisReport>,
    now: u64,
) -> CapabilityResult<RegressionPlan> {
    let link = proposal.candidate.clone().ok_or_else(|| {
        CapabilityError::new(
            ErrorCode::InvalidTransition,
            format!("proposal {} has no candidate", proposal.proposal_id),
        )
    })?;
    let parent = &state
        .record(&proposal.capability_id, proposal.parent_version)?
        .definition;
    let intended = apply_content(parent, &allowed_changes(proposal));
    let task_class = parent.applicability.task_classes.first().cloned();
    let preconditions = parent
        .applicability
        .preconditions
        .iter()
        .map(|condition| condition.id.clone())
        .collect::<Vec<_>>();
    let mut scenarios = Vec::new();
    let mut historical = Vec::new();
    if let Some(analysis) = analysis {
        let contexts = analysis
            .observations
            .iter()
            .filter(|observation| {
                observation.duplicate_of.is_none()
                    && observation.class == OutcomeClass::VerifiedSuccess
                    && observation.capability_digest == proposal.parent_digest
            })
            .map(|observation| observation.context.environment.clone())
            .collect::<BTreeSet<_>>();
        for (index, environment) in contexts.into_iter().enumerate() {
            historical.push(format!(
                "environment {}",
                environment.as_deref().unwrap_or("unknown")
            ));
            scenarios.push(applicability(
                format!("verified-context-{}", index + 1),
                format!(
                    "The parent verifiably succeeded in environment {}; it must still apply",
                    environment.as_deref().unwrap_or("unknown")
                ),
                task_class.clone(),
                environment,
                preconditions.clone(),
                Truth::Yes,
            ));
        }
    }
    scenarios.push(applicability(
        "unknown-environment".into(),
        "An unknown environment never matches".into(),
        task_class.clone(),
        None,
        preconditions.clone(),
        Truth::Unknown,
    ));
    for (index, environment) in parent.applicability.environments.iter().enumerate() {
        let situation = ApplicabilityQuery {
            task_class: task_class.clone(),
            environment: Some(environment.clone()),
            satisfied_preconditions: preconditions.clone(),
            entities: Vec::new(),
        };
        let expect = ExactMatcher.evaluate(&intended, &situation).applicable;
        scenarios.push(applicability(
            format!("environment-{}", index + 1),
            format!("Environment {environment} decides as the declared change set intends"),
            task_class.clone(),
            Some(environment.clone()),
            preconditions.clone(),
            expect,
        ));
    }
    let mut pins = 0;
    for dependency in intended.dependencies.iter().filter(|dependency| {
        dependency.required
            && matches!(
                dependency.kind,
                DependencyKind::Zone | DependencyKind::Flow | DependencyKind::Policy
            )
    }) {
        let table = match dependency.kind {
            DependencyKind::Zone => &sources.current.zones,
            DependencyKind::Flow => &sources.current.flows,
            _ => &sources.current.policies,
        };
        if let Some(current) = table.get(&dependency.target_id) {
            pins += 1;
            scenarios.push(Scenario {
                scenario_id: format!("pin-{pins}"),
                description: format!(
                    "{:?} {} at its current version {current} resolves",
                    dependency.kind, dependency.target_id
                ),
                check: ScenarioCheck::StaleDependency {
                    dependency: dependency.kind,
                    id: dependency.target_id.clone(),
                    current_version: current.clone(),
                    expect: RefOutcome::Resolved,
                },
            });
        }
    }
    let mut evidence_scenarios = Vec::new();
    for claim in &parent.verification.claims {
        let resolved = claim
            .evidence_refs
            .iter()
            .filter_map(|id| sources.evidence(id).1.map(|record| record.evidence_type))
            .collect::<Vec<_>>();
        let types = resolved.iter().copied().collect::<BTreeSet<_>>();
        if let (1, true) = (types.len(), resolved.len() == claim.evidence_refs.len()) {
            let Some(evidence_type) = types.into_iter().next() else {
                continue;
            };
            evidence_scenarios.push(Scenario {
                scenario_id: format!("evidence-{}", evidence_scenarios.len() + 1),
                description: format!(
                    "Without {evidence_type:?} evidence, claim {} is unresolved",
                    claim.claim_id
                ),
                check: ScenarioCheck::MissingEvidence {
                    evidence_type,
                    expect_unresolved: vec![claim.claim_id.clone()],
                },
            });
        }
    }
    let probe = TestPlan {
        plan_id: "probe".into(),
        version: 1,
        description: "parent probe".into(),
        scenarios: evidence_scenarios.clone(),
    };
    let mut dropped = Vec::new();
    for (scenario, check) in evidence_scenarios
        .into_iter()
        .zip(scenarios::run(&probe, parent, state, sources))
    {
        if check.status == CheckStatus::Passed {
            scenarios.push(scenario);
        } else {
            dropped.push(format!(
                "{}: the expectation does not hold for the parent ({:?}), so it cannot detect a regression",
                scenario.scenario_id, check.status
            ));
        }
    }
    for (index, case) in parent.verification.negative_cases.iter().enumerate() {
        scenarios.push(Scenario {
            scenario_id: format!("negative-{}", index + 1),
            description: "The parent's negative case is still declared".into(),
            check: ScenarioCheck::NegativeCase {
                declares: case.clone(),
            },
        });
    }
    for (index, change) in allowed_changes(proposal).iter().enumerate() {
        if let ChangeKind::AddKnownFailureCase { text } = &change.change {
            scenarios.push(Scenario {
                scenario_id: format!("failure-{}", index + 1),
                description: "The observed failure the proposal records is declared".into(),
                check: ScenarioCheck::NegativeCase {
                    declares: text.clone(),
                },
            });
        }
    }
    let short = &proposal.proposal_id[..proposal.proposal_id.len().min(20)];
    let lab_plan = TestPlan {
        plan_id: format!("evolution-{}", short.to_ascii_lowercase()),
        version: 1,
        description: format!(
            "Regression of {} v{} against v{}",
            proposal.capability_id, link.version, proposal.parent_version
        ),
        scenarios,
    };
    let lab_plan = TestPlan::parse(&json!(lab_plan).to_string())?;
    let mut unsupported = vec![
        "Parent-versus-candidate execution: the Lab evaluates contracts and evidence; it never executes procedures, so behavior is compared only when the procedure is identical".to_string(),
    ];
    if analysis.is_none() {
        unsupported.push(
            "No analysis report: contexts with verified successes are not represented".into(),
        );
    }
    let mut plan = RegressionPlan {
        plan_id: String::new(),
        schema_version: EVOLUTION_SCHEMA_VERSION,
        regression_version: REGRESSION_VERSION.into(),
        proposal_id: proposal.proposal_id.clone(),
        capability_id: proposal.capability_id.clone(),
        parent_version: proposal.parent_version,
        parent_digest: proposal.parent_digest.clone(),
        candidate_version: link.version,
        candidate_digest: link.digest.clone(),
        lab_plan,
        historical_contexts: historical,
        dropped,
        dimensions: vec![
            Dimension::Structural,
            Dimension::ClaimIntegrity,
            Dimension::Applicability,
            Dimension::VerificationCoverage,
            Dimension::HistoricalFailure,
            Dimension::Governance,
            Dimension::Behavioral,
            Dimension::Lab,
        ],
        unsupported,
        created_at: now,
        digest: String::new(),
    };
    let content_digest = digest_of(
        "evolution/regression-plan-content/v1",
        &(
            &plan.lab_plan,
            &plan.parent_digest,
            &plan.candidate_digest,
            &plan.dropped,
        ),
    );
    plan.plan_id = stable_id("rgp", &[&proposal.proposal_id, &content_digest]);
    plan.seal();
    Ok(plan)
}

/** Build a check
 * Input
    - id: &str - check id
    - dimension: Dimension - dimension
    - status: CheckStatus - status
    - detail: impl Into<String> - explanation
 * Output
    - RegressionCheck
*/
fn check(
    id: &str,
    dimension: Dimension,
    status: CheckStatus,
    detail: impl Into<String>,
) -> RegressionCheck {
    RegressionCheck {
        check_id: id.into(),
        dimension,
        required: true,
        status,
        parent_status: None,
        detail: detail.into(),
        counterexample: None,
        evidence_refs: Vec::new(),
    }
}

/** Preflight reasons that block a run (empty when it may run)
 * Input
    - state: &RegistryState - registry
    - sources: &Sources - canonical sources
    - proposal: &Proposal - proposal
    - plan: &RegressionPlan - plan
    - signal: &Signal - motivating signal
 * Output
    - Vec<String>
*/
pub(crate) fn preflight(
    state: &RegistryState,
    sources: &Sources,
    proposal: &Proposal,
    plan: &RegressionPlan,
    signal: &Signal,
) -> Vec<String> {
    let mut blocked = Vec::new();
    if !proposal.verify() || !plan.verify() {
        blocked.push("the proposal or plan does not match its digest".into());
    }
    let Some(CandidateLink {
        version, digest, ..
    }) = &proposal.candidate
    else {
        blocked.push("the proposal has no candidate".into());
        return blocked;
    };
    if plan.candidate_digest != *digest
        || plan.parent_digest != proposal.parent_digest
        || plan.proposal_id != proposal.proposal_id
    {
        blocked.push("the plan was made for another candidate, parent, or proposal".into());
    }
    match state.record(&proposal.capability_id, *version) {
        Err(_) => blocked.push(format!(
            "candidate v{version} does not exist in the registry"
        )),
        Ok(record) => {
            if record.definition_digest != *digest || record.definition.digest() != *digest {
                blocked.push(format!(
                    "candidate v{version} does not match digest {digest}"
                ));
            }
            if !record
                .definition
                .provenance
                .change_reason
                .as_deref()
                .is_some_and(|reason| reason.contains(&proposal.proposal_id))
            {
                blocked.push("the candidate was not created through this proposal".into());
            }
            if matches!(
                record.state,
                LifecycleState::Rejected | LifecycleState::Revoked | LifecycleState::Deprecated
            ) {
                blocked.push(format!("the candidate is {:?}", record.state));
            }
            if record.state == LifecycleState::Published {
                blocked.push("the candidate is already published; regression of a published version is not a proposal step".into());
            }
            if record.definition.dependencies.iter().any(|dependency| {
                matches!(dependency.kind, DependencyKind::Zone | DependencyKind::Flow)
            }) && !sources.current.landscape_known
                || record
                    .definition
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.kind == DependencyKind::Policy)
                    && !sources.current.crane_known
            {
                blocked.push("the current Landscape or Crane versions cannot be read; staleness cannot be established".into());
            }
            if let Ok(statuses) = dependencies(state, &proposal.capability_id, *version, sources) {
                for status in statuses.iter().filter(|status| {
                    status.dependency.required
                        && matches!(
                            status.dependency.kind,
                            DependencyKind::Zone | DependencyKind::Flow | DependencyKind::Policy
                        )
                        && matches!(
                            status.outcome,
                            RefOutcome::Stale | RefOutcome::Missing | RefOutcome::Mismatch
                        )
                }) {
                    blocked.push(format!(
                        "stale candidate: {:?} {} is {:?}",
                        status.dependency.kind, status.dependency.target_id, status.outcome
                    ));
                }
            }
        }
    }
    match state.record(&proposal.capability_id, proposal.parent_version) {
        Err(_) => blocked.push("the parent no longer exists".into()),
        Ok(parent) if parent.definition_digest != proposal.parent_digest => {
            blocked.push("the parent does not match its recorded digest".into())
        }
        Ok(parent)
            if matches!(
                parent.state,
                LifecycleState::Revoked | LifecycleState::Rejected
            ) =>
        {
            blocked.push(format!("the parent is {:?}", parent.state))
        }
        Ok(_) => {}
    }
    for reference in signal.evidence.iter().filter(|reference| {
        signal.supporting.contains(&reference.invocation_id)
            && reference.source == super::models::EvidenceSource::WorkCapture
    }) {
        if sources.evidence(&reference.ref_id).0.outcome != RefOutcome::Resolved {
            blocked.push(format!(
                "evidence {} is not readable in scope",
                reference.ref_id
            ));
        }
    }
    if TestPlan::parse(&json!(plan.lab_plan).to_string()).is_err() {
        blocked.push("the plan contains an unsupported scenario".into());
    }
    blocked
}

/** Order check statuses from worst to best for comparison
 * Input
    - status: CheckStatus - status
 * Output
    - u8 rank (higher is better)
*/
fn rank(status: CheckStatus) -> u8 {
    match status {
        CheckStatus::Failed => 0,
        CheckStatus::Blocked => 1,
        CheckStatus::Inconclusive | CheckStatus::NotRun => 2,
        CheckStatus::Passed => 3,
    }
}

/** Order validation outcomes from worst to best
 * Input
    - outcome: RunOutcome - outcome
 * Output
    - u8 rank
*/
fn outcome_rank(outcome: RunOutcome) -> u8 {
    match outcome {
        RunOutcome::StructurallyInvalid => 0,
        RunOutcome::Failed => 1,
        RunOutcome::NeedsEvidence => 2,
        RunOutcome::Passed => 3,
    }
}

/** The dimension of a generated scenario
 * Input
    - scenario_id: &str - scenario id
 * Output
    - Dimension
*/
fn scenario_dimension(scenario_id: &str) -> Dimension {
    match scenario_id.split('-').next().unwrap_or_default() {
        "verified" | "unknown" | "environment" | "pin" => Dimension::Applicability,
        "evidence" => Dimension::ClaimIntegrity,
        "negative" | "failure" => Dimension::HistoricalFailure,
        _ => Dimension::Lab,
    }
}

/** Compare the Lab reports of parent and candidate
 * Input
    - parent: &LabReport - parent report
    - candidate: &LabReport - candidate report
 * Output
    - Vec<RegressionCheck>
*/
fn compare_lab(parent: &LabReport, candidate: &LabReport) -> Vec<RegressionCheck> {
    let mut checks = Vec::new();
    for lab in &candidate.checks {
        let before = parent
            .checks
            .iter()
            .find(|other| other.check_id == lab.check_id)
            .map(|other| other.status);
        if lab.category == CheckCategory::Scenario {
            let id = lab.check_id.trim_start_matches("scenario:");
            let mut result = check(
                &format!("lab:{}", lab.check_id),
                scenario_dimension(id),
                lab.status,
                lab.detail.clone(),
            );
            result.parent_status = before;
            result.counterexample = lab.counterexample.clone();
            result.evidence_refs = lab.evidence_refs.clone();
            checks.push(result);
            continue;
        }
        if lab.category == CheckCategory::Conformance {
            let mut result = check(&format!("lab:{}", lab.check_id), Dimension::Lab, CheckStatus::NotRun, "step conformance is re-derived only for compiler output; evolution candidates are compared with their parent structurally instead");
            result.required = false;
            result.parent_status = before;
            checks.push(result);
            continue;
        }
        let mut result = match before {
            Some(before) if before == lab.status && lab.status != CheckStatus::Passed => {
                let mut same = check(
                    &format!("lab:{}", lab.check_id),
                    Dimension::Lab,
                    lab.status,
                    format!("unchanged from the parent ({:?} for both); established for neither, so it does not decide the regression: {}", lab.status, lab.detail),
                );
                same.required = false;
                same
            }
            Some(before) if rank(lab.status) >= rank(before) => check(
                &format!("lab:{}", lab.check_id),
                Dimension::Lab,
                CheckStatus::Passed,
                format!("no regression from the parent ({before:?} -> {:?}); this establishes non-regression, not the tested property: {}", lab.status, lab.detail),
            ),
            Some(before) => {
                let mut failed = check(
                    &format!("lab:{}", lab.check_id),
                    Dimension::Lab,
                    CheckStatus::Failed,
                    format!("regressed from the parent ({before:?} -> {:?}): {}", lab.status, lab.detail),
                );
                failed.counterexample = Some(json!({"parent": before, "candidate": lab.status, "detail": lab.detail}));
                failed
            }
            None if lab.status == CheckStatus::Passed => check(&format!("lab:{}", lab.check_id), Dimension::Lab, CheckStatus::Passed, lab.detail.clone()),
            None => check(
                &format!("lab:{}", lab.check_id),
                Dimension::Lab,
                if lab.status == CheckStatus::Failed { CheckStatus::Failed } else { CheckStatus::Inconclusive },
                format!("new on the candidate ({:?}): {}", lab.status, lab.detail),
            ),
        };
        result.parent_status = before;
        result.evidence_refs = lab.evidence_refs.clone();
        if result.counterexample.is_none() {
            result.counterexample = lab
                .counterexample
                .clone()
                .filter(|_| result.status == CheckStatus::Failed);
        }
        checks.push(result);
    }
    checks
}

/** The checks of a candidate's content against its parent
 * Input
    - registry: &mut CapabilityRegistry - registry (validation dry runs change nothing)
    - sources: &Sources - canonical sources
    - actor: &Actor - actor
    - proposal: &Proposal - proposal
    - parent: &CapabilityDefinition - parent
    - candidate: &CapabilityDefinition - candidate
    - now: u64 - Unix milliseconds
 * Output
    - CapabilityResult<Vec<RegressionCheck>>
*/
fn content_checks(
    registry: &mut CapabilityRegistry,
    sources: &Sources,
    actor: &Actor,
    proposal: &Proposal,
    parent: &CapabilityDefinition,
    candidate: &CapabilityDefinition,
    now: u64,
) -> CapabilityResult<Vec<RegressionCheck>> {
    let mut checks = Vec::new();
    let identity = candidate.capability_id == parent.capability_id
        && candidate.previous_version_ref == Some(parent.version)
        && candidate.version == parent.version + 1;
    checks.push(check(
        "structural:identity",
        Dimension::Structural,
        if identity {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        format!(
            "candidate {} v{} links to predecessor {:?}; parent is v{}",
            candidate.capability_id,
            candidate.version,
            candidate.previous_version_ref,
            parent.version
        ),
    ));
    let provenance = candidate.provenance.source_intake_refs
        == parent.provenance.source_intake_refs
        && candidate.provenance.source_episode_refs == parent.provenance.source_episode_refs
        && candidate.provenance.source_evidence_pack_refs
            == parent.provenance.source_evidence_pack_refs;
    checks.push(check(
        "structural:provenance",
        Dimension::Structural,
        if provenance {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        "the candidate keeps the parent's intake, episode, and Evidence Pack references",
    ));
    let intended = apply_content(parent, &allowed_changes(proposal));
    let declared = content(&intended) == content(candidate);
    let mut declared_check = check(
        "structural:declared-changes",
        Dimension::Structural,
        if declared {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        if declared {
            "the candidate differs from its parent only by the declared change set".to_string()
        } else {
            "the candidate contains changes outside the declared change set".to_string()
        },
    );
    if !declared {
        let left = json!(content(&intended));
        let right = json!(content(candidate));
        let differing = left
            .as_object()
            .map(|object| {
                object
                    .keys()
                    .filter(|key| left.get(*key) != right.get(*key))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        declared_check.counterexample = Some(json!({"fields": differing}));
    }
    checks.push(declared_check);
    let parent_run = registry.validate(
        actor,
        &parent.capability_id,
        parent.version,
        sources,
        true,
        now,
    )?;
    let candidate_run = registry.validate(
        actor,
        &candidate.capability_id,
        candidate.version,
        sources,
        true,
        now,
    )?;
    let mut dry = check(
        "structural:registry-gates",
        Dimension::Structural,
        if outcome_rank(candidate_run.outcome) >= outcome_rank(parent_run.outcome) {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        format!(
            "Component 2 dry-run validation (records nothing): parent {:?}, candidate {:?}",
            parent_run.outcome, candidate_run.outcome
        ),
    );
    if dry.status == CheckStatus::Failed {
        dry.counterexample = Some(
            json!({"findings": candidate_run.gates.iter().flat_map(|gate| gate.findings.iter().map(|finding| finding.message.clone())).collect::<Vec<_>>()}),
        );
    }
    checks.push(dry);
    let weak = weakenings(parent, candidate);
    let (claims, governance): (Vec<_>, Vec<_>) = weak.iter().cloned().partition(|item| {
        item.contains("claim") || item.contains("invariant") || item.contains("negative case")
    });
    let mut claim_check = check(
        "claims:preserved",
        Dimension::ClaimIntegrity,
        if claims.is_empty() {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        if claims.is_empty() {
            "every parent claim, invariant, and negative case is kept with its evidence".to_string()
        } else {
            claims.join("; ")
        },
    );
    if !claims.is_empty() {
        claim_check.counterexample = Some(json!({"violations": claims}));
    }
    checks.push(claim_check);
    let mut upgraded = Vec::new();
    let mut evidence = Vec::new();
    for claim in &candidate.verification.claims {
        let (after, _, refs) = effective_claim(candidate, claim, sources);
        evidence.extend(refs);
        let before = parent
            .verification
            .claims
            .iter()
            .find(|other| other.claim_id == claim.claim_id)
            .map(|other| effective_claim(parent, other, sources).0);
        let gained = after == ClaimStatus::Verified && before != Some(ClaimStatus::Verified);
        if gained {
            upgraded.push(claim.claim_id.clone());
        }
    }
    let mut recomputed = check(
        "claims:recomputed",
        Dimension::ClaimIntegrity,
        if upgraded.is_empty() {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        if upgraded.is_empty() {
            "recomputed from evidence with Component 2's rules, no claim becomes verified that was not verified for the parent".to_string()
        } else {
            format!(
                "claims became verified through the revision: {}",
                upgraded.join(", ")
            )
        },
    );
    recomputed.evidence_refs = evidence
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    checks.push(recomputed);
    let required = |definition: &CapabilityDefinition| {
        definition
            .verification
            .claims
            .iter()
            .filter(|claim| claim.required)
            .count()
    };
    checks.push(check(
        "coverage:required-claims",
        Dimension::VerificationCoverage,
        if required(candidate) >= required(parent) && candidate.verification.method == parent.verification.method {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        format!(
            "required claims: parent {}, candidate {}; verification method {} kept; a new requirement stays unresolved until validated",
            required(parent),
            required(candidate),
            candidate.verification.method
        ),
    ));
    let mut governance_check = check(
        "governance:not-weakened",
        Dimension::Governance,
        if governance.is_empty() {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        if governance.is_empty() {
            "risk, review level, policies, dependencies, exclusions, scope, preconditions, and steps are not weakened".to_string()
        } else {
            governance.join("; ")
        },
    );
    if !governance.is_empty() {
        governance_check.counterexample = Some(json!({"violations": governance}));
    }
    checks.push(governance_check);
    let same_procedure = parent.execution == candidate.execution;
    checks.push(check(
        "behavioral:procedure",
        Dimension::Behavioral,
        if same_procedure { CheckStatus::Passed } else { CheckStatus::Inconclusive },
        if same_procedure {
            "the execution contract is identical to the parent's, so the revision changes no procedure behavior".to_string()
        } else {
            "the execution contract changed; the Lab cannot execute either procedure, so behavioral equivalence cannot be established".to_string()
        },
    ));
    Ok(checks)
}

/** Run a regression of a proposal's candidate against its parent
 * Input
    - registry: &mut CapabilityRegistry - registry (read; dry runs record nothing)
    - sources: &Sources - canonical sources and the actor's access
    - compilation_reports: &[CompilationReport] - verified compilation reports (for the parent)
    - proposal: &Proposal - proposal
    - plan: &RegressionPlan - plan
    - signal: &Signal - motivating signal
    - previous: Option<&RegressionRun> - the previous run, when this retries
    - actor: &Actor - actor (needs the Contributor role)
    - now: u64 - Unix milliseconds
 * Output
    - CapabilityResult<(RegressionRun, Vec<LabReport>)> a sealed run in a terminal state, and the
      Lab reports it produced (parent, candidate; none when blocked)
    - Error (Unauthorized) when the actor holds no role
*/
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute(
    registry: &mut CapabilityRegistry,
    sources: &Sources,
    compilation_reports: &[CompilationReport],
    proposal: &Proposal,
    plan: &RegressionPlan,
    signal: &Signal,
    previous: Option<&RegressionRun>,
    actor: &Actor,
    now: u64,
) -> CapabilityResult<(RegressionRun, Vec<LabReport>)> {
    registry.grants.require(actor, Role::Contributor)?;
    let transitions_before = registry.state.transitions.len();
    let inputs_digest = digest_of(
        "evolution/regression-inputs/v1",
        &(
            &plan.digest,
            &plan.parent_digest,
            &plan.candidate_digest,
            sources.current,
        ),
    );
    let mut run = RegressionRun {
        run_id: stable_id(
            "rgr",
            &[
                &proposal.proposal_id,
                &plan.plan_id,
                &proposal.runs.len().to_string(),
                &inputs_digest,
            ],
        ),
        schema_version: EVOLUTION_SCHEMA_VERSION,
        regression_version: REGRESSION_VERSION.into(),
        proposal_id: proposal.proposal_id.clone(),
        plan_id: plan.plan_id.clone(),
        plan_digest: plan.digest.clone(),
        capability_id: proposal.capability_id.clone(),
        parent_version: plan.parent_version,
        parent_digest: plan.parent_digest.clone(),
        candidate_version: plan.candidate_version,
        candidate_digest: plan.candidate_digest.clone(),
        governance: sources.current.clone(),
        inputs_digest: inputs_digest.clone(),
        previous_run: previous.map(|run| run.run_id.clone()),
        inputs_changed: previous.map(|run| run.inputs_digest != inputs_digest),
        state: RunState::Created,
        transitions: vec![Transition {
            sequence: 1,
            from: None,
            to: RunState::Created,
            actor: actor.clone(),
            reason: "regression run created".into(),
            at: now,
        }],
        checks: Vec::new(),
        lab_reports: Vec::new(),
        skipped: plan.dropped.clone(),
        counterexamples: Vec::new(),
        limitations: plan.unsupported.clone(),
        reason: String::new(),
        actor: actor.clone(),
        created_at: now,
        digest: String::new(),
    };
    let advance = |run: &mut RegressionRun, to: RunState, reason: &str| {
        super::models::advance(&mut run.state, &mut run.transitions, to, actor, reason, now)
    };
    advance(
        &mut run,
        RunState::Preflight,
        "checking identities, digests, staleness, integrity, and the plan",
    )?;
    let blocked = preflight(&registry.state, sources, proposal, plan, signal);
    if !blocked.is_empty() {
        run.reason = format!("blocked before any check ran: {}", blocked.join("; "));
        run.skipped.push("every check: preflight failed".into());
        let reason = run.reason.clone();
        advance(&mut run, RunState::Blocked, &reason)?;
        run.seal();
        return Ok((run, Vec::new()));
    }
    advance(
        &mut run,
        RunState::Running,
        "evaluating supported deterministic checks",
    )?;
    let parent = registry
        .state
        .record(&proposal.capability_id, plan.parent_version)?
        .definition
        .clone();
    let candidate = registry
        .state
        .record(&proposal.capability_id, plan.candidate_version)?
        .definition
        .clone();
    let parent_compilation = compilation_reports.iter().find(|report| {
        report.verify_digest()
            && report
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.digest == plan.parent_digest)
    });
    let parent_lab = run_lab(
        registry,
        sources,
        actor,
        &Target {
            capability_id: proposal.capability_id.clone(),
            version: plan.parent_version,
            expected_digest: plan.parent_digest.clone(),
        },
        parent_compilation,
        &plan.lab_plan,
        now,
    )?;
    let candidate_lab = run_lab(
        registry,
        sources,
        actor,
        &Target {
            capability_id: proposal.capability_id.clone(),
            version: plan.candidate_version,
            expected_digest: plan.candidate_digest.clone(),
        },
        None,
        &plan.lab_plan,
        now,
    )?;
    run.lab_reports = vec![
        (parent_lab.report_id.clone(), parent_lab.digest.clone()),
        (
            candidate_lab.report_id.clone(),
            candidate_lab.digest.clone(),
        ),
    ];
    run.limitations
        .extend(candidate_lab.limitations.iter().cloned());
    run.checks.extend(content_checks(
        registry, sources, actor, proposal, &parent, &candidate, now,
    )?);
    run.checks.extend(compare_lab(&parent_lab, &candidate_lab));
    let untouched = registry.state.transitions.len() == transitions_before
        && registry
            .state
            .record(&proposal.capability_id, plan.candidate_version)
            .map(|record| record.state != LifecycleState::Published)
            .unwrap_or(false);
    run.checks.push(check(
        "governance:publication-boundary",
        Dimension::Governance,
        if untouched { CheckStatus::Passed } else { CheckStatus::Failed },
        "the run changed no lifecycle state and the candidate is unpublished; publication stays with Component 2's authorized workflow",
    ));
    for item in &run.checks {
        if item.status == CheckStatus::NotRun {
            run.skipped
                .push(format!("{}: {}", item.check_id, item.detail));
        }
        if let Some(counterexample) = &item.counterexample {
            run.counterexamples
                .push(json!({"check": item.check_id, "counterexample": counterexample}));
        }
    }
    let required = run
        .checks
        .iter()
        .filter(|item| item.required)
        .collect::<Vec<_>>();
    let failed = required
        .iter()
        .filter(|item| item.status == CheckStatus::Failed)
        .map(|item| item.check_id.clone())
        .collect::<Vec<_>>();
    let open = required
        .iter()
        .filter(|item| item.status != CheckStatus::Passed && item.status != CheckStatus::Failed)
        .map(|item| item.check_id.clone())
        .collect::<Vec<_>>();
    let (to, reason) = if !failed.is_empty() {
        (
            RunState::Failed,
            format!("regression found by {}", failed.join(", ")),
        )
    } else if !open.is_empty() {
        (
            RunState::Inconclusive,
            format!("required checks not established: {}", open.join(", ")),
        )
    } else {
        (
            RunState::Passed,
            format!(
                "all {} required supported checks passed; this is not publication approval",
                required.len()
            ),
        )
    };
    run.reason = reason.clone();
    advance(&mut run, to, &reason)?;
    run.seal();
    Ok((run, vec![parent_lab, candidate_lab]))
}
