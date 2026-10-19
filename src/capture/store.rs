// The Work Capture store. Like the rest of Crane's runtime evidence it lives in the trust
// directory's runtime folder (runtime/capture), outside the repository, as JSON files kept
// logically separate: raw artifact metadata and content blobs, normalized evidence, entities and
// relations, assertions, contributions, episodes, questions, packs, query audit records, and
// ingestion state (seen deliveries, source cursors). In memory it keeps indexes by source record,
// evidence type, episode, and observed time. The layout is versioned (state.json); a newer layout
// is refused, an older one migrated. Ingestion is idempotent: a repeated webhook delivery or a
// record already captured is recognized, and every path that observed an artifact is kept.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capture::broker::run::QueryAuditRecord;
use crate::capture::correlate::assertions::Assertion;
use crate::capture::correlate::graph::{Direction, EvidenceGraph};
use crate::capture::correlate::model::{
    entity_id, Entity, EntityKind, NodeRef, Relation, RelationStatus, RelationType,
};
use crate::capture::format::artifact::{capture, RawArtifact};
use crate::capture::format::parsers::normalize;
use crate::capture::format::record::{EvidenceRecord, EvidenceType, ParseStatus};
use crate::capture::ingest::contributions::{
    ContextContribution, ContributionLedger, ContributionStatus, Proposal, TargetKind,
};
use crate::capture::ingest::episode::{Question, WorkEpisode};
use crate::capture::ingest::pack::EvidencePack;
use crate::capture::resources::connector::SourceRecord;
use crate::capture::resources::registry::ResourceDefinition;
use crate::capture::{AccessPolicy, Actor, CaptureError, CaptureResult, ErrorCode, SCHEMA_VERSION};
use crate::platform::files::{read_lines, write_atomic, Lock};

/** How a record arrived
 * Fields
    - kind: String - query, webhook, retry, or backfill
    - id: String - delivery id (webhook) or query and step
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IngestPath {
    pub(crate) kind: String,
    pub(crate) id: String,
}

impl IngestPath {
    /** The path as one string ("webhook:DELIVERY")
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn label(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}

/** What ingestion did with a record
 * Variants
    - New - a new artifact and evidence record
    - Duplicate - already captured (same delivery, or same record, revision, and content)
    - NewRevision - a changed version of a record captured before (it supersedes the older one)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IngestKind {
    New,
    Duplicate,
    NewRevision,
}

/** The result of ingesting one record
 * Fields
    - evidence_id: String - evidence record
    - kind: IngestKind - new, duplicate, or new revision
    - parse_status: ParseStatus - parse status of the record
    - payload_bytes: usize - stored content size
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IngestOutcome {
    pub(crate) evidence_id: String,
    pub(crate) kind: IngestKind,
    pub(crate) parse_status: ParseStatus,
    pub(crate) payload_bytes: usize,
}

/** Ingestion state kept with the store
 * Fields
    - schema_version: u32 - store layout version
    - deliveries: BTreeMap<String, String> - webhook delivery id to evidence id
    - cursors: BTreeMap<String, String> - resource id to its incremental cursor
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct StoreState {
    schema_version: u32,
    deliveries: BTreeMap<String, String>,
    cursors: BTreeMap<String, String>,
}

/** The Work Capture store
 * Fields
    - directory: Option<PathBuf> - persistence directory, None for an in-memory store
    - artifacts: BTreeMap<String, RawArtifact> - raw artifacts by id
    - blobs: BTreeMap<String, Value> - redacted content by content hash
    - evidence: BTreeMap<String, EvidenceRecord> - evidence by id
    - graph: EvidenceGraph - entities and relations
    - assertions: BTreeMap<String, Assertion> - assertions by id
    - contributions: ContributionLedger - contributions
    - episodes: BTreeMap<String, WorkEpisode> - episodes by id
    - questions: BTreeMap<String, Question> - questions by id
    - packs: BTreeMap<String, EvidencePack> - packs by id
    - audits: Vec<QueryAuditRecord> - query audit records in order
    - state: StoreState - deliveries and cursors
    - by_source: BTreeMap<(String, String), Vec<String>> - (resource, record) to evidence ids
    - by_type: BTreeMap<EvidenceType, BTreeSet<String>> - type to evidence ids
    - by_episode: BTreeMap<String, BTreeSet<String>> - episode to evidence ids
    - by_time: BTreeMap<u64, BTreeSet<String>> - observed time to evidence ids
    - by_artifact: BTreeMap<String, String> - artifact id to evidence id
*/
#[derive(Debug, Default)]
pub(crate) struct CaptureStore {
    directory: Option<PathBuf>,
    pub(crate) artifacts: BTreeMap<String, RawArtifact>,
    pub(crate) blobs: BTreeMap<String, Value>,
    pub(crate) evidence: BTreeMap<String, EvidenceRecord>,
    pub(crate) graph: EvidenceGraph,
    pub(crate) assertions: BTreeMap<String, Assertion>,
    pub(crate) contributions: ContributionLedger,
    pub(crate) episodes: BTreeMap<String, WorkEpisode>,
    pub(crate) questions: BTreeMap<String, Question>,
    pub(crate) packs: BTreeMap<String, EvidencePack>,
    pub(crate) audits: Vec<QueryAuditRecord>,
    state: StoreState,
    by_source: BTreeMap<(String, String), Vec<String>>,
    by_type: BTreeMap<EvidenceType, BTreeSet<String>>,
    by_episode: BTreeMap<String, BTreeSet<String>>,
    by_time: BTreeMap<u64, BTreeSet<String>>,
    by_artifact: BTreeMap<String, String>,
}

