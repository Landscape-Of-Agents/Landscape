// The connector contract. Every evidence source implements discover, fetch, query, get_metadata,
// and health_check. Queries carry an explicit scope, validated filters, a time window, a page
// limit, and a cursor; they are validated against the resource definition before a connector
// sees them. Every operation returns a structured outcome: a failure is unavailable,
// unauthorized, rate_limited, invalid_query, timeout, or failed, and is never turned into an
// empty successful result.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::registry::{Operation, ResourceDefinition};
use crate::capture::format::record::EvidenceType;
use crate::capture::{CaptureError, CaptureResult, ErrorCode};
use crate::trust::crypto::digest_json;

/** Largest page a single query may request */
pub(crate) const MAX_PAGE: usize = 200;

/** Outcome category of a connector operation
 * Variants
    - Ok - complete result
    - Partial - some records were returned, more exist or some failed (see the error)
    - Unavailable - the source could not be reached
    - Unauthorized - the credentials do not allow the operation
    - RateLimited - the source asked to slow down
    - InvalidQuery - the query was rejected
    - Timeout - the source did not answer in time
    - Failed - any other failure
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConnectorStatus {
    Ok,
    Partial,
    Unavailable,
    Unauthorized,
    RateLimited,
    InvalidQuery,
    Timeout,
    Failed,
}

impl ConnectorStatus {
    /** Report whether records may be used: Ok or Partial
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn usable(self) -> bool {
        matches!(self, Self::Ok | Self::Partial)
    }
}

/**
The scope a query is confined to; None means the query is not confined by that dimension, which validation allows only when the resource itself is not scoped by it
 * Fields
    - repository: Option<String> - repository
    - environment: Option<String> - environment
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QueryScope {
    pub(crate) repository: Option<String>,
    pub(crate) environment: Option<String>,
}

/** A bounded query
 * Fields
    - evidence_type: Option<EvidenceType> - evidence wanted, None for any the resource provides
    - filters: BTreeMap<String, String> - exact-match filters the resource declares
    - since: Option<u64> - earliest source time (Unix ms), inclusive
    - until: Option<u64> - latest source time (Unix ms), inclusive
    - limit: usize - page size, 1 to MAX_PAGE
    - cursor: Option<String> - continuation from a previous page
    - scope: QueryScope - confinement
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QuerySpec {
    pub(crate) evidence_type: Option<EvidenceType>,
    pub(crate) filters: BTreeMap<String, String>,
    pub(crate) since: Option<u64>,
    pub(crate) until: Option<u64>,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
    pub(crate) scope: QueryScope,
}

impl QuerySpec {
    /**
     Validate the query against the resource it is sent to: the operation and evidence type are supported, filters are declared and exact, limits are bounded, and the scope stays inside the resource's scope (a scoped resource is never queried without that scope)
     * Input
        - definition: &ResourceDefinition - target resource
     * Output
        - CaptureResult<()>
        - Error (InvalidQuery) naming the problem
    */
    pub(crate) fn validate(&self, definition: &ResourceDefinition) -> CaptureResult<()> {
        let fail = |message: String| Err(CaptureError::new(ErrorCode::InvalidQuery, message));
        if !definition
            .capabilities
            .operations
            .contains(&Operation::Query)
        {
            return fail(format!(
                "{} does not support queries",
                definition.resource_id
            ));
        }
        if let Some(kind) = self.evidence_type {
            if !definition.capabilities.evidence_types.contains(&kind) {
                return fail(format!(
                    "{} does not provide {kind:?}",
                    definition.resource_id
                ));
            }
        }
        if self.limit == 0 || self.limit > MAX_PAGE {
            return fail(format!("limit must be between 1 and {MAX_PAGE}"));
        }
        for (key, value) in &self.filters {
            if !definition.capabilities.filters.contains(key) {
                return fail(format!(
                    "{} does not accept filter '{key}'",
                    definition.resource_id
                ));
            }
            if value.is_empty() || value.contains('*') || value.contains('?') || value.len() > 256 {
                return fail(format!("filter '{key}' needs one exact value"));
            }
        }
        if let (Some(since), Some(until)) = (self.since, self.until) {
            if since > until {
                return fail("the time window ends before it starts".into());
            }
        }
        let scope = &definition.scope;
        if !scope.repositories.is_empty() {
            match &self.scope.repository {
                Some(repository) if scope.repositories.contains(repository) => {}
                Some(repository) => {
                    return fail(format!(
                        "repository '{repository}' is outside {}",
                        definition.resource_id
                    ))
                }
                None => {
                    return fail(format!(
                        "{} needs a known repository",
                        definition.resource_id
                    ))
                }
            }
        }
        if let Some(environment) = &self.scope.environment {
            if !scope.environments.is_empty() && !scope.environments.contains(environment) {
                return fail(format!(
                    "environment '{environment}' is outside {}",
                    definition.resource_id
                ));
            }
        }
        Ok(())
    }
}

