// The Evidence Pack: the bounded, traceable output of an investigation and the primary input of
// future Capability Compiler components. It holds the resolved scope, selected evidence as bounded
// excerpts with durable artifact references (never whole logs or files, never withheld content),
// the entities and relationship paths of a Context View, assertions with their supporting and
// contradicting evidence, task-local and reusable contributions kept apart, the evidence that was
// required but missing (and why), per-source states, the retrieval recipe and query history, and
// warnings. A versioned manifest digests every included item. Inclusion in a pack never publishes
// a contribution to a zone or flow.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::contributions::ContextContribution;
use crate::capture::broker::run::{BranchDecision, ExecutionMode, StepState, StopReason};
use crate::capture::correlate::assertions::Assertion;
use crate::capture::correlate::graph::ContextView;
use crate::capture::correlate::model::Entity;
use crate::capture::format::record::{
    EvidenceRecord, EvidenceType, ParseIssue, ParseStatus, SourceRef,
};
use crate::capture::resources::scope::ScopeResolution;
use crate::capture::{stable_id, Sensitivity, SCHEMA_VERSION};
use crate::trust::crypto::digest_json;

/** Bounds of a pack
 * Fields
    - max_records: usize - most evidence records
    - max_bytes: usize - largest serialized pack
    - max_excerpt_chars: usize - longest excerpt per record
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PackLimits {
    pub(crate) max_records: usize,
    pub(crate) max_bytes: usize,
    pub(crate) max_excerpt_chars: usize,
}

impl Default for PackLimits {
    /** 50 records, 512 KiB, 800-character excerpts
     * Input
        - None
     * Output
        - PackLimits
    */
    fn default() -> Self {
        Self {
            max_records: 50,
            max_bytes: 512 * 1024,
            max_excerpt_chars: 800,
        }
    }
}

