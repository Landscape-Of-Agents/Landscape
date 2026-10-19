// Candidate evaluation and ranking. Candidates come from Component 2's registry queries. Each is
// explained condition by condition (satisfied, unsatisfied, or unknown), and its disposition
// follows the mandatory gates in a fixed order:
//
//   1. eligibility   - a published, currently eligible version exists (Component 2's rule) and its
//                      validation is not stale against the current references (checked live, not
//                      only as marked); otherwise rejected
//   2. applicability - Component 2's exact matcher says yes; no means rejected, unknown means
//                      unresolved (an empty environment list never matches)
//   3. scope         - the task's observed zone and flow versions equal the pinned ones; a
//                      dependency the context says nothing about is unknown
//   4. dependencies  - every required dependency resolves now; stale, missing, or mismatched ones
//                      block, unverifiable ones leave the candidate unresolved
//   5. lab reports   - Component 4 reports for this exact digest: a failed report blocks automatic
//                      recommendation; inconclusive, blocked, and not-run reports are shown and
//                      never counted as success
//
// Only candidates that pass every gate are ranked, by RANKING_VERSION rules: a passed lab report
// for the exact digest, then fewer unverified assumptions, lower risk, the higher version, and the
// capability id as the final tie-break.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use super::resolve::ResolvedContext;
use super::RANKING_VERSION;
use crate::capability::lifecycle::LifecycleState;
use crate::capability::model::{
    CapabilityDefinition, ClaimStatus, DependencyKind, VersionConstraint,
};
use crate::capability::query::{
    dependencies, eligible, latest_eligible, list, ApplicabilityMatcher, ApplicabilityQuery,
    ExactMatcher, RegistryQuery,
};
use crate::capability::refs::{RefOutcome, Sources};
use crate::capability::registry::{stale_reasons, RegistryState, VersionRecord};
use crate::capability_lab::report::LabReport;
use crate::capability_lab::CheckStatus;
use crate::capture::Truth;

/** Most capabilities one discovery evaluates */
pub(crate) const MAX_CANDIDATES: usize = 200;

/** The status of one condition
 * Variants
    - Satisfied - established to hold
    - Unsatisfied - established not to hold
    - Unknown - not established (never counted as satisfied)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConditionStatus {
    Satisfied,
    Unsatisfied,
    Unknown,
}

/** One evaluated condition
 * Fields
    - gate: String - eligibility, applicability, scope, dependency, or lab
    - condition: String - what was evaluated
    - status: ConditionStatus - result
    - detail: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConditionResult {
    pub(crate) gate: String,
    pub(crate) condition: String,
    pub(crate) status: ConditionStatus,
    pub(crate) detail: String,
}

/** A Component 4 report relevant to a candidate
 * Fields
    - report_id: String - lab report
    - digest: String - its digest
    - overall: CheckStatus - its overall status
    - failed_checks: Vec<String> - its failed checks
    - note: String - how discovery treated it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LabSummary {
    pub(crate) report_id: String,
    pub(crate) digest: String,
    pub(crate) overall: CheckStatus,
    pub(crate) failed_checks: Vec<String>,
    pub(crate) note: String,
}

/** What happened to a candidate
 * Variants
    - Recommended - passed every gate
    - Rejected - not eligible, not applicable, or out of scope
    - BlockedByDependency - a required dependency is stale, missing, or mismatched
    - BlockedByLab - a lab report for the exact digest failed
    - Unresolved - unknown conditions prevent a decision
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Disposition {
    Recommended,
    Rejected,
    BlockedByDependency,
    BlockedByLab,
    Unresolved,
}

/** The evaluation of one capability
 * Fields
    - capability_id: String - capability
    - version: Option<u32> - version evaluated (the latest eligible, else the latest)
    - digest: Option<String> - its definition digest
    - state: Option<LifecycleState> - its lifecycle state
    - conditions: Vec<ConditionResult> - evaluated conditions
    - lab: Vec<LabSummary> - lab reports for the exact digest
    - disposition: Disposition - outcome
    - reasons: Vec<String> - why
    - rank: Option<usize> - rank among recommended candidates (1 is best)
    - ranking_reasons: Vec<String> - the ranking keys
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CandidateEvaluation {
    pub(crate) capability_id: String,
    pub(crate) version: Option<u32>,
    pub(crate) digest: Option<String>,
    pub(crate) state: Option<LifecycleState>,
    pub(crate) conditions: Vec<ConditionResult>,
    pub(crate) lab: Vec<LabSummary>,
    pub(crate) disposition: Disposition,
    pub(crate) reasons: Vec<String>,
    pub(crate) rank: Option<usize>,
    pub(crate) ranking_reasons: Vec<String>,
}

/** Collects conditions and the gates that failed
 * Fields
    - conditions: Vec<ConditionResult> - conditions
    - rejected, blocked_dependency, blocked_lab, unresolved: Vec<String> - reasons per disposition
*/
#[derive(Default)]
struct Tally {
    conditions: Vec<ConditionResult>,
    rejected: Vec<String>,
    blocked_dependency: Vec<String>,
    blocked_lab: Vec<String>,
    unresolved: Vec<String>,
}

