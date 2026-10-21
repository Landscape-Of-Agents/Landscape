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

use crate::capture::format::record::{EvidenceRecord, EvidenceType, Payload};
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
pub(crate) const MAX_RECIPE_STEPS: usize = 32;

/** Prefix of the identifier of an exploration (a recipe built from the registry for a question no
 * recipe serves) */
pub(crate) const EXPLORATION_PREFIX: &str = "explore-";

/** Most records one exploration step retrieves: an exploration samples what a source holds */
pub(crate) const EXPLORATION_STEP_LIMIT: usize = 10;

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
    - FieldEquals - a record of the type has the field equal to the value (case-insensitive); with a
      subject (a template such as "{deployment}"), only records about that subject count, so one
      failed deployment does not trigger steps for another
    - AnyFileMatches - a retrieved code change touches a file matching the glob (for example
      any file under migrations/), so a branch can follow what a change observably did, not only what a
      record declares
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
        // Omitted from the serialized form when absent, so recipes published before subjects
        // existed keep their digests
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
    },
    AnyFileMatches {
        glob: String,
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
                subject,
            } => {
                let subject = match subject {
                    Some(template) => match fill(template, scope) {
                        Ok(value) => Some(value),
                        // An unknown subject matches nothing rather than everything
                        Err(_) => return false,
                    },
                    None => None,
                };
                of_type(*evidence_type).any(|record| {
                    subject
                        .as_deref()
                        .is_none_or(|subject| about(record, subject))
                        && record
                            .field(field)
                            .is_some_and(|found| found.eq_ignore_ascii_case(value))
                })
            }
            Self::AnyFileMatches { glob } => {
                of_type(EvidenceType::CodeChange).any(|record| match &record.payload {
                    Some(Payload::Code { files, .. }) => files
                        .iter()
                        .any(|file| crate::platform::glob::matches(glob, &file.path)),
                    _ => false,
                })
            }
            Self::ScopeKnown { variable } => scope.variable(variable).is_some(),
            Self::All { of } => of.iter().all(|predicate| predicate.holds(evidence, scope)),
            Self::Any { of } => of.iter().any(|predicate| predicate.holds(evidence, scope)),
            Self::Not { predicate } => !predicate.holds(evidence, scope),
        }
    }

    /** Report whether the predicate cannot be judged on the evidence retrieved: it compares a field
     * of, or files touched by, evidence of which no record was retrieved. Such a condition is not
     * false, it is unknown (presence conditions, Exists and Missing, are always judgeable)
     * Input
        - evidence: &[&EvidenceRecord] - evidence retrieved so far
     * Output
        - bool
    */
    pub(crate) fn unevaluable(&self, evidence: &[&EvidenceRecord]) -> bool {
        let none_of =
            |kind: EvidenceType| !evidence.iter().any(|record| record.evidence_type == kind);
        match self {
            Self::FieldEquals { evidence_type, .. } => none_of(*evidence_type),
            Self::AnyFileMatches { .. } => none_of(EvidenceType::CodeChange),
            Self::All { of } | Self::Any { of } => {
                of.iter().any(|predicate| predicate.unevaluable(evidence))
            }
            Self::Not { predicate } => predicate.unevaluable(evidence),
            _ => false,
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
            Self::FieldEquals { evidence_type, .. }
                if matches!(
                    evidence_type.payload_kind(),
                    crate::capture::format::record::PayloadKind::Text
                        | crate::capture::format::record::PayloadKind::Measurement
                ) =>
            {
                Err(CaptureError::new(
                    ErrorCode::InvalidRecipe,
                    format!("{evidence_type:?} evidence has no fields to compare; a field condition needs structured, code, or graph evidence"),
                ))
            }
            Self::FieldEquals {
                subject: Some(template),
                ..
            } => check_template(template),
            Self::AnyFileMatches { glob } if glob.is_empty() || glob.len() > 256 => {
                Err(CaptureError::new(
                    ErrorCode::InvalidRecipe,
                    "a file glob needs 1 to 256 characters",
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

/** Report whether a record is about a subject: it is the subject's own record, or it refers to the
 * subject as its deployment, commit, pull request, task, or session
 * Input
    - record: &EvidenceRecord - record
    - subject: &str - subject identifier
 * Output
    - bool
*/
fn about(record: &EvidenceRecord, subject: &str) -> bool {
    let refs = &record.refs;
    record.source_ref.record_id == subject
        || [
            &refs.deployment,
            &refs.commit,
            &refs.pull_request,
            &refs.task,
            &refs.session,
        ]
        .iter()
        .any(|reference| reference.as_deref() == Some(subject))
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
    - mandatory: bool - run even after the stopping rule has found the required evidence complete
      (a verification that could contradict what was found); omitted from the serialized form when
      false, so recipes published before keep their digests
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) mandatory: bool,
}

/** Report whether a flag is false (a serialization helper: false flags are omitted)
 * Input
    - value: &bool - flag
 * Output
    - bool
*/
fn is_false(value: &bool) -> bool {
    !*value
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

/**
Check a parameter template: a literal without braces, or exactly "{variable}"
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
    /**
    Validate the recipe: identity, unique steps, supported operations, templates, predicates, and limits within the ceiling
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

    /** Report whether the recipe is an exploration (built from the registry, not registered)
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn is_exploration(&self) -> bool {
        self.recipe_id.starts_with(EXPLORATION_PREFIX)
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
        // One question class, one recipe: versions of a recipe share its classes, but two recipes
        // serving the same class would make the choice depend on their names
        if let Some((other, class)) = self.recipes.iter().find_map(|(id, versions)| {
            let latest = versions.values().next_back()?;
            (id != &recipe.recipe_id)
                .then(|| {
                    recipe
                        .investigation_classes
                        .iter()
                        .find(|class| latest.investigation_classes.contains(class))
                        .map(|class| (id.clone(), class.clone()))
                })
                .flatten()
        }) {
            return Err(CaptureError::new(
                ErrorCode::InvalidRecipe,
                format!(
                    "{} cannot serve '{class}': {other} already does",
                    recipe.recipe_id
                ),
            ));
        }
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

    /** Every recipe version in the book
     * Input
        - None (uses self)
     * Output
        - impl Iterator<Item = &EvidenceRecipe>
    */
    pub(crate) fn all(&self) -> impl Iterator<Item = &EvidenceRecipe> {
        self.recipes.values().flat_map(|versions| versions.values())
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
        mandatory: false,
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
        subject: None,
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
                    subject: None,
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

/** Version 2 of the failed-deployment recipe. Version 1 stays published unchanged (audits that
 * name it keep meaning it). Version 2 differs only in its conditions: logs, metrics, and their
 * requirements follow the deployment under investigation, not any failed deployment retrieved;
 * and schema evidence is retrieved when the deployment declares a migration OR the change itself
 * touches a migrations directory, so a deployment that says "no migration" while its commit adds
 * one cannot hide the schema evidence
 * Input
    - None
 * Output
    - EvidenceRecipe
*/
pub(crate) fn failed_deployment_v2() -> EvidenceRecipe {
    use EvidenceType as E;
    let mut recipe = failed_deployment();
    recipe.version = 2;
    recipe.description =
        "Investigate a failed deployment associated with a repository change (conditions bound to the deployment, migrations detected from the change)".into();
    let failed = Predicate::FieldEquals {
        evidence_type: E::Deployment,
        field: "status".into(),
        value: "failed".into(),
        subject: Some("{deployment}".into()),
    };
    let migration = Predicate::Any {
        of: vec![
            Predicate::FieldEquals {
                evidence_type: E::Deployment,
                field: "migration".into(),
                value: "true".into(),
                subject: Some("{deployment}".into()),
            },
            Predicate::AnyFileMatches {
                glob: "migrations/**".into(),
            },
        ],
    };
    for step in &mut recipe.steps {
        match step.step_id.as_str() {
            "deploy-logs" | "runtime-metrics" => step.when = failed.clone(),
            "schema" => step.when = migration.clone(),
            _ => {}
        }
    }
    for required in &mut recipe.required {
        if matches!(required.evidence_type, E::RuntimeLog | E::Metric) {
            required.when = failed.clone();
        }
    }
    recipe
}

/** The built-in discovery recipe: which deployments happened in an environment (within the
 * investigation's time window). It needs only the environment, so an imprecise question ("prod
 * broke yesterday") can start here; the deployments it finds become part of the scope
 * Input
    - None
 * Output
    - EvidenceRecipe
*/
pub(crate) fn deployment_discovery() -> EvidenceRecipe {
    use EvidenceType as E;
    use ResourceType as R;
    EvidenceRecipe {
        recipe_id: "deployment-discovery".into(),
        version: 1,
        description: "Find the deployments of an environment within a time window".into(),
        investigation_classes: vec!["deployment-discovery".into()],
        starting_conditions: vec!["environment".into()],
        steps: vec![
            step(
                "deployments",
                "Deployments of the environment",
                R::Deployment,
                E::Deployment,
                StepTarget::Query(&[("environment", "{environment}")]),
                Predicate::Always,
            ),
            step(
                "verification",
                "Post-deployment checks of the first deployment found",
                R::Deployment,
                E::Verification,
                StepTarget::Query(&[("deployment", "{deployment}")]),
                Predicate::ScopeKnown {
                    variable: "deployment".into(),
                },
            ),
        ],
        required: vec![required(
            E::Deployment,
            "the deployments of the environment",
            Predicate::Always,
        )],
        stopping: StoppingRules {
            stop_when_satisfied: false,
        },
        limits: Budget {
            max_steps: 8,
            max_records: 200,
            max_payload_bytes: 1024 * 1024,
            max_time_ms: 30_000,
        },
    }
}

/** The built-in pull request review recipe: the pull request, its head commit and changed files,
 * CI and tests of that commit, the agent work of its task, the zones and flows, and the policies
 * with the validation report
 * Input
    - None
 * Output
    - EvidenceRecipe
*/
pub(crate) fn pr_review() -> EvidenceRecipe {
    use EvidenceType as E;
    use ResourceType as R;
    EvidenceRecipe {
        recipe_id: "pr-review".into(),
        version: 1,
        description:
            "Review a pull request: the change, its verification, and the governance it touches"
                .into(),
        investigation_classes: vec!["pull-request-review".into()],
        starting_conditions: vec!["pull_request".into()],
        steps: vec![
            step(
                "pull-request",
                "The pull request",
                R::Github,
                E::PullRequest,
                StepTarget::Fetch("{pull_request}"),
                Predicate::Always,
            ),
            step(
                "code-change",
                "The head commit and its changed files",
                R::Github,
                E::CodeChange,
                StepTarget::Query(&[("commit", "{commit}")]),
                Predicate::Always,
            ),
            step(
                "ci",
                "CI results of the head commit",
                R::Ci,
                E::CiRun,
                StepTarget::Query(&[("commit", "{commit}")]),
                Predicate::Always,
            ),
            step(
                "tests",
                "Test reports of the head commit",
                R::Ci,
                E::TestReport,
                StepTarget::Query(&[("commit", "{commit}")]),
                Predicate::Always,
            ),
            step(
                "agent",
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
        ],
        required: vec![
            required(E::PullRequest, "the pull request", Predicate::Always),
            required(E::CodeChange, "what changed", Predicate::Always),
            required(E::CiRun, "whether CI passed", Predicate::Always),
            required(E::TestReport, "whether tests passed", Predicate::Always),
        ],
        stopping: StoppingRules {
            stop_when_satisfied: false,
        },
        limits: Budget {
            max_steps: 16,
            max_records: 400,
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

/** Turn the steps the broker derived from the registry into an exploration recipe for a question
 * class no recipe serves. It requires nothing (an exploration finds out what the sources hold; it
 * cannot know what "complete" means) and never stops early, and it passes the same validation as
 * every recipe. Its identifier comes from its content, so the same registry and request give the
 * same exploration
 * Input
    - steps: Vec<RecipeStep> - one query per resource type and evidence type
    - class: &str - the question class no recipe serves
 * Output
    - CaptureResult<EvidenceRecipe>
    - Error when there is no step or a step is invalid
*/
pub(crate) fn exploration(steps: Vec<RecipeStep>, class: &str) -> CaptureResult<EvidenceRecipe> {
    let mut recipe = EvidenceRecipe {
        recipe_id: "explore".into(),
        version: 0,
        description: format!(
            "An exploration of the permitted sources for '{class}', which no recipe serves"
        ),
        investigation_classes: Vec::new(),
        starting_conditions: Vec::new(),
        steps,
        required: Vec::new(),
        stopping: StoppingRules {
            stop_when_satisfied: false,
        },
        limits: Budget {
            max_steps: 24,
            max_records: 200,
            max_payload_bytes: 1024 * 1024,
            max_time_ms: 30_000,
        },
    };
    recipe.validate()?;
    recipe.recipe_id = format!("{EXPLORATION_PREFIX}{}", &recipe.digest()[..12]);
    Ok(recipe)
}
