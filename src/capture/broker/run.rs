// The Evidence Query Broker. An investigation request names a task or episode, a question, known
// references, required evidence, a time window, and a budget. The broker resolves scope and
// authorization first (a request that explicitly asks for something the actor may not access is
// rejected before any source is touched), selects a registered recipe (or validates a proposed
// plan), runs each step only when its branch condition holds and its scope values are known,
// validates every query against the resource, executes permitted connector operations within the
// budget, ingests, normalizes, and correlates the results, derives assertions deterministically,
// checks required evidence, and builds a bounded Evidence Pack. Every decision, branch, query,
// source snapshot, error, and budget figure is recorded in the query audit record. A replay re-runs
// the recorded recipe and parameters against the sources as they are now and reports whether the
// sources' snapshots still match; it never claims exactness a source cannot prove.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::recipe::{
    self, fill, Budget, EvidenceRecipe, Predicate, RecipeBook, RecipeRef, RecipeStep,
    RequiredEvidence,
};
use crate::capture::correlate::assertions::{derive, AssertionStatus, Subjects};
use crate::capture::correlate::graph::{Direction, TraversalRequest};
use crate::capture::correlate::link::{link, link_governance, LinkContext};
use crate::capture::correlate::model::{entity_id, EntityKind, NodeRef, RelationType};
use crate::capture::format::record::{EvidenceRecord, EvidenceType, ParseStatus, RecordScope};
use crate::capture::ingest::contributions::{ContributionStatus, Locality, TargetKind, TargetRef};
use crate::capture::ingest::episode::{episode_id, WorkEpisode};
use crate::capture::ingest::pack::{
    pack_evidence, EvidencePack, InvestigationStatus, MissingEvidence, PackContributions,
    PackLimits, Retrieval, SourceReport, SourceState, StepSummary, Warning, WarningKind,
};
use crate::capture::ingest::selection::{
    select, Criteria, DeterministicRanker, ExclusionReason, Ranker,
};
use crate::capture::resources::connector::{
    ConnectorOutcome, ConnectorStatus, QueryScope, QuerySpec, ResourceConnector,
};
use crate::capture::resources::registry::{
    Operation, ResourceDefinition, ResourceRegistry, ResourceType,
};
use crate::capture::resources::scope::{ScopeResolution, TimeWindow};
use crate::capture::store::{CaptureStore, IngestKind, IngestPath};
use crate::capture::{
    stable_id, AccessPolicy, Actor, CaptureError, CaptureResult, Clock, ErrorCode, Metrics,
    SCHEMA_VERSION,
};

/** References the requester already knows
 * Fields
    - repository: Option<String> - repository
    - branch: Option<String> - branch
    - commits: Vec<String> - commits
    - pull_requests: Vec<String> - pull requests
    - deployments: Vec<String> - deployments
    - environment: Option<String> - environment
    - sessions: Vec<String> - agent sessions
    - zones: Vec<String> - Landscape zone ids
    - flows: Vec<String> - Landscape flow ids
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KnownRefs {
    pub(crate) repository: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) commits: Vec<String>,
    pub(crate) pull_requests: Vec<String>,
    pub(crate) deployments: Vec<String>,
    pub(crate) environment: Option<String>,
    pub(crate) sessions: Vec<String>,
    pub(crate) zones: Vec<String>,
    pub(crate) flows: Vec<String>,
}

/** An investigation request
 * Fields
    - episode_id: Option<String> - work episode, None to derive it from the task
    - task_id: Option<String> - task
    - question: String - the investigation question
    - investigation_class: String - class used to pick a recipe
    - recipe: Option<RecipeRef> - explicit recipe
    - plan: Option<Vec<RecipeStep>> - a proposed plan (validated like a recipe) instead of a recipe
    - known: KnownRefs - known references
    - resources: Vec<String> - resources to confine the investigation to (empty: any permitted)
    - required_evidence: Vec<EvidenceType> - evidence required beyond the recipe's
    - evidence_types: Vec<EvidenceType> - evidence types wanted (empty: the recipe's)
    - time_window: TimeWindow - investigation window
    - budget: Budget - retrieval budget (tightened by the recipe's and the ceiling)
    - freshness_ms: Option<u64> - evidence older than this is flagged
    - pack_limits: PackLimits - Evidence Pack bounds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvidenceQueryRequest {
    pub(crate) episode_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) question: String,
    pub(crate) investigation_class: String,
    pub(crate) recipe: Option<RecipeRef>,
    pub(crate) plan: Option<Vec<RecipeStep>>,
    pub(crate) known: KnownRefs,
    pub(crate) resources: Vec<String>,
    pub(crate) required_evidence: Vec<EvidenceType>,
    pub(crate) evidence_types: Vec<EvidenceType>,
    pub(crate) time_window: TimeWindow,
    pub(crate) budget: Budget,
    pub(crate) freshness_ms: Option<u64>,
    pub(crate) pack_limits: PackLimits,
}

/** Whether an investigation ran fresh or replayed an earlier one
 * Variants
    - Fresh - a new execution against current sources
    - Replay - a re-execution of a recorded investigation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum ExecutionMode {
    Fresh,
    Replay { of_query: String },
}

/** What happened to a step against a resource
 * Variants
    - Executed - the connector was called and answered usably
    - Skipped - the branch condition did not hold
    - NotQueried - a scope value was unknown, the type was not requested, or no resource exists
    - Denied - authorization refused the step
    - Failed - the query was invalid or the connector failed
    - BudgetExhausted - a budget limit was reached before the step
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepState {
    Executed,
    Skipped,
    NotQueried,
    Denied,
    Failed,
    BudgetExhausted,
}

/** A branch decision
 * Fields
    - step_id: String - conditional step
    - condition: Predicate - its condition
    - held: bool - whether it held
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BranchDecision {
    pub(crate) step_id: String,
    pub(crate) condition: Predicate,
    pub(crate) held: bool,
}

/** One step of the query history
 * Fields
    - step_id: String - recipe step
    - resource_id: Option<String> - resource asked
    - operation: Operation - operation
    - evidence_type: EvidenceType - evidence wanted
    - parameters: Value - the exact record id or query sent
    - state: StepState - outcome
    - connector_status: Option<ConnectorStatus> - connector outcome
    - record_ids: Vec<String> - source records returned
    - evidence_ids: Vec<String> - evidence records produced or matched
    - duplicates: usize - records already captured
    - snapshot: Option<String> - source snapshot the answer reflects
    - next_cursor: Option<String> - continuation the source offered
    - duration_ms: u64 - time spent
    - error: Option<String> - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QueryStepRecord {
    pub(crate) step_id: String,
    pub(crate) resource_id: Option<String>,
    pub(crate) operation: Operation,
    pub(crate) evidence_type: EvidenceType,
    pub(crate) parameters: Value,
    pub(crate) state: StepState,
    pub(crate) connector_status: Option<ConnectorStatus>,
    pub(crate) record_ids: Vec<String>,
    pub(crate) evidence_ids: Vec<String>,
    pub(crate) duplicates: usize,
    pub(crate) snapshot: Option<String>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) duration_ms: u64,
    pub(crate) error: Option<String>,
}

/** An authorization decision
 * Variants
    - Allow - permitted
    - Deny - refused
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Effect {
    Allow,
    Deny,
}

/** A recorded authorization decision
 * Fields
    - subject: String - resource, repository, environment, step, or evidence type
    - effect: Effect - allow or deny
    - reason: String - why
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PolicyDecision {
    pub(crate) subject: String,
    pub(crate) effect: Effect,
    pub(crate) reason: String,
}

/** Why retrieval stopped
 * Variants
    - Completed - every step was considered
    - Satisfied - the required evidence was complete (stopping rule)
    - BudgetExhausted - a limit was reached
    - Rejected - the request was refused before any retrieval
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StopReason {
    Completed,
    Satisfied,
    BudgetExhausted,
    Rejected,
}

/** Budget consumed
 * Fields
    - steps: usize - connector operations
    - records: usize - records ingested
    - payload_bytes: usize - content bytes ingested
    - time_ms: u64 - execution time
*/
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BudgetUsage {
    pub(crate) steps: usize,
    pub(crate) records: usize,
    pub(crate) payload_bytes: usize,
    pub(crate) time_ms: u64,
}

