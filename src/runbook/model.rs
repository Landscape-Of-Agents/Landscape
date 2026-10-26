// The runbook schema (RUNBOOK_SCHEMA_VERSION) and its static validation. A runbook composes exact,
// pinned skill versions (capability id, version, digest) into steps with explicit dependencies,
// simple branches (run a step only when an earlier step succeeded, failed, or was blocked),
// bounded retries, approval points, a failure policy, completion requirements, and a budget of
// operations. Each step carries its reviewed binding set, so nothing in a runbook names a command:
// steps run only through the invocation engine's registered, typed operations.
//
// Static validation refuses: invalid or duplicate ids, dependencies on unknown steps, cycles,
// branches on steps that are not dependencies, inputs from undeclared run inputs or from steps that
// are not dependencies, unknown completion steps, unpinned skills, binding sets for another skill
// or version, retries above the ceiling, and a budget that cannot cover every planned operation
// including retries.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::RUNBOOK_SCHEMA_VERSION;
use crate::invocation::plan::BindingSet;
use crate::trust::crypto::digest_json;

/** Most retries a step may declare */
pub(crate) const MAX_RETRIES: u32 = 2;

/** Most operations a runbook budget may grant */
pub(crate) const MAX_BUDGET_OPERATIONS: u64 = 256;

/** Most steps in a runbook */
pub(crate) const MAX_STEPS: usize = 32;

/** Where a step input comes from
 * Variants
    - Literal - a fixed value written in the runbook
    - RunInput - a named input given when the run starts
    - StepOutput - a field of an operation result of an earlier step
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub(crate) enum InputSource {
    Literal {
        value: String,
    },
    RunInput {
        name: String,
    },
    StepOutput {
        step: String,
        operation: String,
        field: String,
    },
}

/** The outcome of an earlier step a branch waits for
 * Variants
    - Succeeded - it succeeded with verified postconditions
    - Failed - it failed
    - Blocked - it was blocked before any effect
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Succeeded,
    Failed,
    Blocked,
}

/** A branch condition
 * Fields
    - step: String - the earlier step
    - outcome: Outcome - the outcome it must have
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct When {
    pub(crate) step: String,
    pub(crate) outcome: Outcome,
}

/** What a failed or blocked step does to the rest of the run
 * Variants
    - Stop - nothing else starts, except steps that branch on this failure
    - Continue - independent steps continue; dependents are skipped unless they branch on it
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailurePolicy {
    #[default]
    Stop,
    Continue,
}

/** One step
 * Fields
    - step_id: String - stable identifier
    - skill_id, skill_version, skill_digest - the exact skill version (capability)
    - bindings: BindingSet - the reviewed binding set of that version
    - inputs: BTreeMap<String, InputSource> - the skill's inputs
    - preconditions: Vec<String> - precondition ids the person starting the run confirms
    - depends_on: Vec<String> - steps that must finish first
    - when: Option<When> - run only when an earlier step had this outcome
    - approval: bool - a person other than the one who started the run approves before it starts
    - max_retries: u32 - extra attempts after a block or a failure with no effect (at most 2)
    - on_failure: FailurePolicy - what a failure does to the rest of the run
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    pub(crate) step_id: String,
    pub(crate) skill_id: String,
    pub(crate) skill_version: u32,
    pub(crate) skill_digest: String,
    pub(crate) bindings: BindingSet,
    #[serde(default)]
    pub(crate) inputs: BTreeMap<String, InputSource>,
    #[serde(default)]
    pub(crate) preconditions: Vec<String>,
    #[serde(default)]
    pub(crate) depends_on: Vec<String>,
    #[serde(default)]
    pub(crate) when: Option<When>,
    #[serde(default)]
    pub(crate) approval: bool,
    #[serde(default)]
    pub(crate) max_retries: u32,
    #[serde(default)]
    pub(crate) on_failure: FailurePolicy,
}

impl Step {
    /** The operations one attempt of the step plans (one per bound step)
     * Input
        - None (uses self)
     * Output
        - u64
    */
    pub(crate) fn cost(&self) -> u64 {
        self.bindings.steps.len() as u64
    }
}

