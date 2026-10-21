// Reference resolution against the canonical sources. Work Capture records (episodes, packs,
// evidence, artifacts, assertions) are read from the capture store; current Landscape zone and
// flow versions and Crane policy versions come from their existing connectors. Every reference
// resolves to an explicit outcome (resolved, missing, unauthorized, stale, mismatch, unverifiable)
// and nothing unavailable is replaced by an inferred record. Reading a capability or intake never
// grants access to the evidence it references: each evidence read is checked against the
// reader's own Work Capture access policy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::capture::format::record::EvidenceRecord;
use crate::capture::resources::connector::ResourceConnector;
use crate::capture::resources::local::{CraneConnector, LandscapeConnector};
use crate::capture::store::CaptureStore;
use crate::capture::AccessPolicy;
use crate::governance::workspace::Workspace;

/** How a reference resolved
 * Variants
    - Resolved - it exists, is readable, and matches
    - Missing - it does not exist
    - Unauthorized - it exists but the reader may not read it
    - Stale - it exists but its version or revision changed since it was referenced
    - Mismatch - it exists but its digest or content does not match
    - Unverifiable - its current state cannot be established (the source is unavailable)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefOutcome {
    Resolved,
    Missing,
    Unauthorized,
    Stale,
    Mismatch,
    Unverifiable,
}

/** The check of one reference
 * Fields
    - kind: String - episode, evidence_pack, evidence, artifact, assertion, zone, flow, policy, ...
    - reference: String - identifier (with version where relevant)
    - outcome: RefOutcome - how it resolved
    - detail: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReferenceCheck {
    pub(crate) kind: String,
    pub(crate) reference: String,
    pub(crate) outcome: RefOutcome,
    pub(crate) detail: String,
}

impl ReferenceCheck {
    /** Build a check
     * Input
        - kind: &str - reference kind
        - reference: &str - identifier
        - outcome: RefOutcome - outcome
        - detail: impl Into<String> - explanation
     * Output
        - ReferenceCheck
    */
    pub(crate) fn new(
        kind: &str,
        reference: &str,
        outcome: RefOutcome,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            reference: reference.into(),
            outcome,
            detail: detail.into(),
        }
    }
}

/** Current versions of governance sources
 * Fields
    - landscape_known: bool - whether Landscape could be read
    - crane_known: bool - whether Crane policies could be read
    - zones: BTreeMap<String, String> - zone id to version ("v3")
    - flows: BTreeMap<String, String> - flow id to version
    - policies: BTreeMap<String, String> - policy name to version digest
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CurrentVersions {
    pub(crate) landscape_known: bool,
    pub(crate) crane_known: bool,
    pub(crate) zones: BTreeMap<String, String>,
    pub(crate) flows: BTreeMap<String, String>,
    pub(crate) policies: BTreeMap<String, String>,
}

impl CurrentVersions {
    /** Read current versions from a repository's Landscape and Crane connectors
     * Input
        - workspace: &Workspace - repository
     * Output
        - CurrentVersions (a source that cannot be read stays unknown)
    */
    pub(crate) fn from_workspace(workspace: &Workspace) -> Self {
        let mut current = Self::default();
        let landscape = LandscapeConnector {
            workspace: workspace.clone(),
        }
        .discover();
        if landscape.status.usable() {
            current.landscape_known = true;
            for record in landscape.records {
                let version = record.source_revision.clone().unwrap_or_default();
                match record.record_id.split_once(':') {
                    Some(("zone", id)) => {
                        current.zones.insert(id.into(), version);
                    }
                    Some(("flow", id)) => {
                        current.flows.insert(id.into(), version);
                    }
                    _ => {}
                }
            }
        }
        let crane = CraneConnector::new(workspace.clone()).discover();
        if crane.status.usable() {
            current.crane_known = true;
            for record in crane.records {
                if let Some(name) = record.record_id.strip_prefix("policy:") {
                    current.policies.insert(
                        name.into(),
                        record.source_revision.clone().unwrap_or_default(),
                    );
                }
            }
        }
        current
    }

    /** Check a referenced zone, flow, or policy version against the current one
     * Input
        - kind: &str - zone, flow, or policy
        - id: &str - identifier
        - version: Option<&str> - referenced version, None when unknown
     * Output
        - ReferenceCheck
    */
    pub(crate) fn check(&self, kind: &str, id: &str, version: Option<&str>) -> ReferenceCheck {
        let (known, table) = match kind {
            "zone" => (self.landscape_known, &self.zones),
            "flow" => (self.landscape_known, &self.flows),
            _ => (self.crane_known, &self.policies),
        };
        let reference = format!("{id}@{}", version.unwrap_or("unknown"));
        if !known {
            return ReferenceCheck::new(
                kind,
                &reference,
                RefOutcome::Unverifiable,
                "the source could not be read",
            );
        }
        match (table.get(id), version) {
            (None, _) => {
                ReferenceCheck::new(kind, &reference, RefOutcome::Missing, "no longer exists")
            }
            (Some(_), None) => ReferenceCheck::new(
                kind,
                &reference,
                RefOutcome::Unverifiable,
                "the referenced version is unknown",
            ),
            (Some(current), Some(version)) if current == version => {
                ReferenceCheck::new(kind, &reference, RefOutcome::Resolved, "current")
            }
            (Some(current), Some(_)) => ReferenceCheck::new(
                kind,
                &reference,
                RefOutcome::Stale,
                format!("the current version is {current}"),
            ),
        }
    }
}