/** How faithfully a replay reproduced its original
 * Variants
    - Exact - every source reported the same snapshot and returned the same records
    - Diverged - a source changed (different snapshot or records)
    - Unverifiable - a source cannot identify its snapshot, so sameness cannot be proven
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReplayFidelity {
    Exact,
    Diverged,
    Unverifiable,
}

/** The comparison of a replay with its original
 * Fields
    - original_query: String - original investigation
    - fidelity: ReplayFidelity - exact, diverged, or unverifiable
    - differences: Vec<String> - what differed
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReplayComparison {
    pub(crate) original_query: String,
    pub(crate) fidelity: ReplayFidelity,
    pub(crate) differences: Vec<String>,
}

/** The audit record of an investigation
 * Fields
    - query_id: String - identifier
    - schema_version: u32 - capture::SCHEMA_VERSION
    - actor: Actor - requester
    - mode: ExecutionMode - fresh or replay
    - request: EvidenceQueryRequest - request as received
    - recipe: Option<EvidenceRecipe> - the exact recipe executed, None when rejected before one
      was selected
    - recipe_digest: Option<String> - its content digest
    - scope: ScopeResolution - resolved scope
    - policy_decisions: Vec<PolicyDecision> - authorization decisions
    - branches: Vec<BranchDecision> - branch decisions
    - steps: Vec<QueryStepRecord> - query history
    - budget: Budget - budget applied
    - usage: BudgetUsage - budget consumed
    - stop_reason: StopReason - why retrieval stopped
    - status: InvestigationStatus - overall state
    - metrics: Metrics - operational counters
    - replay: Option<ReplayComparison> - comparison with the original, for replays
    - errors: Vec<String> - errors met
    - pack_id: Option<String> - Evidence Pack produced
    - started_at: u64 - Unix milliseconds
    - finished_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QueryAuditRecord {
    pub(crate) query_id: String,
    pub(crate) schema_version: u32,
    pub(crate) actor: Actor,
    pub(crate) mode: ExecutionMode,
    pub(crate) request: EvidenceQueryRequest,
    pub(crate) recipe: Option<EvidenceRecipe>,
    pub(crate) recipe_digest: Option<String>,
    pub(crate) scope: ScopeResolution,
    pub(crate) policy_decisions: Vec<PolicyDecision>,
    pub(crate) branches: Vec<BranchDecision>,
    pub(crate) steps: Vec<QueryStepRecord>,
    pub(crate) budget: Budget,
    pub(crate) usage: BudgetUsage,
    pub(crate) stop_reason: StopReason,
    pub(crate) status: InvestigationStatus,
    pub(crate) metrics: Metrics,
    pub(crate) replay: Option<ReplayComparison>,
    pub(crate) errors: Vec<String>,
    pub(crate) pack_id: Option<String>,
    pub(crate) started_at: u64,
    pub(crate) finished_at: u64,
}

/** The result of an investigation
 * Fields
    - pack: EvidencePack - Evidence Pack
    - audit: QueryAuditRecord - audit trail
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EvidenceQueryResult {
    pub(crate) pack: EvidencePack,
    pub(crate) audit: QueryAuditRecord,
}

/** The broker: registered resources, attached connectors, recipes, the store, and the clock
 * Fields
    - registry: ResourceRegistry - registered resources
    - connectors: BTreeMap<String, Box<dyn ResourceConnector>> - connectors by resource id
    - recipes: RecipeBook - recipes
    - store: CaptureStore - store
    - clock: Clock - timestamps
*/
pub(crate) struct EvidenceQueryBroker {
    pub(crate) registry: ResourceRegistry,
    pub(crate) connectors: BTreeMap<String, Box<dyn ResourceConnector>>,
    pub(crate) recipes: RecipeBook,
    pub(crate) store: CaptureStore,
    pub(crate) clock: Clock,
}

/** An investigation in progress
 * Fields
    - audit: QueryAuditRecord - audit record being filled
    - collected: Vec<String> - evidence of this investigation, in retrieval order
    - started: Instant - wall-clock start, for time budgets
*/
struct Investigation {
    audit: QueryAuditRecord,
    collected: Vec<String>,
    started: Instant,
}

impl Investigation {
    /** Report whether a budget limit is reached
     * Input
        - None (uses self)
     * Output
        - Option<&'static str> - the limit reached
    */
    fn exhausted(&self) -> Option<&'static str> {
        let (usage, budget) = (&self.audit.usage, &self.audit.budget);
        if usage.steps >= budget.max_steps {
            Some("max_steps")
        } else if usage.records >= budget.max_records {
            Some("max_records")
        } else if usage.payload_bytes >= budget.max_payload_bytes {
            Some("max_payload_bytes")
        } else if self.started.elapsed().as_millis() as u64 >= budget.max_time_ms {
            Some("max_time_ms")
        } else {
            None
        }
    }

    /** Record a step that did not call a connector
     * Input
        - step: &RecipeStep - step
        - resource_id: Option<&str> - resource
        - state: StepState - outcome
        - error: &str - explanation
     * Output
        - None
    */
    fn note(
        &mut self,
        step: &RecipeStep,
        resource_id: Option<&str>,
        state: StepState,
        error: &str,
    ) {
        self.audit.steps.push(QueryStepRecord {
            step_id: step.step_id.clone(),
            resource_id: resource_id.map(str::to_string),
            operation: step.operation,
            evidence_type: step.evidence_type,
            parameters: Value::Null,
            state,
            connector_status: None,
            record_ids: Vec::new(),
            evidence_ids: Vec::new(),
            duplicates: 0,
            snapshot: None,
            next_cursor: None,
            duration_ms: 0,
            error: Some(error.into()),
        });
    }

    /** Record an authorization denial
     * Input
        - subject: String - what was denied
        - reason: &str - why
     * Output
        - None
    */
    fn deny(&mut self, subject: String, reason: &str) {
        self.audit.metrics.authorization_denials += 1;
        self.audit.policy_decisions.push(PolicyDecision {
            subject,
            effect: Effect::Deny,
            reason: reason.into(),
        });
    }
}

