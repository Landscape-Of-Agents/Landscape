// The Work Capture store. Like the rest of Crane's runtime evidence it lives in the trust
// directory's runtime folder (runtime/capture), outside the repository, as JSON files kept
// logically separate: raw artifact metadata and content blobs, normalized evidence, entities and
// relations, assertions, contributions, episodes (with pack feedback), questions, packs, query
// audit records, recipe proposals, and ingestion state (seen deliveries, source cursors, pinned
// recipe digests). In memory it keeps indexes by source record,
// evidence type, episode, and observed time. The layout is versioned (state.json); a newer layout
// is refused, an older one migrated. Ingestion is idempotent: a repeated webhook delivery or a
// record already captured is recognized, and every path that observed an artifact is kept.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capture::broker::proposals::{ProposalStatus, RecipeProposal};
use crate::capture::broker::recipe::EvidenceRecipe;
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
use crate::capture::ingest::episode::{
    EpisodeOutcome, PackFeedback, Question, QuestionState, WorkEpisode,
};
use crate::capture::ingest::pack::EvidencePack;
use crate::capture::resources::connector::SourceRecord;
use crate::capture::resources::registry::ResourceDefinition;
use crate::capture::{
    AccessPolicy, Actor, ActorKind, CaptureError, CaptureResult, ErrorCode, Truth, SCHEMA_VERSION,
};
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
    - recipes: BTreeMap<String, String> - "recipe@vN" to the digest of the recipe version this
      store has run (absent in stores written before recipes were pinned)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct StoreState {
    schema_version: u32,
    deliveries: BTreeMap<String, String>,
    cursors: BTreeMap<String, String>,
    #[serde(default)]
    recipes: BTreeMap<String, String>,
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
    - recipe_proposals: BTreeMap<String, RecipeProposal> - recipe proposals by id
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
    pub(crate) recipe_proposals: BTreeMap<String, RecipeProposal>,
    audit_index: BTreeMap<String, usize>,
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
    // A collection that did not change is left as it is (no rewrite, no new file time)
    if std::fs::read(path).is_ok_and(|current| current == text.as_bytes()) {
        return Ok(());
    }
    write_atomic(path, text.as_bytes()).map_err(storage)
}

/** Merge a collection read from disk with the one held in memory, by identifier: an item only on
 * one side is kept, an item on both sides is combined (memory first)
 * Input
    - stored: Vec<T> - the collection as another process may have saved it
    - mine: &BTreeMap<String, T> - this store's collection
    - key: impl Fn(&T) -> String - an item's identifier
    - combine: impl Fn(&T, &T) -> T - combine this store's item with the stored one
 * Output
    - BTreeMap<String, T>
*/
fn merge_keyed<T: Clone>(
    stored: Vec<T>,
    mine: &BTreeMap<String, T>,
    key: impl Fn(&T) -> String,
    combine: impl Fn(&T, &T) -> T,
) -> BTreeMap<String, T> {
    let mut merged = stored
        .into_iter()
        .map(|item| (key(&item), item))
        .collect::<BTreeMap<_, _>>();
    for (id, item) in mine {
        let value = match merged.get(id) {
            Some(stored) => combine(item, stored),
            None => item.clone(),
        };
        merged.insert(id.clone(), value);
    }
    merged
}

/** Unite two lists, keeping the first list's order and appending what only the second holds
 * Input
    - first: &[String] - first list
    - second: &[String] - second list
 * Output
    - Vec<String>
*/
fn union(first: &[String], second: &[String]) -> Vec<String> {
    let mut united = first.to_vec();
    for item in second {
        if !united.contains(item) {
            united.push(item.clone());
        }
    }
    united
}

/** How far a recipe proposal has moved in its lifecycle: a merge keeps the decided one
 * Input
    - status: ProposalStatus - status
 * Output
    - u8
*/
fn proposal_rank(status: ProposalStatus) -> u8 {
    match status {
        ProposalStatus::Proposed => 0,
        ProposalStatus::Accepted | ProposalStatus::Rejected => 1,
    }
}