impl Tally {
    /** Record a condition; unsatisfied ones go to the given reason list, unknown ones to unresolved
     * Input
        - gate: &str - gate
        - condition: String - condition
        - status: ConditionStatus - result
        - detail: String - explanation
        - unsatisfied: fn(&mut Tally) -> &mut Vec<String> - where an unsatisfied condition counts
     * Output
        - None
    */
    fn record(
        &mut self,
        gate: &str,
        condition: String,
        status: ConditionStatus,
        detail: String,
        unsatisfied: fn(&mut Tally) -> &mut Vec<String>,
    ) {
        match status {
            ConditionStatus::Unsatisfied => {
                unsatisfied(self).push(format!("{condition}: {detail}"))
            }
            ConditionStatus::Unknown => self.unresolved.push(format!("{condition}: {detail}")),
            ConditionStatus::Satisfied => {}
        }
        self.conditions.push(ConditionResult {
            gate: gate.into(),
            condition,
            status,
            detail,
        });
    }
}

/** Where rejections go
 * Input
    - tally: &mut Tally - tally
 * Output
    - &mut Vec<String>
*/
fn rejected(tally: &mut Tally) -> &mut Vec<String> {
    &mut tally.rejected
}

/** Where dependency blocks go
 * Input
    - tally: &mut Tally - tally
 * Output
    - &mut Vec<String>
*/
fn blocked_dependency(tally: &mut Tally) -> &mut Vec<String> {
    &mut tally.blocked_dependency
}

/** Applicability conditions, one per declared element, with Component 2's decision as the gate
 * Input
    - definition: &CapabilityDefinition - definition
    - context: &ResolvedContext - context
    - tally: &mut Tally - tally
 * Output
    - None
*/
fn applicability(definition: &CapabilityDefinition, context: &ResolvedContext, tally: &mut Tally) {
    let contract = &definition.applicability;
    let class = if contract.task_classes.contains(&context.task_class) {
        ConditionStatus::Satisfied
    } else {
        ConditionStatus::Unsatisfied
    };
    tally.conditions.push(ConditionResult {
        gate: "applicability".into(),
        condition: format!("task class {}", context.task_class),
        status: class,
        detail: format!("declared: {}", contract.task_classes.join(", ")),
    });
    let environment = match (&context.environment, contract.environments.is_empty()) {
        (_, true) => (
            ConditionStatus::Unsatisfied,
            "the capability declares no known environment".to_string(),
        ),
        (None, _) => (
            ConditionStatus::Unknown,
            format!(
                "the task's environment is unknown; declared: {}",
                contract.environments.join(", ")
            ),
        ),
        (Some(environment), _) if contract.environments.contains(environment) => (
            ConditionStatus::Satisfied,
            format!("{environment} is declared"),
        ),
        (Some(environment), _) => (
            ConditionStatus::Unsatisfied,
            format!(
                "{environment} is not declared ({})",
                contract.environments.join(", ")
            ),
        ),
    };
    tally.conditions.push(ConditionResult {
        gate: "applicability".into(),
        condition: "environment".into(),
        status: environment.0,
        detail: environment.1,
    });
    for precondition in contract
        .preconditions
        .iter()
        .chain(&contract.required_state)
    {
        let status = if context.confirmed_preconditions.contains(&precondition.id) {
            ConditionStatus::Satisfied
        } else {
            ConditionStatus::Unknown
        };
        tally.conditions.push(ConditionResult {
            gate: "applicability".into(),
            condition: format!("precondition {}", precondition.id),
            status,
            detail: precondition.description.clone(),
        });
    }
    let situation = ApplicabilityQuery {
        task_class: Some(context.task_class.clone()),
        environment: context.environment.clone(),
        satisfied_preconditions: context.confirmed_preconditions.clone(),
        entities: context.entities.clone(),
    };
    let decision = ExactMatcher.evaluate(definition, &situation);
    match decision.applicable {
        Truth::Yes => {}
        Truth::No => tally
            .rejected
            .push(format!("not applicable: {}", decision.reasons.join("; "))),
        Truth::Unknown => tally.unresolved.push(format!(
            "applicability unknown: {}",
            decision.reasons.join("; ")
        )),
    }
}

