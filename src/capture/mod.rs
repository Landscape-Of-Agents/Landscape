// Work Capture & Evidence Ingestion: the ground-zero foundation of the Capability Compiler. It
// captures, normalizes, correlates, queries, and packages evidence about one work episode, so a
// later component can turn completed work into reusable capabilities. It is organized in four
// categories plus the cross-cutting broker:
//
//   resources  - the resource registry, the connector contract, scope resolution, connectors
//   format     - raw artifacts, the normalized Evidence Record, typed payload parsers
//   correlate  - entities, typed relations, bounded traversal, assertions and their lineage
//   ingest     - evidence selection, context contributions, questions, Evidence Packs, outcomes
//   broker     - versioned Evidence Recipes and the bounded Evidence Query Broker
//
// Work Capture reads Landscape (zones and flows), DataSea (the event store), and Crane (policies)
// through connectors; it never writes to them. Nothing a contribution says becomes trusted
// context or policy by itself, missing evidence stays unknown (never false or zero), and every
// derived claim keeps the evidence it came from.

pub(crate) mod broker;
pub(crate) mod correlate;
pub(crate) mod demo;
pub(crate) mod format;
pub(crate) mod ingest;
pub(crate) mod resources;
pub(crate) mod store;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::telemetry::identity::derive_id;
use format::record::EvidenceType;

/** Version of every Work Capture schema (records, packs, audits, the store layout) */
pub(crate) const SCHEMA_VERSION: u32 = 1;

/** Digest domain of Work Capture identifiers */
const ID_DOMAIN: &str = "capture/id/v1";

/** Derive a stable Work Capture identifier from its defining parts
 * Input
    - prefix: &str - identifier prefix such as "evd"
    - parts: &[&str] - the values that define the object
 * Output
    - String such as "evd_0f3a..." (24 hex characters after the prefix)
*/
pub(crate) fn stable_id(prefix: &str, parts: &[&str]) -> String {
    derive_id(prefix, ID_DOMAIN, parts)
}

/** A three-valued truth for facts that may be unknown: a missing test report is Unknown, never No
 * Variants
    - Yes - established as true
    - No - established as false
    - Unknown - not established either way
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Truth {
    Yes,
    No,
    Unknown,
}

impl Truth {
    /** Convert an optional boolean, keeping a missing value unknown
     * Input
        - value: Option<bool> - known value or None
     * Output
        - Truth
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn from_option(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Yes,
            Some(false) => Self::No,
            None => Self::Unknown,
        }
    }
}

/** Machine-readable category of a Work Capture error
 * Variants
    - InvalidRegistration - a resource definition failed validation
    - UnknownConnector - the connector type is not registered
    - InvalidQuery - a query or plan is outside its allowed operations, filters, or limits
    - Unauthorized - the actor may not perform the operation
    - InvalidRelation - a relation type, endpoint, or status is not allowed
    - InvalidTransition - a lifecycle transition is not allowed
    - SelfApproval - an actor tried to approve its own contribution
    - PolicyChange - a contribution tried to change policy outside Crane's workflow
    - EvidenceRequired - the operation needs supporting evidence
    - InvalidRecipe - a recipe failed validation
    - NotFound - the referenced object does not exist
    - Storage - the store could not be read or written
    - SchemaVersion - the stored schema version is not supported
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ErrorCode {
    InvalidRegistration,
    UnknownConnector,
    InvalidQuery,
    Unauthorized,
    InvalidRelation,
    InvalidTransition,
    SelfApproval,
    PolicyChange,
    EvidenceRequired,
    InvalidRecipe,
    NotFound,
    Storage,
    SchemaVersion,
}

/** A Work Capture error: a category and an explanation
 * Fields
    - code: ErrorCode - category
    - message: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaptureError {
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
}

impl CaptureError {
    /** Build an error
     * Input
        - code: ErrorCode - category
        - message: impl Into<String> - explanation
     * Output
        - CaptureError
    */
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for CaptureError {
    /** Render the error as "code: message"
     * Input
        - formatter: &mut fmt::Formatter - output
     * Output
        - fmt::Result
    */
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = serde_json::to_value(self.code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        write!(formatter, "{code}: {}", self.message)
    }
}