/** Build a storage error
 * Input
    - message: String - explanation
 * Output
    - CaptureError
*/
fn storage(message: String) -> CaptureError {
    CaptureError::new(ErrorCode::Storage, message)
}

/** Read a JSON-lines collection
 * Input
    - path: &Path - file
 * Output
    - CaptureResult<Vec<T>>
    - Error when the file is unreadable or a line does not match the schema
*/
fn read_collection<T: DeserializeOwned>(path: &Path) -> CaptureResult<Vec<T>> {
    read_lines(path)
        .map_err(storage)?
        .into_iter()
        .map(|value| {
            serde_json::from_value(value)
                .map_err(|error| storage(format!("{}: {error}", path.display())))
        })
        .collect()
}

/** Write a JSON-lines collection atomically
 * Input
    - path: &Path - file
    - items: impl Iterator<Item = &T> - items
 * Output
    - CaptureResult<()>
    - Error when the file cannot be written
*/
fn write_collection<'a, T: Serialize + 'a>(
    path: &Path,
    items: impl Iterator<Item = &'a T>,
) -> CaptureResult<()> {
    let mut text = String::new();
    for item in items {
        text.push_str(&serde_json::to_string(item).map_err(|error| storage(error.to_string()))?);
        text.push('\n');
    }
    write_atomic(path, text.as_bytes()).map_err(storage)
}

impl CaptureStore {
    /** An empty in-memory store (fixtures and tests)
     * Input
        - None
     * Output
        - CaptureStore
    */
    pub(crate) fn in_memory() -> Self {
        Self {
            state: StoreState {
                schema_version: SCHEMA_VERSION,
                ..StoreState::default()
            },
            ..Self::default()
        }
    }

    /** Open a persistent store, loading and migrating what it holds
     * Input
        - directory: &Path - store directory
     * Output
        - CaptureResult<CaptureStore>
        - Error when the layout is newer than this Crane or a file is unreadable
    */
    pub(crate) fn open(directory: &Path) -> CaptureResult<Self> {
        let _lock = Lock::acquire(&directory.join("capture.lock")).map_err(storage)?;
        let mut store = Self::in_memory();
        store.directory = Some(directory.to_path_buf());
        let state_path = directory.join("state.json");
        if state_path.exists() {
            let text =
                std::fs::read_to_string(&state_path).map_err(|error| storage(error.to_string()))?;
            let state: StoreState = serde_json::from_str(&text)
                .map_err(|error| storage(format!("state.json: {error}")))?;
            store.state = migrate(state)?;
        }
        for artifact in read_collection::<RawArtifact>(&directory.join("artifacts.jsonl"))? {
            store
                .artifacts
                .insert(artifact.artifact_id.clone(), artifact);
        }
        for line in read_collection::<(String, Value)>(&directory.join("blobs.jsonl"))? {
            store.blobs.insert(line.0, line.1);
        }
        for record in read_collection::<EvidenceRecord>(&directory.join("evidence.jsonl"))? {
            store.index(&record);
            store
                .graph
                .register(NodeRef::Evidence(record.evidence_id.clone()));
            store.evidence.insert(record.evidence_id.clone(), record);
        }
        for entity in read_collection::<Entity>(&directory.join("entities.jsonl"))? {
            store.graph.add_entity(entity);
        }
        for assertion in read_collection::<Assertion>(&directory.join("assertions.jsonl"))? {
            store
                .graph
                .register(NodeRef::Assertion(assertion.assertion_id.clone()));
            store
                .assertions
                .insert(assertion.assertion_id.clone(), assertion);
        }
        for contribution in
            read_collection::<ContextContribution>(&directory.join("contributions.jsonl"))?
        {
            store
                .graph
                .register(NodeRef::Contribution(contribution.contribution_id.clone()));
            store
                .contributions
                .contributions
                .insert(contribution.contribution_id.clone(), contribution);
        }
        for relation in read_collection::<Relation>(&directory.join("relations.jsonl"))? {
            store.graph.add_relation(relation)?;
        }
        for episode in read_collection::<WorkEpisode>(&directory.join("episodes.jsonl"))? {
            store.episodes.insert(episode.episode_id.clone(), episode);
        }
        for question in read_collection::<Question>(&directory.join("questions.jsonl"))? {
            store
                .questions
                .insert(question.question_id.clone(), question);
        }
        for pack in read_collection::<EvidencePack>(&directory.join("packs.jsonl"))? {
            store.packs.insert(pack.pack_id.clone(), pack);
        }
        store.audits = read_collection(&directory.join("audit.jsonl"))?;
        Ok(store)
    }

