// Deterministic scenario checks from an approved test plan. A plan is data, not instructions:
// every scenario is one of a closed set of typed checks (an unknown kind is rejected when the plan
// is loaded), and each is evaluated against the candidate's declared contracts with the registry's
// own deterministic functions. Nothing the candidate describes is run:
//
//   applicability     - does the exact matcher decide yes, no, or unknown for a situation?
//   stale_dependency  - with a changed zone, flow, or policy version, is the dependency stale?
//   missing_evidence  - without one evidence type, do the dependent claims become unresolved?
//   negative_case     - does the contract declare this negative or failure case?

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{CheckCategory, CheckStatus, LabCheck};
use crate::capability::model::{CapabilityDefinition, ClaimStatus, DependencyKind};
use crate::capability::query::{
    dependencies, ApplicabilityMatcher, ApplicabilityQuery, ExactMatcher,
};
use crate::capability::refs::{RefOutcome, Sources};
use crate::capability::registry::RegistryState;
use crate::capability::validate::effective_claim;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capture::format::record::EvidenceType;
use crate::capture::resources::registry::valid_identifier;
use crate::capture::{AccessPolicy, Truth};
use crate::trust::crypto::digest_json;

/** An approved scenario check (a closed set; nothing in it is executed)
 * Variants
    - Applicability - a situation and the expected applicability decision
    - StaleDependency - a changed governance version and the expected dependency outcome
    - MissingEvidence - an evidence type withheld and the claims expected to become unresolved
    - NegativeCase - text the contract's negative cases or failure conditions must declare
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ScenarioCheck {
    Applicability {
        task_class: Option<String>,
        environment: Option<String>,
        preconditions: Vec<String>,
        expect: Truth,
    },
    StaleDependency {
        dependency: DependencyKind,
        id: String,
        current_version: String,
        expect: RefOutcome,
    },
    MissingEvidence {
        evidence_type: EvidenceType,
        expect_unresolved: Vec<String>,
    },
    NegativeCase {
        declares: String,
    },
}

/** One scenario
 * Fields
    - scenario_id: String - identifier
    - description: String - what it tests
    - check: ScenarioCheck - the approved check
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Scenario {
    pub(crate) scenario_id: String,
    pub(crate) description: String,
    pub(crate) check: ScenarioCheck,
}

/** A versioned test plan
 * Fields
    - plan_id: String - identifier
    - version: u32 - version
    - description: String - purpose
    - scenarios: Vec<Scenario> - scenarios
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestPlan {
    pub(crate) plan_id: String,
    pub(crate) version: u32,
    pub(crate) description: String,
    pub(crate) scenarios: Vec<Scenario>,
}

impl TestPlan {
    /** Parse and validate a plan: known scenario kinds only, valid and unique ids
     * Input
        - text: &str - JSON plan
     * Output
        - CapabilityResult<TestPlan>
        - Error (InvalidSchema) for malformed plans or unknown scenario kinds
    */
    pub(crate) fn parse(text: &str) -> CapabilityResult<Self> {
        let plan: Self = serde_json::from_str(text).map_err(|error| {
            CapabilityError::new(ErrorCode::InvalidSchema, format!("test plan: {error}"))
        })?;
        let mut ids = BTreeSet::new();
        let valid = valid_identifier(&plan.plan_id)
            && plan.version > 0
            && !plan.scenarios.is_empty()
            && plan.scenarios.iter().all(|scenario| {
                valid_identifier(&scenario.scenario_id) && ids.insert(scenario.scenario_id.clone())
            });
        if valid {
            Ok(plan)
        } else {
            Err(CapabilityError::new(ErrorCode::InvalidSchema, "a test plan needs a valid id, a positive version, and scenarios with valid, unique ids"))
        }
    }

    /** The plan's content digest
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json("lab/test-plan/v1", &json!(self))[..32].to_string()
    }
}

/** Evaluate every scenario of a plan
 * Input
    - plan: &TestPlan - plan
    - definition: &CapabilityDefinition - candidate
    - state: &RegistryState - registry
    - sources: &Sources - canonical sources and the requester's access
 * Output
    - Vec<LabCheck>
*/
pub(crate) fn run(
    plan: &TestPlan,
    definition: &CapabilityDefinition,
    state: &RegistryState,
    sources: &Sources,
) -> Vec<LabCheck> {
    plan.scenarios
        .iter()
        .map(|scenario| evaluate(scenario, definition, state, sources))
        .collect()
}