/** Largest note of a pack feedback, in characters */
const MAX_FEEDBACK_NOTES: usize = 2000;

/** How far a contribution has moved in its lifecycle: a merge keeps the further one, so a status
 * never moves backwards (proposed, then accepted, then a final state)
 * Input
    - status: ContributionStatus - status
 * Output
    - u8
*/
fn lifecycle_rank(status: ContributionStatus) -> u8 {
    match status {
        ContributionStatus::Proposed => 0,
        ContributionStatus::Accepted => 1,
        ContributionStatus::Rejected
        | ContributionStatus::Superseded
        | ContributionStatus::Expired => 2,
    }
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
        for proposal in
            read_collection::<RecipeProposal>(&directory.join("recipe_proposals.jsonl"))?
        {
            store
                .recipe_proposals
                .insert(proposal.proposal_id.clone(), proposal);
        }
        store.audits = read_collection(&directory.join("audit.jsonl"))?;
        store.audit_index = store
            .audits
            .iter()
            .enumerate()
            .map(|(position, audit)| (audit.query_id.clone(), position))
            .collect();
        Ok(store)
    }

    /** Persist every collection (each file replaced atomically); an in-memory store does nothing.
     * Another process may have saved since this store was opened, so under the lock every
     * collection is read again and merged with what this store holds before it is written: nothing
     * another investigation saved is lost. Identifiers are derived from content, so the same id is
     * the same object and a union is safe; lists of paths, provenance, packs, and feedback are
     * united, and a contribution's status never moves backwards in its lifecycle
     * Input
        - None (uses self)
     * Output
        - CaptureResult<()>
        - Error when a file cannot be read or written
    */
    pub(crate) fn save(&self) -> CaptureResult<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        let _lock = Lock::acquire(&directory.join("capture.lock")).map_err(storage)?;
        let path = |name: &str| directory.join(name);
        let artifacts = merge_keyed(
            read_collection::<RawArtifact>(&path("artifacts.jsonl"))?,
            &self.artifacts,
            |artifact| artifact.artifact_id.clone(),
            |mine, stored| RawArtifact {
                observed_via: union(&mine.observed_via, &stored.observed_via),
                ..mine.clone()
            },
        );
        write_collection(&path("artifacts.jsonl"), artifacts.values())?;
        let blobs = merge_keyed(
            read_collection::<(String, Value)>(&path("blobs.jsonl"))?,
            &self
                .blobs
                .iter()
                .map(|(hash, value)| (hash.clone(), (hash.clone(), value.clone())))
                .collect::<BTreeMap<_, _>>(),
            |(hash, _)| hash.clone(),
            |mine, _| mine.clone(),
        );
        write_collection(&path("blobs.jsonl"), blobs.values())?;
        let evidence = merge_keyed(
            read_collection::<EvidenceRecord>(&path("evidence.jsonl"))?,
            &self.evidence,
            |record| record.evidence_id.clone(),
            |mine, stored| {
                let mut record = mine.clone();
                record.provenance.ingestion_paths = union(
                    &mine.provenance.ingestion_paths,
                    &stored.provenance.ingestion_paths,
                );
                record.supersedes = mine.supersedes.clone().or(stored.supersedes.clone());
                record
            },
        );
        write_collection(&path("evidence.jsonl"), evidence.values())?;
        let entities = merge_keyed(
            read_collection::<Entity>(&path("entities.jsonl"))?,
            &self.graph.entities,
            |entity| entity.entity_id.clone(),
            |mine, stored| {
                let mut entity = mine.clone();
                for (key, value) in &stored.attributes {
                    entity
                        .attributes
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
                entity.version = mine.version.clone().or(stored.version.clone());
                entity.first_evidence = mine
                    .first_evidence
                    .clone()
                    .or(stored.first_evidence.clone());
                entity
            },
        );
        write_collection(&path("entities.jsonl"), entities.values())?;
        let relations = merge_keyed(
            read_collection::<Relation>(&path("relations.jsonl"))?,
            &self.graph.relations,
            |relation| relation.relation_id.clone(),
            |mine, stored| {
                let mut relation = mine.clone();
                relation.provenance.evidence =
                    union(&mine.provenance.evidence, &stored.provenance.evidence);
                relation
            },
        );
        write_collection(&path("relations.jsonl"), relations.values())?;
        let assertions = merge_keyed(
            read_collection::<Assertion>(&path("assertions.jsonl"))?,
            &self.assertions,
            |assertion| assertion.assertion_id.clone(),
            |mine, _| mine.clone(),
        );
        write_collection(&path("assertions.jsonl"), assertions.values())?;
        let contributions = merge_keyed(
            read_collection::<ContextContribution>(&path("contributions.jsonl"))?,
            &self.contributions.contributions,
            |contribution| contribution.contribution_id.clone(),
            |mine, stored| {
                if lifecycle_rank(stored.status) > lifecycle_rank(mine.status) {
                    stored.clone()
                } else {
                    mine.clone()
                }
            },
        );
        write_collection(&path("contributions.jsonl"), contributions.values())?;
        let episodes = merge_keyed(
            read_collection::<WorkEpisode>(&path("episodes.jsonl"))?,
            &self.episodes,
            |episode| episode.episode_id.clone(),
            |mine, stored| {
                let mut episode = mine.clone();
                episode.packs = union(&mine.packs, &stored.packs);
                for feedback in &stored.feedback {
                    if !episode.feedback.contains(feedback) {
                        episode.feedback.push(feedback.clone());
                    }
                }
                for change in &stored.outcome_history {
                    if !episode.outcome_history.contains(change) {
                        episode.outcome_history.push(change.clone());
                    }
                }
                if stored.ended_at > mine.ended_at {
                    episode.outcome = stored.outcome;
                    episode
                        .outcome_evidence
                        .clone_from(&stored.outcome_evidence);
                    episode
                        .outcome_recorded_by
                        .clone_from(&stored.outcome_recorded_by);
                    episode.ended_at = stored.ended_at;
                }
                episode
            },
        );
        write_collection(&path("episodes.jsonl"), episodes.values())?;
        let questions = merge_keyed(
            read_collection::<Question>(&path("questions.jsonl"))?,
            &self.questions,
            |question| question.question_id.clone(),
            |mine, _| mine.clone(),
        );
        write_collection(&path("questions.jsonl"), questions.values())?;
        let packs = merge_keyed(
            read_collection::<EvidencePack>(&path("packs.jsonl"))?,
            &self.packs,
            |pack| pack.pack_id.clone(),
            |mine, _| mine.clone(),
        );
        write_collection(&path("packs.jsonl"), packs.values())?;
        let proposals = merge_keyed(
            read_collection::<RecipeProposal>(&path("recipe_proposals.jsonl"))?,
            &self.recipe_proposals,
            |proposal| proposal.proposal_id.clone(),
            |mine, stored| {
                if proposal_rank(stored.status) > proposal_rank(mine.status) {
                    stored.clone()
                } else {
                    mine.clone()
                }
            },
        );
        write_collection(&path("recipe_proposals.jsonl"), proposals.values())?;
        // Audits keep their order: what is on disk first, then this store's new ones; a stored
        // audit is replaced only by a copy that adds the replay comparison
        let mut audits = read_collection::<QueryAuditRecord>(&path("audit.jsonl"))?;
        for audit in &self.audits {
            match audits
                .iter_mut()
                .find(|stored| stored.query_id == audit.query_id)
            {
                Some(stored) if stored.replay.is_none() && audit.replay.is_some() => {
                    *stored = audit.clone();
                }
                Some(_) => {}
                None => audits.push(audit.clone()),
            }
        }
        write_collection(&path("audit.jsonl"), audits.iter())?;
        let mut state = self.state.clone();
        if let Ok(text) = std::fs::read_to_string(path("state.json")) {
            if let Ok(stored) = serde_json::from_str::<StoreState>(&text) {
                for (delivery, evidence) in stored.deliveries {
                    state.deliveries.entry(delivery).or_insert(evidence);
                }
                for (resource, cursor) in stored.cursors {
                    state.cursors.entry(resource).or_insert(cursor);
                }
                for (recipe, digest) in stored.recipes {
                    state.recipes.entry(recipe).or_insert(digest);
                }
            }
        }
        let state =
            serde_json::to_vec_pretty(&state).map_err(|error| storage(error.to_string()))?;
        write_atomic(&path("state.json"), &state).map_err(storage)
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
                return self.duplicate(&existing, &label, episode_id);
            }
        }
        let (artifact, stored) = capture(definition, record, now, &label);
        if let Some(existing) = self.by_artifact.get(&artifact.artifact_id).cloned() {
            if path.kind == "webhook" {
                self.state
                    .deliveries
                    .insert(path.id.clone(), existing.clone());
            }
            return self.duplicate(&existing, &label, episode_id);
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
        - episode: Option<&str> - work episode that observed it now
     * Output
        - IngestOutcome (Duplicate)
    */
    fn duplicate(
        &mut self,
        evidence_id: &str,
        label: &str,
        episode: Option<&str>,
    ) -> IngestOutcome {
        let mut parse_status = ParseStatus::Failed;
        if let Some(evidence) = self.evidence.get_mut(evidence_id) {
            parse_status = evidence.parse_status;
            // Every work episode that observed the record is remembered, not only the first
            if let Some(episode) = episode {
                if evidence.episode_id.as_deref() != Some(episode)
                    && !evidence.observed_in.iter().any(|seen| seen == episode)
                {
                    evidence.observed_in.push(episode.to_string());
                }
            }
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
        // The target must already exist. Nothing a contribution says may create or change an entity
        // (a zone's version is what evidence observed, never what a proposer claims), so the graph
        // is only touched after the ledger has accepted the proposal
        let target = match proposal.target_ref.kind {
            TargetKind::Entity => proposal.target_ref.id.clone(),
            TargetKind::WorkEpisode => {
                if !self.episodes.contains_key(&proposal.target_ref.id) {
                    return Err(CaptureError::new(
                        ErrorCode::NotFound,
                        format!("no work episode '{}'", proposal.target_ref.id),
                    ));
                }
                entity_id(EntityKind::WorkEpisode, &proposal.target_ref.id)
            }
            TargetKind::Zone => entity_id(EntityKind::Zone, &proposal.target_ref.id),
            TargetKind::Flow => entity_id(EntityKind::Flow, &proposal.target_ref.id),
        };
        if !self.graph.entities.contains_key(&target) {
            return Err(CaptureError::new(
                ErrorCode::NotFound,
                format!(
                    "no {:?} '{}' has been observed; context can only be proposed for an observed target",
                    proposal.target_ref.kind, proposal.target_ref.id
                ),
            ));
        }
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

    /** Record a work episode's outcome. Every evidence id cited must exist and belong to the
     * episode (captured for it, or included in one of its Evidence Packs), so an outcome can never
     * rest on evidence that does not exist or concerns other work
     * Input
        - episode_id: &str - episode
        - actor: &Actor - recorder
        - may_review: bool - whether the recorder may review (needed to change another's outcome)
        - outcome: EpisodeOutcome - outcome
        - evidence: Vec<String> - supporting evidence ids
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<()>
        - Error when the episode is unknown, cited evidence does not exist or belongs to other
          work, or the episode refuses the outcome (WorkEpisode::record_outcome)
    */
    pub(crate) fn record_outcome(
        &mut self,
        episode_id: &str,
        actor: &Actor,
        may_review: bool,
        outcome: EpisodeOutcome,
        evidence: Vec<String>,
        now: u64,
    ) -> CaptureResult<()> {
        let episode = self.episodes.get(episode_id).ok_or_else(|| {
            CaptureError::new(
                ErrorCode::NotFound,
                format!("no work episode '{episode_id}'"),
            )
        })?;
        let in_packs = episode
            .packs
            .iter()
            .filter_map(|pack| self.packs.get(pack))
            .flat_map(|pack| pack.evidence.iter().map(|item| item.evidence_id.as_str()))
            .collect::<BTreeSet<_>>();
        for id in &evidence {
            let Some(record) = self.evidence.get(id) else {
                return Err(CaptureError::new(
                    ErrorCode::EvidenceRequired,
                    format!("evidence '{id}' does not exist"),
                ));
            };
            if record.episode_id.as_deref() != Some(episode_id) && !in_packs.contains(id.as_str()) {
                return Err(CaptureError::new(
                    ErrorCode::EvidenceRequired,
                    format!("evidence '{id}' does not belong to work episode '{episode_id}'"),
                ));
            }
        }
        match self.episodes.get_mut(episode_id) {
            Some(episode) => episode.record_outcome(actor, may_review, outcome, evidence, now),
            None => Ok(()),
        }
    }

    /** Record whether an Evidence Pack served the person who used it. Only a person judges a pack
     * (an agent's opinion of its own evidence is not feedback), the pack must be kept here and
     * belong to a work episode, and the notes are redacted and bounded. Feedback is kept, never
     * replaced: a later judgement is added beside the earlier one
     * Input
        - pack_id: &str - pack
        - requester: &Actor - the person giving feedback
        - sufficient: Truth - whether the pack was sufficient
        - additional_evidence: Vec<EvidenceType> - evidence that was still needed
        - notes: &str - remarks
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<String> the pack's work episode
        - Error when the requester is not a person, or the pack or its episode is unknown
    */
    pub(crate) fn record_feedback(
        &mut self,
        pack_id: &str,
        requester: &Actor,
        sufficient: Truth,
        additional_evidence: Vec<EvidenceType>,
        notes: &str,
        now: u64,
    ) -> CaptureResult<String> {
        if requester.kind != ActorKind::Human {
            return Err(CaptureError::new(
                ErrorCode::Unauthorized,
                "only a person gives feedback on an Evidence Pack",
            ));
        }
        let episode_id = self
            .packs
            .get(pack_id)
            .ok_or_else(|| {
                CaptureError::new(
                    ErrorCode::NotFound,
                    format!("no Evidence Pack '{pack_id}' is kept here"),
                )
            })?
            .episode_id
            .clone()
            .ok_or_else(|| {
                CaptureError::new(
                    ErrorCode::NotFound,
                    format!("Evidence Pack '{pack_id}' belongs to no work episode"),
                )
            })?;
        let episode = self.episodes.get_mut(&episode_id).ok_or_else(|| {
            CaptureError::new(
                ErrorCode::NotFound,
                format!("no work episode '{episode_id}'"),
            )
        })?;
        let mut notes = Value::String(notes.chars().take(MAX_FEEDBACK_NOTES).collect());
        crate::telemetry::events::redact_value(&mut notes);
        let mut additional_evidence = additional_evidence;
        additional_evidence.sort();
        additional_evidence.dedup();
        episode.feedback.push(PackFeedback {
            pack_id: pack_id.into(),
            requester: requester.clone(),
            sufficient,
            additional_evidence,
            notes: notes.as_str().unwrap_or_default().to_string(),
            at: now,
        });
        Ok(episode_id)
    }

    /** Keep a recipe proposal. A proposal for a recipe version that another open or accepted
     * proposal already gives other content is refused, so one version can never mean two recipes
     * Input
        - proposal: RecipeProposal - proposal
     * Output
        - CaptureResult<String> proposal id
        - Error when the same proposal exists or the version is taken by other content
    */
    pub(crate) fn propose_recipe(&mut self, proposal: RecipeProposal) -> CaptureResult<String> {
        if self.recipe_proposals.contains_key(&proposal.proposal_id) {
            return Err(CaptureError::new(
                ErrorCode::InvalidTransition,
                format!("recipe proposal '{}' exists", proposal.proposal_id),
            ));
        }
        if let Some(taken) = self.recipe_proposals.values().find(|other| {
            other.status != ProposalStatus::Rejected
                && other.recipe.recipe_id == proposal.recipe.recipe_id
                && other.recipe.version == proposal.recipe.version
        }) {
            return Err(CaptureError::new(
                ErrorCode::InvalidRecipe,
                format!(
                    "{} v{} is already proposed or published ({}); propose a new version",
                    proposal.recipe.recipe_id, proposal.recipe.version, taken.proposal_id
                ),
            ));
        }
        let id = proposal.proposal_id.clone();
        self.recipe_proposals.insert(id.clone(), proposal);
        Ok(id)
    }

    /** The recipes of accepted proposals, by recipe and version (the broker adds them to its
     * RecipeBook)
     * Input
        - None (uses self)
     * Output
        - Vec<EvidenceRecipe>
    */
    pub(crate) fn accepted_recipes(&self) -> Vec<EvidenceRecipe> {
        let mut accepted = self
            .recipe_proposals
            .values()
            .filter(|proposal| proposal.status == ProposalStatus::Accepted)
            .map(|proposal| proposal.recipe.clone())
            .collect::<Vec<_>>();
        accepted.sort_by(|a, b| (&a.recipe_id, a.version).cmp(&(&b.recipe_id, b.version)));
        accepted
    }

    /** Pin a recipe version to its content: the first time a version is used its digest is kept, and
     * a later run with different content under the same version is refused, so a published recipe
     * version can never silently change meaning (old audits that name it stay true)
     * Input
        - recipe_id: &str - recipe
        - version: u32 - version
        - digest: &str - content digest
     * Output
        - CaptureResult<()>
        - Error when this store ran that version with other content
    */
    pub(crate) fn pin_recipe(
        &mut self,
        recipe_id: &str,
        version: u32,
        digest: &str,
    ) -> CaptureResult<()> {
        let key = format!("{recipe_id}@v{version}");
        match self.state.recipes.get(&key) {
            Some(pinned) if pinned != digest => Err(CaptureError::new(
                ErrorCode::InvalidRecipe,
                format!(
                    "{recipe_id} v{version} has changed since this store used it (digest {pinned}, now {digest}); publish the change as a new version"
                ),
            )),
            Some(_) => Ok(()),
            None => {
                self.state.recipes.insert(key, digest.to_string());
                Ok(())
            }
        }
    }

    /** Ask a question: store it as open (its identifier must be new)
     * Input
        - question: Question - question
     * Output
        - CaptureResult<String> question id
        - Error when a question with that identifier exists, or it names no permitted resources
    */
    pub(crate) fn ask(&mut self, mut question: Question) -> CaptureResult<String> {
        if self.questions.contains_key(&question.question_id) {
            return Err(CaptureError::new(
                ErrorCode::InvalidTransition,
                format!("question '{}' was already asked", question.question_id),
            ));
        }
        if question.permitted_resources.is_empty() {
            return Err(CaptureError::new(
                ErrorCode::InvalidQuery,
                "a question must name the resources it may query",
            ));
        }
        question.state = QuestionState::Open;
        question.answer_pack = None;
        let id = question.question_id.clone();
        self.questions.insert(id.clone(), question);
        Ok(id)
    }

    /** Move a question through its lifecycle: an open or blocked question is answered (by a kept
     * Evidence Pack), blocked, or found unresolvable; a blocked question may be opened again
     * Input
        - question_id: &str - question
        - state: QuestionState - new state
        - answer_pack: Option<&str> - the pack that answers it (required to answer)
     * Output
        - CaptureResult<()>
        - Error when the question is unknown, the transition is not allowed, or the answering pack
          is missing or unknown
    */
    pub(crate) fn set_question_state(
        &mut self,
        question_id: &str,
        state: QuestionState,
        answer_pack: Option<&str>,
    ) -> CaptureResult<()> {
        let known_pack = answer_pack.is_some_and(|pack| self.packs.contains_key(pack));
        let question = self.questions.get_mut(question_id).ok_or_else(|| {
            CaptureError::new(ErrorCode::NotFound, format!("no question '{question_id}'"))
        })?;
        use QuestionState::{Answered, Blocked, Open, Unresolvable};
        let allowed = matches!(
            (question.state, state),
            (Open, Answered | Blocked | Unresolvable) | (Blocked, Open | Answered | Unresolvable)
        );
        if !allowed {
            return Err(CaptureError::new(
                ErrorCode::InvalidTransition,
                format!("a {:?} question cannot become {state:?}", question.state),
            ));
        }
        if state == Answered && !known_pack {
            return Err(CaptureError::new(
                ErrorCode::EvidenceRequired,
                "a question is answered by a kept Evidence Pack",
            ));
        }
        question.state = state;
        if state == Answered {
            question.answer_pack = answer_pack.map(str::to_string);
        }
        Ok(())
    }

    /** Confirm a relation as a human reviewer: a new relation of the same type between the same
     * nodes is recorded with status Reviewed (the original keeps how it was first known)
     * Input
        - reviewer: &AccessPolicy - reviewer (a human with review rights)
        - relation_id: &str - relation to confirm
     * Output
        - CaptureResult<String> the reviewed relation's id
        - Error when the reviewer may not review, or the relation is unknown or already reviewed
    */
    pub(crate) fn review_relation(
        &mut self,
        reviewer: &AccessPolicy,
        relation_id: &str,
    ) -> CaptureResult<String> {
        if reviewer.actor.kind != crate::capture::ActorKind::Human || !reviewer.may_review {
            return Err(CaptureError::new(
                ErrorCode::Unauthorized,
                "only an authorized human reviewer can confirm a relation",
            ));
        }
        let original = self
            .graph
            .relations
            .get(relation_id)
            .cloned()
            .ok_or_else(|| {
                CaptureError::new(ErrorCode::NotFound, format!("no relation '{relation_id}'"))
            })?;
        if original.status == RelationStatus::Reviewed {
            return Err(CaptureError::new(
                ErrorCode::InvalidTransition,
                "the relation is already a reviewed one",
            ));
        }
        let mut reviewed = Relation::new(
            original.relation_type,
            original.source.clone(),
            original.target.clone(),
            RelationStatus::Reviewed,
            &format!("review:{}", reviewer.actor.id),
            original.provenance.evidence.clone(),
        );
        reviewed.version_scope = original.version_scope.clone();
        reviewed.valid_from = original.valid_from;
        reviewed.valid_to = original.valid_to;
        self.graph.add_relation(reviewed)
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
        self.audit_position(query_id)
            .and_then(|position| self.audits.get(position))
    }

    /** Find a recorded investigation's audit to update it (a replay adds its comparison)
     * Input
        - query_id: &str - investigation
     * Output
        - Option<&mut QueryAuditRecord>
    */
    pub(crate) fn audit_mut(&mut self, query_id: &str) -> Option<&mut QueryAuditRecord> {
        self.audit_position(query_id)
            .and_then(|position| self.audits.get_mut(position))
    }

    /** Record an investigation's audit and index it by query id
     * Input
        - audit: QueryAuditRecord - audit
     * Output
        - None
    */
    pub(crate) fn push_audit(&mut self, audit: QueryAuditRecord) {
        self.audit_index
            .insert(audit.query_id.clone(), self.audits.len());
        self.audits.push(audit);
    }

    /** The position of an audit: from the index, or by a scan when the list was changed directly
     * Input
        - query_id: &str - investigation
     * Output
        - Option<usize>
    */
    fn audit_position(&self, query_id: &str) -> Option<usize> {
        match self.audit_index.get(query_id) {
            Some(&position)
                if self
                    .audits
                    .get(position)
                    .is_some_and(|audit| audit.query_id == query_id) =>
            {
                Some(position)
            }
            _ => self
                .audits
                .iter()
                .position(|audit| audit.query_id == query_id),
        }
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