impl From<CaptureError> for String {
    /** Convert to Crane's String error type
     * Input
        - error: CaptureError - error
     * Output
        - String
    */
    fn from(error: CaptureError) -> Self {
        error.to_string()
    }
}

/** Result of a Work Capture operation */
pub(crate) type CaptureResult<T> = Result<T, CaptureError>;

/** What kind of actor acts
 * Variants
    - Human - a person
    - Agent - an AI agent
    - System - Work Capture itself or another deterministic component
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ActorKind {
    Human,
    Agent,
    System,
}

/** An actor requesting evidence, proposing context, or reviewing it
 * Fields
    - id: String - stable identity
    - kind: ActorKind - human, agent, or system
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Actor {
    pub(crate) id: String,
    pub(crate) kind: ActorKind,
}

impl Actor {
    /** Build an actor
     * Input
        - id: &str - identity
        - kind: ActorKind - kind
     * Output
        - Actor
    */
    pub(crate) fn new(id: &str, kind: ActorKind) -> Self {
        Self {
            id: id.into(),
            kind,
        }
    }

    /** The Work Capture system actor, author of deterministic derivations
     * Input
        - None
     * Output
        - Actor
    */
    pub(crate) fn system() -> Self {
        Self::new("work-capture", ActorKind::System)
    }

    /** The person operating Crane, as a human actor. Being human is what lets an actor record
     * observed context and, with review rights, accept contributions, so it is never taken from the
     * caller's word: inside an AI agent session no human actor can be made at all
     * Input
        - id: &str - the operator's identity
     * Output
        - CaptureResult<Actor>
        - Error when an AI agent session is detected
    */
    pub(crate) fn human_operator(id: &str) -> CaptureResult<Self> {
        match crate::trust::agent_environment() {
            Some(marker) => Err(CaptureError::new(
                ErrorCode::Unauthorized,
                format!("an AI agent session is active ({marker} is set); only a person acts as a human here"),
            )),
            None => Ok(Self::new(id, ActorKind::Human)),
        }
    }
}

/** How sensitive a piece of evidence is (ordered from least to most)
 * Variants
    - Public - may be shown anywhere
    - Internal - for members of the repository
    - Confidential - restricted; secrets were found and redacted
    - Secret - never stored as content and never placed in agent-facing context
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Sensitivity {
    Public,
    #[default]
    Internal,
    Confidential,
    Secret,
}

/** What an actor may retrieve and decide. It is enforced by the broker, the graph, and the
 * contribution ledger at execution time, never by instructions given to a model
 * Fields
    - actor: Actor - the actor
    - resources: BTreeSet<String> - resource identifiers the actor may query
    - repositories: BTreeSet<String> - repositories the actor may see
    - environments: BTreeSet<String> - environments the actor may see
    - evidence_types: Option<BTreeSet<EvidenceType>> - permitted evidence types, None for all
    - max_sensitivity: Sensitivity - most sensitive evidence the actor may receive
    - may_review: bool - whether the actor may accept or reject contributions of others
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccessPolicy {
    pub(crate) actor: Actor,
    pub(crate) resources: BTreeSet<String>,
    pub(crate) repositories: BTreeSet<String>,
    pub(crate) environments: BTreeSet<String>,
    pub(crate) evidence_types: Option<BTreeSet<EvidenceType>>,
    pub(crate) max_sensitivity: Sensitivity,
    pub(crate) may_review: bool,
}

impl AccessPolicy {
    /** Report whether the actor may query a resource
     * Input
        - resource_id: &str - resource
     * Output
        - bool
    */
    pub(crate) fn permits_resource(&self, resource_id: &str) -> bool {
        self.resources.contains(resource_id)
    }

    /** Report whether the actor may see a repository; an unknown repository is never permitted
     * Input
        - repository: Option<&str> - repository, None when unknown
     * Output
        - bool
    */
    pub(crate) fn permits_repository(&self, repository: Option<&str>) -> bool {
        repository.is_some_and(|name| self.repositories.contains(name))
    }

    /** Report whether the actor may see an environment; evidence without an environment is
     * permitted (it is not environment-specific)
     * Input
        - environment: Option<&str> - environment, None when not environment-specific
     * Output
        - bool
    */
    pub(crate) fn permits_environment(&self, environment: Option<&str>) -> bool {
        environment.is_none_or(|name| self.environments.contains(name))
    }

    /** Report whether the actor may receive an evidence type
     * Input
        - evidence_type: EvidenceType - type
     * Output
        - bool
    */
    pub(crate) fn permits_type(&self, evidence_type: EvidenceType) -> bool {
        self.evidence_types
            .as_ref()
            .is_none_or(|types| types.contains(&evidence_type))
    }
}