impl EvidenceQueryBroker {
    /** Build a broker with the built-in recipes
     * Input
        - registry: ResourceRegistry - registered resources
        - store: CaptureStore - store
        - clock: Clock - clock
     * Output
        - CaptureResult<EvidenceQueryBroker>
        - Error if a built-in recipe is invalid
    */
    pub(crate) fn new(
        registry: ResourceRegistry,
        store: CaptureStore,
        clock: Clock,
    ) -> CaptureResult<Self> {
        let mut recipes = RecipeBook::default();
        recipes.add(recipe::failed_deployment())?;
        recipes.add(recipe::local_change())?;
        Ok(Self {
            registry,
            connectors: BTreeMap::new(),
            recipes,
            store,
            clock,
        })
    }

    /** Attach the connector of a registered resource
     * Input
        - resource_id: &str - resource
        - connector: Box<dyn ResourceConnector> - its connector
     * Output
        - CaptureResult<()>
        - Error if the resource is not registered
    */
    pub(crate) fn connect(
        &mut self,
        resource_id: &str,
        connector: Box<dyn ResourceConnector>,
    ) -> CaptureResult<()> {
        if self.registry.get(resource_id).is_none() {
            return Err(CaptureError::new(
                ErrorCode::NotFound,
                format!("no resource '{resource_id}' is registered"),
            ));
        }
        self.connectors.insert(resource_id.into(), connector);
        Ok(())
    }

    /** Run a fresh investigation
     * Input
        - request: EvidenceQueryRequest - request
        - policy: &AccessPolicy - requester's permissions
     * Output
        - CaptureResult<EvidenceQueryResult>
        - Error when no recipe applies or a proposed plan is invalid (authorization refusals are
          results with status Rejected, so they are audited)
    */
    pub(crate) fn investigate(
        &mut self,
        request: EvidenceQueryRequest,
        policy: &AccessPolicy,
    ) -> CaptureResult<EvidenceQueryResult> {
        let recipe = self.select_recipe(&request)?;
        self.run(request, recipe, policy, ExecutionMode::Fresh)
    }

    /** Replay a recorded investigation with its exact recipe and parameters against the sources as
     * they are now, and compare the sources' snapshots and records with the original
     * Input
        - query_id: &str - original investigation
        - policy: &AccessPolicy - requester's permissions (checked again)
     * Output
        - CaptureResult<EvidenceQueryResult>
        - Error when the original is unknown or its recipe no longer matches its digest
    */
    pub(crate) fn replay(
        &mut self,
        query_id: &str,
        policy: &AccessPolicy,
    ) -> CaptureResult<EvidenceQueryResult> {
        let original = self.store.audit(query_id).cloned().ok_or_else(|| {
            CaptureError::new(
                ErrorCode::NotFound,
                format!("no investigation '{query_id}'"),
            )
        })?;
        let recipe = original.recipe.clone().ok_or_else(|| {
            CaptureError::new(
                ErrorCode::InvalidRecipe,
                "the original investigation executed no recipe",
            )
        })?;
        if Some(recipe.digest()) != original.recipe_digest {
            return Err(CaptureError::new(
                ErrorCode::InvalidRecipe,
                "the recorded recipe does not match its digest",
            ));
        }
        let mut result = self.run(
            original.request.clone(),
            recipe,
            policy,
            ExecutionMode::Replay {
                of_query: query_id.into(),
            },
        )?;
        let comparison = compare(&original, &result.audit);
        result.audit.replay = Some(comparison.clone());
        if let Some(stored) = self
            .store
            .audits
            .iter_mut()
            .rev()
            .find(|audit| audit.query_id == result.audit.query_id)
        {
            stored.replay = Some(comparison);
        }
        Ok(result)
    }

    /** Choose the recipe: a proposed plan (validated), an explicit recipe, or the recipe of the
     * investigation class
     * Input
        - request: &EvidenceQueryRequest - request
     * Output
        - CaptureResult<EvidenceRecipe>
        - Error when nothing applies or the plan is invalid
    */
    fn select_recipe(&self, request: &EvidenceQueryRequest) -> CaptureResult<EvidenceRecipe> {
        if let Some(plan) = &request.plan {
            let required = request
                .required_evidence
                .iter()
                .map(|kind| RequiredEvidence {
                    evidence_type: *kind,
                    description: "required by the request".into(),
                    when: Predicate::Always,
                })
                .collect();
            return recipe::from_plan(plan.clone(), required);
        }
        let found = match &request.recipe {
            Some(reference) => self.recipes.get(reference),
            None => self.recipes.for_class(&request.investigation_class),
        };
        found.cloned().ok_or_else(|| {
            CaptureError::new(
                ErrorCode::InvalidRecipe,
                format!("no recipe for '{}'", request.investigation_class),
            )
        })
    }

    /** Execute an investigation with a chosen recipe
     * Input
        - request: EvidenceQueryRequest - request
        - recipe: EvidenceRecipe - recipe
        - policy: &AccessPolicy - requester's permissions
        - mode: ExecutionMode - fresh or replay
     * Output
        - CaptureResult<EvidenceQueryResult>
        - Error when an assertion cannot be stored
    */
    fn run(
        &mut self,
        request: EvidenceQueryRequest,
        recipe: EvidenceRecipe,
        policy: &AccessPolicy,
        mode: ExecutionMode,
    ) -> CaptureResult<EvidenceQueryResult> {
        let started_at = self.clock.now();
        let started = started_at.to_string();
        let count = self.store.audits.len().to_string();
        let query_id = stable_id(
            "qry",
            &[&policy.actor.id, &request.question, &started, &count],
        );
        let budget = request
            .budget
            .tighter(recipe.limits)
            .tighter(Budget::ceiling());
        let mut scope = ScopeResolution::resolve(&request, &self.registry, policy);
        let episode = request
            .episode_id
            .clone()
            .unwrap_or_else(|| episode_id(request.task_id.as_deref(), &request.question, &started));
        scope.episode_id = Some(episode.clone());
        let mut run = Investigation {
            audit: QueryAuditRecord {
                query_id: query_id.clone(),
                schema_version: SCHEMA_VERSION,
                actor: policy.actor.clone(),
                mode,
                request: request.clone(),
                recipe_digest: Some(recipe.digest()),
                recipe: Some(recipe.clone()),
                scope: ScopeResolution::default(),
                policy_decisions: Vec::new(),
                branches: Vec::new(),
                steps: Vec::new(),
                budget,
                usage: BudgetUsage::default(),
                stop_reason: StopReason::Completed,
                status: InvestigationStatus::Incomplete,
                metrics: Metrics::default(),
                replay: None,
                errors: Vec::new(),
                pack_id: None,
                started_at,
                finished_at: started_at,
            },
            collected: Vec::new(),
            started: Instant::now(),
        };
        for resource in &scope.permitted_resources {
            run.audit.policy_decisions.push(PolicyDecision {
                subject: resource.clone(),
                effect: Effect::Allow,
                reason: "granted by the actor's policy and within scope".into(),
            });
        }
        for denial in &scope.denied {
            run.deny(denial.resource_id.clone(), &denial.reason);
        }
        let rejected = scope.denied.iter().any(|denial| denial.explicit);
        if rejected {
            run.audit.stop_reason = StopReason::Rejected;
            run.audit.errors.push("the request explicitly named a repository, environment, or resource the actor may not access; nothing was queried".into());
        } else {
            let mut opened =
                WorkEpisode::open(request.task_id.as_deref(), &request.question, started_at);
            opened.episode_id = episode.clone();
            self.store.ensure_episode(opened);
            self.resolve_versions(&mut run, &mut scope, policy);
            self.execute(&mut run, &mut scope, &recipe, &request, policy);
            let outcome = link_governance(&mut self.store.graph);
            run.audit.metrics.correlation_failures += outcome.failures.len() as u64;
        }
        if let Some(episode) = self.store.episodes.get_mut(&episode) {
            episode.zones.clone_from(&scope.zones);
            episode.flows.clone_from(&scope.flows);
        }
        self.finish(run, scope, &recipe, &request, policy)
    }