/** A runbook's budget
 * Fields
    - operations: u64 - most operations the whole run may plan, retries included
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Budget {
    pub(crate) operations: u64,
}

/** One immutable runbook version
 * Fields
    - runbook_id: String - stable identity
    - version: u32 - version
    - schema_version: u32 - RUNBOOK_SCHEMA_VERSION
    - name: String - name
    - description: String - what it does, in plain words
    - department: Option<String> - owning department or team
    - owner: Option<String> - owner
    - environment: Option<String> - the environment its skills act in (their applicability is
      checked against it; None leaves applicability unestablished, which blocks)
    - repository: Option<String> - the repository its skills act on
    - inputs: Vec<String> - names of the inputs a run is started with
    - steps: Vec<Step> - steps
    - completion: Vec<String> - steps that must succeed for the run to succeed (empty: every step
      without a branch condition)
    - budget: Budget - budget
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Runbook {
    pub(crate) runbook_id: String,
    pub(crate) version: u32,
    pub(crate) schema_version: u32,
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) department: Option<String>,
    #[serde(default)]
    pub(crate) owner: Option<String>,
    #[serde(default)]
    pub(crate) environment: Option<String>,
    #[serde(default)]
    pub(crate) repository: Option<String>,
    #[serde(default)]
    pub(crate) inputs: Vec<String>,
    pub(crate) steps: Vec<Step>,
    #[serde(default)]
    pub(crate) completion: Vec<String>,
    pub(crate) budget: Budget,
}

impl Runbook {
    /** The digest of the definition, the identity of an immutable version
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json(
            "runbook/definition/v1",
            &serde_json::to_value(self).unwrap_or(Value::Null),
        )[..32]
            .to_string()
    }

    /** Find a step
     * Input
        - step_id: &str - step
     * Output
        - Option<&Step>
    */
    pub(crate) fn step(&self, step_id: &str) -> Option<&Step> {
        self.steps.iter().find(|step| step.step_id == step_id)
    }

    /** The steps that must succeed for the run to succeed
     * Input
        - None (uses self)
     * Output
        - Vec<String>
    */
    pub(crate) fn required(&self) -> Vec<String> {
        if self.completion.is_empty() {
            self.steps
                .iter()
                .filter(|step| step.when.is_none())
                .map(|step| step.step_id.clone())
                .collect()
        } else {
            self.completion.clone()
        }
    }

    /** The steps in an order where every dependency comes first
     * Input
        - None (uses self)
     * Output
        - Result<Vec<String>, String>
        - Error naming a cycle or an unknown dependency
    */
    pub(crate) fn order(&self) -> Result<Vec<String>, String> {
        let ids = self
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut done = Vec::<String>::new();
        let mut remaining = self.steps.iter().collect::<Vec<_>>();
        while !remaining.is_empty() {
            let before = remaining.len();
            let mut next = Vec::new();
            for step in remaining {
                if let Some(unknown) = step
                    .depends_on
                    .iter()
                    .find(|dependency| !ids.contains(dependency.as_str()))
                {
                    return Err(format!(
                        "step {} depends on {unknown}, which is not a step",
                        step.step_id
                    ));
                }
                if step
                    .depends_on
                    .iter()
                    .all(|dependency| done.contains(dependency))
                {
                    done.push(step.step_id.clone());
                } else {
                    next.push(step);
                }
            }
            if next.len() == before {
                return Err(format!(
                    "steps {} depend on each other in a cycle",
                    next.iter()
                        .map(|step| step.step_id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            remaining = next;
        }
        Ok(done)
    }
}

/** Parse a runbook from JSON: the schema version first (another one is refused, never
 * reinterpreted), then fields, types, and enumerations
 * Input
    - value: &Value - JSON definition
 * Output
    - Result<Runbook, String>
*/
pub(crate) fn parse(value: &Value) -> Result<Runbook, String> {
    match value.get("schema_version").and_then(Value::as_u64) {
        Some(version) if version == u64::from(RUNBOOK_SCHEMA_VERSION) => {}
        Some(version) => return Err(format!("runbook schema version {version} is not supported (this Crane reads {RUNBOOK_SCHEMA_VERSION})")),
        None => return Err("schema_version is missing or not an integer".into()),
    }
    serde_json::from_value(value.clone()).map_err(|error| format!("invalid runbook: {error}"))
}

/** All transitive dependencies of a step
 * Input
    - runbook: &Runbook - runbook
    - step: &Step - step
 * Output
    - BTreeSet<String>
*/
pub(crate) fn ancestors(runbook: &Runbook, step: &Step) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut queue = step.depends_on.clone();
    while let Some(id) = queue.pop() {
        if found.insert(id.clone()) {
            if let Some(dependency) = runbook.step(&id) {
                queue.extend(dependency.depends_on.iter().cloned());
            }
        }
    }
    found
}

/** Validate a runbook's structure (without the registry)
 * Input
    - runbook: &Runbook - runbook
 * Output
    - Vec<Value> findings {code, message}; empty when it is valid
*/
pub(crate) fn validate(runbook: &Runbook) -> Vec<Value> {
    let mut findings = Vec::new();
    let mut push =
        |code: &str, message: String| findings.push(json!({"code": code, "message": message}));
    if crate::platform::validate_name("runbook", &runbook.runbook_id).is_err() {
        push(
            "invalid_id",
            format!(
                "runbook id '{}' must use letters, digits, '_' or '-', starting with a letter",
                runbook.runbook_id
            ),
        );
    }
    if runbook.version == 0 {
        push("invalid_version", "the version must be at least 1".into());
    }
    if runbook.name.trim().is_empty() || runbook.description.trim().is_empty() {
        push(
            "missing_description",
            "a runbook needs a name and a plain-language description".into(),
        );
    }
    if runbook.steps.is_empty() || runbook.steps.len() > MAX_STEPS {
        push(
            "step_count",
            format!("a runbook has 1 to {MAX_STEPS} steps"),
        );
    }
    let mut seen = BTreeSet::new();
    for step in &runbook.steps {
        if crate::platform::validate_name("step", &step.step_id).is_err()
            || !seen.insert(step.step_id.clone())
        {
            push(
                "invalid_step_id",
                format!("step id '{}' is invalid or used twice", step.step_id),
            );
        }
    }
    if let Err(problem) = runbook.order() {
        push("dependency_cycle", problem);
    }
    let declared = runbook.inputs.iter().collect::<BTreeSet<_>>();
    let mut total = 0u64;
    for step in &runbook.steps {
        let id = &step.step_id;
        let before = ancestors(runbook, step);
        if step.skill_version == 0 || step.skill_digest.is_empty() {
            push(
                "skill_unpinned",
                format!("step {id} must pin an exact skill version and digest"),
            );
        }
        if step.bindings.capability_id != step.skill_id
            || step.bindings.capability_version != step.skill_version
        {
            push(
                "bindings_mismatch",
                format!(
                    "step {id} binds operations for {} v{}, not {} v{}",
                    step.bindings.capability_id,
                    step.bindings.capability_version,
                    step.skill_id,
                    step.skill_version
                ),
            );
        }
        if step.bindings.steps.is_empty() {
            push("no_operations", format!("step {id} binds no operation"));
        }
        if let Some(when) = &step.when {
            if !step.depends_on.contains(&when.step) {
                push(
                    "branch_not_dependency",
                    format!(
                        "step {id} branches on {}, which must be one of its dependencies",
                        when.step
                    ),
                );
            }
        }
        for (name, source) in &step.inputs {
            match source {
                InputSource::RunInput { name: input } if !declared.contains(input) => push("undeclared_input", format!("step {id} input {name} reads run input {input}, which the runbook does not declare")),
                InputSource::StepOutput { step: from, .. } if !before.contains(from) => push("output_not_dependency", format!("step {id} input {name} reads step {from}, which is not one of its dependencies")),
                _ => {}
            }
        }
        if step.max_retries > MAX_RETRIES {
            push(
                "retries_unbounded",
                format!(
                    "step {id} declares {} retries; at most {MAX_RETRIES}",
                    step.max_retries
                ),
            );
        }
        total += step.cost() * (1 + u64::from(step.max_retries.min(MAX_RETRIES)));
    }
    for id in &runbook.completion {
        if runbook.step(id).is_none() {
            push(
                "unknown_completion_step",
                format!("completion names {id}, which is not a step"),
            );
        }
    }
    if runbook.budget.operations == 0 || runbook.budget.operations > MAX_BUDGET_OPERATIONS {
        push(
            "budget_out_of_range",
            format!("the budget must grant 1 to {MAX_BUDGET_OPERATIONS} operations"),
        );
    } else if total > runbook.budget.operations {
        push(
            "budget_insufficient",
            format!(
                "the steps plan up to {total} operations with retries, more than the budget of {}",
                runbook.budget.operations
            ),
        );
    }
    findings
}
