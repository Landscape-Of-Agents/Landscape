// Precondition evaluation: each precondition of a skill (applicability preconditions and required
// state) judged from what is known now. A precondition whose check names a verification claim
// holds only when that claim verifies from its evidence; a check naming anything else is not a
// registered check and stays unresolved; a precondition without a check is a manual fact that holds
// only when a person confirms it, and is reported as declared, never as observed. Missing, stale,
// or contradictory evidence never makes a precondition hold.

use serde::Serialize;

use super::model::{CapabilityDefinition, ClaimStatus, Condition};
use super::refs::Sources;
use super::validate::effective_claim;
use crate::capture::{Actor, ActorKind};

/** Where a precondition's truth comes from
 * Variants
    - Claim - a verification claim of the skill, verified from evidence
    - UnregisteredCheck - a named check Crane does not know
    - Manual - a fact a person confirms
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Source {
    Claim,
    UnregisteredCheck,
    Manual,
}

/** Whether a precondition holds
 * Variants
    - Holds - established from evidence
    - Declared - confirmed by a person (not observed)
    - Unresolved - not established
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Truth {
    Holds,
    Declared,
    Unresolved,
}

/** One evaluated precondition
 * Fields
    - id: String - precondition
    - description: String - what must hold
    - source: Source - where its truth comes from
    - truth: Truth - result
    - detail: String - why
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Evaluation {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) source: Source,
    pub(crate) truth: Truth,
    pub(crate) detail: String,
}

/** The preconditions of a skill, optionally only those a step needs
 * Input
    - definition: &CapabilityDefinition - skill
    - step: Option<&str> - step
 * Output
    - Result<Vec<&Condition>, String>
    - Error for an unknown step
*/
pub(crate) fn conditions<'a>(
    definition: &'a CapabilityDefinition,
    step: Option<&str>,
) -> Result<Vec<&'a Condition>, String> {
    let all = definition
        .applicability
        .preconditions
        .iter()
        .chain(definition.applicability.required_state.iter())
        .collect::<Vec<_>>();
    let Some(step) = step else {
        return Ok(all);
    };
    let wanted = definition
        .execution
        .steps
        .iter()
        .find(|candidate| candidate.step_id == step)
        .ok_or_else(|| format!("{} has no step {step}", definition.capability_id))?;
    Ok(all
        .into_iter()
        .filter(|condition| wanted.preconditions.contains(&condition.id))
        .collect())
}

/** The source of a condition
 * Input
    - definition: &CapabilityDefinition - skill
    - condition: &Condition - precondition
 * Output
    - Source
*/
pub(crate) fn source(definition: &CapabilityDefinition, condition: &Condition) -> Source {
    match &condition.check {
        None => Source::Manual,
        Some(check)
            if definition
                .verification
                .claims
                .iter()
                .any(|claim| &claim.claim_id == check) =>
        {
            Source::Claim
        }
        Some(_) => Source::UnregisteredCheck,
    }
}

/** Evaluate preconditions now
 * Input
    - definition: &CapabilityDefinition - skill
    - step: Option<&str> - only this step's preconditions
    - sources: &Sources - evidence, read through the evaluator's access
    - confirmed: &[String] - manual preconditions a person confirms
    - confirmer: &Actor - who confirms them (an agent's confirmation never counts)
 * Output
    - Result<Vec<Evaluation>, String>
*/
pub(crate) fn evaluate(
    definition: &CapabilityDefinition,
    step: Option<&str>,
    sources: &Sources,
    confirmed: &[String],
    confirmer: &Actor,
) -> Result<Vec<Evaluation>, String> {
    let mut evaluations = Vec::new();
    for condition in conditions(definition, step)? {
        let source = source(definition, condition);
        let (truth, detail) = match source {
            Source::Claim => {
                let check = condition.check.clone().unwrap_or_default();
                let claim = definition
                    .verification
                    .claims
                    .iter()
                    .find(|claim| claim.claim_id == check)
                    .ok_or("claim vanished")?;
                let (status, reasons, relied) = effective_claim(definition, claim, sources);
                if status == ClaimStatus::Verified {
                    (
                        Truth::Holds,
                        format!("claim {check} verifies from {}", relied.join(", ")),
                    )
                } else {
                    (
                        Truth::Unresolved,
                        format!("claim {check} is {status:?}: {}", reasons.join("; ")),
                    )
                }
            }
            Source::UnregisteredCheck => (
                Truth::Unresolved,
                format!(
                    "check '{}' is not a claim of this skill or a registered check",
                    condition.check.clone().unwrap_or_default()
                ),
            ),
            Source::Manual
                if confirmed.contains(&condition.id) && confirmer.kind == ActorKind::Human =>
            {
                (
                    Truth::Declared,
                    format!("confirmed by {} (declared, not observed)", confirmer.id),
                )
            }
            Source::Manual if confirmed.contains(&condition.id) => (
                Truth::Unresolved,
                "an agent's confirmation does not count".into(),
            ),
            Source::Manual => (Truth::Unresolved, "a person must confirm it".into()),
        };
        evaluations.push(Evaluation {
            id: condition.id.clone(),
            description: condition.description.clone(),
            source,
            truth,
            detail,
        });
    }
    Ok(evaluations)
}

/** Validate a skill's preconditions: steps name only known preconditions, ids are unique, and
 * checks name a claim of the skill (manual ones are allowed but reported as declared-only)
 * Input
    - definition: &CapabilityDefinition - skill
 * Output
    - (Vec<String>, Vec<String>) problems that fail validation, and warnings
*/
pub(crate) fn validate(definition: &CapabilityDefinition) -> (Vec<String>, Vec<String>) {
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let all = conditions(definition, None).unwrap_or_default();
    let mut ids = std::collections::BTreeSet::new();
    for condition in &all {
        if !ids.insert(condition.id.as_str()) {
            problems.push(format!("precondition id {} is used twice", condition.id));
        }
        match source(definition, condition) {
            Source::UnregisteredCheck => problems.push(format!("precondition {} names check '{}', which is not a claim of this skill", condition.id, condition.check.clone().unwrap_or_default())),
            Source::Manual => warnings.push(format!("precondition {} has no check: a person must confirm it, and it is recorded as declared", condition.id)),
            Source::Claim => {}
        }
    }
    for step in &definition.execution.steps {
        for id in &step.preconditions {
            if !ids.contains(id.as_str()) {
                problems.push(format!(
                    "step {} requires precondition {id}, which the skill does not declare",
                    step.step_id
                ));
            }
        }
    }
    for id in &definition.execution.preconditions {
        if !ids.contains(id.as_str()) {
            problems.push(format!(
                "the procedure requires precondition {id}, which the skill does not declare"
            ));
        }
    }
    (problems, warnings)
}
