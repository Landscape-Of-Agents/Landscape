// The unified, versioned event envelope and its append-only store. Every event carries identity
// (canonical Foxx identifiers plus provider identifiers with their provenance), authority
// (decision, policy and registry bindings), effect (execution outcome, resources), assessment,
// enforcement outcome, telemetry status, measurements, and evidence references. Events are
// hash-chained in the trust directory's runtime folder (outside the repository), deduplicated by
// idempotency key, and redacted before they are written.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{Assessment, Decision, Domain, Enforcement, Execution, Severity, Telemetry};
use super::redact::redact;
use crate::platform::files::{append_line, read_lines, write_atomic, Lock};
use crate::platform::{io_error, now_millis};
use crate::trust::crypto::{digest, digest_json};

/** Version of the event schema */
pub(crate) const SCHEMA_VERSION: u32 = 1;

/** Chain value before the first event */
pub(crate) const GENESIS: &str = "genesis";

/** Largest serialized payload kept in an event, in bytes; larger payloads are replaced by a
 * digest reference */
const MAX_PAYLOAD_BYTES: usize = 8 * 1024;

/** Idempotency keys remembered for deduplication */
const REMEMBERED_KEYS: usize = 2048;

/** Where an event came from
 * Variants
    - Crane - the Crane CLI or runtime
    - ProviderAdapter - a hook translated from an agent provider
    - FoxxExtension - the Foxx editor extension
    - Dashboard - the Foxx dashboard
    - Ingestion - the ingestion backend
    - Integration - an external integration
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Source {
    Crane,
    ProviderAdapter,
    FoxxExtension,
    Dashboard,
    Ingestion,
    Integration,
}

/** Identity and correlation of an event; canonical Foxx identifiers are always set by Crane,
 * provider identifiers are kept exactly as reported (never invented) with their provenance
 * Fields
    - foxx_session_id: Option<String> - canonical session
    - foxx_task_id: Option<String> - canonical task
    - external_task_id: Option<String> - tracker task (CRANE_TASK_ID), if given
    - provider: Option<String> - claude, codex, or generic
    - provider_session_id: Option<String> - as reported by the provider
    - provider_turn_id: Option<String> - as reported by the provider
    - provider_task_id: Option<String> - as reported by the provider
    - agent_instance_id: Option<String> - canonical agent instance (provider plus session)
    - model: Option<String> - model reported by the provider
    - tool_call_id: Option<String> - provider tool_use_id, or a derived identifier
    - hook_event_id: Option<String> - identifier of the hook invocation
    - trace_id: Option<String> - trace (the session)
    - span_id: Option<String> - span (the tool call or hook)
    - parent_event_id: Option<String> - causal parent, only when evidence establishes it
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Scope {
    pub(crate) foxx_session_id: Option<String>,
    pub(crate) foxx_task_id: Option<String>,
    pub(crate) external_task_id: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) provider_session_id: Option<String>,
    pub(crate) provider_turn_id: Option<String>,
    pub(crate) provider_task_id: Option<String>,
    pub(crate) agent_instance_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) tool_call_id: Option<String>,
    pub(crate) hook_event_id: Option<String>,
    pub(crate) trace_id: Option<String>,
    pub(crate) span_id: Option<String>,
    pub(crate) parent_event_id: Option<String>,
}

/** Governance versions an event was decided under
 * Fields
    - registry_generation: u64 - signed registry generation
    - registry_head: Option<String> - registry manifest head
    - policies: Vec<String> - policies involved
    - selections: Vec<String> - selections involved
    - checkpoint: Option<String> - checkpoint involved
    - autonomy_mode: Option<String> - session autonomy mode
    - safety: Option<String> - session safety state
    - zones: Vec<String> - zones and flows covering the resources involved
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Bindings {
    pub(crate) registry_generation: u64,
    pub(crate) registry_head: Option<String>,
    pub(crate) policies: Vec<String>,
    pub(crate) selections: Vec<String>,
    pub(crate) checkpoint: Option<String>,
    pub(crate) autonomy_mode: Option<String>,
    pub(crate) safety: Option<String>,
    #[serde(default)]
    pub(crate) zones: Vec<String>,
}

/** A resource an action touched or tried to touch
 * Fields
    - kind: String - file, selection, command, url, host, tool, or metadata
    - id: String - path, identifier, redacted command, or destination
    - access: String - read, write, execute, network, delete, or other
    - classification: Option<String> - protected, target, metadata, trust, or external
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Resource {
    pub(crate) kind: String,
    pub(crate) id: String,
    pub(crate) access: String,
    pub(crate) classification: Option<String>,
}

/** A measurement with its unit and how it is known; a missing value stays None, never zero
 * Fields
    - name: String - metric name
    - value: Option<f64> - value, None when not measured
    - unit: String - unit such as ms, credits, count, bytes
    - status: Telemetry - observed, inferred, estimated, unobserved, ...
    - method: String - how it was collected
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Measurement {
    pub(crate) name: String,
    pub(crate) value: Option<f64>,
    pub(crate) unit: String,
    pub(crate) status: Telemetry,
    pub(crate) method: String,
}

impl Measurement {
    /** Build an observed measurement
     * Input
        - name: &str - metric name
        - value: f64 - value
        - unit: &str - unit
        - method: &str - collection method
     * Output
        - Measurement
    */
    pub(crate) fn observed(name: &str, value: f64, unit: &str, method: &str) -> Self {
        Self {
            name: name.into(),
            value: Some(value),
            unit: unit.into(),
            status: Telemetry::Observed,
            method: method.into(),
        }
    }

    /** Build a measurement that was not or cannot be taken
     * Input
        - name: &str - metric name
        - unit: &str - unit
        - status: Telemetry - unobserved, unsupported, or lost
        - method: &str - why
     * Output
        - Measurement
    */
    pub(crate) fn missing(name: &str, unit: &str, status: Telemetry, method: &str) -> Self {
        Self {
            name: name.into(),
            value: None,
            unit: unit.into(),
            status,
            method: method.into(),
        }
    }
}