/** The canonical sources a validation reads, and who reads them
 * Fields
    - capture: &'a CaptureStore - Work Capture records
    - current: &'a CurrentVersions - current governance versions
    - reader: &'a AccessPolicy - the reader's Work Capture access policy
*/
#[derive(Clone, Copy)]
pub(crate) struct Sources<'a> {
    pub(crate) capture: &'a CaptureStore,
    pub(crate) current: &'a CurrentVersions,
    pub(crate) reader: &'a AccessPolicy,
}

/** Report whether a reader may read an evidence record
 * Input
    - reader: &AccessPolicy - reader
    - record: &EvidenceRecord - record
 * Output
    - bool
*/
pub(crate) fn can_read(reader: &AccessPolicy, record: &EvidenceRecord) -> bool {
    reader.permits_resource(&record.source_ref.resource_id)
        && reader.permits_type(record.evidence_type)
        && reader.permits_environment(record.scope.environment.as_deref())
        && record
            .scope
            .repository
            .as_deref()
            .is_none_or(|repository| reader.permits_repository(Some(repository)))
        && record.sensitivity <= reader.max_sensitivity
}

impl Sources<'_> {
    /** Resolve an evidence record for this reader, checking its artifact's content hash
     * Input
        - evidence_id: &str - evidence
     * Output
        - (ReferenceCheck, Option<&EvidenceRecord>) the record only when resolved
    */
    pub(crate) fn evidence(&self, evidence_id: &str) -> (ReferenceCheck, Option<&EvidenceRecord>) {
        let Some(record) = self.capture.evidence.get(evidence_id) else {
            return (
                ReferenceCheck::new(
                    "evidence",
                    evidence_id,
                    RefOutcome::Missing,
                    "no such evidence record",
                ),
                None,
            );
        };
        if !can_read(self.reader, record) {
            return (
                ReferenceCheck::new(
                    "evidence",
                    evidence_id,
                    RefOutcome::Unauthorized,
                    "the reader may not read this evidence",
                ),
                None,
            );
        }
        let artifact = self.capture.artifacts.get(&record.source_ref.artifact_id);
        let stored = artifact
            .and_then(|artifact| artifact.content_hash.as_ref())
            .is_none_or(|hash| {
                self.capture.blobs.contains_key(hash)
                    || artifact.is_some_and(|artifact| artifact.storage_reference == "withheld")
            });
        match artifact {
            None => (
                ReferenceCheck::new(
                    "evidence",
                    evidence_id,
                    RefOutcome::Missing,
                    "its raw artifact is missing",
                ),
                None,
            ),
            Some(_) if !stored => (
                ReferenceCheck::new(
                    "evidence",
                    evidence_id,
                    RefOutcome::Missing,
                    "its artifact content is missing",
                ),
                None,
            ),
            Some(_) => (
                ReferenceCheck::new(
                    "evidence",
                    evidence_id,
                    RefOutcome::Resolved,
                    format!("{:?}", record.evidence_type),
                ),
                Some(record),
            ),
        }
    }
}