/** One record returned by a source, before normalization
 * Fields
    - record_id: String - the source's own identifier
    - evidence_type: EvidenceType - what it is about
    - media_type: String - content type such as application/json or text/plain
    - content: Value - content as returned (Null when the source gives only a reference)
    - source_timestamp: Option<u64> - when it happened per the source (Unix ms)
    - source_revision: Option<String> - source revision (commit, etag, version)
    - metadata: BTreeMap<String, String> - filterable attributes (commit, deployment, ...)
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceRecord {
    pub(crate) record_id: String,
    pub(crate) evidence_type: EvidenceType,
    pub(crate) media_type: String,
    pub(crate) content: Value,
    pub(crate) source_timestamp: Option<u64>,
    pub(crate) source_revision: Option<String>,
    pub(crate) metadata: BTreeMap<String, String>,
}

/** The outcome of a connector operation
 * Fields
    - status: ConnectorStatus - outcome category
    - records: Vec<SourceRecord> - records (empty unless usable)
    - next_cursor: Option<String> - continuation, when more records exist
    - snapshot: Option<String> - source snapshot or revision the answer reflects, None when the
      source cannot identify one (replays of it cannot be exact)
    - error: Option<String> - explanation of a failure or a partial result
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ConnectorOutcome {
    pub(crate) status: ConnectorStatus,
    pub(crate) records: Vec<SourceRecord>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) snapshot: Option<String>,
    pub(crate) error: Option<String>,
}

impl ConnectorOutcome {
    /** A complete result
     * Input
        - records: Vec<SourceRecord> - records
        - snapshot: Option<String> - source snapshot
     * Output
        - ConnectorOutcome
    */
    pub(crate) fn ok(records: Vec<SourceRecord>, snapshot: Option<String>) -> Self {
        Self {
            status: ConnectorStatus::Ok,
            records,
            next_cursor: None,
            snapshot,
            error: None,
        }
    }

    /** A failure without records
     * Input
        - status: ConnectorStatus - failure category
        - message: impl Into<String> - explanation
     * Output
        - ConnectorOutcome
    */
    pub(crate) fn failed(status: ConnectorStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            records: Vec::new(),
            next_cursor: None,
            snapshot: None,
            error: Some(message.into()),
        }
    }
}

/** The result of a health check
 * Fields
    - available: bool - whether the source answered
    - detail: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HealthReport {
    pub(crate) available: bool,
    pub(crate) detail: String,
}

/** The contract every evidence source implements */
pub(crate) trait ResourceConnector {
    /** List the objects and operations the source offers (records carry descriptions only)
     * Input
        - None (uses self)
     * Output
        - ConnectorOutcome
    */
    fn discover(&self) -> ConnectorOutcome;

    /** Retrieve one known record
     * Input
        - record_id: &str - source record identifier
     * Output
        - ConnectorOutcome with at most one record
    */
    fn fetch(&self, record_id: &str) -> ConnectorOutcome;

    /** Retrieve records by a validated, bounded query
     * Input
        - spec: &QuerySpec - query
     * Output
        - ConnectorOutcome
    */
    fn query(&self, spec: &QuerySpec) -> ConnectorOutcome;

