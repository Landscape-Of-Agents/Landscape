// Preflight: every execution-time condition is re-evaluated against the current state, before any
// effect and again immediately before execution starts. Nothing is taken from an earlier decision:
// a discovery report is advisory and only its differences are recorded. A failed check blocks; an
// unresolved mandatory check blocks too (fail closed); unresolved advisory checks are recorded.
//
//   identity      - the exact version exists and its stored and recomputed digests equal the request
//   eligibility   - published and currently eligible (Component 2's rule)
//   staleness     - not marked stale, and nothing it was validated against changed (checked live)
//   lab           - lab reports for this exact digest: failed blocks; inconclusive, blocked, not run,
//                   or missing are recorded (and, for medium or higher risk, require approval)
//   dependencies  - every required dependency resolves now (Landscape and Crane versions); an
//                   unverifiable required dependency blocks
//   applicability - Component 2's exact matcher says yes (unknown blocks)
//   preconditions - every declared precondition is confirmed by the requester
//   policy        - no operation touches Crane policy; an agent may only run read-only plans
//   evidence      - every required claim still verifies from its evidence (Component 2's rule)
//   adapters      - every operation's adapter is healthy; transactions are available when required

use serde::{Deserialize, Serialize};

use super::adapter::{AdapterRegistry, TransactionRuntime};
use super::lifecycle::EffectClass;
use super::plan::InvocationPlan;
use crate::capability::model::{CapabilityDefinition, ClaimStatus, RiskLevel};
use crate::capability::query::{
    dependencies, eligible, ApplicabilityMatcher, ApplicabilityQuery, ExactMatcher,
};
use crate::capability::refs::{RefOutcome, Sources};
use crate::capability::registry::{stale_reasons, RegistryState};
use crate::capability::validate::effective_claim;
use crate::capability_lab::report::LabReport;
use crate::capability_lab::CheckStatus;
use crate::capture::{Actor, ActorKind, Truth};
use crate::discovery::report::DiscoveryReport;

/** The status of a preflight check
 * Variants
    - Passed - established to hold
    - Failed - established not to hold
    - Unresolved - not established
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PreflightStatus {
    Passed,
    Failed,
    Unresolved,
}

/** One preflight check
 * Fields
    - check_id: String - check
    - status: PreflightStatus - status
    - mandatory: bool - whether an unresolved result blocks
    - detail: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PreflightCheck {
    pub(crate) check_id: String,
    pub(crate) status: PreflightStatus,
    pub(crate) mandatory: bool,
    pub(crate) detail: String,
}

impl PreflightCheck {
    /** Report whether the check blocks execution
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn blocks(&self) -> bool {
        self.status == PreflightStatus::Failed
            || (self.mandatory && self.status == PreflightStatus::Unresolved)
    }
}

/** Build a check
 * Input
    - id: &str - check id
    - status: PreflightStatus - status
    - mandatory: bool - mandatory
    - detail: impl Into<String> - explanation
 * Output
    - PreflightCheck
*/
fn check(
    id: &str,
    status: PreflightStatus,
    mandatory: bool,
    detail: impl Into<String>,
) -> PreflightCheck {
    PreflightCheck {
        check_id: id.into(),
        status,
        mandatory,
        detail: detail.into(),
    }
}

/** What a preflight run needs
 * Fields
    - state: &'a RegistryState - registry
    - sources: &'a Sources<'a> - Work Capture records, current versions, the requester's access
    - lab_reports: &'a [LabReport] - lab reports
    - discovery_reports: &'a [DiscoveryReport] - discovery reports
    - adapters: &'a AdapterRegistry - adapters
    - runtime: &'a dyn TransactionRuntime - transaction runtime
*/
pub(crate) struct PreflightInputs<'a> {
    pub(crate) state: &'a RegistryState,
    pub(crate) sources: &'a Sources<'a>,
    pub(crate) lab_reports: &'a [LabReport],
    pub(crate) discovery_reports: &'a [DiscoveryReport],
    pub(crate) adapters: &'a AdapterRegistry,
    pub(crate) runtime: &'a dyn TransactionRuntime,
}

/** What the request names
 * Fields
    - capability_id, version, digest - exact capability
    - environment: Option<String> - environment
    - confirmed_preconditions: &'a [String] - confirmed preconditions
    - discovery_report: Option<&'a str> - discovery report id
*/
pub(crate) struct Target<'a> {
    pub(crate) capability_id: &'a str,
    pub(crate) version: u32,
    pub(crate) digest: &'a str,
    pub(crate) environment: Option<String>,
    pub(crate) confirmed_preconditions: &'a [String],
    pub(crate) discovery_report: Option<&'a str>,
}