/** One event
 * Fields
    - event_id: String - unique identifier (digest-derived)
    - event_type: String - typed name such as tool_call.decided
    - schema_version: u32 - SCHEMA_VERSION
    - sequence: u64 - position in the repository's event chain
    - occurred_at: u64 - Unix milliseconds when it happened
    - recorded_at: u64 - Unix milliseconds when it was written
    - repository_id: String - repository identity
    - source: Source - producer
    - domain: Domain - security domain
    - severity: Severity - severity
    - environment: String - local, ci, or hosted
    - scope: Scope - identity and correlation
    - decision: Option<Decision> - authority decision, where relevant
    - execution: Option<Execution> - execution outcome, where relevant
    - assessment: Option<Assessment> - security assessment, where relevant
    - enforcement: Enforcement - enforcement outcome
    - telemetry: Telemetry - how the event's facts are known
    - bindings: Bindings - governance versions
    - resources: Vec<Resource> - resources touched
    - measurements: Vec<Measurement> - measurements
    - reasons: Vec<String> - reason codes and explanations (redacted)
    - payload: Value - event-specific structured data (redacted, bounded)
    - evidence: Vec<String> - evidence references (digests, report identifiers)
    - idempotency_key: String - deduplication key
    - previous_digest: String - digest of the previous event
    - digest: String - SHA-512 of this event without the digest field
    - spec: Vec<Value> - the event's specification events (DASHBOARD.md: the 13 attribute groups),
      computed when it is stored and covered by its digest; empty in events stored before them
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Event {
    pub(crate) event_id: String,
    pub(crate) event_type: String,
    pub(crate) schema_version: u32,
    pub(crate) sequence: u64,
    pub(crate) occurred_at: u64,
    pub(crate) recorded_at: u64,
    pub(crate) repository_id: String,
    pub(crate) source: Source,
    pub(crate) domain: Domain,
    pub(crate) severity: Severity,
    pub(crate) environment: String,
    pub(crate) scope: Scope,
    pub(crate) decision: Option<Decision>,
    pub(crate) execution: Option<Execution>,
    pub(crate) assessment: Option<Assessment>,
    pub(crate) enforcement: Enforcement,
    pub(crate) telemetry: Telemetry,
    pub(crate) bindings: Bindings,
    pub(crate) resources: Vec<Resource>,
    pub(crate) measurements: Vec<Measurement>,
    pub(crate) reasons: Vec<String>,
    pub(crate) payload: Value,
    pub(crate) evidence: Vec<String>,
    pub(crate) idempotency_key: String,
    pub(crate) previous_digest: String,
    pub(crate) digest: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) spec: Vec<Value>,
}

impl Event {
    /** Start an event with routine defaults: observed, informational, no decision
     * Input
        - event_type: &str - typed name
        - domain: Domain - security domain
        - repository_id: &str - repository identity
        - idempotency_key: &str - deduplication key
     * Output
        - Event (sequence, digests, and identifier are set when stored)
    */
    pub(crate) fn new(
        event_type: &str,
        domain: Domain,
        repository_id: &str,
        idempotency_key: &str,
    ) -> Self {
        Self {
            event_id: String::new(),
            event_type: event_type.into(),
            schema_version: SCHEMA_VERSION,
            sequence: 0,
            occurred_at: now_millis(),
            recorded_at: 0,
            repository_id: repository_id.into(),
            source: Source::Crane,
            domain,
            severity: Severity::Info,
            environment: environment(),
            scope: Scope::default(),
            decision: None,
            execution: None,
            assessment: None,
            enforcement: Enforcement::NotApplicable,
            telemetry: Telemetry::Observed,
            bindings: Bindings::default(),
            resources: Vec::new(),
            measurements: Vec::new(),
            reasons: Vec::new(),
            payload: json!({}),
            evidence: Vec::new(),
            idempotency_key: idempotency_key.into(),
            previous_digest: String::new(),
            digest: String::new(),
            spec: Vec::new(),
        }
    }

    /** Compute the event digest from every field but the digest
     * Input
        - None (uses self)
     * Output
        - String of 128 hex characters
    */
    pub(crate) fn compute_digest(&self) -> String {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.remove("digest");
        }
        digest_json("telemetry/event/v1", &value)
    }

    /** Find a measurement value by name
     * Input
        - name: &str - metric name
     * Output
        - Option<f64>, None when absent or not measured
    */
    pub(crate) fn measurement(&self, name: &str) -> Option<f64> {
        self.measurements
            .iter()
            .find(|measurement| measurement.name == name)
            .and_then(|measurement| measurement.value)
    }
}