/** Scope conditions: pinned zone and flow versions against the task's observed context
 * Input
    - definition: &CapabilityDefinition - definition
    - context: &ResolvedContext - context
    - tally: &mut Tally - tally
 * Output
    - None
*/
fn scope(definition: &CapabilityDefinition, context: &ResolvedContext, tally: &mut Tally) {
    for dependency in definition.dependencies.iter().filter(|dependency| {
        dependency.required
            && matches!(dependency.kind, DependencyKind::Zone | DependencyKind::Flow)
    }) {
        let observed = if dependency.kind == DependencyKind::Zone {
            &context.zones
        } else {
            &context.flows
        };
        let pinned = match &dependency.constraint {
            VersionConstraint::Exact(version) => Some(version.as_str()),
            _ => None,
        };
        let label = format!("{:?} {}", dependency.kind, dependency.target_id).to_ascii_lowercase();
        let (status, detail) = match observed
            .iter()
            .find(|reference| reference.id == dependency.target_id)
        {
            None if observed.is_empty() => (
                ConditionStatus::Unknown,
                "the task's context does not establish which zones and flows it touches"
                    .to_string(),
            ),
            None => (
                ConditionStatus::Unsatisfied,
                "the task's context does not involve it".to_string(),
            ),
            Some(reference) => match (&reference.version, pinned) {
                (Some(found), Some(pinned)) if found == pinned => (
                    ConditionStatus::Satisfied,
                    format!("the task runs under {found}, as pinned"),
                ),
                (Some(found), Some(pinned)) => (
                    ConditionStatus::Unsatisfied,
                    format!("the task runs under {found}, the capability is pinned to {pinned}"),
                ),
                _ => (
                    ConditionStatus::Unknown,
                    "the version the task runs under or the pin is unknown".to_string(),
                ),
            },
        };
        tally.record("scope", label, status, detail, rejected);
    }
    if let Some(dependency) = definition
        .dependencies
        .iter()
        .find(|dependency| dependency.kind == DependencyKind::Repository)
    {
        let status = match &context.repository {
            None => ConditionStatus::Unknown,
            Some(repository) if repository == &dependency.target_id => ConditionStatus::Satisfied,
            Some(_) => ConditionStatus::Unsatisfied,
        };
        let detail = format!("established in {}", dependency.target_id);
        if dependency.required || status == ConditionStatus::Unsatisfied {
            tally.record("scope", "repository".into(), status, detail, rejected);
        } else {
            tally.conditions.push(ConditionResult {
                gate: "scope".into(),
                condition: "repository".into(),
                status,
                detail,
            });
        }
    }
}