    /** Persist every collection (each file replaced atomically); an in-memory store does nothing
     * Input
        - None (uses self)
     * Output
        - CaptureResult<()>
        - Error when a file cannot be written
    */
    pub(crate) fn save(&self) -> CaptureResult<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        let _lock = Lock::acquire(&directory.join("capture.lock")).map_err(storage)?;
        write_collection(&directory.join("artifacts.jsonl"), self.artifacts.values())?;
        let blobs = self.blobs.iter().collect::<Vec<_>>();
        write_collection(&directory.join("blobs.jsonl"), blobs.iter())?;
        write_collection(&directory.join("evidence.jsonl"), self.evidence.values())?;
        write_collection(
            &directory.join("entities.jsonl"),
            self.graph.entities.values(),
        )?;
        write_collection(
            &directory.join("relations.jsonl"),
            self.graph.relations.values(),
        )?;
        write_collection(
            &directory.join("assertions.jsonl"),
            self.assertions.values(),
        )?;
        write_collection(
            &directory.join("contributions.jsonl"),
            self.contributions.contributions.values(),
        )?;
        write_collection(&directory.join("episodes.jsonl"), self.episodes.values())?;
        write_collection(&directory.join("questions.jsonl"), self.questions.values())?;
        write_collection(&directory.join("packs.jsonl"), self.packs.values())?;
        write_collection(&directory.join("audit.jsonl"), self.audits.iter())?;
        let state =
            serde_json::to_vec_pretty(&self.state).map_err(|error| storage(error.to_string()))?;
        write_atomic(&directory.join("state.json"), &state).map_err(storage)
    }

    /** Add a record to the indexes
     * Input
        - record: &EvidenceRecord - record
     * Output
        - None
    */
    fn index(&mut self, record: &EvidenceRecord) {
        let id = record.evidence_id.clone();
        self.by_artifact
            .insert(record.source_ref.artifact_id.clone(), id.clone());
        self.by_source
            .entry((
                record.source_ref.resource_id.clone(),
                record.source_ref.record_id.clone(),
            ))
            .or_default()
            .push(id.clone());
        self.by_type
            .entry(record.evidence_type)
            .or_default()
            .insert(id.clone());
        if let Some(episode) = &record.episode_id {
            self.by_episode
                .entry(episode.clone())
                .or_default()
                .insert(id.clone());
        }
        if let Some(at) = record.observed_at {
            self.by_time.entry(at).or_default().insert(id);
        }
    }

    /** Ingest one source record: recognize repeated deliveries and already-captured content (the
     * new path is added to the artifact and the record), keep a changed revision as a new record
     * that supersedes the older one, otherwise capture, normalize, and index it
     * Input
        - definition: &ResourceDefinition - resource it came from
        - record: &SourceRecord - record
        - path: &IngestPath - how it arrived
        - episode_id: Option<&str> - work episode
        - query_id: Option<&str> - investigation
        - now: u64 - Unix milliseconds
     * Output
        - IngestOutcome
    */
    pub(crate) fn ingest(
        &mut self,
        definition: &ResourceDefinition,
        record: &SourceRecord,
        path: &IngestPath,
        episode_id: Option<&str>,
        query_id: Option<&str>,
        now: u64,
    ) -> IngestOutcome {
        let label = path.label();
        if path.kind == "webhook" {
            if let Some(existing) = self.state.deliveries.get(&path.id).cloned() {
                return self.duplicate(&existing, &label);
            }
        }
        let (artifact, stored) = capture(definition, record, now, &label);
        if let Some(existing) = self.by_artifact.get(&artifact.artifact_id).cloned() {
            if path.kind == "webhook" {
                self.state
                    .deliveries
                    .insert(path.id.clone(), existing.clone());
            }
            return self.duplicate(&existing, &label);
        }
        let mut evidence = normalize(
            definition,
            record,
            &artifact,
            stored.as_ref(),
            now,
            episode_id,
            query_id,
        );
        let previous = self
            .by_source
            .get(&(definition.resource_id.clone(), record.record_id.clone()))
            .and_then(|ids| ids.last().cloned());
        let kind = if previous.is_some() {
            IngestKind::NewRevision
        } else {
            IngestKind::New
        };
        evidence.supersedes = previous.clone();
        if let (Some(hash), Some(content)) = (&artifact.content_hash, stored) {
            self.blobs.insert(hash.clone(), content);
        }
        let outcome = IngestOutcome {
            evidence_id: evidence.evidence_id.clone(),
            kind,
            parse_status: evidence.parse_status,
            payload_bytes: artifact.size_bytes,
        };
        if path.kind == "webhook" {
            self.state
                .deliveries
                .insert(path.id.clone(), evidence.evidence_id.clone());
        }
        self.artifacts
            .insert(artifact.artifact_id.clone(), artifact);
        self.index(&evidence);
        self.graph
            .register(NodeRef::Evidence(evidence.evidence_id.clone()));
        if let Some(previous) = previous {
            let relation = Relation::new(
                RelationType::Supersedes,
                NodeRef::Evidence(evidence.evidence_id.clone()),
                NodeRef::Evidence(previous),
                RelationStatus::Observed,
                "source_revision",
                vec![evidence.evidence_id.clone()],
            );
            let _ = self.graph.add_relation(relation);
        }
        self.evidence.insert(evidence.evidence_id.clone(), evidence);
        outcome
    }

    /** Record that another path observed an already captured record
     * Input
        - evidence_id: &str - existing record
        - label: &str - ingestion path
     * Output
        - IngestOutcome (Duplicate)
    */
    fn duplicate(&mut self, evidence_id: &str, label: &str) -> IngestOutcome {
        let mut parse_status = ParseStatus::Failed;
        if let Some(evidence) = self.evidence.get_mut(evidence_id) {
            parse_status = evidence.parse_status;
            if !evidence
                .provenance
                .ingestion_paths
                .iter()
                .any(|path| path == label)
            {
                evidence.provenance.ingestion_paths.push(label.into());
            }
            if let Some(artifact) = self.artifacts.get_mut(&evidence.source_ref.artifact_id) {
                if !artifact.observed_via.iter().any(|path| path == label) {
                    artifact.observed_via.push(label.into());
                }
            }
        }
        IngestOutcome {
            evidence_id: evidence_id.into(),
            kind: IngestKind::Duplicate,
            parse_status,
            payload_bytes: 0,
        }
    }

    /** Remember a resource's incremental cursor
     * Input
        - resource_id: &str - resource
        - cursor: &str - cursor (event id, commit, timestamp, page token)
     * Output
        - None
    */
    pub(crate) fn set_cursor(&mut self, resource_id: &str, cursor: &str) {
        self.state.cursors.insert(resource_id.into(), cursor.into());
    }

    /** A resource's incremental cursor
     * Input
        - resource_id: &str - resource
     * Output
        - Option<&String>
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn cursor(&self, resource_id: &str) -> Option<&String> {
        self.state.cursors.get(resource_id)
    }

    /** Evidence observed within a time window, oldest first, bounded
     * Input
        - since: u64 - start (Unix ms), inclusive
        - until: u64 - end (Unix ms), inclusive
        - limit: usize - most records
     * Output
        - Vec<&EvidenceRecord>
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn evidence_in_window(
        &self,
        since: u64,
        until: u64,
        limit: usize,
    ) -> Vec<&EvidenceRecord> {
        self.by_time
            .range(since..=until)
            .flat_map(|(_, ids)| ids.iter())
            .filter_map(|id| self.evidence.get(id))
            .take(limit)
            .collect()
    }

    /** Evidence of a type, bounded
     * Input
        - evidence_type: EvidenceType - type
        - limit: usize - most records
     * Output
        - Vec<&EvidenceRecord>
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn evidence_of_type(
        &self,
        evidence_type: EvidenceType,
        limit: usize,
    ) -> Vec<&EvidenceRecord> {
        self.by_type
            .get(&evidence_type)
            .into_iter()
            .flatten()
            .filter_map(|id| self.evidence.get(id))
            .take(limit)
            .collect()
    }

    /** Evidence observed on an entity, bounded
     * Input
        - entity_id: &str - entity
        - limit: usize - most records
     * Output
        - Vec<&EvidenceRecord>
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn evidence_for_entity(
        &self,
        entity_id: &str,
        limit: usize,
    ) -> Vec<&EvidenceRecord> {
        self.graph
            .relations_of(
                &NodeRef::Entity(entity_id.into()),
                &[RelationType::ObservedOn],
                Direction::Incoming,
            )
            .into_iter()
            .filter_map(|relation| match &relation.source {
                NodeRef::Evidence(id) => self.evidence.get(id),
                _ => None,
            })
            .take(limit)
            .collect()
    }

    /** Assertions an evidence record supports or contradicts
     * Input
        - evidence_id: &str - evidence
     * Output
        - (Vec<&Assertion>, Vec<&Assertion>) supported and contradicted
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn assertions_for_evidence(
        &self,
        evidence_id: &str,
    ) -> (Vec<&Assertion>, Vec<&Assertion>) {
        let node = NodeRef::Evidence(evidence_id.into());
        let mut supported = Vec::new();
        let mut contradicted = Vec::new();
        for relation in self.graph.relations_of(
            &node,
            &[RelationType::Supports, RelationType::Contradicts],
            Direction::Outgoing,
        ) {
            if let Some(assertion) = self.assertions.get(relation.target.id()) {
                if relation.relation_type == RelationType::Supports {
                    supported.push(assertion);
                } else {
                    contradicted.push(assertion);
                }
            }
        }
        (supported, contradicted)
    }

    /** Store an assertion and its relations: SUPPORTS and CONTRADICTS from evidence, DERIVED_FROM
     * to parent assertions, APPLIES_TO its subject when the subject is in the graph
     * Input
        - assertion: Assertion - assertion
     * Output
        - CaptureResult<String> assertion id
        - Error when a relation is invalid
    */
    pub(crate) fn add_assertion(&mut self, assertion: Assertion) -> CaptureResult<String> {
        let id = assertion.assertion_id.clone();
        let node = NodeRef::Assertion(id.clone());
        self.graph.register(node.clone());
        let mut relations = Vec::new();
        for (evidence, relation_type) in assertion
            .supporting
            .iter()
            .map(|id| (id, RelationType::Supports))
            .chain(
                assertion
                    .contradicting
                    .iter()
                    .map(|id| (id, RelationType::Contradicts)),
            )
        {
            relations.push(Relation::new(
                relation_type,
                NodeRef::Evidence(evidence.clone()),
                node.clone(),
                RelationStatus::Inferred,
                &assertion.derivation.rule,
                vec![evidence.clone()],
            ));
        }
        for parent in &assertion.derived_from {
            relations.push(Relation::new(
                RelationType::DerivedFrom,
                node.clone(),
                NodeRef::Assertion(parent.clone()),
                RelationStatus::Inferred,
                &assertion.derivation.rule,
                Vec::new(),
            ));
        }
        if self.graph.entities.contains_key(&assertion.claim.subject) {
            relations.push(Relation::new(
                RelationType::AppliesTo,
                node.clone(),
                NodeRef::Entity(assertion.claim.subject.clone()),
                RelationStatus::Declared,
                &assertion.derivation.rule,
                Vec::new(),
            ));
        }
        self.assertions.insert(id.clone(), assertion);
        for relation in relations {
            self.graph.add_relation(relation)?;
        }
        Ok(id)
    }

    /** Make sure an episode exists and is in the graph (with the task it belongs to)
     * Input
        - episode: WorkEpisode - episode to add when missing
     * Output
        - String episode id
    */
    pub(crate) fn ensure_episode(&mut self, episode: WorkEpisode) -> String {
        let id = episode.episode_id.clone();
        let entity = self
            .graph
            .add_entity(Entity::new(EntityKind::WorkEpisode, &id));
        if let Some(task) = &episode.task_id {
            let task_entity = self.graph.add_entity(Entity::new(EntityKind::Task, task));
            let relation = Relation::new(
                RelationType::Contains,
                NodeRef::Entity(task_entity),
                NodeRef::Entity(entity),
                RelationStatus::Declared,
                "episode.task",
                Vec::new(),
            );
            let _ = self.graph.add_relation(relation);
        }
        self.episodes.entry(id.clone()).or_insert(episode);
        id
    }

    /** Propose a contribution and link it: PROPOSED_FOR its target (a zone or flow is referenced
     * by its existing Landscape id and version) and DERIVED_FROM its evidence
     * Input
        - actor: &Actor - proposing actor
        - proposal: Proposal - proposal
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<String> contribution id
        - Error when the ledger refuses it or an entity target does not exist
    */
    pub(crate) fn propose(
        &mut self,
        actor: &Actor,
        proposal: Proposal,
        now: u64,
    ) -> CaptureResult<String> {
        let target = match proposal.target_ref.kind {
            TargetKind::Entity => {
                if !self.graph.entities.contains_key(&proposal.target_ref.id) {
                    return Err(CaptureError::new(
                        ErrorCode::NotFound,
                        format!("no entity '{}'", proposal.target_ref.id),
                    ));
                }
                proposal.target_ref.id.clone()
            }
            TargetKind::WorkEpisode => {
                if !self.episodes.contains_key(&proposal.target_ref.id) {
                    return Err(CaptureError::new(
                        ErrorCode::NotFound,
                        format!("no work episode '{}'", proposal.target_ref.id),
                    ));
                }
                entity_id(EntityKind::WorkEpisode, &proposal.target_ref.id)
            }
            TargetKind::Zone | TargetKind::Flow => {
                let kind = if proposal.target_ref.kind == TargetKind::Zone {
                    EntityKind::Zone
                } else {
                    EntityKind::Flow
                };
                let mut entity = Entity::new(kind, &proposal.target_ref.id);
                entity.version = proposal.target_ref.version.clone();
                self.graph.add_entity(entity)
            }
        };
        let evidence = &self.evidence;
        let id =
            self.contributions
                .propose(actor, proposal, &|id| evidence.contains_key(id), now)?;
        let node = NodeRef::Contribution(id.clone());
        self.graph.register(node.clone());
        self.graph.add_relation(Relation::new(
            RelationType::ProposedFor,
            node.clone(),
            NodeRef::Entity(target),
            RelationStatus::Declared,
            "contribution.target",
            Vec::new(),
        ))?;
        let refs = self
            .contributions
            .contributions
            .get(&id)
            .map(|contribution| contribution.evidence_refs.clone())
            .unwrap_or_default();
        for evidence in refs {
            self.graph.add_relation(Relation::new(
                RelationType::DerivedFrom,
                node.clone(),
                NodeRef::Evidence(evidence.clone()),
                RelationStatus::Declared,
                "contribution.evidence",
                vec![evidence],
            ))?;
        }
        Ok(id)
    }

    /** Review a contribution (see ContributionLedger::review)
     * Input
        - reviewer: &AccessPolicy - reviewer
        - id: &str - contribution
        - decision: ContributionStatus - Accepted or Rejected
        - reason: &str - why
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<()>
        - Error when the review is refused
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn review(
        &mut self,
        reviewer: &AccessPolicy,
        id: &str,
        decision: ContributionStatus,
        reason: &str,
        now: u64,
    ) -> CaptureResult<()> {
        self.contributions
            .review(reviewer, id, decision, reason, now)
    }

    /** Find a query audit record
     * Input
        - query_id: &str - investigation
     * Output
        - Option<&QueryAuditRecord>
    */
    pub(crate) fn audit(&self, query_id: &str) -> Option<&QueryAuditRecord> {
        self.audits.iter().find(|audit| audit.query_id == query_id)
    }
}

/** Bring stored ingestion state to the current layout
 * Input
    - state: StoreState - stored state
 * Output
    - CaptureResult<StoreState>
    - Error when the layout is newer than this Crane understands
*/
fn migrate(mut state: StoreState) -> CaptureResult<StoreState> {
    if state.schema_version > SCHEMA_VERSION {
        return Err(CaptureError::new(ErrorCode::SchemaVersion, format!("the capture store has layout {} but this Crane understands up to {SCHEMA_VERSION}; upgrade Crane", state.schema_version)));
    }
    // Layout 0 (no version recorded) is identical to layout 1
    state.schema_version = SCHEMA_VERSION;
    Ok(state)
}
