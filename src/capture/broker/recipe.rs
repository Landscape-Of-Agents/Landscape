// Evidence Recipes: versioned, immutable, deterministic retrieval plans. A recipe lists steps
// (which kind of resource, which operation, which evidence type, parameter templates), branch
// conditions that are explicit predicates over the evidence retrieved so far, the evidence it
// requires (possibly conditionally), stopping rules, and limits. Parameters are either literals or
// exactly one "{variable}" from the resolved scope; an unresolved variable makes the step "not
// queried" instead of widening it. A plan proposed by a model is turned into a recipe and passes
// the same validation before anything runs.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::capture::format::record::{EvidenceRecord, EvidenceType};
use crate::capture::resources::connector::MAX_PAGE;
use crate::capture::resources::registry::{Operation, ResourceType};
use crate::capture::resources::scope::ScopeResolution;
use crate::capture::{CaptureError, CaptureResult, ErrorCode};
use crate::trust::crypto::digest_json;

/** Variables a parameter template may use */
pub(crate) const VARIABLES: &[&str] = &[
    "task",
    "repository",
    "branch",
    "commit",
    "pull_request",
    "deployment",
    "environment",
    "session",
];

/** Most steps a recipe may declare */
const MAX_RECIPE_STEPS: usize = 32;

/** A reference to a recipe
 * Fields
    - id: String - recipe id
    - version: Option<u32> - version, None for the latest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecipeRef {
    pub(crate) id: String,
    pub(crate) version: Option<u32>,
}

/** A branch condition over the evidence retrieved so far and the resolved scope
 * Variants
    - Always - holds
    - Exists - a record of the type was retrieved
    - Missing - no record of the type was retrieved
    - FieldEquals - a record of the type has the field equal to the value (case-insensitive)
    - ScopeKnown - the scope variable is known
    - All - every predicate holds
    - Any - some predicate holds
    - Not - the predicate does not hold
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Predicate {
    Always,
    Exists {
        evidence_type: EvidenceType,
    },
    Missing {
        evidence_type: EvidenceType,
    },
    FieldEquals {
        evidence_type: EvidenceType,
        field: String,
        value: String,
    },
    ScopeKnown {
        variable: String,
    },
    All {
        of: Vec<Predicate>,
    },
    Any {
        of: Vec<Predicate>,
    },
    Not {
        predicate: Box<Predicate>,
    },
}

impl Predicate {
    /** Evaluate the predicate
     * Input
        - evidence: &[&EvidenceRecord] - evidence retrieved so far
        - scope: &ScopeResolution - resolved scope
     * Output
        - bool
    */
    pub(crate) fn holds(&self, evidence: &[&EvidenceRecord], scope: &ScopeResolution) -> bool {
        let of_type = |kind: EvidenceType| {
            evidence
                .iter()
                .filter(move |record| record.evidence_type == kind)
        };
        match self {
            Self::Always => true,
            Self::Exists { evidence_type } => of_type(*evidence_type).next().is_some(),
            Self::Missing { evidence_type } => of_type(*evidence_type).next().is_none(),
            Self::FieldEquals {
                evidence_type,
                field,
                value,
            } => of_type(*evidence_type).any(|record| {
                record
                    .field(field)
                    .is_some_and(|found| found.eq_ignore_ascii_case(value))
            }),
            Self::ScopeKnown { variable } => scope.variable(variable).is_some(),
            Self::All { of } => of.iter().all(|predicate| predicate.holds(evidence, scope)),
            Self::Any { of } => of.iter().any(|predicate| predicate.holds(evidence, scope)),
            Self::Not { predicate } => !predicate.holds(evidence, scope),
        }
    }

    /** Check that scope variables named by the predicate exist, and nesting is bounded
     * Input
        - depth: usize - current nesting depth
     * Output
        - CaptureResult<()>
        - Error naming an unknown variable or excessive nesting
    */
    fn validate(&self, depth: usize) -> CaptureResult<()> {
        if depth > 8 {
            return Err(CaptureError::new(
                ErrorCode::InvalidRecipe,
                "predicates nest too deeply",
            ));
        }
        match self {
            Self::ScopeKnown { variable } if !VARIABLES.contains(&variable.as_str()) => {
                Err(CaptureError::new(
                    ErrorCode::InvalidRecipe,
                    format!("unknown scope variable '{variable}'"),
                ))
            }
            Self::All { of } | Self::Any { of } => of
                .iter()
                .try_for_each(|predicate| predicate.validate(depth + 1)),
            Self::Not { predicate } => predicate.validate(depth + 1),
            _ => Ok(()),
        }
    }
}