/** Return the environment label: CRANE_ENVIRONMENT, "ci" when CI is set, otherwise "local"
 * Input
    - None
 * Output
    - String
*/
pub(crate) fn environment() -> String {
    std::env::var("CRANE_ENVIRONMENT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if std::env::var_os("CI").is_some() {
                "ci".into()
            } else {
                "local".into()
            }
        })
}

/** Redact a JSON value in place, string by string, returning the number of secrets found
 * Input
    - value: &mut Value - value to redact
 * Output
    - usize secrets found
*/
pub(crate) fn redact_value(value: &mut Value) -> usize {
    match value {
        Value::String(text) => {
            let (redacted, count) = redact(text);
            *text = redacted;
            count
        }
        Value::Array(items) => items.iter_mut().map(redact_value).sum(),
        Value::Object(map) => map.values_mut().map(redact_value).sum(),
        _ => 0,
    }
}

/** The chain head and recent idempotency keys, kept next to the event log so appends need not
 * read the whole log
 * Fields
    - sequence: u64 - last sequence number
    - digest: String - last event digest
    - keys: VecDeque<String> - recent idempotency keys
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Head {
    sequence: u64,
    digest: String,
    keys: VecDeque<String>,
}

/** The append-only event store of one repository
 * Fields
    - directory: PathBuf - the runtime directory in the trust directory
*/
#[derive(Debug, Clone)]
pub(crate) struct EventStore {
    pub(crate) directory: PathBuf,
}

