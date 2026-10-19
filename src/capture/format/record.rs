// The normalized Evidence Record (schema version capture::SCHEMA_VERSION) and its typed payloads.
// Event time (observed_at) and ingestion time (ingested_at) are separate; a value the source did
// not provide stays None, and nothing is defaulted: a missing timestamp is not "now", a missing
// metric is not zero, and a parse failure is not an empty successful record.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::capture::Sensitivity;
use crate::telemetry::model::Telemetry;

/** What a piece of evidence is about
 * Variants
    - Task - a task, ticket, acceptance criteria, or approval
    - PullRequest - a pull request and its review state
    - CodeChange - a commit, diff, or changed files
    - CiRun - a CI/CD job or check result
    - TestReport - a test report
    - Deployment - a deployment and its outcome
    - Verification - a post-change verification or health check
    - RuntimeLog - logs and stack traces
    - Metric - measurements and time series
    - AgentSession - an agent session (DataSea)
    - ToolCall - an agent tool call and its result (DataSea)
    - AgentEvent - another agent execution event (DataSea)
    - GraphState - Landscape zones, flows, and system relationships
    - Policy - a Crane/AgentScript policy or contract
    - Validation - a Crane validation result
    - Configuration - configuration, schema, or state snapshots
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceType {
    Task,
    PullRequest,
    CodeChange,
    CiRun,
    TestReport,
    Deployment,
    Verification,
    RuntimeLog,
    Metric,
    AgentSession,
    ToolCall,
    AgentEvent,
    GraphState,
    Policy,
    Validation,
    Configuration,
}

impl EvidenceType {
    /** The payload family the evidence type is normalized into
     * Input
        - None (uses self)
     * Output
        - PayloadKind
    */
    pub(crate) fn payload_kind(self) -> PayloadKind {
        match self {
            Self::RuntimeLog => PayloadKind::Text,
            Self::CodeChange => PayloadKind::Code,
            Self::Metric => PayloadKind::Measurement,
            Self::GraphState => PayloadKind::Graph,
            _ => PayloadKind::Structured,
        }
    }

    /** The fields a well-formed record of this type carries; a record without one of them is
     * partially parsed, and the field stays absent (never defaulted)
     * Input
        - None (uses self)
     * Output
        - &'static [&'static str]
    */
    pub(crate) fn expected_fields(self) -> &'static [&'static str] {
        match self {
            Self::Task => &["title"],
            Self::PullRequest => &["number", "head_sha"],
            Self::CodeChange => &["commit", "files"],
            Self::CiRun => &["commit", "status"],
            Self::TestReport => &["commit", "passed", "failed"],
            Self::Deployment => &["environment", "status", "commit"],
            Self::Verification => &["deployment", "healthy"],
            Self::Metric => &["points"],
            Self::GraphState => &["id", "version"],
            Self::Policy => &["name"],
            _ => &[],
        }
    }
}

/** The payload families, each with its own parser
 * Variants
    - Text - text, documents, and logs
    - Structured - JSON and key-value records
    - Code - commits, diffs, and file references
    - Measurement - measurements and time series
    - Graph - graph and state snapshots
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PayloadKind {
    Text,
    Structured,
    Code,
    Measurement,
    Graph,
}

/** How completely a record was parsed
 * Variants
    - Parsed - every expected field was valid
    - PartiallyParsed - usable, but some fields were missing or malformed (see the issues)
    - Failed - not usable; the raw artifact is kept for reprocessing
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ParseStatus {
    Parsed,
    PartiallyParsed,
    Failed,
}

/** A field a parser found missing or malformed
 * Fields
    - field: String - field name
    - problem: String - missing, malformed, or why
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ParseIssue {
    pub(crate) field: String,
    pub(crate) problem: String,
}

/** One changed file of a code change
 * Fields
    - path: String - repository-relative path
    - additions: Option<u64> - lines added, None when not reported
    - deletions: Option<u64> - lines deleted, None when not reported
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileChange {
    pub(crate) path: String,
    pub(crate) additions: Option<u64>,
    pub(crate) deletions: Option<u64>,
}

/** One measurement point; a missing value stays None with its telemetry status, never zero
 * Fields
    - name: String - metric name
    - value: Option<f64> - value, None when not measured
    - unit: Option<String> - unit, None when not reported
    - at: Option<u64> - Unix milliseconds, None when not reported
    - status: Telemetry - observed, unobserved, ...
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Point {
    pub(crate) name: String,
    pub(crate) value: Option<f64>,
    pub(crate) unit: Option<String>,
    pub(crate) at: Option<u64>,
    pub(crate) status: Telemetry,
}

/** A typed, normalized payload
 * Variants
    - Text - a bounded excerpt of text
    - Structured - the source's fields, preserved
    - Code - a commit, its files, and a bounded diff excerpt
    - Measurement - measurement points
    - Graph - nodes, edges, and the graph version
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Payload {
    Text {
        excerpt: String,
        total_chars: usize,
        truncated: bool,
    },
    Structured {
        fields: Map<String, Value>,
    },
    Code {
        commit: Option<String>,
        files: Vec<FileChange>,
        diff_excerpt: Option<String>,
        truncated: bool,
    },
    Measurement {
        points: Vec<Point>,
    },
    Graph {
        nodes: Vec<Value>,
        edges: Vec<Value>,
        version: Option<String>,
    },
}

impl Payload {
    /** Read a field of a structured payload (or the commit of a code payload) as text
     * Input
        - name: &str - field name
     * Output
        - Option<String>, None when absent or not a scalar
    */
    pub(crate) fn field(&self, name: &str) -> Option<String> {
        match self {
            Self::Structured { fields } => fields.get(name).and_then(scalar),
            Self::Code { commit, .. } if name == "commit" => commit.clone(),
            Self::Graph { nodes, .. } => nodes
                .first()
                .and_then(|node| node.get(name))
                .and_then(scalar),
            _ => None,
        }
    }
}

