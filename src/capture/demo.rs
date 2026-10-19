// The end-to-end demonstration ('crane capture demo'): investigate a failed deployment associated
// with a repository change, deterministically, from fixtures (fixture connectors stand in for the
// tracker, GitHub, CI, deployment, logs, metrics, Landscape, and Crane sources). It starts from a
// task with known pull request and deployment references, resolves scope and the zone and flow
// versions, retrieves the change, CI, and agent evidence, follows the recipe's branches (logs and
// metrics because the deployment failed; no schema evidence because no migration ran), derives
// assertions, and builds a bounded Evidence Pack. The test report is deliberately absent and the
// metrics source deliberately unavailable, so the pack must say so instead of concluding anything.
// An agent then proposes a hypothesis (it stays proposed), a replay is compared with the original,
// feedback is recorded, and a human records the episode outcome with evidence.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::broker::recipe::{Budget, RecipeRef};
use super::broker::run::{
    EvidenceQueryBroker, EvidenceQueryRequest, KnownRefs, QueryAuditRecord, ReplayComparison,
};
use super::format::record::EvidenceType;
use super::ingest::contributions::{
    ContextContribution, ContributionType, Locality, Namespace, Proposal, TargetKind, TargetRef,
};
use super::ingest::episode::{EpisodeOutcome, PackFeedback, WorkEpisode};
use super::ingest::pack::{EvidencePack, PackLimits};
use super::resources::connector::{ConnectorStatus, SourceRecord};
use super::resources::fixture::{FixtureBehavior, FixtureConnector};
use super::resources::registry::{
    Capabilities, Operation, ResourceDefinition, ResourceRegistry, ResourceScope, ResourceStatus,
    ResourceType,
};
use super::resources::scope::TimeWindow;
use super::store::CaptureStore;
use super::{
    AccessPolicy, Actor, ActorKind, CaptureError, CaptureResult, Clock, ErrorCode, Sensitivity,
    Truth,
};

/** The fixture of the demonstration */
const FIXTURE: &str = include_str!("fixtures/failed_deployment.json");

/** Fixed start of the demonstration clock (Unix milliseconds) */
pub(crate) const DEMO_EPOCH: u64 = 1_760_000_000_000;

/** One fixture resource
 * Fields
    - resource_id: String - resource
    - resource_type: ResourceType - kind
    - evidence_types: Vec<EvidenceType> - evidence it serves
    - filters: Vec<String> - filters it accepts
    - unavailable: bool - whether the source is down in the scenario
    - records: Vec<SourceRecord> - its records
*/
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FixtureResource {
    pub(crate) resource_id: String,
    pub(crate) resource_type: ResourceType,
    pub(crate) evidence_types: Vec<EvidenceType>,
    pub(crate) filters: Vec<String>,
    #[serde(default)]
    pub(crate) unavailable: bool,
    pub(crate) records: Vec<SourceRecord>,
}

/** The demonstration scenario
 * Fields
    - repository: String - repository
    - environments: Vec<String> - environments
    - task: String - task
    - pull_request: String - pull request
    - deployment: String - deployment
    - zones: Vec<String> - Landscape zones
    - flows: Vec<String> - Landscape flows
    - resources: Vec<FixtureResource> - sources
*/
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Scenario {
    pub(crate) repository: String,
    pub(crate) environments: Vec<String>,
    pub(crate) task: String,
    pub(crate) pull_request: String,
    pub(crate) deployment: String,
    pub(crate) zones: Vec<String>,
    pub(crate) flows: Vec<String>,
    pub(crate) resources: Vec<FixtureResource>,
}

/** Load the demonstration scenario
 * Input
    - None
 * Output
    - CaptureResult<Scenario>
    - Error if the embedded fixture is malformed
*/
pub(crate) fn scenario() -> CaptureResult<Scenario> {
    serde_json::from_str(FIXTURE)
        .map_err(|error| CaptureError::new(ErrorCode::Storage, format!("demo fixture: {error}")))
}