    /** Resolve the versions of known zones and flows through permitted Landscape resources, and
     * ingest them as evidence
     * Input
        - run: &mut Investigation - investigation
        - scope: &mut ScopeResolution - scope
        - policy: &AccessPolicy - requester's permissions
     * Output
        - None
    */
    fn resolve_versions(
        &mut self,
        run: &mut Investigation,
        scope: &mut ScopeResolution,
        policy: &AccessPolicy,
    ) {
        if scope.zones.is_empty() && scope.flows.is_empty() {
            return;
        }
        let landscapes = self
            .registry
            .of_type(ResourceType::Landscape)
            .into_iter()
            .filter(|definition| scope.permitted_resources.contains(&definition.resource_id))
            .cloned()
            .collect::<Vec<_>>();
        let wanted = scope
            .zones
            .iter()
            .map(|zone| ("zone", zone.id.clone()))
            .chain(scope.flows.iter().map(|flow| ("flow", flow.id.clone())))
            .collect::<Vec<_>>();
        for (kind, id) in wanted {
            let step = RecipeStep {
                step_id: format!("scope-{kind}"),
                description: "Resolve the Landscape version".into(),
                resource_type: ResourceType::Landscape,
                operation: Operation::Fetch,
                evidence_type: EvidenceType::GraphState,
                record: Some(format!("{kind}:{id}")),
                filters: BTreeMap::new(),
                limit: 1,
                when: Predicate::Always,
            };
            for definition in &landscapes {
                let evidence = self.call(run, scope, &step, definition, policy);
                let version = evidence
                    .iter()
                    .filter_map(|id| self.store.evidence.get(id))
                    .find_map(|record| record.source_ref.revision.clone());
                let list = if kind == "zone" {
                    &mut scope.zones
                } else {
                    &mut scope.flows
                };
                if let Some(reference) = list
                    .iter_mut()
                    .find(|reference| reference.id == id && reference.version.is_none())
                {
                    reference.version = version;
                }
            }
        }
        scope.refresh_unknown();
    }