/** The identity check alone, run before any plan is built
 * Input
    - inputs: &PreflightInputs - inputs
    - target: &Target - exact version
 * Output
    - (PreflightCheck, Option<CapabilityDefinition>) - the check and the definition when it passed
*/
pub(crate) fn identity(
    inputs: &PreflightInputs,
    target: &Target,
) -> (PreflightCheck, Option<CapabilityDefinition>) {
    let Ok(record) = inputs.state.record(target.capability_id, target.version) else {
        return (
            check(
                "identity",
                PreflightStatus::Failed,
                true,
                format!(
                    "{} v{} does not exist",
                    target.capability_id, target.version
                ),
            ),
            None,
        );
    };
    let recomputed = record.definition.digest();
    if record.definition_digest != target.digest || recomputed != target.digest {
        return (
            check(
                "identity",
                PreflightStatus::Failed,
                true,
                format!(
                    "requested digest {}, stored {}, recomputed {recomputed}",
                    target.digest, record.definition_digest
                ),
            ),
            None,
        );
    }
    (
        check(
            "identity",
            PreflightStatus::Passed,
            true,
            format!(
                "{} v{} digest {recomputed}",
                target.capability_id, target.version
            ),
        ),
        Some(record.definition.clone()),
    )
}

/** Run every preflight check after identity
 * Input
    - inputs: &PreflightInputs - inputs
    - target: &Target - exact version
    - definition: &CapabilityDefinition - its definition
    - plan: &InvocationPlan - the plan
    - requester: &Actor - requester
 * Output
    - Vec<PreflightCheck>
*/
pub(crate) fn run(
    inputs: &PreflightInputs,
    target: &Target,
    definition: &CapabilityDefinition,
    plan: &InvocationPlan,
    requester: &Actor,
) -> Vec<PreflightCheck> {
    let mut checks = Vec::new();
    let Ok(record) = inputs.state.record(target.capability_id, target.version) else {
        return vec![check(
            "eligibility",
            PreflightStatus::Failed,
            true,
            "the version disappeared",
        )];
    };
    checks.push(if eligible(record) {
        check(
            "eligibility",
            PreflightStatus::Passed,
            true,
            "published, not revoked or deprecated, not marked stale",
        )
    } else {
        check(
            "eligibility",
            PreflightStatus::Failed,
            true,
            format!(
                "the version is {:?}{}",
                record.state,
                if record.stale.is_empty() {
                    ""
                } else {
                    " and marked stale"
                }
            ),
        )
    });
    let live = inputs
        .state
        .latest_run(record)
        .map(|run| stale_reasons(run, inputs.state, inputs.sources))
        .unwrap_or_else(|| vec!["no validation run exists".into()]);
    checks.push(if live.is_empty() {
        check(
            "staleness",
            PreflightStatus::Passed,
            true,
            "nothing it was validated against changed",
        )
    } else {
        check("staleness", PreflightStatus::Failed, true, live.join("; "))
    });
    let reports = inputs
        .lab_reports
        .iter()
        .filter(|report| {
            report.capability_id == target.capability_id
                && report.version == target.version
                && report.candidate_digest == target.digest
        })
        .collect::<Vec<_>>();
    let risky = definition.applicability.risk >= RiskLevel::Medium;
    checks.push(
        if let Some(failed) = reports
            .iter()
            .find(|report| report.overall == CheckStatus::Failed)
        {
            check(
                "lab",
                PreflightStatus::Failed,
                true,
                format!("lab report {} for this digest failed", failed.report_id),
            )
        } else if reports
            .iter()
            .any(|report| report.overall == CheckStatus::Passed)
        {
            check(
                "lab",
                PreflightStatus::Passed,
                false,
                "a lab report for this digest passed every tested condition",
            )
        } else if reports.is_empty() {
            check(
                "lab",
                PreflightStatus::Unresolved,
                false,
                format!(
                    "no lab report for this digest{}",
                    if risky { "; approval is required" } else { "" }
                ),
            )
        } else {
            check(
                "lab",
                PreflightStatus::Unresolved,
                false,
                format!(
                    "lab reports for this digest are {:?}; not counted as passed{}",
                    reports
                        .iter()
                        .map(|report| report.overall)
                        .collect::<Vec<_>>(),
                    if risky { "; approval is required" } else { "" }
                ),
            )
        },
    );
    match dependencies(
        inputs.state,
        target.capability_id,
        target.version,
        inputs.sources,
    ) {
        Ok(statuses) => {
            let failed = statuses
                .iter()
                .filter(|status| {
                    status.dependency.required
                        && !matches!(
                            status.outcome,
                            RefOutcome::Resolved | RefOutcome::Unverifiable
                        )
                })
                .map(|status| {
                    format!(
                        "{:?} {} is {:?}",
                        status.dependency.kind, status.dependency.target_id, status.outcome
                    )
                })
                .collect::<Vec<_>>();
            let unverifiable = statuses
                .iter()
                .filter(|status| {
                    status.dependency.required && status.outcome == RefOutcome::Unverifiable
                })
                .map(|status| {
                    format!(
                        "{:?} {}",
                        status.dependency.kind, status.dependency.target_id
                    )
                })
                .collect::<Vec<_>>();
            checks.push(if !failed.is_empty() {
                check(
                    "dependencies",
                    PreflightStatus::Failed,
                    true,
                    failed.join("; "),
                )
            } else if !unverifiable.is_empty() {
                check(
                    "dependencies",
                    PreflightStatus::Unresolved,
                    true,
                    format!("required but unverifiable: {}", unverifiable.join(", ")),
                )
            } else {
                check(
                    "dependencies",
                    PreflightStatus::Passed,
                    true,
                    "every required dependency resolves to its pinned version",
                )
            });
        }
        Err(error) => checks.push(check(
            "dependencies",
            PreflightStatus::Failed,
            true,
            error.to_string(),
        )),
    }
    let situation = ApplicabilityQuery {
        task_class: definition.applicability.task_classes.first().cloned(),
        environment: target.environment.clone(),
        satisfied_preconditions: target.confirmed_preconditions.to_vec(),
        entities: Vec::new(),
    };
    let decision = ExactMatcher.evaluate(definition, &situation);
    checks.push(match decision.applicable {
        Truth::Yes => check(
            "applicability",
            PreflightStatus::Passed,
            true,
            "applies exactly",
        ),
        Truth::No => check(
            "applicability",
            PreflightStatus::Failed,
            true,
            decision.reasons.join("; "),
        ),
        Truth::Unknown => check(
            "applicability",
            PreflightStatus::Unresolved,
            true,
            decision.reasons.join("; "),
        ),
    });
    let missing = definition
        .applicability
        .preconditions
        .iter()
        .chain(&definition.applicability.required_state)
        .filter(|condition| !target.confirmed_preconditions.contains(&condition.id))
        .map(|condition| condition.id.clone())
        .collect::<Vec<_>>();
    checks.push(if missing.is_empty() {
        check(
            "preconditions",
            PreflightStatus::Passed,
            true,
            "every precondition is confirmed",
        )
    } else {
        check(
            "preconditions",
            PreflightStatus::Unresolved,
            true,
            format!("not confirmed: {}", missing.join(", ")),
        )
    });
    let mutating = plan
        .operations
        .iter()
        .any(|operation| operation.effect != EffectClass::ReadOnly);
    checks.push(if requester.kind == ActorKind::Agent && mutating {
        check("policy", PreflightStatus::Failed, true, "an agent may only run read-only plans")
    } else {
        check("policy", PreflightStatus::Passed, true, "no operation touches Crane policy; policy dependencies are checked with the other dependencies")
    });
    let unverified = definition
        .verification
        .claims
        .iter()
        .filter(|claim| {
            claim.required
                && effective_claim(definition, claim, inputs.sources).0 != ClaimStatus::Verified
        })
        .map(|claim| claim.claim_id.clone())
        .collect::<Vec<_>>();
    checks.push(if unverified.is_empty() {
        check(
            "evidence",
            PreflightStatus::Passed,
            true,
            "every required claim still verifies from its evidence",
        )
    } else {
        check(
            "evidence",
            PreflightStatus::Failed,
            true,
            format!(
                "required claims no longer verify: {}",
                unverified.join(", ")
            ),
        )
    });
    let mut adapter_problems = Vec::new();
    for operation in &plan.operations {
        match inputs.adapters.adapter(&operation.adapter_id) {
            None => adapter_problems.push(format!("{} is not registered", operation.adapter_id)),
            Some(adapter) if !adapter.healthy() => {
                adapter_problems.push(format!("{} is unavailable", operation.adapter_id))
            }
            _ => {}
        }
        if operation.requires_transaction && !inputs.runtime.available() {
            adapter_problems.push(format!(
                "{} requires a durable transaction and no transaction runtime is available",
                operation.step_id
            ));
        }
    }
    checks.push(if adapter_problems.is_empty() {
        check(
            "adapters",
            PreflightStatus::Passed,
            true,
            "every adapter is available",
        )
    } else {
        check(
            "adapters",
            PreflightStatus::Failed,
            true,
            adapter_problems.join("; "),
        )
    });
    if let Some(report_id) = target.discovery_report {
        checks.push(
            match inputs
                .discovery_reports
                .iter()
                .find(|report| report.report_id == report_id)
            {
                None => check(
                    "discovery",
                    PreflightStatus::Unresolved,
                    false,
                    "the discovery report was not found; it is advisory only",
                ),
                Some(report) if !report.verify_digest() => check(
                    "discovery",
                    PreflightStatus::Unresolved,
                    false,
                    "the discovery report does not match its digest; ignored",
                ),
                Some(report) => match report
                    .recommendations
                    .iter()
                    .find(|recommendation| recommendation.capability_id == target.capability_id)
                {
                    Some(recommendation)
                        if recommendation.version == target.version
                            && recommendation.digest == target.digest =>
                    {
                        check(
                            "discovery",
                            PreflightStatus::Passed,
                            false,
                            format!(
                                "recommended at rank {} then; re-evaluated now",
                                recommendation.rank
                            ),
                        )
                    }
                    Some(recommendation) => check(
                        "discovery",
                        PreflightStatus::Unresolved,
                        false,
                        format!(
                            "discovery recommended v{} ({}), not this version",
                            recommendation.version, recommendation.digest
                        ),
                    ),
                    None => check(
                        "discovery",
                        PreflightStatus::Unresolved,
                        false,
                        format!(
                            "discovery did not recommend it then ({:?}); re-evaluated now",
                            report.outcome
                        ),
                    ),
                },
            },
        );
    }
    checks
}