/** Evidence a recipe requires
 * Fields
    - evidence_type: EvidenceType - type
    - description: String - what it establishes
    - when: Predicate - condition under which it is required
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RequiredEvidence {
    pub(crate) evidence_type: EvidenceType,
    pub(crate) description: String,
    pub(crate) when: Predicate,
}

/** One retrieval step
 * Fields
    - step_id: String - identifier, unique in the recipe
    - description: String - purpose
    - resource_type: ResourceType - kind of resource to ask
    - operation: Operation - Fetch or Query
    - evidence_type: EvidenceType - evidence wanted
    - record: Option<String> - record template (Fetch)
    - filters: BTreeMap<String, String> - filter templates (Query)
    - limit: usize - most records
    - when: Predicate - branch condition
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecipeStep {
    pub(crate) step_id: String,
    pub(crate) description: String,
    pub(crate) resource_type: ResourceType,
    pub(crate) operation: Operation,
    pub(crate) evidence_type: EvidenceType,
    pub(crate) record: Option<String>,
    pub(crate) filters: BTreeMap<String, String>,
    pub(crate) limit: usize,
    pub(crate) when: Predicate,
}

/** Retrieval limits
 * Fields
    - max_steps: usize - most connector operations
    - max_records: usize - most records ingested
    - max_payload_bytes: usize - most content bytes ingested
    - max_time_ms: u64 - most execution time; a step starting later is not run
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Budget {
    pub(crate) max_steps: usize,
    pub(crate) max_records: usize,
    pub(crate) max_payload_bytes: usize,
    pub(crate) max_time_ms: u64,
}

impl Budget {
    /** The hard ceiling no request or recipe can exceed
     * Input
        - None
     * Output
        - Budget
    */
    pub(crate) fn ceiling() -> Self {
        Self {
            max_steps: 64,
            max_records: 2000,
            max_payload_bytes: 8 * 1024 * 1024,
            max_time_ms: 120_000,
        }
    }

    /** The tighter of two budgets, limit by limit
     * Input
        - other: Budget - other budget
     * Output
        - Budget
    */
    pub(crate) fn tighter(self, other: Self) -> Self {
        Self {
            max_steps: self.max_steps.min(other.max_steps),
            max_records: self.max_records.min(other.max_records),
            max_payload_bytes: self.max_payload_bytes.min(other.max_payload_bytes),
            max_time_ms: self.max_time_ms.min(other.max_time_ms),
        }
    }
}

impl Default for Budget {
    /** A default request budget: the ceiling
     * Input
        - None
     * Output
        - Budget
    */
    fn default() -> Self {
        Self::ceiling()
    }
}