impl EventStore {
    /** Open the store in a runtime directory
     * Input
        - directory: &Path - runtime directory
     * Output
        - EventStore
    */
    pub(crate) fn open(directory: &Path) -> Self {
        Self {
            directory: directory.to_path_buf(),
        }
    }

    /** Return the event log path
     * Input
        - None (uses self)
     * Output
        - PathBuf
    */
    pub(crate) fn log_path(&self) -> PathBuf {
        self.directory.join("events.jsonl")
    }

    /** Append an event: redact it, bound its payload, drop it when its idempotency key was seen
     * (a retried hook), assign sequence, chain digests, and identifier, and write it under the
     * store lock
     * Input
        - event: Event - event to store
     * Output
        - Result<Option<Event>, String> the stored event, None when it was a duplicate
        - Error if the store cannot be locked or written
    */
    pub(crate) fn append(&self, mut event: Event) -> Result<Option<Event>, String> {
        let _lock = Lock::acquire(&self.directory.join("events.lock"))?;
        let head_path = self.directory.join("events.head.json");
        let mut head: Head = fs::read_to_string(&head_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if head.digest.is_empty() {
            head.digest = GENESIS.into();
        }
        if head.keys.iter().any(|key| *key == event.idempotency_key) {
            return Ok(None);
        }
        // Facts noted deep inside Crane belong to the next stored event
        let notes = crate::platform::notes::drain();
        if !notes.is_empty() {
            if !event.payload.is_object() {
                event.payload = json!({});
            }
            event.payload["notes"] = json!(notes);
        }
        let mut secrets = 0;
        for reason in &mut event.reasons {
            let (redacted, count) = redact(reason);
            *reason = redacted;
            secrets += count;
        }
        for resource in &mut event.resources {
            let (redacted, count) = redact(&resource.id);
            resource.id = redacted;
            secrets += count;
        }
        secrets += redact_value(&mut event.payload);
        if secrets > 0 {
            event.measurements.push(Measurement::observed(
                "secrets_redacted",
                secrets as f64,
                "count",
                "pattern redaction before persistence",
            ));
        }
        let size = serde_json::to_string(&event.payload)
            .map(|text| text.len())
            .unwrap_or(0);
        if size > MAX_PAYLOAD_BYTES {
            let reference = digest(
                "telemetry/payload/v1",
                &[event.payload.to_string().as_bytes()],
            );
            event.payload = json!({"truncated": true, "bytes": size, "digest": reference});
        }
        event.sequence = head.sequence + 1;
        event.recorded_at = now_millis();
        event.previous_digest = head.digest.clone();
        event.event_id = format!(
            "evt_{}",
            &digest(
                "telemetry/event-id/v1",
                &[
                    event.repository_id.as_bytes(),
                    event.idempotency_key.as_bytes()
                ]
            )[..32]
        );
        event.spec = super::spec::project(&event);
        event.digest = event.compute_digest();
        append_line(
            &self.log_path(),
            &serde_json::to_string(&event).map_err(io_error)?,
        )?;
        head.sequence = event.sequence;
        head.digest = event.digest.clone();
        head.keys.push_back(event.idempotency_key.clone());
        while head.keys.len() > REMEMBERED_KEYS {
            head.keys.pop_front();
        }
        write_atomic(
            &head_path,
            serde_json::to_string(&head).map_err(io_error)?.as_bytes(),
        )?;
        Ok(Some(event))
    }

    /** Read every stored event in order
     * Input
        - None (uses self)
     * Output
        - Result<Vec<Event>, String>
        - Error if the log is unreadable or an event is malformed
    */
    pub(crate) fn read_all(&self) -> Result<Vec<Event>, String> {
        read_lines(&self.log_path())?
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                serde_json::from_value(value)
                    .map_err(|error| format!("event {} is malformed: {error}", index + 1))
            })
            .collect()
    }
}