/** The state of one source step of the investigation
 * Variants
    - Collected - records were retrieved and parsed
    - Empty - the source answered with no matching record
    - Unavailable - the source failed (unavailable, timeout, rate limited, failed)
    - Denied - authorization refused the step or the source refused the credentials
    - NotQueried - skipped by a branch, an unknown scope value, or an exhausted budget
    - ParseFailed - records were retrieved but none could be parsed
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceState {
    Collected,
    Empty,
    Unavailable,
    Denied,
    NotQueried,
    ParseFailed,
}

/** The state of one recipe step against one resource
 * Fields
    - step_id: String - recipe step
    - resource_id: Option<String> - resource asked, None when none was asked
    - evidence_type: EvidenceType - evidence wanted
    - state: SourceState - outcome
    - detail: String - explanation
    - records: usize - records retrieved
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceReport {
    pub(crate) step_id: String,
    pub(crate) resource_id: Option<String>,
    pub(crate) evidence_type: EvidenceType,
    pub(crate) state: SourceState,
    pub(crate) detail: String,
    pub(crate) records: usize,
}

/** Required evidence that is missing
 * Fields
    - evidence_type: EvidenceType - type
    - requirement: String - what it would establish
    - state: SourceState - why it is missing (empty = not found, unavailable, denied, not queried,
      parse failed)
    - detail: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MissingEvidence {
    pub(crate) evidence_type: EvidenceType,
    pub(crate) requirement: String,
    pub(crate) state: SourceState,
    pub(crate) detail: String,
}

/** Kinds of warnings
 * Variants
    - Access - something was denied
    - Freshness - evidence is old or its time is unknown
    - Completeness - required evidence is missing or a source answered partially
    - Uncertainty - an assertion is unresolved or only proposed
    - Conflict - evidence disagrees
    - Budget - a retrieval limit stopped the investigation
    - Truncation - the pack had to leave records out to stay within its limits
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WarningKind {
    Access,
    Freshness,
    Completeness,
    Uncertainty,
    Conflict,
    Budget,
    Truncation,
}

/** A warning
 * Fields
    - kind: WarningKind - kind
    - message: String - explanation
    - refs: Vec<String> - related evidence, assertions, steps, or resources
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Warning {
    pub(crate) kind: WarningKind,
    pub(crate) message: String,
    pub(crate) refs: Vec<String>,
}

/** A raw artifact reference in a pack
 * Fields
    - artifact_id: String - artifact
    - content_hash: Option<String> - content digest
    - storage_reference: String - where the full content is
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArtifactRef {
    pub(crate) artifact_id: String,
    pub(crate) content_hash: Option<String>,
    pub(crate) storage_reference: String,
}

/** An evidence record in a pack: metadata and a bounded excerpt
 * Fields
    - evidence_id: String - record
    - evidence_type: EvidenceType - type
    - source_ref: SourceRef - source
    - artifact: ArtifactRef - raw artifact
    - observed_at: Option<u64> - event time
    - ingested_at: u64 - ingestion time
    - parse_status: ParseStatus - parse status
    - parse_issues: Vec<ParseIssue> - parse issues
    - sensitivity: Sensitivity - classification
    - excerpt: Value - bounded payload excerpt (Null when parsing failed)
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PackEvidence {
    pub(crate) evidence_id: String,
    pub(crate) evidence_type: EvidenceType,
    pub(crate) source_ref: SourceRef,
    pub(crate) artifact: ArtifactRef,
    pub(crate) observed_at: Option<u64>,
    pub(crate) ingested_at: u64,
    pub(crate) parse_status: ParseStatus,
    pub(crate) parse_issues: Vec<ParseIssue>,
    pub(crate) sensitivity: Sensitivity,
    pub(crate) excerpt: Value,
}

/** Contributions in a pack, task-local context kept apart from reusable system context
 * Fields
    - task_local: Vec<ContextContribution> - context of this work episode
    - reusable: Vec<ContextContribution> - proposed or accepted context for zones, flows, entities
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PackContributions {
    pub(crate) task_local: Vec<ContextContribution>,
    pub(crate) reusable: Vec<ContextContribution>,
}

/** A step of the retrieval history in a pack
 * Fields
    - step_id: String - recipe step
    - resource_id: Option<String> - resource
    - state: StepState - outcome
    - records: usize - records retrieved
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StepSummary {
    pub(crate) step_id: String,
    pub(crate) resource_id: Option<String>,
    pub(crate) state: StepState,
    pub(crate) records: usize,
}

/** How the evidence was retrieved
 * Fields
    - query_id: String - investigation (its full audit record has the same id)
    - mode: ExecutionMode - fresh or replay
    - recipe_id: String - recipe
    - recipe_version: u32 - version
    - recipe_digest: String - content digest
    - ranker: String - ranking strategy
    - branches: Vec<BranchDecision> - conditional steps and whether their condition held
    - steps: Vec<StepSummary> - steps in order
    - stop_reason: StopReason - why retrieval stopped
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Retrieval {
    pub(crate) query_id: String,
    pub(crate) mode: ExecutionMode,
    pub(crate) recipe_id: String,
    pub(crate) recipe_version: u32,
    pub(crate) recipe_digest: String,
    pub(crate) ranker: String,
    pub(crate) branches: Vec<BranchDecision>,
    pub(crate) steps: Vec<StepSummary>,
    pub(crate) stop_reason: StopReason,
}

/** One item of a pack's manifest
 * Fields
    - kind: String - evidence, artifact, entity, relation, assertion, or contribution
    - id: String - identifier
    - digest: String - digest of the item as included
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManifestEntry {
    pub(crate) kind: String,
    pub(crate) id: String,
    pub(crate) digest: String,
}

/** The versioned manifest of a pack
 * Fields
    - version: u32 - manifest layout version (capture::SCHEMA_VERSION)
    - entries: Vec<ManifestEntry> - every included item
    - digest: String - digest over the entries
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) version: u32,
    pub(crate) entries: Vec<ManifestEntry>,
    pub(crate) digest: String,
}

/** The overall state of an investigation
 * Variants
    - Sufficient - every required evidence category was collected
    - Incomplete - required evidence is missing (see missing_evidence)
    - BudgetExhausted - a retrieval limit stopped the investigation
    - Rejected - the request asked for something the actor may not access
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvestigationStatus {
    Sufficient,
    Incomplete,
    BudgetExhausted,
    Rejected,
}

/** An Evidence Pack
 * Fields
    - pack_id: String - stable identifier (query and manifest)
    - schema_version: u32 - capture::SCHEMA_VERSION
    - episode_id: Option<String> - work episode
    - investigation: String - the original task or question
    - status: InvestigationStatus - overall state
    - scope: ScopeResolution - resolved scope with zone and flow versions
    - evidence: Vec<PackEvidence> - selected evidence, best first
    - entities: Vec<Entity> - relevant entities
    - context_view: ContextView - relationship paths and the bounded view
    - assertions: Vec<Assertion> - assertions with supporting and contradicting evidence
    - contributions: PackContributions - task-local and reusable contributions
    - missing_evidence: Vec<MissingEvidence> - required evidence not collected
    - sources: Vec<SourceReport> - collected, empty, unavailable, denied, not queried, unparsed
    - retrieval: Retrieval - recipe and query history
    - warnings: Vec<Warning> - access, freshness, completeness, uncertainty, conflict, budget
    - limits: PackLimits - bounds applied
    - manifest: Manifest - manifest of the included records
    - size_bytes: usize - serialized size
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EvidencePack {
    pub(crate) pack_id: String,
    pub(crate) schema_version: u32,
    pub(crate) episode_id: Option<String>,
    pub(crate) investigation: String,
    pub(crate) status: InvestigationStatus,
    pub(crate) scope: ScopeResolution,
    pub(crate) evidence: Vec<PackEvidence>,
    pub(crate) entities: Vec<Entity>,
    pub(crate) context_view: ContextView,
    pub(crate) assertions: Vec<Assertion>,
    pub(crate) contributions: PackContributions,
    pub(crate) missing_evidence: Vec<MissingEvidence>,
    pub(crate) sources: Vec<SourceReport>,
    pub(crate) retrieval: Retrieval,
    pub(crate) warnings: Vec<Warning>,
    pub(crate) limits: PackLimits,
    pub(crate) manifest: Manifest,
    pub(crate) size_bytes: usize,
    pub(crate) created_at: u64,
}

/** Bound a payload to an excerpt
 * Input
    - record: &EvidenceRecord - record
    - max_chars: usize - longest excerpt
 * Output
    - Value, Null when there is no payload
*/
fn excerpt(record: &EvidenceRecord, max_chars: usize) -> Value {
    let Some(payload) = &record.payload else {
        return Value::Null;
    };
    let value = json!(payload);
    let text = value.to_string();
    if text.chars().count() <= max_chars {
        value
    } else {
        json!({"truncated": true, "chars": text.chars().count(), "preview": text.chars().take(max_chars).collect::<String>()})
    }
}