    /** Retrieve a record's identifiers, timestamps, revision, and metadata without its content
     * Input
        - record_id: &str - source record identifier
     * Output
        - ConnectorOutcome whose record content is Null
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    fn get_metadata(&self, record_id: &str) -> ConnectorOutcome {
        let mut outcome = self.fetch(record_id);
        for record in &mut outcome.records {
            record.content = Value::Null;
        }
        outcome
    }

    /** Report whether the source is available
     * Input
        - None (uses self)
     * Output
        - HealthReport
    */
    fn health_check(&self) -> HealthReport;

    /** Start an investigation: drop anything cached for the previous one, so each investigation
     * reads its sources once and sees one consistent snapshot of them
     * Input
        - None (uses self)
     * Output
        - None
    */
    fn begin(&self) {}

    /** Fetch within a deadline. A connector whose work can outlast the investigation's time budget
     * honors the deadline and answers Timeout; the default fetches as usual
     * Input
        - record_id: &str - source record id
        - deadline: Instant - when the investigation's time budget runs out
     * Output
        - ConnectorOutcome
    */
    fn fetch_within(&self, record_id: &str, deadline: Instant) -> ConnectorOutcome {
        let _ = deadline;
        self.fetch(record_id)
    }

    /** Query within a deadline (see fetch_within)
     * Input
        - spec: &QuerySpec - query
        - deadline: Instant - when the investigation's time budget runs out
     * Output
        - ConnectorOutcome
    */
    fn query_within(&self, spec: &QuerySpec, deadline: Instant) -> ConnectorOutcome {
        let _ = deadline;
        self.query(spec)
    }
}

/**
Answer a query from an in-memory list of records: exact filters on metadata, the evidence type, the time window (a record without a source time never matches a time-bounded query), the cursor (an offset), and the page limit
 * Input
    - records: &[SourceRecord] - every record the source holds
    - spec: &QuerySpec - query
    - snapshot: Option<String> - source snapshot
 * Output
    - ConnectorOutcome, Ok with next_cursor when more records match
*/
pub(crate) fn page(
    records: &[SourceRecord],
    spec: &QuerySpec,
    snapshot: Option<String>,
) -> ConnectorOutcome {
    let offset = match spec.cursor.as_deref().map(str::parse::<usize>) {
        None => 0,
        Some(Ok(offset)) => offset,
        Some(Err(_)) => {
            return ConnectorOutcome::failed(ConnectorStatus::InvalidQuery, "malformed cursor")
        }
    };
    let in_window = |record: &SourceRecord| {
        // Timeless evidence (zones, policies, validation) is never filtered by a time window
        if (spec.since.is_none() && spec.until.is_none()) || !record.evidence_type.is_timed() {
            return true;
        }
        record.source_timestamp.is_some_and(|at| {
            spec.since.is_none_or(|since| at >= since) && spec.until.is_none_or(|until| at <= until)
        })
    };
    let matching = records
        .iter()
        .filter(|record| {
            spec.evidence_type
                .is_none_or(|kind| record.evidence_type == kind)
        })
        .filter(|record| {
            spec.filters
                .iter()
                .all(|(key, value)| record.metadata.get(key) == Some(value))
        })
        .filter(|record| in_window(record))
        .collect::<Vec<_>>();
    let records = matching
        .iter()
        .skip(offset)
        .take(spec.limit)
        .map(|record| (*record).clone())
        .collect::<Vec<_>>();
    let next = offset + records.len();
    let mut outcome = ConnectorOutcome::ok(records, snapshot);
    if next < matching.len() {
        outcome.next_cursor = Some(next.to_string());
    }
    outcome
}

/** Digest a list of records into a snapshot identifier, so a changed source is detectable
 * Input
    - records: &[SourceRecord] - records
 * Output
    - String "snap:" followed by 24 hex characters
*/
pub(crate) fn snapshot_of(records: &[SourceRecord]) -> String {
    let value = serde_json::to_value(records).unwrap_or(Value::Null);
    format!("snap:{}", &digest_json("capture/snapshot/v1", &value)[..24])
}