    /** Run the recipe's steps in order within the budget
     * Input
        - run: &mut Investigation - investigation
        - scope: &mut ScopeResolution - scope (learns facts from evidence)
        - recipe: &EvidenceRecipe - recipe
        - request: &EvidenceQueryRequest - request
        - policy: &AccessPolicy - requester's permissions
     * Output
        - None
    */
    fn execute(
        &mut self,
        run: &mut Investigation,
        scope: &mut ScopeResolution,
        recipe: &EvidenceRecipe,
        request: &EvidenceQueryRequest,
        policy: &AccessPolicy,
    ) {
        for (index, step) in recipe.steps.iter().enumerate() {
            if let Some(limit) = run.exhausted() {
                run.audit.metrics.budget_exhausted += 1;
                run.audit.stop_reason = StopReason::BudgetExhausted;
                for remaining in &recipe.steps[index..] {
                    run.note(
                        remaining,
                        None,
                        StepState::BudgetExhausted,
                        &format!("budget limit {limit} reached"),
                    );
                }
                return;
            }
            if step.when != Predicate::Always {
                let collected = self.records(&run.collected);
                let held = step.when.holds(&collected, scope);
                run.audit.branches.push(BranchDecision {
                    step_id: step.step_id.clone(),
                    condition: step.when.clone(),
                    held,
                });
                if !held {
                    run.audit.metrics.branches_skipped += 1;
                    run.note(
                        step,
                        None,
                        StepState::Skipped,
                        "branch condition did not hold",
                    );
                    continue;
                }
                run.audit.metrics.branches_taken += 1;
            }
            if !request.evidence_types.is_empty()
                && !request.evidence_types.contains(&step.evidence_type)
            {
                run.note(
                    step,
                    None,
                    StepState::NotQueried,
                    "evidence type not requested",
                );
                continue;
            }
            if !policy.permits_type(step.evidence_type) {
                run.deny(
                    format!("{}:{:?}", step.step_id, step.evidence_type),
                    "evidence type not permitted",
                );
                run.note(
                    step,
                    None,
                    StepState::Denied,
                    "the actor may not receive this evidence type",
                );
                continue;
            }
            if !policy.permits_environment(scope.environment.as_deref()) {
                run.deny(
                    format!(
                        "environment:{}",
                        scope.environment.clone().unwrap_or_default()
                    ),
                    "environment not permitted",
                );
                run.note(
                    step,
                    None,
                    StepState::Denied,
                    "the actor may not see this environment",
                );
                continue;
            }
            let candidates = self
                .registry
                .of_type(step.resource_type)
                .into_iter()
                .filter(|definition| {
                    definition
                        .capabilities
                        .evidence_types
                        .contains(&step.evidence_type)
                        && definition.capabilities.operations.contains(&step.operation)
                })
                .cloned()
                .collect::<Vec<_>>();
            let permitted = candidates
                .iter()
                .filter(|definition| scope.permitted_resources.contains(&definition.resource_id))
                .cloned()
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                run.note(
                    step,
                    None,
                    StepState::NotQueried,
                    &format!(
                        "no registered {:?} resource provides {:?}",
                        step.resource_type, step.evidence_type
                    ),
                );
                continue;
            }
            if permitted.is_empty() {
                run.deny(step.step_id.clone(), "no permitted resource for this step");
                run.note(
                    step,
                    None,
                    StepState::Denied,
                    "no resource for this step is permitted to the actor",
                );
                continue;
            }
            for definition in &permitted {
                if let Some(limit) = run.exhausted() {
                    run.audit.metrics.budget_exhausted += 1;
                    run.audit.stop_reason = StopReason::BudgetExhausted;
                    run.note(
                        step,
                        Some(&definition.resource_id),
                        StepState::BudgetExhausted,
                        &format!("budget limit {limit} reached"),
                    );
                    continue;
                }
                self.call(run, scope, step, definition, policy);
            }
            if recipe.stopping.stop_when_satisfied
                && self.missing(run, scope, recipe, request).is_empty()
            {
                run.audit.stop_reason = StopReason::Satisfied;
                for remaining in &recipe.steps[index + 1..] {
                    run.note(
                        remaining,
                        None,
                        StepState::NotQueried,
                        "stopped: required evidence was complete",
                    );
                }
                return;
            }
        }
    }

    /** Execute one step against one resource: fill and validate parameters, call the connector,
     * ingest, normalize, correlate, and learn scope facts
     * Input
        - run: &mut Investigation - investigation
        - scope: &mut ScopeResolution - scope
        - step: &RecipeStep - step
        - definition: &ResourceDefinition - resource
        - policy: &AccessPolicy - requester's permissions
     * Output
        - Vec<String> evidence ids produced or matched
    */
    fn call(
        &mut self,
        run: &mut Investigation,
        scope: &mut ScopeResolution,
        step: &RecipeStep,
        definition: &ResourceDefinition,
        policy: &AccessPolicy,
    ) -> Vec<String> {
        let resource_id = definition.resource_id.as_str();
        if !definition.scope.repositories.is_empty() && scope.repository.is_none() {
            run.note(
                step,
                Some(resource_id),
                StepState::NotQueried,
                "the repository is unknown; a scoped resource is never queried without it",
            );
            return Vec::new();
        }
        let Some(connector) = self.connectors.get(resource_id) else {
            run.note(
                step,
                Some(resource_id),
                StepState::Failed,
                "no connector is attached to this resource",
            );
            return Vec::new();
        };
        let (parameters, timer, outcome) = match step.operation {
            Operation::Fetch => {
                let template = step.record.clone().unwrap_or_default();
                let record = match fill(&template, scope) {
                    Ok(record) => record,
                    Err(variable) => {
                        run.note(
                            step,
                            Some(resource_id),
                            StepState::NotQueried,
                            &format!("scope value '{variable}' is unknown"),
                        );
                        return Vec::new();
                    }
                };
                let timer = Instant::now();
                (json!({"record": record}), timer, connector.fetch(&record))
            }
            _ => {
                let mut filters = BTreeMap::new();
                for (key, template) in &step.filters {
                    match fill(template, scope) {
                        Ok(value) => {
                            filters.insert(key.clone(), value);
                        }
                        Err(variable) => {
                            run.note(
                                step,
                                Some(resource_id),
                                StepState::NotQueried,
                                &format!("scope value '{variable}' is unknown"),
                            );
                            return Vec::new();
                        }
                    }
                }
                let remaining = run
                    .audit
                    .budget
                    .max_records
                    .saturating_sub(run.audit.usage.records)
                    .max(1);
                let spec = QuerySpec {
                    evidence_type: Some(step.evidence_type),
                    filters,
                    since: scope.time_window.since,
                    until: scope.time_window.until,
                    limit: step.limit.min(remaining),
                    cursor: None,
                    scope: QueryScope {
                        repository: scope.repository.clone(),
                        environment: scope.environment.clone(),
                    },
                };
                if let Err(error) = spec.validate(definition) {
                    run.audit.steps.push(QueryStepRecord {
                        step_id: step.step_id.clone(),
                        resource_id: Some(resource_id.into()),
                        operation: step.operation,
                        evidence_type: step.evidence_type,
                        parameters: json!(spec),
                        state: StepState::Failed,
                        connector_status: Some(ConnectorStatus::InvalidQuery),
                        record_ids: Vec::new(),
                        evidence_ids: Vec::new(),
                        duplicates: 0,
                        snapshot: None,
                        next_cursor: None,
                        duration_ms: 0,
                        error: Some(error.message),
                    });
                    return Vec::new();
                }
                let timer = Instant::now();
                let outcome = connector.query(&spec);
                (json!(spec), timer, outcome)
            }
        };
        let duration_ms = timer.elapsed().as_millis() as u64;
        run.audit.usage.steps += 1;
        run.audit.metrics.connector_calls += 1;
        run.audit.metrics.connector_latency_ms += duration_ms;
        let ConnectorOutcome {
            status,
            records,
            next_cursor,
            snapshot,
            error,
        } = outcome;
        let mut entry = QueryStepRecord {
            step_id: step.step_id.clone(),
            resource_id: Some(resource_id.into()),
            operation: step.operation,
            evidence_type: step.evidence_type,
            parameters,
            state: StepState::Executed,
            connector_status: Some(status),
            record_ids: Vec::new(),
            evidence_ids: Vec::new(),
            duplicates: 0,
            snapshot,
            next_cursor: next_cursor.clone(),
            duration_ms,
            error,
        };
        if !status.usable() {
            run.audit.metrics.connector_failures += 1;
            entry.state = StepState::Failed;
            run.audit.steps.push(entry);
            return Vec::new();
        }
        if let Some(cursor) = &next_cursor {
            self.store.set_cursor(resource_id, cursor);
        }
        run.audit.metrics.records_retrieved += records.len() as u64;
        let path = IngestPath {
            kind: "query".into(),
            id: format!("{}:{}", run.audit.query_id, step.step_id),
        };
        let now = self.clock.now();
        let task = scope.task_id.clone();
        let episode_entity = scope
            .episode_id
            .as_deref()
            .map(|episode| entity_id(EntityKind::WorkEpisode, episode));
        for record in records {
            if run.audit.usage.records >= run.audit.budget.max_records {
                run.audit.metrics.budget_exhausted += 1;
                run.audit.stop_reason = StopReason::BudgetExhausted;
                entry.error =
                    Some("record budget reached; remaining records were not ingested".into());
                break;
            }
            if !policy.permits_type(record.evidence_type) {
                run.deny(
                    format!("{}:{}", resource_id, record.record_id),
                    "the source returned an evidence type the actor may not receive",
                );
                continue;
            }
            let size = record.content.to_string().len();
            if run.audit.usage.payload_bytes + size > run.audit.budget.max_payload_bytes {
                run.audit.metrics.budget_exhausted += 1;
                run.audit.stop_reason = StopReason::BudgetExhausted;
                entry.error =
                    Some("payload budget reached; remaining records were not ingested".into());
                break;
            }
            let ingested = self.store.ingest(
                definition,
                &record,
                &path,
                scope.episode_id.as_deref(),
                Some(&run.audit.query_id),
                now,
            );
            run.audit.usage.records += 1;
            run.audit.usage.payload_bytes += size;
            entry.record_ids.push(record.record_id.clone());
            entry.evidence_ids.push(ingested.evidence_id.clone());
            match ingested.kind {
                IngestKind::Duplicate => {
                    entry.duplicates += 1;
                    run.audit.metrics.duplicates += 1;
                }
                _ => run.audit.metrics.records_normalized += 1,
            }
            match ingested.parse_status {
                ParseStatus::PartiallyParsed => run.audit.metrics.malformed += 1,
                ParseStatus::Failed => run.audit.metrics.parse_failures += 1,
                ParseStatus::Parsed => {}
            }
            if let Some(evidence) = self.store.evidence.get(&ingested.evidence_id).cloned() {
                let context = LinkContext {
                    task: task.as_deref(),
                    episode: episode_entity.as_deref(),
                };
                let linked = link(&mut self.store.graph, &evidence, &context);
                run.audit.metrics.correlation_failures += linked.failures.len() as u64;
                run.audit.errors.extend(linked.failures);
                scope.learn(&evidence);
            }
            if !run.collected.contains(&ingested.evidence_id) {
                run.collected.push(ingested.evidence_id);
            }
        }
        run.audit.usage.time_ms = run.started.elapsed().as_millis() as u64;
        let produced = entry.evidence_ids.clone();
        run.audit.steps.push(entry);
        produced
    }

    /** Look up evidence records by id
     * Input
        - ids: &[String] - evidence ids
     * Output
        - Vec<&EvidenceRecord>
    */
    fn records(&self, ids: &[String]) -> Vec<&EvidenceRecord> {
        ids.iter()
            .filter_map(|id| self.store.evidence.get(id))
            .collect()
    }

    /** List required evidence that is not yet collected (records that failed to parse do not count)
     * Input
        - run: &Investigation - investigation
        - scope: &ScopeResolution - scope
        - recipe: &EvidenceRecipe - recipe
        - request: &EvidenceQueryRequest - request
     * Output
        - Vec<RequiredEvidence>
    */
    fn missing(
        &self,
        run: &Investigation,
        scope: &ScopeResolution,
        recipe: &EvidenceRecipe,
        request: &EvidenceQueryRequest,
    ) -> Vec<RequiredEvidence> {
        let collected = self.records(&run.collected);
        let extra = request
            .required_evidence
            .iter()
            .map(|kind| RequiredEvidence {
                evidence_type: *kind,
                description: "required by the request".into(),
                when: Predicate::Always,
            });
        recipe
            .required
            .iter()
            .cloned()
            .chain(extra)
            .filter(|required| required.when.holds(&collected, scope))
            .filter(|required| {
                !collected.iter().any(|record| {
                    record.evidence_type == required.evidence_type
                        && record.parse_status != ParseStatus::Failed
                })
            })
            .collect()
    }

    /** Derive assertions, select evidence, build the context view and the pack, and record the
     * audit
     * Input
        - run: Investigation - investigation
        - scope: ScopeResolution - final scope
        - recipe: &EvidenceRecipe - recipe
        - request: &EvidenceQueryRequest - request
        - policy: &AccessPolicy - requester's permissions
     * Output
        - CaptureResult<EvidenceQueryResult>
        - Error when an assertion cannot be stored
    */
    fn finish(
        &mut self,
        mut run: Investigation,
        scope: ScopeResolution,
        recipe: &EvidenceRecipe,
        request: &EvidenceQueryRequest,
        policy: &AccessPolicy,
    ) -> CaptureResult<EvidenceQueryResult> {
        let rejected = run.audit.stop_reason == StopReason::Rejected;
        let build_timer = Instant::now();
        let mut assertion_ids = Vec::new();
        if !rejected {
            let hashes = self
                .store
                .artifacts
                .values()
                .filter_map(|artifact| {
                    artifact
                        .content_hash
                        .clone()
                        .map(|hash| (artifact.artifact_id.clone(), hash))
                })
                .collect::<BTreeMap<_, _>>();
            let subjects = Subjects {
                task: scope.task_id.clone(),
                commits: scope.commits.clone(),
                deployments: scope.deployments.clone(),
            };
            let record_scope = RecordScope {
                repository: scope.repository.clone(),
                environment: scope.environment.clone(),
                zones: scope.zones.iter().map(|zone| zone.id.clone()).collect(),
                flows: scope.flows.iter().map(|flow| flow.id.clone()).collect(),
            };
            let records = self
                .records(&run.collected)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let references = records.iter().collect::<Vec<_>>();
            for assertion in derive(
                &references,
                &hashes,
                &subjects,
                &record_scope,
                self.clock.now(),
            ) {
                assertion_ids.push(self.store.add_assertion(assertion)?);
            }
        }
        let missing = if rejected {
            Vec::new()
        } else {
            self.missing(&run, &scope, recipe, request)
        };
        run.audit.metrics.missing_required = missing.len() as u64;
        let status = match run.audit.stop_reason {
            StopReason::Rejected => InvestigationStatus::Rejected,
            StopReason::BudgetExhausted => InvestigationStatus::BudgetExhausted,
            _ if missing.is_empty() => InvestigationStatus::Sufficient,
            _ => InvestigationStatus::Incomplete,
        };
        let sources = run
            .audit
            .steps
            .iter()
            .map(|step| self.source_report(step))
            .collect::<Vec<_>>();
        let missing_evidence = missing
            .iter()
            .map(|required| {
                let reports = sources
                    .iter()
                    .filter(|source| source.evidence_type == required.evidence_type)
                    .collect::<Vec<_>>();
                let state = [
                    SourceState::ParseFailed,
                    SourceState::Denied,
                    SourceState::Unavailable,
                    SourceState::Empty,
                ]
                .into_iter()
                .find(|state| reports.iter().any(|report| report.state == *state))
                .unwrap_or(SourceState::NotQueried);
                let detail = reports
                    .iter()
                    .map(|report| format!("{}: {}", report.step_id, report.detail))
                    .collect::<Vec<_>>()
                    .join("; ");
                MissingEvidence {
                    evidence_type: required.evidence_type,
                    requirement: required.description.clone(),
                    state,
                    detail: if detail.is_empty() {
                        "no step retrieves this evidence".into()
                    } else {
                        detail
                    },
                }
            })
            .collect::<Vec<_>>();
        let collected = run.collected.clone();
        let candidates = self.records(&collected);
        let limits = request.pack_limits;
        let ranker = DeterministicRanker;
        let criteria = Criteria {
            policy,
            repository: scope.repository.as_deref(),
            // The candidates are this investigation's own retrievals; a record first captured by
            // another episode (a duplicate observed again) still belongs here
            episode_id: None,
            types: &request.evidence_types,
            window: &scope.time_window,
            max_records: limits.max_records,
        };
        let selection = select(&candidates, &criteria, &ranker);
        let selected = selection.selected.iter().cloned().collect::<BTreeSet<_>>();
        let derived = assertion_ids.iter().cloned().collect::<BTreeSet<_>>();
        let mut roots = Vec::new();
        if let Some(task) = &scope.task_id {
            roots.push(NodeRef::Entity(entity_id(EntityKind::Task, task)));
        }
        if let Some(episode) = &scope.episode_id {
            roots.push(NodeRef::Entity(entity_id(EntityKind::WorkEpisode, episode)));
        }
        roots.extend(
            scope
                .deployments
                .iter()
                .map(|deployment| NodeRef::Entity(entity_id(EntityKind::Deployment, deployment))),
        );
        roots.extend(
            scope
                .commits
                .iter()
                .map(|commit| NodeRef::Entity(entity_id(EntityKind::Commit, commit))),
        );
        roots.retain(|root| self.store.graph.entities.contains_key(root.id()));
        let traversal = TraversalRequest {
            start: roots,
            relation_types: ALL_RELATIONS.to_vec(),
            statuses: Vec::new(),
            direction: Direction::Both,
            max_depth: 3,
            max_results: 400,
            max_bytes: limits.max_bytes / 2,
            at: None,
        };
        let visible = |node: &NodeRef| match node {
            NodeRef::Entity(_) => true,
            NodeRef::Evidence(id) => selected.contains(id),
            NodeRef::Assertion(id) => derived.contains(id),
            NodeRef::Contribution(_) => false,
        };
        let (view, walked) = self.store.graph.context_view(
            &request.question,
            &traversal,
            &visible,
            self.clock.now(),
        )?;
        run.audit.metrics.traversal_depth = walked.depth_reached as u64;
        let assertions = assertion_ids
            .iter()
            .filter_map(|id| self.store.assertions.get(id).cloned())
            .collect::<Vec<_>>();
        let contributions = self.contributions_for(
            &scope,
            &view
                .entities
                .iter()
                .map(|entity| entity.entity_id.clone())
                .collect(),
        );
        let mut warnings = self.warnings(
            &run,
            &scope,
            &missing_evidence,
            &assertions,
            &selection.excluded,
            &selected,
            request,
        );
        if !walked.limits.is_empty() {
            warnings.push(Warning {
                kind: WarningKind::Truncation,
                message: format!("the context view reached its limits: {:?}", walked.limits),
                refs: Vec::new(),
            });
        }
        let evidence = selection
            .selected
            .iter()
            .filter_map(|id| self.store.evidence.get(id))
            .map(|record| {
                pack_evidence(
                    record,
                    self.store
                        .artifacts
                        .get(&record.source_ref.artifact_id)
                        .and_then(|artifact| artifact.content_hash.clone()),
                    &limits,
                )
            })
            .collect();
        run.audit.status = status;
        run.audit.scope = scope.clone();
        let mut pack = EvidencePack {
            pack_id: String::new(),
            schema_version: SCHEMA_VERSION,
            episode_id: scope.episode_id.clone(),
            investigation: request.question.clone(),
            status,
            entities: view.entities.clone(),
            context_view: view,
            scope: scope.clone(),
            evidence,
            assertions,
            contributions,
            missing_evidence,
            sources,
            retrieval: Retrieval {
                query_id: run.audit.query_id.clone(),
                mode: run.audit.mode.clone(),
                recipe_id: recipe.recipe_id.clone(),
                recipe_version: recipe.version,
                recipe_digest: recipe.digest(),
                ranker: ranker.name().into(),
                branches: run.audit.branches.clone(),
                steps: run
                    .audit
                    .steps
                    .iter()
                    .map(|step| StepSummary {
                        step_id: step.step_id.clone(),
                        resource_id: step.resource_id.clone(),
                        state: step.state,
                        records: step.record_ids.len(),
                    })
                    .collect(),
                stop_reason: run.audit.stop_reason,
            },
            warnings,
            limits,
            manifest: Default::default(),
            size_bytes: 0,
            created_at: self.clock.now(),
        };
        pack.seal();
        run.audit.metrics.pack_bytes = pack.size_bytes as u64;
        run.audit.metrics.pack_build_ms = build_timer.elapsed().as_millis() as u64;
        run.audit.usage.time_ms = run.started.elapsed().as_millis() as u64;
        run.audit.pack_id = Some(pack.pack_id.clone());
        run.audit.finished_at = self.clock.now();
        if let Some(episode) = scope
            .episode_id
            .as_ref()
            .and_then(|id| self.store.episodes.get_mut(id))
        {
            episode.packs.push(pack.pack_id.clone());
        }
        self.store.packs.insert(pack.pack_id.clone(), pack.clone());
        self.store.audits.push(run.audit.clone());
        Ok(EvidenceQueryResult {
            pack,
            audit: run.audit,
        })
    }

    /** Summarize a step as a source report
     * Input
        - step: &QueryStepRecord - step
     * Output
        - SourceReport
    */
    fn source_report(&self, step: &QueryStepRecord) -> SourceReport {
        let parsed = step
            .evidence_ids
            .iter()
            .filter_map(|id| self.store.evidence.get(id))
            .any(|record| record.parse_status != ParseStatus::Failed);
        let state = match step.state {
            StepState::Executed if step.record_ids.is_empty() => SourceState::Empty,
            StepState::Executed if !parsed => SourceState::ParseFailed,
            StepState::Executed => SourceState::Collected,
            StepState::Denied => SourceState::Denied,
            StepState::Failed if step.connector_status == Some(ConnectorStatus::Unauthorized) => {
                SourceState::Denied
            }
            StepState::Failed => SourceState::Unavailable,
            StepState::Skipped | StepState::NotQueried | StepState::BudgetExhausted => {
                SourceState::NotQueried
            }
        };
        let status = step
            .connector_status
            .map(|status| format!("{status:?}"))
            .unwrap_or_else(|| format!("{:?}", step.state));
        SourceReport {
            step_id: step.step_id.clone(),
            resource_id: step.resource_id.clone(),
            evidence_type: step.evidence_type,
            state,
            detail: match &step.error {
                Some(error) => format!("{status}: {error}"),
                None => status,
            },
            records: step.record_ids.len(),
        }
    }

    /** The contributions relevant to the pack: task-local ones of this episode, and reusable ones
     * about entities in the context view, when proposed or accepted
     * Input
        - scope: &ScopeResolution - scope
        - entities: &BTreeSet<String> - entity ids in the context view
     * Output
        - PackContributions
    */
    fn contributions_for(
        &self,
        scope: &ScopeResolution,
        entities: &BTreeSet<String>,
    ) -> PackContributions {
        let mut result = PackContributions::default();
        for contribution in self.store.contributions.contributions.values() {
            if !matches!(
                contribution.status,
                ContributionStatus::Proposed | ContributionStatus::Accepted
            ) {
                continue;
            }
            match contribution.locality {
                Locality::TaskLocal
                    if contribution.episode_id.is_some()
                        && contribution.episode_id == scope.episode_id =>
                {
                    result.task_local.push(contribution.clone())
                }
                Locality::Reusable
                    if entities.contains(&target_entity(&contribution.target_ref)) =>
                {
                    result.reusable.push(contribution.clone())
                }
                _ => {}
            }
        }
        result
    }

    /** Build the pack's warnings
     * Input
        - run: &Investigation - investigation
        - scope: &ScopeResolution - scope
        - missing: &[MissingEvidence] - missing required evidence
        - assertions: &[Assertion] - derived assertions
        - excluded: &[Exclusion] - records excluded by selection
        - selected: &BTreeSet<String> - selected records
        - request: &EvidenceQueryRequest - request
     * Output
        - Vec<Warning>
    */
    #[allow(clippy::too_many_arguments)]
    fn warnings(
        &self,
        run: &Investigation,
        scope: &ScopeResolution,
        missing: &[MissingEvidence],
        assertions: &[crate::capture::correlate::assertions::Assertion],
        excluded: &[crate::capture::ingest::selection::Exclusion],
        selected: &BTreeSet<String>,
        request: &EvidenceQueryRequest,
    ) -> Vec<Warning> {
        let mut warnings = Vec::new();
        let warn = |kind: WarningKind, message: String, refs: Vec<String>| Warning {
            kind,
            message,
            refs,
        };
        for denial in &scope.denied {
            warnings.push(warn(
                WarningKind::Access,
                format!("{} was not used: {}", denial.resource_id, denial.reason),
                vec![denial.resource_id.clone()],
            ));
        }
        for step in &run.audit.steps {
            match (step.state, step.connector_status) {
                (StepState::Denied, _)
                | (StepState::Failed, Some(ConnectorStatus::Unauthorized)) => warnings.push(warn(
                    WarningKind::Access,
                    format!(
                        "step {} was denied: {}",
                        step.step_id,
                        step.error.clone().unwrap_or_default()
                    ),
                    vec![step.step_id.clone()],
                )),
                (StepState::Executed, Some(ConnectorStatus::Partial)) => warnings.push(warn(
                    WarningKind::Completeness,
                    format!(
                        "step {} returned a partial result: {}",
                        step.step_id,
                        step.error.clone().unwrap_or_default()
                    ),
                    vec![step.step_id.clone()],
                )),
                _ => {}
            }
        }
        let unauthorized = excluded
            .iter()
            .filter(|exclusion| {
                matches!(
                    exclusion.reason,
                    ExclusionReason::Unauthorized | ExclusionReason::TooSensitive
                )
            })
            .map(|exclusion| exclusion.evidence_id.clone())
            .collect::<Vec<_>>();
        if !unauthorized.is_empty() {
            warnings.push(warn(
                WarningKind::Access,
                format!(
                    "{} records were withheld by authorization or sensitivity",
                    unauthorized.len()
                ),
                unauthorized,
            ));
        }
        for item in missing {
            warnings.push(warn(
                WarningKind::Completeness,
                format!(
                    "required {:?} evidence is missing ({:?}): {}",
                    item.evidence_type, item.state, item.requirement
                ),
                Vec::new(),
            ));
        }
        for assertion in assertions {
            if assertion.conflicted() {
                let mut refs = assertion.supporting.clone();
                refs.extend(assertion.contradicting.iter().cloned());
                warnings.push(warn(
                    WarningKind::Conflict,
                    format!(
                        "evidence disagrees on '{}'; no side was chosen",
                        assertion.statement
                    ),
                    refs,
                ));
            } else if assertion.status == AssertionStatus::Unresolved {
                warnings.push(warn(
                    WarningKind::Uncertainty,
                    format!("could not be established: {}", assertion.statement),
                    vec![assertion.assertion_id.clone()],
                ));
            }
        }
        let now = self.clock.now();
        for id in selected {
            let Some(record) = self.store.evidence.get(id) else {
                continue;
            };
            let timed = !matches!(
                record.evidence_type,
                EvidenceType::GraphState
                    | EvidenceType::Policy
                    | EvidenceType::Validation
                    | EvidenceType::Configuration
                    | EvidenceType::Task
            );
            match (record.observed_at, request.freshness_ms) {
                (None, _) if timed => warnings.push(warn(
                    WarningKind::Freshness,
                    format!(
                        "{:?} evidence {} has no observation time",
                        record.evidence_type, record.source_ref.record_id
                    ),
                    vec![id.clone()],
                )),
                (Some(at), Some(limit)) if now.saturating_sub(at) > limit => warnings.push(warn(
                    WarningKind::Freshness,
                    format!(
                        "{:?} evidence {} is older than {limit} ms",
                        record.evidence_type, record.source_ref.record_id
                    ),
                    vec![id.clone()],
                )),
                _ => {}
            }
        }
        if run.audit.stop_reason == StopReason::BudgetExhausted {
            warnings.push(warn(
                WarningKind::Budget,
                "a retrieval budget limit stopped the investigation; later steps were not queried"
                    .into(),
                Vec::new(),
            ));
        }
        if run.audit.stop_reason == StopReason::Rejected {
            warnings.push(warn(
                WarningKind::Access,
                "the request was rejected; no source was queried".into(),
                Vec::new(),
            ));
        }
        warnings
    }
}