/** The source of timestamps: the system clock, or a fixed clock that advances one millisecond
 * per reading, so fixtures and tests are deterministic and still order their records
 * Fields
    - fixed: Option<AtomicU64> - next fixed reading, None for the system clock (atomic, so a clock
      can be shared by work running on several threads)
*/
#[derive(Debug, Default)]
pub(crate) struct Clock {
    fixed: Option<AtomicU64>,
}

impl Clone for Clock {
    /** Copy the clock: a fixed clock continues from its current reading, independently
     * Input
        - None (uses self)
     * Output
        - Clock
    */
    fn clone(&self) -> Self {
        Self {
            fixed: self
                .fixed
                .as_ref()
                .map(|next| AtomicU64::new(next.load(Ordering::SeqCst))),
        }
    }
}

impl Clock {
    /** The system clock
     * Input
        - None
     * Output
        - Clock
    */
    pub(crate) fn system() -> Self {
        Self { fixed: None }
    }

    /** A deterministic clock starting at a time
     * Input
        - start: u64 - first reading in Unix milliseconds
     * Output
        - Clock
    */
    pub(crate) fn fixed(start: u64) -> Self {
        Self {
            fixed: Some(AtomicU64::new(start)),
        }
    }

    /** Read the clock
     * Input
        - None (uses self)
     * Output
        - u64 Unix milliseconds
    */
    pub(crate) fn now(&self) -> u64 {
        match &self.fixed {
            Some(next) => next.fetch_add(1, Ordering::SeqCst),
            None => crate::platform::now_millis(),
        }
    }

    /** A value that tells apart identifiers made at the same instant by different processes:
     * random under the system clock, empty under a fixed clock (so fixtures stay deterministic)
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn nonce(&self) -> String {
        if self.fixed.is_some() {
            return String::new();
        }
        let mut bytes = [0u8; 8];
        match getrandom::fill(&mut bytes) {
            Ok(()) => bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
            Err(_) => format!("{}-{}", std::process::id(), crate::platform::now_millis()),
        }
    }
}

/** Operational counters of one investigation (Work Capture's own telemetry; DataSea does not
 * observe Work Capture's retrieval by itself)
 * Fields
    - connector_calls: u64 - connector operations performed
    - connector_failures: u64 - operations that did not return Ok or Partial
    - connector_latency_ms: u64 - total time spent in connectors
    - records_retrieved: u64 - source records returned
    - records_normalized: u64 - evidence records produced
    - duplicates: u64 - records already captured
    - malformed: u64 - records parsed partially
    - parse_failures: u64 - records that could not be parsed
    - correlation_failures: u64 - relations rejected by validation
    - branches_taken: u64 - conditional steps whose condition held
    - branches_skipped: u64 - conditional steps whose condition did not hold
    - traversal_depth: u64 - deepest graph traversal
    - pack_bytes: u64 - serialized size of the Evidence Pack
    - pack_build_ms: u64 - time spent building it
    - missing_required: u64 - required evidence categories not satisfied
    - budget_exhausted: u64 - budget limits reached
    - authorization_denials: u64 - resources or records refused by policy
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Metrics {
    pub(crate) connector_calls: u64,
    pub(crate) connector_failures: u64,
    pub(crate) connector_latency_ms: u64,
    pub(crate) records_retrieved: u64,
    pub(crate) records_normalized: u64,
    pub(crate) duplicates: u64,
    pub(crate) malformed: u64,
    pub(crate) parse_failures: u64,
    pub(crate) correlation_failures: u64,
    pub(crate) branches_taken: u64,
    pub(crate) branches_skipped: u64,
    pub(crate) traversal_depth: u64,
    pub(crate) pack_bytes: u64,
    pub(crate) pack_build_ms: u64,
    pub(crate) missing_required: u64,
    pub(crate) budget_exhausted: u64,
    pub(crate) authorization_denials: u64,
}
