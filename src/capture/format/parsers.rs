// Typed payload parsers and the normalizer. Each payload family (text, structured, code,
// measurement, graph) has a parser implementing EvidenceParser: it validates its input, keeps the
// source's own fields, extracts common references (task, session, commit, deployment, ...), and
// reports parsed, partially_parsed, or failed with the fields it found missing or malformed. A
// parser never fills a gap: a missing test count stays absent, a missing metric value stays None
// (unobserved), and a failed parse produces a record with no payload, not an empty one.

use serde_json::{Map, Value};

use super::artifact::RawArtifact;
use super::record::{
    scalar, EvidenceRecord, EvidenceType, FileChange, ParseIssue, ParseStatus, Payload,
    PayloadKind, Point, Provenance, RecordRefs, RecordScope, SourceRef,
};
use crate::capture::resources::connector::SourceRecord;
use crate::capture::resources::registry::ResourceDefinition;
use crate::capture::{stable_id, SCHEMA_VERSION};
use crate::telemetry::model::Telemetry;

/** Longest text excerpt kept in a payload, in characters */
const TEXT_EXCERPT_CHARS: usize = 2000;

/** Longest diff excerpt kept in a payload, in characters */
const DIFF_EXCERPT_CHARS: usize = 4000;

/** What a parser produced
 * Fields
    - payload: Option<Payload> - normalized payload, None when parsing failed
    - issues: Vec<ParseIssue> - missing or malformed fields
*/
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParseOutcome {
    pub(crate) payload: Option<Payload>,
    pub(crate) issues: Vec<ParseIssue>,
}

impl ParseOutcome {
    /** A failed parse
     * Input
        - field: &str - offending field ("content" for the whole input)
        - problem: &str - what is wrong
     * Output
        - ParseOutcome
    */
    fn failed(field: &str, problem: &str) -> Self {
        Self {
            payload: None,
            issues: vec![issue(field, problem)],
        }
    }

    /** The parse status the outcome represents
     * Input
        - None (uses self)
     * Output
        - ParseStatus
    */
    pub(crate) fn status(&self) -> ParseStatus {
        match (&self.payload, self.issues.is_empty()) {
            (None, _) => ParseStatus::Failed,
            (Some(_), true) => ParseStatus::Parsed,
            (Some(_), false) => ParseStatus::PartiallyParsed,
        }
    }
}

/** Build a parse issue
 * Input
    - field: &str - field
    - problem: &str - problem
 * Output
    - ParseIssue
*/
fn issue(field: &str, problem: &str) -> ParseIssue {
    ParseIssue {
        field: field.into(),
        problem: problem.into(),
    }
}

/** The shared parser interface */
pub(crate) trait EvidenceParser {
    /** The parser's name
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str;

    /** The parser's version, recorded in every record it produces
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    fn version(&self) -> &'static str;

    /** Parse redacted content of a record
     * Input
        - evidence_type: EvidenceType - what the record is about
        - content: &Value - redacted content
     * Output
        - ParseOutcome
    */
    fn parse(&self, evidence_type: EvidenceType, content: &Value) -> ParseOutcome;
}

/** Report the expected fields of a type that an object lacks
 * Input
    - evidence_type: EvidenceType - type
    - object: &Map<String, Value> - content
 * Output
    - Vec<ParseIssue>
*/
fn missing_fields(evidence_type: EvidenceType, object: &Map<String, Value>) -> Vec<ParseIssue> {
    evidence_type
        .expected_fields()
        .iter()
        .filter(|field| object.get(**field).is_none_or(Value::is_null))
        .map(|field| issue(field, "missing"))
        .collect()
}

/** Parser of text, documents, and logs */
pub(crate) struct TextParser;

impl EvidenceParser for TextParser {
    /** Name
     * Input
        - None
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str {
        "text"
    }

    /** Version
     * Input
        - None
     * Output
        - &'static str
    */
    fn version(&self) -> &'static str {
        "1"
    }

    /** Accept a string, an object with "text" or "message", or an object with "lines"; keep a
     * bounded excerpt
     * Input
        - _evidence_type: EvidenceType - unused
        - content: &Value - content
     * Output
        - ParseOutcome
    */
    fn parse(&self, _evidence_type: EvidenceType, content: &Value) -> ParseOutcome {
        let text = match content {
            Value::String(text) => Some(text.clone()),
            Value::Object(object) => object
                .get("text")
                .or_else(|| object.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    object.get("lines").and_then(Value::as_array).map(|lines| {
                        lines
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                }),
            _ => None,
        };
        let Some(text) = text else {
            return ParseOutcome::failed("content", "expected text, a message, or lines");
        };
        let total_chars = text.chars().count();
        ParseOutcome {
            payload: Some(Payload::Text {
                excerpt: text.chars().take(TEXT_EXCERPT_CHARS).collect(),
                total_chars,
                truncated: total_chars > TEXT_EXCERPT_CHARS,
            }),
            issues: Vec::new(),
        }
    }
}