/** Every relation type, followed when building a context view */
const ALL_RELATIONS: [RelationType; 19] = [
    RelationType::Contains,
    RelationType::DependsOn,
    RelationType::Calls,
    RelationType::Reads,
    RelationType::Writes,
    RelationType::Triggered,
    RelationType::DeployedTo,
    RelationType::ExecutedDuring,
    RelationType::ResultedIn,
    RelationType::BelongsToZone,
    RelationType::BelongsToFlow,
    RelationType::GovernedBy,
    RelationType::Supports,
    RelationType::Contradicts,
    RelationType::DerivedFrom,
    RelationType::ObservedOn,
    RelationType::ProposedFor,
    RelationType::AppliesTo,
    RelationType::Supersedes,
];

/** The entity id a contribution target corresponds to
 * Input
    - target: &TargetRef - target
 * Output
    - String
*/
pub(crate) fn target_entity(target: &TargetRef) -> String {
    match target.kind {
        TargetKind::Entity => target.id.clone(),
        TargetKind::Zone => entity_id(EntityKind::Zone, &target.id),
        TargetKind::Flow => entity_id(EntityKind::Flow, &target.id),
        TargetKind::WorkEpisode => entity_id(EntityKind::WorkEpisode, &target.id),
    }
}

/** Compare a replay with its original: per step and resource, the source snapshot and the
 * records returned
 * Input
    - original: &QueryAuditRecord - original investigation
    - replay: &QueryAuditRecord - replay
 * Output
    - ReplayComparison
*/
fn compare(original: &QueryAuditRecord, replay: &QueryAuditRecord) -> ReplayComparison {
    let mut differences = Vec::new();
    let mut unverifiable = false;
    for step in original
        .steps
        .iter()
        .filter(|step| step.state == StepState::Executed)
    {
        let key = format!(
            "{}@{}",
            step.step_id,
            step.resource_id.clone().unwrap_or_default()
        );
        let Some(again) = replay.steps.iter().find(|candidate| {
            candidate.step_id == step.step_id && candidate.resource_id == step.resource_id
        }) else {
            differences.push(format!("{key}: not executed in the replay"));
            continue;
        };
        if again.state != step.state {
            differences.push(format!("{key}: {:?} became {:?}", step.state, again.state));
        }
        match (&step.snapshot, &again.snapshot) {
            (Some(before), Some(after)) if before != after => {
                differences.push(format!("{key}: source snapshot {before} became {after}"))
            }
            (None, _) | (_, None) => unverifiable = true,
            _ => {}
        }
        if step.evidence_ids != again.evidence_ids {
            differences.push(format!("{key}: returned different records"));
        }
    }
    let fidelity = if !differences.is_empty() {
        ReplayFidelity::Diverged
    } else if unverifiable {
        ReplayFidelity::Unverifiable
    } else {
        ReplayFidelity::Exact
    };
    ReplayComparison {
        original_query: original.query_id.clone(),
        fidelity,
        differences,
    }
}