/** The definition of a fixture resource
 * Input
    - resource: &FixtureResource - fixture resource
    - scenario: &Scenario - scenario
 * Output
    - ResourceDefinition
*/
pub(crate) fn definition(resource: &FixtureResource, scenario: &Scenario) -> ResourceDefinition {
    ResourceDefinition {
        resource_id: resource.resource_id.clone(),
        resource_type: resource.resource_type,
        connector_type: "fixture".into(),
        capabilities: Capabilities {
            operations: vec![
                Operation::Discover,
                Operation::Fetch,
                Operation::Query,
                Operation::Metadata,
                Operation::Health,
            ],
            evidence_types: resource.evidence_types.clone(),
            filters: resource.filters.clone(),
        },
        scope: ResourceScope {
            repositories: vec![scenario.repository.clone()],
            environments: scenario.environments.clone(),
            ..ResourceScope::default()
        },
        auth_reference: Some(format!(
            "env:DEMO_{}_TOKEN",
            resource.resource_id.to_ascii_uppercase().replace('-', "_")
        )),
        configuration: [("fixture".to_string(), "failed_deployment".to_string())]
            .into_iter()
            .collect(),
        configuration_version: 1,
        status: ResourceStatus::Active,
        created_at: 0,
        updated_at: 0,
    }
}

/** Build a broker over the scenario's fixture resources
 * Input
    - scenario: &Scenario - scenario
    - clock: Clock - clock
 * Output
    - CaptureResult<EvidenceQueryBroker>
    - Error if a resource fails registration
*/
pub(crate) fn broker(scenario: &Scenario, clock: Clock) -> CaptureResult<EvidenceQueryBroker> {
    let mut registry = ResourceRegistry::default();
    for resource in &scenario.resources {
        registry.register(definition(resource, scenario), clock.now())?;
    }
    let mut broker = EvidenceQueryBroker::new(registry, CaptureStore::in_memory(), clock)?;
    for resource in &scenario.resources {
        let mut connector = FixtureConnector::new(resource.records.clone());
        if resource.unavailable {
            connector =
                connector.with_behavior(FixtureBehavior::Fail(ConnectorStatus::Unavailable));
        }
        broker.connect(&resource.resource_id, Box::new(connector))?;
    }
    Ok(broker)
}

/** The on-call engineer's permissions: every scenario resource, the repository, its environments
 * Input
    - scenario: &Scenario - scenario
 * Output
    - AccessPolicy
*/
pub(crate) fn engineer(scenario: &Scenario) -> AccessPolicy {
    AccessPolicy {
        actor: Actor::new("oncall@acme.example", ActorKind::Human),
        resources: scenario
            .resources
            .iter()
            .map(|resource| resource.resource_id.clone())
            .collect(),
        repositories: BTreeSet::from([scenario.repository.clone()]),
        environments: scenario.environments.iter().cloned().collect(),
        evidence_types: None,
        max_sensitivity: Sensitivity::Confidential,
        may_review: true,
    }
}

/** The investigation request of the scenario
 * Input
    - scenario: &Scenario - scenario
 * Output
    - EvidenceQueryRequest
*/
pub(crate) fn request(scenario: &Scenario) -> EvidenceQueryRequest {
    EvidenceQueryRequest {
        episode_id: None,
        task_id: Some(scenario.task.clone()),
        question: format!(
            "Why did deployment {} of {} fail?",
            scenario.deployment, scenario.task
        ),
        investigation_class: "deployment-failure".into(),
        recipe: Some(RecipeRef {
            id: "failed-deployment".into(),
            version: Some(1),
        }),
        plan: None,
        known: KnownRefs {
            repository: Some(scenario.repository.clone()),
            pull_requests: vec![scenario.pull_request.clone()],
            deployments: vec![scenario.deployment.clone()],
            zones: scenario.zones.clone(),
            flows: scenario.flows.clone(),
            ..KnownRefs::default()
        },
        resources: Vec::new(),
        required_evidence: Vec::new(),
        evidence_types: Vec::new(),
        time_window: TimeWindow::default(),
        budget: Budget::default(),
        freshness_ms: None,
        pack_limits: PackLimits::default(),
    }
}