/** Parser of JSON and key-value records */
pub(crate) struct StructuredParser;

impl EvidenceParser for StructuredParser {
    /** Name
     * Input
        - None
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str {
        "structured"
    }

    /** Version
     * Input
        - None
     * Output
        - &'static str
    */
    fn version(&self) -> &'static str {
        "1"
    }

    /** Keep every field of an object; flag expected fields that are missing and well-known
     * fields with the wrong type (they are kept as the source sent them, and derivations treat
     * them as unknown)
     * Input
        - evidence_type: EvidenceType - type
        - content: &Value - content
     * Output
        - ParseOutcome
    */
    fn parse(&self, evidence_type: EvidenceType, content: &Value) -> ParseOutcome {
        let Some(object) = content.as_object() else {
            return ParseOutcome::failed("content", "expected a JSON object");
        };
        let mut issues = missing_fields(evidence_type, object);
        for (field, valid) in [
            (
                "status",
                object
                    .get("status")
                    .is_none_or(|value| value.is_string() || value.is_null()),
            ),
            (
                "passed",
                object
                    .get("passed")
                    .is_none_or(|value| value.is_u64() || value.is_null()),
            ),
            (
                "failed",
                object
                    .get("failed")
                    .is_none_or(|value| value.is_u64() || value.is_null()),
            ),
            (
                "healthy",
                object
                    .get("healthy")
                    .is_none_or(|value| value.is_boolean() || value.is_null()),
            ),
        ] {
            if !valid {
                issues.push(issue(field, "malformed"));
            }
        }
        ParseOutcome {
            payload: Some(Payload::Structured {
                fields: object.clone(),
            }),
            issues,
        }
    }
}

/** Parser of commits, diffs, and file references */
pub(crate) struct CodeParser;

impl EvidenceParser for CodeParser {
    /** Name
     * Input
        - None
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str {
        "code"
    }

    /** Version
     * Input
        - None
     * Output
        - &'static str
    */
    fn version(&self) -> &'static str {
        "1"
    }

    /** Read the commit, the changed files (entries without a path are dropped and flagged), and a
     * bounded diff excerpt
     * Input
        - evidence_type: EvidenceType - type
        - content: &Value - content
     * Output
        - ParseOutcome
    */
    fn parse(&self, evidence_type: EvidenceType, content: &Value) -> ParseOutcome {
        let Some(object) = content.as_object() else {
            return ParseOutcome::failed("content", "expected a JSON object");
        };
        let mut issues = missing_fields(evidence_type, object);
        let mut files = Vec::new();
        for entry in object
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match entry.get("path").and_then(Value::as_str) {
                Some(path) => files.push(FileChange {
                    path: path.into(),
                    additions: entry.get("additions").and_then(Value::as_u64),
                    deletions: entry.get("deletions").and_then(Value::as_u64),
                }),
                None => issues.push(issue("files", "entry without a path")),
            }
        }
        let diff = object.get("diff").and_then(Value::as_str);
        let truncated = object
            .get("diff_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || diff.is_some_and(|diff| diff.chars().count() > DIFF_EXCERPT_CHARS);
        ParseOutcome {
            payload: Some(Payload::Code {
                commit: object
                    .get("commit")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                files,
                diff_excerpt: diff.map(|diff| diff.chars().take(DIFF_EXCERPT_CHARS).collect()),
                truncated,
            }),
            issues,
        }
    }
}

/** Parser of measurements and time series */
pub(crate) struct MeasurementParser;