/** When a recipe stops early
 * Fields
    - stop_when_satisfied: bool - stop as soon as every required evidence category is present
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoppingRules {
    pub(crate) stop_when_satisfied: bool,
}

/** A versioned Evidence Recipe
 * Fields
    - recipe_id: String - identifier
    - version: u32 - version (a published version never changes)
    - description: String - purpose
    - investigation_classes: Vec<String> - investigation classes it serves
    - starting_conditions: Vec<String> - scope variables that must be known to start
    - steps: Vec<RecipeStep> - steps in order
    - required: Vec<RequiredEvidence> - required evidence
    - stopping: StoppingRules - stopping rules
    - limits: Budget - the recipe's own limits
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvidenceRecipe {
    pub(crate) recipe_id: String,
    pub(crate) version: u32,
    pub(crate) description: String,
    pub(crate) investigation_classes: Vec<String>,
    pub(crate) starting_conditions: Vec<String>,
    pub(crate) steps: Vec<RecipeStep>,
    pub(crate) required: Vec<RequiredEvidence>,
    pub(crate) stopping: StoppingRules,
    pub(crate) limits: Budget,
}

/** Check a parameter template: a literal without braces, or exactly "{variable}"
 * Input
    - template: &str - template
 * Output
    - CaptureResult<()>
    - Error for malformed templates or unknown variables
*/
fn check_template(template: &str) -> CaptureResult<()> {
    let invalid = |message: String| Err(CaptureError::new(ErrorCode::InvalidRecipe, message));
    match template
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    {
        Some(variable) if VARIABLES.contains(&variable) => Ok(()),
        Some(variable) => invalid(format!("unknown template variable '{variable}'")),
        None if template.contains('{')
            || template.contains('}')
            || template.contains('*')
            || template.is_empty() =>
        {
            invalid(format!(
                "template '{template}' must be a literal or exactly one {{variable}}"
            ))
        }
        None => Ok(()),
    }
}

/** Fill a template from the scope
 * Input
    - template: &str - template
    - scope: &ScopeResolution - resolved scope
 * Output
    - Result<String, String> value, or the name of the unknown variable
*/
pub(crate) fn fill(template: &str, scope: &ScopeResolution) -> Result<String, String> {
    match template
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    {
        Some(variable) => scope.variable(variable).ok_or_else(|| variable.to_string()),
        None => Ok(template.to_string()),
    }
}

impl EvidenceRecipe {
    /** Validate the recipe: identity, unique steps, supported operations, templates, predicates,
     * and limits within the ceiling
     * Input
        - None (uses self)
     * Output
        - CaptureResult<()>
        - Error (InvalidRecipe) naming the problem
    */
    pub(crate) fn validate(&self) -> CaptureResult<()> {
        let invalid = |message: String| Err(CaptureError::new(ErrorCode::InvalidRecipe, message));
        if !crate::capture::resources::registry::valid_identifier(&self.recipe_id) {
            return invalid(format!("invalid recipe id '{}'", self.recipe_id));
        }
        if self.steps.is_empty() || self.steps.len() > MAX_RECIPE_STEPS {
            return invalid(format!("a recipe needs 1 to {MAX_RECIPE_STEPS} steps"));
        }
        let ceiling = Budget::ceiling();
        if self.limits.tighter(ceiling) != self.limits
            || self.limits.max_steps == 0
            || self.limits.max_records == 0
        {
            return invalid("recipe limits must be positive and within the ceiling".into());
        }
        for variable in &self.starting_conditions {
            if !VARIABLES.contains(&variable.as_str()) {
                return invalid(format!("unknown starting condition '{variable}'"));
            }
        }
        let mut ids = BTreeSet::new();
        for step in &self.steps {
            if !ids.insert(step.step_id.as_str())
                || !crate::capture::resources::registry::valid_identifier(&step.step_id)
            {
                return invalid(format!("step id '{}' is invalid or repeated", step.step_id));
            }
            match step.operation {
                Operation::Fetch => match &step.record {
                    Some(record) => check_template(record)?,
                    None => {
                        return invalid(format!(
                            "fetch step '{}' needs a record template",
                            step.step_id
                        ))
                    }
                },
                Operation::Query => {}
                other => {
                    return invalid(format!(
                        "step '{}' uses {other:?}; recipes fetch or query",
                        step.step_id
                    ))
                }
            }
            for value in step.filters.values() {
                check_template(value)?;
            }
            if step.limit == 0 || step.limit > MAX_PAGE {
                return invalid(format!(
                    "step '{}' needs a limit from 1 to {MAX_PAGE}",
                    step.step_id
                ));
            }
            step.when.validate(0)?;
        }
        for required in &self.required {
            required.when.validate(0)?;
        }
        Ok(())
    }