/** Build a pack entry for a record
 * Input
    - record: &EvidenceRecord - record
    - content_hash: Option<String> - its artifact's content hash
    - limits: &PackLimits - bounds
 * Output
    - PackEvidence
*/
pub(crate) fn pack_evidence(
    record: &EvidenceRecord,
    content_hash: Option<String>,
    limits: &PackLimits,
) -> PackEvidence {
    PackEvidence {
        evidence_id: record.evidence_id.clone(),
        evidence_type: record.evidence_type,
        source_ref: record.source_ref.clone(),
        artifact: ArtifactRef {
            artifact_id: record.source_ref.artifact_id.clone(),
            content_hash,
            storage_reference: record.payload_ref.clone(),
        },
        observed_at: record.observed_at,
        ingested_at: record.ingested_at,
        parse_status: record.parse_status,
        parse_issues: record.parse_issues.clone(),
        sensitivity: record.sensitivity,
        excerpt: excerpt(record, limits.max_excerpt_chars),
    }
}

/** Digest one manifest item
 * Input
    - kind: &str - item kind
    - id: &str - identifier
    - value: Value - item as included
 * Output
    - ManifestEntry
*/
fn entry(kind: &str, id: &str, value: Value) -> ManifestEntry {
    ManifestEntry {
        kind: kind.into(),
        id: id.into(),
        digest: digest_json("capture/pack-item/v1", &value)[..32].to_string(),
    }
}