/** Render a scalar JSON value as text
 * Input
    - value: &Value - value
 * Output
    - Option<String>, None for null, arrays, and objects
*/
pub(crate) fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/** Where the evidence came from
 * Fields
    - resource_id: String - registered resource
    - record_id: String - the source's own record identifier
    - artifact_id: String - the raw artifact kept for audit and reprocessing
    - revision: Option<String> - source revision, None when the source has none
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceRef {
    pub(crate) resource_id: String,
    pub(crate) record_id: String,
    pub(crate) artifact_id: String,
    pub(crate) revision: Option<String>,
}

/** How a record was produced
 * Fields
    - connector_type: String - connector that retrieved the source record
    - configuration_version: u32 - connector configuration version
    - parser: String - parser that normalized it
    - parser_version: String - parser version
    - ingestion_paths: Vec<String> - every path that observed the artifact (query, webhook, ...)
    - query_id: Option<String> - investigation that retrieved it, None for pushed records
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Provenance {
    pub(crate) connector_type: String,
    pub(crate) configuration_version: u32,
    pub(crate) parser: String,
    pub(crate) parser_version: String,
    pub(crate) ingestion_paths: Vec<String>,
    pub(crate) query_id: Option<String>,
}

/** The scope a record belongs to
 * Fields
    - repository: Option<String> - repository, None when not repository-specific or unknown
    - environment: Option<String> - environment, None when not environment-specific
    - zones: Vec<String> - Landscape zones
    - flows: Vec<String> - Landscape flows
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecordScope {
    pub(crate) repository: Option<String>,
    pub(crate) environment: Option<String>,
    pub(crate) zones: Vec<String>,
    pub(crate) flows: Vec<String>,
}

/** References a record carries, each None (or empty) unless the source states it
 * Fields
    - task: Option<String> - task
    - agent: Option<String> - agent
    - session: Option<String> - agent session
    - parent_run: Option<String> - parent run or event
    - commit: Option<String> - commit
    - pull_request: Option<String> - pull request
    - deployment: Option<String> - deployment
    - ci_run: Option<String> - CI run
    - commits: Vec<String> - commits listed by the record (a pull request's commits)
    - related_records: Vec<String> - other source records it names
    - graph_version: Option<String> - Landscape graph version it was observed under
    - policy_versions: Vec<String> - policies (name@version) that applied
    - contract_versions: Vec<String> - contracts (registry generation) that applied
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecordRefs {
    pub(crate) task: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) session: Option<String>,
    pub(crate) parent_run: Option<String>,
    pub(crate) commit: Option<String>,
    pub(crate) pull_request: Option<String>,
    pub(crate) deployment: Option<String>,
    pub(crate) ci_run: Option<String>,
    pub(crate) commits: Vec<String>,
    pub(crate) related_records: Vec<String>,
    pub(crate) graph_version: Option<String>,
    pub(crate) policy_versions: Vec<String>,
    pub(crate) contract_versions: Vec<String>,
}

/** A normalized Evidence Record
 * Fields
    - evidence_id: String - stable identifier (resource, record, revision, content)
    - schema_version: u32 - capture::SCHEMA_VERSION
    - source_ref: SourceRef - source and raw artifact
    - evidence_type: EvidenceType - what it is about
    - payload: Option<Payload> - normalized payload, None when parsing failed
    - payload_ref: String - storage reference of the raw content
    - observed_at: Option<u64> - when it happened (Unix ms), None when unknown
    - ingested_at: u64 - when Work Capture ingested it (Unix ms)
    - provenance: Provenance - how it was produced
    - scope: RecordScope - repository, environment, zones, flows
    - sensitivity: Sensitivity - sensitivity
    - parse_status: ParseStatus - parsed, partially parsed, or failed
    - parse_issues: Vec<ParseIssue> - missing or malformed fields
    - episode_id: Option<String> - work episode, None when not tied to one
    - refs: RecordRefs - references to agents, commits, deployments, ...
    - supersedes: Option<String> - earlier record of the same source record (older revision)
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EvidenceRecord {
    pub(crate) evidence_id: String,
    pub(crate) schema_version: u32,
    pub(crate) source_ref: SourceRef,
    pub(crate) evidence_type: EvidenceType,
    pub(crate) payload: Option<Payload>,
    pub(crate) payload_ref: String,
    pub(crate) observed_at: Option<u64>,
    pub(crate) ingested_at: u64,
    pub(crate) provenance: Provenance,
    pub(crate) scope: RecordScope,
    pub(crate) sensitivity: Sensitivity,
    pub(crate) parse_status: ParseStatus,
    pub(crate) parse_issues: Vec<ParseIssue>,
    pub(crate) episode_id: Option<String>,
    pub(crate) refs: RecordRefs,
    pub(crate) supersedes: Option<String>,
}

impl EvidenceRecord {
    /** Read a payload field as text
     * Input
        - name: &str - field name
     * Output
        - Option<String>, None when absent or the payload failed to parse
    */
    pub(crate) fn field(&self, name: &str) -> Option<String> {
        self.payload
            .as_ref()
            .and_then(|payload| payload.field(name))
    }
}