    /** The digest of the recipe's content, recorded with every execution
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json("capture/recipe/v1", &json!(self))[..32].to_string()
    }
}

/** The registered recipes, by id and version; a version, once added, cannot change
 * Fields
    - recipes: BTreeMap<String, BTreeMap<u32, EvidenceRecipe>> - by id, then version
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct RecipeBook {
    recipes: BTreeMap<String, BTreeMap<u32, EvidenceRecipe>>,
}

impl RecipeBook {
    /** Register a recipe version
     * Input
        - recipe: EvidenceRecipe - recipe
     * Output
        - CaptureResult<()>
        - Error when invalid or when the version exists with other content
    */
    pub(crate) fn add(&mut self, recipe: EvidenceRecipe) -> CaptureResult<()> {
        recipe.validate()?;
        let versions = self.recipes.entry(recipe.recipe_id.clone()).or_default();
        if let Some(existing) = versions.get(&recipe.version) {
            if existing.digest() != recipe.digest() {
                return Err(CaptureError::new(
                    ErrorCode::InvalidRecipe,
                    format!(
                        "{} v{} already exists with other content; publish a new version",
                        recipe.recipe_id, recipe.version
                    ),
                ));
            }
        }
        versions.insert(recipe.version, recipe);
        Ok(())
    }

    /** Find a recipe version, or the latest
     * Input
        - reference: &RecipeRef - id and optional version
     * Output
        - Option<&EvidenceRecipe>
    */
    pub(crate) fn get(&self, reference: &RecipeRef) -> Option<&EvidenceRecipe> {
        let versions = self.recipes.get(&reference.id)?;
        match reference.version {
            Some(version) => versions.get(&version),
            None => versions.values().next_back(),
        }
    }

    /** Find the latest recipe serving an investigation class
     * Input
        - class: &str - investigation class
     * Output
        - Option<&EvidenceRecipe>
    */
    pub(crate) fn for_class(&self, class: &str) -> Option<&EvidenceRecipe> {
        self.recipes
            .values()
            .filter_map(|versions| versions.values().next_back())
            .find(|recipe| {
                recipe
                    .investigation_classes
                    .iter()
                    .any(|served| served == class)
            })
    }
}

/** Build a step
 * Input
    - id: &str - step id
    - description: &str - purpose
    - resource_type: ResourceType - resource kind
    - evidence_type: EvidenceType - evidence wanted
    - target: StepTarget - fetch a record, or query with filters
    - when: Predicate - branch condition
 * Output
    - RecipeStep
*/
fn step(
    id: &str,
    description: &str,
    resource_type: ResourceType,
    evidence_type: EvidenceType,
    target: StepTarget,
    when: Predicate,
) -> RecipeStep {
    let (operation, record, filters) = match target {
        StepTarget::Fetch(record) => (Operation::Fetch, Some(record.to_string()), BTreeMap::new()),
        StepTarget::Query(filters) => (
            Operation::Query,
            None,
            filters
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        ),
    };
    RecipeStep {
        step_id: id.into(),
        description: description.into(),
        resource_type,
        operation,
        evidence_type,
        record,
        filters,
        limit: 50,
        when,
    }
}