/** The result of verifying an event chain
 * Fields
    - status: String - verified, empty, or broken
    - length: usize - events checked
    - head: Option<String> - last digest
    - problem: Option<String> - first problem found
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) struct ChainStatus {
    pub(crate) status: String,
    pub(crate) length: usize,
    pub(crate) head: Option<String>,
    pub(crate) problem: Option<String>,
}

/** Verify an event chain: sequences 1, 2, 3 without gaps, every digest correct and chained
 * Input
    - events: &[Event] - events in order
 * Output
    - ChainStatus
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn verify_chain(events: &[Event]) -> ChainStatus {
    let mut previous = GENESIS.to_string();
    for (index, event) in events.iter().enumerate() {
        let problem = if event.sequence != index as u64 + 1 {
            Some(format!(
                "event {} has sequence {} (gap, insertion, or deletion)",
                index + 1,
                event.sequence
            ))
        } else if event.previous_digest != previous {
            Some(format!(
                "event {} does not chain from the event before it",
                index + 1
            ))
        } else if event.compute_digest() != event.digest {
            Some(format!(
                "event {} was modified after it was recorded",
                index + 1
            ))
        } else {
            None
        };
        if let Some(problem) = problem {
            return ChainStatus {
                status: "broken".into(),
                length: events.len(),
                head: None,
                problem: Some(problem),
            };
        }
        previous = event.digest.clone();
    }
    ChainStatus {
        status: if events.is_empty() {
            "empty"
        } else {
            "verified"
        }
        .into(),
        length: events.len(),
        head: (!events.is_empty()).then_some(previous),
        problem: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check deduplication, redaction, chaining, and tamper detection
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn store_dedupes_redacts_and_chains() {
        let directory = tempfile::tempdir().unwrap();
        let store = EventStore::open(directory.path());
        let mut event = Event::new("tool_call.decided", Domain::Authorization, "repo", "k1");
        event
            .reasons
            .push("token=ghp_0123456789abcdefghijklmnopqrstuvwxyz".into());
        let stored = store.append(event.clone()).unwrap().unwrap();
        assert_eq!(stored.sequence, 1);
        assert!(!stored.reasons[0].contains("ghp_0123"));
        assert_eq!(stored.measurement("secrets_redacted"), Some(1.0));
        assert!(store.append(event).unwrap().is_none());
        store
            .append(Event::new("x", Domain::AuditCompliance, "repo", "k2"))
            .unwrap();
        let mut events = store.read_all().unwrap();
        assert_eq!(verify_chain(&events).status, "verified");
        events[0].reasons.clear();
        assert_eq!(verify_chain(&events).status, "broken");
        let mut gap = store.read_all().unwrap();
        gap.remove(0);
        assert_eq!(verify_chain(&gap).status, "broken");
    }

    /** Check that fractional measurements survive the write and read round trip exactly, so a
     * stored chain never looks tampered with because of float parsing
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn fractional_measurements_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let store = EventStore::open(directory.path());
        for (index, value) in [0.1 + 0.2, 269.044_5 + 1e-13, 1e-7, 12_345.678_901_234_5]
            .into_iter()
            .enumerate()
        {
            let mut event = Event::new("x", Domain::AuditCompliance, "repo", &format!("f{index}"));
            event
                .measurements
                .push(Measurement::observed("value", value, "ms", "test"));
            store.append(event).unwrap();
        }
        assert_eq!(verify_chain(&store.read_all().unwrap()).status, "verified");
    }
}