/** What the demonstration produced
 * Fields
    - pack: EvidencePack - the Evidence Pack
    - audit: QueryAuditRecord - the query audit record
    - replay: Option<ReplayComparison> - comparison of a replay with the original
    - contributions: Vec<ContextContribution> - contributions proposed during the episode
    - episode: Option<WorkEpisode> - the work episode with its outcome and feedback
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DemoReport {
    pub(crate) pack: EvidencePack,
    pub(crate) audit: QueryAuditRecord,
    pub(crate) replay: Option<ReplayComparison>,
    pub(crate) contributions: Vec<ContextContribution>,
    pub(crate) episode: Option<WorkEpisode>,
}

/** Run the demonstration
 * Input
    - None
 * Output
    - CaptureResult<DemoReport>
    - Error if any stage fails
*/
pub(crate) fn run() -> CaptureResult<DemoReport> {
    let scenario = scenario()?;
    let mut broker = broker(&scenario, Clock::fixed(DEMO_EPOCH))?;
    let policy = engineer(&scenario);
    let result = broker.investigate(request(&scenario), &policy)?;
    let episode_id = result.pack.episode_id.clone().unwrap_or_default();
    let log = result
        .pack
        .evidence
        .iter()
        .find(|evidence| evidence.evidence_type == EvidenceType::RuntimeLog)
        .map(|evidence| evidence.evidence_id.clone());
    let agent = Actor::new("claude@ses_demo01", ActorKind::Agent);
    let zone_version = result
        .pack
        .scope
        .zones
        .first()
        .and_then(|zone| zone.version.clone());
    let now = broker.clock.now();
    broker.store.propose(
        &agent,
        Proposal {
            target_ref: TargetRef { kind: TargetKind::Zone, id: scenario.zones[0].clone(), version: zone_version },
            episode_id: Some(episode_id.clone()),
            contribution_type: ContributionType::Hypothesis,
            namespace: Namespace::AgentProposed,
            content: json!({"hypothesis": "Jobs queued before the deploy lack idempotency_key, so the new worker crashes on them", "suggested_check": "count queued refund jobs without idempotency_key"}),
            evidence_refs: log.into_iter().collect(),
            locality: Locality::Reusable,
            valid_until: None,
        },
        now,
    )?;
    let now = broker.clock.now();
    broker.store.propose(
        &agent,
        Proposal {
            target_ref: TargetRef { kind: TargetKind::WorkEpisode, id: episode_id.clone(), version: None },
            episode_id: Some(episode_id.clone()),
            contribution_type: ContributionType::Question,
            namespace: Namespace::AgentProposed,
            content: json!({"question": "Did the test suite run against jobs created before the migration?", "required_evidence": ["test_report"]}),
            evidence_refs: Vec::new(),
            locality: Locality::TaskLocal,
            valid_until: None,
        },
        now,
    )?;
    let replay = broker.replay(&result.audit.query_id, &policy)?;
    let deployment = result
        .pack
        .evidence
        .iter()
        .filter(|evidence| {
            matches!(
                evidence.evidence_type,
                EvidenceType::Deployment | EvidenceType::Verification
            )
        })
        .map(|evidence| evidence.evidence_id.clone())
        .collect::<Vec<_>>();
    let now = broker.clock.now();
    if let Some(episode) = broker.store.episodes.get_mut(&episode_id) {
        episode.feedback.push(PackFeedback {
            pack_id: result.pack.pack_id.clone(),
            requester: policy.actor.clone(),
            sufficient: Truth::No,
            additional_evidence: vec![EvidenceType::TestReport, EvidenceType::Metric],
            notes: "The test report and runtime metrics are needed before concluding".into(),
            at: now,
        });
        episode.record_outcome(&policy.actor, EpisodeOutcome::Failed, deployment, now)?;
    }
    Ok(DemoReport {
        contributions: broker
            .store
            .contributions
            .contributions
            .values()
            .cloned()
            .collect(),
        episode: broker.store.episodes.get(&episode_id).cloned(),
        replay: replay.audit.replay,
        pack: result.pack,
        audit: result.audit,
    })
}