/** What a built-in step retrieves
 * Variants
    - Fetch(&'static str) - one record by template
    - Query(&'static [(&'static str, &'static str)]) - filters by template
*/
enum StepTarget {
    Fetch(&'static str),
    Query(&'static [(&'static str, &'static str)]),
}

/** Build a required-evidence entry
 * Input
    - evidence_type: EvidenceType - type
    - description: &str - what it establishes
    - when: Predicate - condition
 * Output
    - RequiredEvidence
*/
fn required(evidence_type: EvidenceType, description: &str, when: Predicate) -> RequiredEvidence {
    RequiredEvidence {
        evidence_type,
        description: description.into(),
        when,
    }
}

/** The built-in recipe for investigating a failed deployment of a repository change: task, pull
 * request, code change, CI and tests, agent execution, the deployment, then, only if the
 * deployment failed, its logs and metrics, and, only if a migration was involved, schema evidence
 * Input
    - None
 * Output
    - EvidenceRecipe
*/
pub(crate) fn failed_deployment() -> EvidenceRecipe {
    use EvidenceType as E;
    use ResourceType as R;
    let failed = Predicate::FieldEquals {
        evidence_type: E::Deployment,
        field: "status".into(),
        value: "failed".into(),
    };
    EvidenceRecipe {
        recipe_id: "failed-deployment".into(),
        version: 1,
        description: "Investigate a failed deployment associated with a repository change".into(),
        investigation_classes: vec!["deployment-failure".into()],
        starting_conditions: vec!["task".into(), "deployment".into()],
        steps: vec![
            step(
                "task",
                "The task and its acceptance criteria",
                R::Tracker,
                E::Task,
                StepTarget::Fetch("{task}"),
                Predicate::Always,
            ),
            step(
                "pull-request",
                "The pull request of the change",
                R::Github,
                E::PullRequest,
                StepTarget::Fetch("{pull_request}"),
                Predicate::Always,
            ),
            step(
                "code-change",
                "The commit and its changed files",
                R::Github,
                E::CodeChange,
                StepTarget::Query(&[("commit", "{commit}")]),
                Predicate::Always,
            ),
            step(
                "ci",
                "CI results of the commit",
                R::Ci,
                E::CiRun,
                StepTarget::Query(&[("commit", "{commit}")]),
                Predicate::Always,
            ),
            step(
                "tests",
                "Test reports of the commit",
                R::Ci,
                E::TestReport,
                StepTarget::Query(&[("commit", "{commit}")]),
                Predicate::Always,
            ),
            step(
                "agent",
                "Agent tool calls during the task",
                R::AgentExecution,
                E::ToolCall,
                StepTarget::Query(&[("task", "{task}")]),
                Predicate::Always,
            ),
            step(
                "deployment",
                "The deployment and its outcome",
                R::Deployment,
                E::Deployment,
                StepTarget::Fetch("{deployment}"),
                Predicate::Always,
            ),
            step(
                "deploy-logs",
                "Logs of the failed deployment",
                R::Logs,
                E::RuntimeLog,
                StepTarget::Query(&[("deployment", "{deployment}")]),
                failed.clone(),
            ),
            step(
                "runtime-metrics",
                "Runtime metrics after the failed deployment",
                R::Metrics,
                E::Metric,
                StepTarget::Query(&[("deployment", "{deployment}")]),
                failed.clone(),
            ),
            step(
                "schema",
                "Migration and schema evidence",
                R::InternalApi,
                E::Configuration,
                StepTarget::Query(&[("deployment", "{deployment}")]),
                Predicate::FieldEquals {
                    evidence_type: E::Deployment,
                    field: "migration".into(),
                    value: "true".into(),
                },
            ),
            step(
                "verification",
                "Post-deployment verification",
                R::Deployment,
                E::Verification,
                StepTarget::Query(&[("deployment", "{deployment}")]),
                Predicate::Always,
            ),
            step(
                "policies",
                "AgentScript policies that govern the change",
                R::Crane,
                E::Policy,
                StepTarget::Query(&[("kind", "policy")]),
                Predicate::Always,
            ),
        ],
        required: vec![
            required(E::Task, "the task being investigated", Predicate::Always),
            required(
                E::PullRequest,
                "the change under investigation",
                Predicate::Always,
            ),
            required(E::CodeChange, "what changed", Predicate::Always),
            required(E::CiRun, "whether CI passed", Predicate::Always),
            required(E::TestReport, "whether tests passed", Predicate::Always),
            required(E::Deployment, "the deployment outcome", Predicate::Always),
            required(E::RuntimeLog, "why the deployment failed", failed.clone()),
            required(E::Metric, "the runtime state after the failure", failed),
        ],
        stopping: StoppingRules {
            stop_when_satisfied: true,
        },
        limits: Budget {
            max_steps: 24,
            max_records: 500,
            max_payload_bytes: 2 * 1024 * 1024,
            max_time_ms: 60_000,
        },
    }
}

/** The built-in recipe over this repository's own sources: the commit (Git), the task's agent
 * events (DataSea), zones and flows (Landscape), and policies with the validation report (Crane)
 * Input
    - None
 * Output
    - EvidenceRecipe
*/
pub(crate) fn local_change() -> EvidenceRecipe {
    use EvidenceType as E;
    use ResourceType as R;
    EvidenceRecipe {
        recipe_id: "local-change".into(),
        version: 1,
        description:
            "Collect local evidence about a repository change and the agent work behind it".into(),
        investigation_classes: vec!["local-change".into()],
        starting_conditions: vec!["commit".into()],
        steps: vec![
            step(
                "commit",
                "The commit and its changed files",
                R::Git,
                E::CodeChange,
                StepTarget::Fetch("{commit}"),
                Predicate::Always,
            ),
            step(
                "agent-calls",
                "Agent tool calls of the task",
                R::AgentExecution,
                E::ToolCall,
                StepTarget::Query(&[("task", "{task}")]),
                Predicate::ScopeKnown {
                    variable: "task".into(),
                },
            ),
            step(
                "landscape",
                "Zones and flows",
                R::Landscape,
                E::GraphState,
                StepTarget::Query(&[]),
                Predicate::Always,
            ),
            step(
                "policies",
                "AgentScript policies",
                R::Crane,
                E::Policy,
                StepTarget::Query(&[("kind", "policy")]),
                Predicate::Always,
            ),
            step(
                "validation",
                "Crane validation report",
                R::Crane,
                E::Validation,
                StepTarget::Fetch("validation"),
                Predicate::Always,
            ),
        ],
        required: vec![
            required(E::CodeChange, "the change", Predicate::Always),
            required(
                E::Validation,
                "whether the policies hold",
                Predicate::Always,
            ),
            required(
                E::ToolCall,
                "the agent work behind the change",
                Predicate::ScopeKnown {
                    variable: "task".into(),
                },
            ),
        ],
        stopping: StoppingRules {
            stop_when_satisfied: false,
        },
        limits: Budget {
            max_steps: 16,
            max_records: 400,
            max_payload_bytes: 4 * 1024 * 1024,
            max_time_ms: 60_000,
        },
    }
}

/** Turn a proposed plan (for example from a model) into a recipe that must pass validation
 * Input
    - steps: Vec<RecipeStep> - proposed steps
    - required: Vec<RequiredEvidence> - evidence the plan must find
 * Output
    - CaptureResult<EvidenceRecipe>
    - Error when the plan is invalid
*/
pub(crate) fn from_plan(
    steps: Vec<RecipeStep>,
    required: Vec<RequiredEvidence>,
) -> CaptureResult<EvidenceRecipe> {
    let mut recipe = EvidenceRecipe {
        recipe_id: "plan".into(),
        version: 0,
        description: "A proposed plan, validated before execution".into(),
        investigation_classes: Vec::new(),
        starting_conditions: Vec::new(),
        steps,
        required,
        stopping: StoppingRules {
            stop_when_satisfied: true,
        },
        limits: Budget {
            max_steps: 16,
            max_records: 200,
            max_payload_bytes: 1024 * 1024,
            max_time_ms: 30_000,
        },
    };
    recipe.validate()?;
    recipe.recipe_id = format!("plan-{}", &recipe.digest()[..12]);
    Ok(recipe)
}