impl EvidenceParser for MeasurementParser {
    /** Name
     * Input
        - None
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str {
        "measurement"
    }

    /** Version
     * Input
        - None
     * Output
        - &'static str
    */
    fn version(&self) -> &'static str {
        "1"
    }

    /** Read "points": a numeric value is observed; a null or missing value is unobserved (None,
     * never zero); a non-numeric value is flagged and also kept as None
     * Input
        - _evidence_type: EvidenceType - unused
        - content: &Value - content
     * Output
        - ParseOutcome
    */
    fn parse(&self, _evidence_type: EvidenceType, content: &Value) -> ParseOutcome {
        let Some(points) = content.get("points").and_then(Value::as_array) else {
            return ParseOutcome::failed("points", "expected a list of measurement points");
        };
        let mut issues = Vec::new();
        let mut parsed = Vec::new();
        for point in points {
            let Some(name) = point.get("name").and_then(Value::as_str) else {
                issues.push(issue("points", "point without a name"));
                continue;
            };
            let (value, status) = match point.get("value") {
                Some(Value::Number(number)) => (number.as_f64(), Telemetry::Observed),
                None | Some(Value::Null) => (None, Telemetry::Unobserved),
                Some(_) => {
                    issues.push(issue(&format!("points.{name}.value"), "malformed"));
                    (None, Telemetry::Unobserved)
                }
            };
            parsed.push(Point {
                name: name.into(),
                value,
                unit: point
                    .get("unit")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                at: point.get("at").and_then(Value::as_u64),
                status,
            });
        }
        ParseOutcome {
            payload: Some(Payload::Measurement { points: parsed }),
            issues,
        }
    }
}

/** Parser of graph and state snapshots */
pub(crate) struct GraphParser;

impl EvidenceParser for GraphParser {
    /** Name
     * Input
        - None
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str {
        "graph"
    }

    /** Version
     * Input
        - None
     * Output
        - &'static str
    */
    fn version(&self) -> &'static str {
        "1"
    }

    /** Read "nodes" and "edges", or treat a single entity (a Landscape zone or flow) as one node
     * Input
        - evidence_type: EvidenceType - type
        - content: &Value - content
     * Output
        - ParseOutcome
    */
    fn parse(&self, evidence_type: EvidenceType, content: &Value) -> ParseOutcome {
        let Some(object) = content.as_object() else {
            return ParseOutcome::failed("content", "expected a JSON object");
        };
        let (nodes, edges, issues) = match object.get("nodes").and_then(Value::as_array) {
            Some(nodes) => (
                nodes.clone(),
                object
                    .get("edges")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                Vec::new(),
            ),
            None => (
                vec![content.clone()],
                Vec::new(),
                missing_fields(evidence_type, object),
            ),
        };
        ParseOutcome {
            payload: Some(Payload::Graph {
                nodes,
                edges,
                version: object.get("version").and_then(scalar),
            }),
            issues,
        }
    }
}

/** Select the parser of a payload family
 * Input
    - kind: PayloadKind - family
 * Output
    - &'static dyn EvidenceParser
*/
pub(crate) fn parser_for(kind: PayloadKind) -> &'static dyn EvidenceParser {
    match kind {
        PayloadKind::Text => &TextParser,
        PayloadKind::Structured => &StructuredParser,
        PayloadKind::Code => &CodeParser,
        PayloadKind::Measurement => &MeasurementParser,
        PayloadKind::Graph => &GraphParser,
    }
}

/** Read a value by dotted path from the content, then from the record's metadata
 * Input
    - content: &Value - content
    - record: &SourceRecord - record
    - paths: &[&str] - candidate paths, in order
 * Output
    - Option<String>
*/
fn lookup(content: &Value, record: &SourceRecord, paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| {
            let mut node = content;
            for part in path.split('.') {
                node = node.get(part)?;
            }
            scalar(node).filter(|value| !value.is_empty())
        })
        .or_else(|| {
            paths
                .iter()
                .find_map(|path| record.metadata.get(*path).cloned())
        })
}

/** Read a list of strings by dotted path
 * Input
    - content: &Value - content
    - path: &str - path
 * Output
    - Vec<String>
*/
fn list(content: &Value, path: &str) -> Vec<String> {
    let mut node = content;
    for part in path.split('.') {
        match node.get(part) {
            Some(next) => node = next,
            None => return Vec::new(),
        }
    }
    node.as_array()
        .map(|items| items.iter().filter_map(scalar).collect())
        .unwrap_or_default()
}

/** Extract the common references a record states (never inferred)
 * Input
    - content: &Value - redacted content
    - record: &SourceRecord - record
 * Output
    - RecordRefs
*/
fn references(content: &Value, record: &SourceRecord) -> RecordRefs {
    let generation = lookup(content, record, &["bindings.registry_generation"]);
    RecordRefs {
        task: lookup(content, record, &["task", "task_id"]),
        agent: lookup(content, record, &["agent", "agent_id", "scope.agent_id"]),
        session: lookup(content, record, &["session", "session_id"]),
        parent_run: lookup(content, record, &["parent_run", "scope.parent_event_id"]),
        commit: lookup(content, record, &["commit", "head_sha", "sha"]),
        pull_request: lookup(content, record, &["pull_request", "pr"]),
        deployment: lookup(content, record, &["deployment", "deployment_id"]),
        ci_run: lookup(content, record, &["ci_run", "run_id"]),
        commits: list(content, "commits"),
        related_records: list(content, "related"),
        graph_version: lookup(content, record, &["graph_version"]),
        policy_versions: list(content, "bindings.policies"),
        contract_versions: generation
            .filter(|generation| generation != "0")
            .map(|generation| vec![format!("registry_generation:{generation}")])
            .unwrap_or_default(),
    }
}