/** Evaluate one capability
 * Input
    - id: &str - capability
    - state: &RegistryState - registry
    - context: &ResolvedContext - context
    - sources: &Sources - sources and current versions
    - lab_reports: &[LabReport] - every lab report
 * Output
    - CandidateEvaluation
*/
fn evaluate_one(
    id: &str,
    state: &RegistryState,
    context: &ResolvedContext,
    sources: &Sources,
    lab_reports: &[LabReport],
) -> CandidateEvaluation {
    let mut tally = Tally::default();
    let latest = state
        .versions
        .get(id)
        .and_then(|versions| versions.values().next_back());
    let record: Option<&VersionRecord> = latest_eligible(state, id);
    let Some(record) = record else {
        let detail = latest.map_or_else(
            || "no version".to_string(),
            |latest| {
                format!(
                    "the latest version v{} is {:?}{}",
                    latest.definition.version,
                    latest.state,
                    if latest.stale.is_empty() {
                        ""
                    } else {
                        " and stale"
                    }
                )
            },
        );
        return CandidateEvaluation {
            capability_id: id.into(),
            version: latest.map(|latest| latest.definition.version),
            digest: latest.map(|latest| latest.definition_digest.clone()),
            state: latest.map(|latest| latest.state),
            conditions: vec![ConditionResult {
                gate: "eligibility".into(),
                condition: "published and currently eligible".into(),
                status: ConditionStatus::Unsatisfied,
                detail: detail.clone(),
            }],
            lab: Vec::new(),
            disposition: Disposition::Rejected,
            reasons: vec![format!(
                "no published, currently eligible version: {detail}"
            )],
            rank: None,
            ranking_reasons: Vec::new(),
        };
    };
    let definition = &record.definition;
    let live = state
        .latest_run(record)
        .map(|run| stale_reasons(run, state, sources))
        .unwrap_or_else(|| vec!["it has no validation run".into()]);
    let eligibility = if eligible(record) {
        ConditionStatus::Satisfied
    } else {
        ConditionStatus::Unsatisfied
    };
    tally.record(
        "eligibility",
        format!("v{} published and currently eligible", definition.version),
        eligibility,
        "published, not revoked or deprecated, not marked stale".into(),
        rejected,
    );
    let current = if live.is_empty() {
        ConditionStatus::Satisfied
    } else {
        ConditionStatus::Unsatisfied
    };
    tally.record(
        "eligibility",
        "validation current against today's references".into(),
        current,
        if live.is_empty() {
            "nothing it was validated against has changed".into()
        } else {
            format!(
                "stale since validation (not yet marked in the registry): {}",
                live.join("; ")
            )
        },
        blocked_dependency,
    );
    applicability(definition, context, &mut tally);
    scope(definition, context, &mut tally);
    match dependencies(state, id, definition.version, sources) {
        Ok(statuses) => {
            for status in statuses {
                let label = format!(
                    "dependency {:?} {}",
                    status.dependency.kind, status.dependency.target_id
                )
                .to_ascii_lowercase();
                let (condition, detail) = match status.outcome {
                    RefOutcome::Resolved => (
                        ConditionStatus::Satisfied,
                        "resolves to its pinned version".to_string(),
                    ),
                    RefOutcome::Unverifiable => (
                        ConditionStatus::Unknown,
                        "cannot be verified by this registry".to_string(),
                    ),
                    other => (ConditionStatus::Unsatisfied, format!("{other:?}")),
                };
                if status.dependency.required {
                    tally.record("dependency", label, condition, detail, blocked_dependency);
                } else {
                    tally.conditions.push(ConditionResult {
                        gate: "dependency".into(),
                        condition: format!("{label} (optional)"),
                        status: condition,
                        detail,
                    });
                }
            }
        }
        Err(error) => tally.blocked_dependency.push(error.to_string()),
    }
    let mut lab = Vec::new();
    for report in lab_reports.iter().filter(|report| {
        report.capability_id == id
            && report.version == definition.version
            && report.candidate_digest == record.definition_digest
    }) {
        let failed_checks = report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Failed)
            .map(|check| check.check_id.clone())
            .collect::<Vec<_>>();
        let note = match report.overall {
            CheckStatus::Failed => {
                tally.blocked_lab.push(format!(
                    "lab report {} failed: {}",
                    report.report_id,
                    failed_checks.join(", ")
                ));
                "failed: blocks automatic recommendation".to_string()
            }
            CheckStatus::Passed => "passed: every tested condition held".to_string(),
            other => format!("{other:?}: shown, not counted as success"),
        };
        lab.push(LabSummary {
            report_id: report.report_id.clone(),
            digest: report.digest.clone(),
            overall: report.overall,
            failed_checks,
            note,
        });
    }
    if lab.is_empty() {
        tally.conditions.push(ConditionResult {
            gate: "lab".into(),
            condition: "lab report for this exact digest".into(),
            status: ConditionStatus::Unknown,
            detail: "none; no lab result is claimed".into(),
        });
    }
    let (disposition, reasons) = if !tally.rejected.is_empty() {
        (Disposition::Rejected, tally.rejected)
    } else if !tally.blocked_dependency.is_empty() {
        (Disposition::BlockedByDependency, tally.blocked_dependency)
    } else if !tally.blocked_lab.is_empty() {
        (Disposition::BlockedByLab, tally.blocked_lab)
    } else if !tally.unresolved.is_empty() {
        (Disposition::Unresolved, tally.unresolved)
    } else {
        (
            Disposition::Recommended,
            vec!["passed every mandatory gate".to_string()],
        )
    };
    CandidateEvaluation {
        capability_id: id.into(),
        version: Some(definition.version),
        digest: Some(record.definition_digest.clone()),
        state: Some(record.state),
        conditions: tally.conditions,
        lab,
        disposition,
        reasons,
        rank: None,
        ranking_reasons: Vec::new(),
    }
}

