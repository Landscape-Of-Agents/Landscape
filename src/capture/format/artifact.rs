// Raw artifacts: the original content of a source record, kept for audit and reprocessing. The
// content hash is taken over the canonical JSON of the content as the source returned it, so the
// same record delivered twice (a webhook retry, a second ingestion path) is recognized. Secrets are
// redacted before anything is stored, and a record the source marks secret keeps no content at
// all, only its reference. Content is held by the store under "blob:" references; metadata stays
// small.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capture::resources::connector::SourceRecord;
use crate::capture::resources::registry::ResourceDefinition;
use crate::capture::{stable_id, Sensitivity};
use crate::telemetry::events::redact_value;
use crate::trust::crypto::digest_json;

/** A captured raw artifact (metadata; the content lives in the store's blob area)
 * Fields
    - artifact_id: String - stable identifier (resource, record, revision, content hash)
    - source_resource_id: String - registered resource
    - source_record_id: String - the source's own identifier
    - content_hash: Option<String> - digest of the original content, None when there is none
    - media_type: String - content type
    - captured_at: u64 - Unix milliseconds of capture
    - source_timestamp: Option<u64> - when it happened per the source
    - source_revision: Option<String> - source revision
    - storage_reference: String - "blob:HASH" for stored content, "external:URI" for a reference,
      "withheld" when the content was secret and not stored
    - sensitivity: Sensitivity - classification
    - size_bytes: usize - size of the stored (redacted) content
    - redactions: usize - secrets removed before storage
    - observed_via: Vec<String> - every ingestion path that observed this artifact
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RawArtifact {
    pub(crate) artifact_id: String,
    pub(crate) source_resource_id: String,
    pub(crate) source_record_id: String,
    pub(crate) content_hash: Option<String>,
    pub(crate) media_type: String,
    pub(crate) captured_at: u64,
    pub(crate) source_timestamp: Option<u64>,
    pub(crate) source_revision: Option<String>,
    pub(crate) storage_reference: String,
    pub(crate) sensitivity: Sensitivity,
    pub(crate) size_bytes: usize,
    pub(crate) redactions: usize,
    pub(crate) observed_via: Vec<String>,
}

/** Classify a record: the source's own "sensitivity" metadata when valid, Confidential when
 * secrets had to be redacted, otherwise Internal
 * Input
    - record: &SourceRecord - record
    - redactions: usize - secrets found
 * Output
    - Sensitivity
*/
fn classify(record: &SourceRecord, redactions: usize) -> Sensitivity {
    let declared = record
        .metadata
        .get("sensitivity")
        .and_then(|value| serde_json::from_value::<Sensitivity>(Value::String(value.clone())).ok());
    let floor = if redactions > 0 {
        Sensitivity::Confidential
    } else {
        Sensitivity::Internal
    };
    declared.map_or(floor, |declared| declared.max(floor))
}

/** Capture a source record as a raw artifact, returning the artifact and the redacted content to
 * store (None when there is nothing to store)
 * Input
    - definition: &ResourceDefinition - resource it came from
    - record: &SourceRecord - record
    - captured_at: u64 - Unix milliseconds
    - path: &str - ingestion path ("query:STEP", "webhook:DELIVERY", ...)
 * Output
    - (RawArtifact, Option<Value>)
*/
pub(crate) fn capture(
    definition: &ResourceDefinition,
    record: &SourceRecord,
    captured_at: u64,
    path: &str,
) -> (RawArtifact, Option<Value>) {
    let content_hash = (!record.content.is_null())
        .then(|| digest_json("capture/artifact/v1", &record.content)[..48].to_string());
    let mut stored = record.content.clone();
    let redactions = redact_value(&mut stored);
    let sensitivity = classify(record, redactions);
    let (storage_reference, stored) = match (&content_hash, sensitivity) {
        (_, Sensitivity::Secret) => ("withheld".to_string(), None),
        (Some(hash), _) => (format!("blob:{hash}"), Some(stored)),
        (None, _) => (
            record
                .metadata
                .get("uri")
                .map_or_else(|| "none".to_string(), |uri| format!("external:{uri}")),
            None,
        ),
    };
    let size_bytes = stored.as_ref().map_or(0, |value| {
        serde_json::to_string(value).map_or(0, |text| text.len())
    });
    let artifact = RawArtifact {
        artifact_id: stable_id(
            "art",
            &[
                &definition.resource_id,
                &record.record_id,
                record.source_revision.as_deref().unwrap_or(""),
                content_hash.as_deref().unwrap_or(""),
            ],
        ),
        source_resource_id: definition.resource_id.clone(),
        source_record_id: record.record_id.clone(),
        content_hash,
        media_type: record.media_type.clone(),
        captured_at,
        source_timestamp: record.source_timestamp,
        source_revision: record.source_revision.clone(),
        storage_reference,
        sensitivity,
        size_bytes,
        redactions,
        observed_via: vec![path.to_string()],
    };
    (artifact, stored)
}