/** Find when a record happened: the source timestamp, else a time field of the content
 * Input
    - record: &SourceRecord - record
    - content: &Value - content
 * Output
    - Option<u64>, None when the source gives no time (never the ingestion time)
*/
fn observed_at(record: &SourceRecord, content: &Value) -> Option<u64> {
    record.source_timestamp.or_else(|| {
        [
            "observed_at",
            "occurred_at",
            "timestamp",
            "created_at",
            "at",
        ]
        .iter()
        .find_map(|field| content.get(*field).and_then(Value::as_u64))
    })
}

/** Report whether an evidence type describes something that happened at a time
 * Input
    - evidence_type: EvidenceType - type
 * Output
    - bool
*/
fn is_event(evidence_type: EvidenceType) -> bool {
    !matches!(
        evidence_type,
        EvidenceType::GraphState
            | EvidenceType::Policy
            | EvidenceType::Validation
            | EvidenceType::Configuration
            | EvidenceType::Task
    )
}

/** Normalize a captured record into an Evidence Record
 * Input
    - definition: &ResourceDefinition - resource it came from
    - record: &SourceRecord - source record
    - artifact: &RawArtifact - its raw artifact
    - stored: Option<&Value> - redacted content, None when withheld or absent
    - ingested_at: u64 - Unix milliseconds
    - episode_id: Option<&str> - work episode
    - query_id: Option<&str> - investigation that retrieved it
 * Output
    - EvidenceRecord
*/
pub(crate) fn normalize(
    definition: &ResourceDefinition,
    record: &SourceRecord,
    artifact: &RawArtifact,
    stored: Option<&Value>,
    ingested_at: u64,
    episode_id: Option<&str>,
    query_id: Option<&str>,
) -> EvidenceRecord {
    let parser = parser_for(record.evidence_type.payload_kind());
    let null = Value::Null;
    let content = stored.unwrap_or(&null);
    let mut outcome = match stored {
        Some(content) => parser.parse(record.evidence_type, content),
        None if artifact.storage_reference == "withheld" => {
            ParseOutcome::failed("content", "withheld: classified secret")
        }
        None => ParseOutcome::failed("content", "the source returned no content"),
    };
    let observed = observed_at(record, content);
    if observed.is_none() && is_event(record.evidence_type) && outcome.payload.is_some() {
        outcome.issues.push(issue("observed_at", "missing"));
    }
    let refs = references(content, record);
    let repository = lookup(content, record, &["repository"]).or_else(|| {
        match definition.scope.repositories.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        }
    });
    let mut zones = list(content, "zones");
    zones.extend(lookup(content, record, &["zone"]));
    let mut flows = list(content, "flows");
    flows.extend(lookup(content, record, &["flow"]));
    EvidenceRecord {
        evidence_id: stable_id(
            "evd",
            &[
                &definition.resource_id,
                &record.record_id,
                record.source_revision.as_deref().unwrap_or(""),
                artifact.content_hash.as_deref().unwrap_or(""),
            ],
        ),
        schema_version: SCHEMA_VERSION,
        source_ref: SourceRef {
            resource_id: definition.resource_id.clone(),
            record_id: record.record_id.clone(),
            artifact_id: artifact.artifact_id.clone(),
            revision: record.source_revision.clone(),
        },
        evidence_type: record.evidence_type,
        parse_status: outcome.status(),
        payload: outcome.payload,
        payload_ref: artifact.storage_reference.clone(),
        observed_at: observed,
        ingested_at,
        provenance: Provenance {
            connector_type: definition.connector_type.clone(),
            configuration_version: definition.configuration_version,
            parser: parser.name().into(),
            parser_version: parser.version().into(),
            ingestion_paths: artifact.observed_via.clone(),
            query_id: query_id.map(str::to_string),
        },
        scope: RecordScope {
            repository,
            environment: lookup(content, record, &["environment"]),
            zones,
            flows,
        },
        sensitivity: artifact.sensitivity,
        parse_issues: outcome.issues,
        episode_id: episode_id.map(str::to_string),
        refs,
        supersedes: None,
    }
}