/** Evaluate one scenario
 * Input
    - scenario: &Scenario - scenario
    - definition: &CapabilityDefinition - candidate
    - state: &RegistryState - registry
    - sources: &Sources - canonical sources
 * Output
    - LabCheck
*/
fn evaluate(
    scenario: &Scenario,
    definition: &CapabilityDefinition,
    state: &RegistryState,
    sources: &Sources,
) -> LabCheck {
    let id = format!("scenario:{}", scenario.scenario_id);
    let check = |status: CheckStatus, detail: String| {
        LabCheck::new(
            id.clone(),
            CheckCategory::Scenario,
            status,
            format!("{}: {detail}", scenario.description),
        )
    };
    match &scenario.check {
        ScenarioCheck::Applicability {
            task_class,
            environment,
            preconditions,
            expect,
        } => {
            let situation = ApplicabilityQuery {
                task_class: task_class.clone(),
                environment: environment.clone(),
                satisfied_preconditions: preconditions.clone(),
                entities: Vec::new(),
            };
            let decision = ExactMatcher.evaluate(definition, &situation);
            if decision.applicable == *expect {
                check(
                    CheckStatus::Passed,
                    format!("decided {:?} as expected", decision.applicable),
                )
            } else {
                check(
                    CheckStatus::Failed,
                    format!("decided {:?}, expected {expect:?}", decision.applicable),
                )
                .with_counterexample(json!({"situation": situation, "decision": decision}))
            }
        }
        ScenarioCheck::StaleDependency {
            dependency,
            id: target,
            current_version,
            expect,
        } => {
            let mut current = sources.current.clone();
            let table = match dependency {
                DependencyKind::Zone => &mut current.zones,
                DependencyKind::Flow => &mut current.flows,
                DependencyKind::Policy => &mut current.policies,
                other => {
                    return check(
                        CheckStatus::NotRun,
                        format!("{other:?} dependencies have no current version to change"),
                    )
                }
            };
            table.insert(target.clone(), current_version.clone());
            let changed = Sources {
                current: &current,
                ..*sources
            };
            let statuses = dependencies(
                state,
                &definition.capability_id,
                definition.version,
                &changed,
            )
            .unwrap_or_default();
            match statuses.iter().find(|status| status.dependency.kind == *dependency && &status.dependency.target_id == target) {
                None => check(CheckStatus::NotRun, format!("the candidate does not depend on {dependency:?} {target}")),
                Some(status) if status.outcome == *expect => check(CheckStatus::Passed, format!("with {target} at {current_version} the dependency is {:?} as expected", status.outcome)),
                Some(status) => check(CheckStatus::Failed, format!("with {target} at {current_version} the dependency is {:?}, expected {expect:?}", status.outcome)).with_counterexample(json!({"dependency": status.dependency, "outcome": status.outcome})),
            }
        }
        ScenarioCheck::MissingEvidence {
            evidence_type,
            expect_unresolved,
        } => {
            let all = [
                EvidenceType::Task,
                EvidenceType::PullRequest,
                EvidenceType::CodeChange,
                EvidenceType::CiRun,
                EvidenceType::TestReport,
                EvidenceType::Deployment,
                EvidenceType::Verification,
                EvidenceType::RuntimeLog,
                EvidenceType::Metric,
                EvidenceType::AgentSession,
                EvidenceType::ToolCall,
                EvidenceType::AgentEvent,
                EvidenceType::GraphState,
                EvidenceType::Policy,
                EvidenceType::Validation,
                EvidenceType::Configuration,
            ];
            let allowed = sources
                .reader
                .evidence_types
                .clone()
                .unwrap_or_else(|| all.into_iter().collect());
            let reader = AccessPolicy {
                evidence_types: Some(
                    allowed
                        .into_iter()
                        .filter(|kind| kind != evidence_type)
                        .collect(),
                ),
                ..sources.reader.clone()
            };
            let withheld = Sources {
                reader: &reader,
                ..*sources
            };
            let mut still_verified = Vec::new();
            let mut missing_claims = Vec::new();
            for claim_id in expect_unresolved {
                match definition
                    .verification
                    .claims
                    .iter()
                    .find(|claim| &claim.claim_id == claim_id)
                {
                    None => missing_claims.push(claim_id.clone()),
                    Some(claim)
                        if effective_claim(definition, claim, &withheld).0
                            == ClaimStatus::Verified =>
                    {
                        still_verified.push(claim_id.clone())
                    }
                    Some(_) => {}
                }
            }
            if !missing_claims.is_empty() {
                check(
                    CheckStatus::NotRun,
                    format!("the candidate has no claims {}", missing_claims.join(", ")),
                )
            } else if still_verified.is_empty() {
                check(
                    CheckStatus::Passed,
                    format!(
                        "without {evidence_type:?} evidence, {} become unresolved",
                        expect_unresolved.join(", ")
                    ),
                )
            } else {
                check(
                    CheckStatus::Failed,
                    format!(
                        "without {evidence_type:?} evidence, {} still verify",
                        still_verified.join(", ")
                    ),
                )
                .with_counterexample(
                    json!({"withheld": evidence_type, "still_verified": still_verified}),
                )
            }
        }
        ScenarioCheck::NegativeCase { declares } => {
            let wanted = declares.to_ascii_lowercase();
            let declared = definition
                .verification
                .negative_cases
                .iter()
                .chain(&definition.execution.failure_conditions)
                .any(|case| case.to_ascii_lowercase().contains(&wanted));
            if declared {
                check(
                    CheckStatus::Passed,
                    format!("the contract declares \"{declares}\""),
                )
            } else {
                check(CheckStatus::Failed, format!("the contract does not declare \"{declares}\"")).with_counterexample(json!({"negative_cases": definition.verification.negative_cases, "failure_conditions": definition.execution.failure_conditions}))
            }
        }
    }
}