/** The ranking key of a recommended candidate
 * Input
    - evaluation: &CandidateEvaluation - evaluation
    - state: &RegistryState - registry
 * Output
    - (bool, usize, u8, u32) - passed lab report, unverified assumptions, risk, version
*/
fn key(evaluation: &CandidateEvaluation, state: &RegistryState) -> (bool, usize, u8, u32) {
    let version = evaluation.version.unwrap_or_default();
    let definition = state
        .record(&evaluation.capability_id, version)
        .map(|record| &record.definition)
        .ok();
    let assumptions = definition.map_or(usize::MAX, |definition| {
        definition
            .applicability
            .assumptions
            .iter()
            .filter(|assumption| assumption.status != ClaimStatus::Verified)
            .count()
    });
    let risk = definition.map_or(u8::MAX, |definition| definition.applicability.risk as u8);
    (
        evaluation
            .lab
            .iter()
            .any(|lab| lab.overall == CheckStatus::Passed),
        assumptions,
        risk,
        version,
    )
}

/** Evaluate every capability the registry lists, then rank the recommended ones
 * Input
    - state: &RegistryState - registry
    - context: &ResolvedContext - context
    - sources: &Sources - sources and current versions
    - lab_reports: &[LabReport] - every lab report
 * Output
    - (Vec<CandidateEvaluation>, bool) evaluations (recommended first, in rank order) and whether
      the candidate limit truncated them
*/
pub(crate) fn evaluate_all(
    state: &RegistryState,
    context: &ResolvedContext,
    sources: &Sources,
    lab_reports: &[LabReport],
) -> (Vec<CandidateEvaluation>, bool) {
    let listed = list(
        state,
        &RegistryQuery {
            include_withdrawn: true,
            ..RegistryQuery::default()
        },
    );
    let truncated = listed.len() > MAX_CANDIDATES;
    let mut evaluations = listed
        .iter()
        .take(MAX_CANDIDATES)
        .map(|summary| evaluate_one(&summary.capability_id, state, context, sources, lab_reports))
        .collect::<Vec<_>>();
    let compare = |a: &CandidateEvaluation, b: &CandidateEvaluation| -> Ordering {
        let (ka, kb) = (key(a, state), key(b, state));
        kb.0.cmp(&ka.0)
            .then(ka.1.cmp(&kb.1))
            .then(ka.2.cmp(&kb.2))
            .then(kb.3.cmp(&ka.3))
            .then(a.capability_id.cmp(&b.capability_id))
    };
    let (mut recommended, others): (Vec<_>, Vec<_>) = evaluations
        .drain(..)
        .partition(|evaluation| evaluation.disposition == Disposition::Recommended);
    recommended.sort_by(compare);
    for (index, evaluation) in recommended.iter_mut().enumerate() {
        let (lab, assumptions, risk, version) = key(evaluation, state);
        evaluation.rank = Some(index + 1);
        evaluation.ranking_reasons = vec![
            format!(
                "ranking rules v{RANKING_VERSION}, applied only after every mandatory gate passed"
            ),
            format!(
                "passed lab report for this digest: {}{}",
                if lab { "yes" } else { "no" },
                if evaluation
                    .lab
                    .iter()
                    .any(|lab| lab.overall != CheckStatus::Passed)
                {
                    " (inconclusive or blocked reports are not counted)"
                } else {
                    ""
                }
            ),
            format!("unverified assumptions: {assumptions}"),
            format!("risk rank: {risk}"),
            format!("version: {version}"),
            "final tie-break: capability id".into(),
        ];
    }
    recommended.extend(others);
    (recommended, truncated)
}