impl EvidencePack {
    /** Enforce the size limit by dropping the lowest-ranked evidence (it is last), then compute
     * the manifest, the identifier, and the size
     * Input
        - None (uses self)
     * Output
        - None
    */
    pub(crate) fn seal(&mut self) {
        let mut dropped = Vec::new();
        while serde_json::to_string(self).map_or(0, |text| text.len()) > self.limits.max_bytes {
            match self.evidence.pop() {
                Some(evidence) => dropped.push(evidence.evidence_id),
                None => break,
            }
        }
        if !dropped.is_empty() {
            let message = format!(
                "{} records were left out to keep the pack within {} bytes",
                dropped.len(),
                self.limits.max_bytes
            );
            self.warnings.push(Warning {
                kind: WarningKind::Truncation,
                message,
                refs: dropped,
            });
        }
        let entries = self.manifest_entries();
        let digest = digest_json("capture/manifest/v1", &json!(entries))[..32].to_string();
        self.pack_id = stable_id("pck", &[&self.retrieval.query_id, &digest]);
        self.manifest = Manifest {
            version: SCHEMA_VERSION,
            entries,
            digest,
        };
        self.size_bytes = 0;
        self.size_bytes = serde_json::to_string(self).map_or(0, |text| text.len());
    }

    /** Digest every included item, in manifest order
     * Input
        - None (uses self)
     * Output
        - Vec<ManifestEntry>
    */
    fn manifest_entries(&self) -> Vec<ManifestEntry> {
        let mut entries = Vec::new();
        for evidence in &self.evidence {
            entries.push(entry("evidence", &evidence.evidence_id, json!(evidence)));
            entries.push(entry(
                "artifact",
                &evidence.artifact.artifact_id,
                json!(evidence.artifact),
            ));
        }
        entries.extend(
            self.entities
                .iter()
                .map(|entity| entry("entity", &entity.entity_id, json!(entity))),
        );
        entries.extend(
            self.context_view
                .relations
                .iter()
                .map(|relation| entry("relation", &relation.relation_id, json!(relation))),
        );
        entries.extend(
            self.assertions
                .iter()
                .map(|assertion| entry("assertion", &assertion.assertion_id, json!(assertion))),
        );
        let contributions = self
            .contributions
            .task_local
            .iter()
            .chain(self.contributions.reusable.iter());
        entries.extend(contributions.map(|contribution| {
            entry(
                "contribution",
                &contribution.contribution_id,
                json!(contribution),
            )
        }));
        entries
    }

    /**
    Recompute the manifest from the pack's content and compare it with the recorded one, match proves the content is unchanged since sealing not that its claims are true.
    - Input
        - None (uses self)
    - Output
        - Result<(), String>
        - Error naming the first item or digest that differs
    */
    pub(crate) fn verify_manifest(&self) -> Result<(), String> {
        let entries = self.manifest_entries();
        if entries.len() != self.manifest.entries.len() {
            return Err(format!(
                "the manifest lists {} items but the pack holds {}",
                self.manifest.entries.len(),
                entries.len()
            ));
        }
        if let Some((found, recorded)) = entries
            .iter()
            .zip(&self.manifest.entries)
            .find(|(found, recorded)| found != recorded)
        {
            return Err(format!(
                "{} {} does not match its manifest digest {}",
                recorded.kind, recorded.id, found.digest
            ));
        }
        let digest = digest_json("capture/manifest/v1", &json!(entries))[..32].to_string();
        if digest != self.manifest.digest {
            return Err(format!(
                "the manifest digest {} does not match the content ({digest})",
                self.manifest.digest
            ));
        }
        Ok(())
    }
}
