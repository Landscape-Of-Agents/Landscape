// The core context model: entities (system objects), relations (typed edges between entities,
// evidence, assertions, and contributions), and the controlled relation vocabulary. A relation is
// accepted only when its type allows its endpoint kinds; the vocabulary grows through versioned
// code changes (VOCABULARY_VERSION), never through data. Every relation records how it is known:
// an inferred relation is never stored as observed.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::capture::{stable_id, CaptureError, CaptureResult, ErrorCode};

/** Version of the relation vocabulary */
pub(crate) const VOCABULARY_VERSION: u32 = 1;

/** Kinds of system objects
 * Variants
    - Repository, Commit, File, PullRequest - repository changes
    - Task, WorkEpisode - work
    - CiRun, Deployment, Environment - delivery
    - AgentSession, ToolCall - agent execution (DataSea identifiers)
    - Zone, Flow, Policy - governance (Landscape and Crane identifiers)
    - Service, Api, Table, Resource - runtime system objects
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EntityKind {
    Repository,
    Commit,
    File,
    PullRequest,
    Task,
    WorkEpisode,
    CiRun,
    Deployment,
    Environment,
    AgentSession,
    ToolCall,
    Zone,
    Flow,
    Policy,
    Service,
    Api,
    Table,
    Resource,
}

/** A system object
 * Fields
    - entity_id: String - stable identifier (kind and key)
    - kind: EntityKind - kind
    - key: String - natural key in its source (commit SHA, zone id, deployment id, ...)
    - version: Option<String> - version where the source versions it (zone v3, policy digest)
    - attributes: Map<String, Value> - descriptive attributes
    - first_evidence: Option<String> - evidence that first named it, None when declared directly
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Entity {
    pub(crate) entity_id: String,
    pub(crate) kind: EntityKind,
    pub(crate) key: String,
    pub(crate) version: Option<String>,
    pub(crate) attributes: Map<String, Value>,
    pub(crate) first_evidence: Option<String>,
}

impl Entity {
    /** Build an entity with its stable identifier
     * Input
        - kind: EntityKind - kind
        - key: &str - natural key
     * Output
        - Entity
    */
    pub(crate) fn new(kind: EntityKind, key: &str) -> Self {
        Self {
            entity_id: entity_id(kind, key),
            kind,
            key: key.into(),
            version: None,
            attributes: Map::new(),
            first_evidence: None,
        }
    }
}

/** The stable identifier of an entity. Keys whose spelling does not change their meaning are
 * normalized first (a commit sha and an environment name are case-insensitive and trimmed), so
 * "9F1C2AB7D4" and "9f1c2ab7d4" are one commit
 * Input
    - kind: EntityKind - kind
    - key: &str - natural key
 * Output
    - String
*/
pub(crate) fn entity_id(kind: EntityKind, key: &str) -> String {
    let name = serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    let key = match kind {
        EntityKind::Commit | EntityKind::Environment => key.trim().to_ascii_lowercase(),
        _ => key.to_string(),
    };
    stable_id("ent", &[&name, &key])
}

/** A node of the evidence graph
 * Variants
    - Entity(String) - an entity
    - Evidence(String) - an evidence record
    - Assertion(String) - an assertion
    - Contribution(String) - a context contribution
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "node", content = "id", rename_all = "snake_case")]
pub(crate) enum NodeRef {
    Entity(String),
    Evidence(String),
    Assertion(String),
    Contribution(String),
}

impl NodeRef {
    /** The node's identifier
     * Input
        - None (uses self)
     * Output
        - &str
    */
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Entity(id)
            | Self::Evidence(id)
            | Self::Assertion(id)
            | Self::Contribution(id) => id,
        }
    }
}

/** The controlled relation vocabulary
 * Variants
    - Contains, DependsOn, Calls, Reads, Writes - structural
    - Triggered, DeployedTo, ExecutedDuring, ResultedIn - operational
    - BelongsToZone, BelongsToFlow, GovernedBy - governance
    - Supports, Contradicts, DerivedFrom, ObservedOn - evidence
    - ProposedFor, AppliesTo, Supersedes - context
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum RelationType {
    Contains,
    DependsOn,
    Calls,
    Reads,
    Writes,
    Triggered,
    DeployedTo,
    ExecutedDuring,
    ResultedIn,
    BelongsToZone,
    BelongsToFlow,
    GovernedBy,
    Supports,
    Contradicts,
    DerivedFrom,
    ObservedOn,
    ProposedFor,
    AppliesTo,
    Supersedes,
}

/** How a relation is known
 * Variants
    - Observed - a source directly reports it
    - Declared - a source states it (configuration, a reference in a record)
    - Inferred - derived by a rule (name matching, selectors); never presented as observed
    - Reviewed - confirmed by an authorized human
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RelationStatus {
    Observed,
    Declared,
    Inferred,
    Reviewed,
}

/** Where a relation came from
 * Fields
    - method: String - rule or source that produced it ("ci_run.commit", "branch_name_match")
    - method_version: String - version of that rule
    - evidence: Vec<String> - evidence records supporting it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RelationProvenance {
    pub(crate) method: String,
    pub(crate) method_version: String,
    pub(crate) evidence: Vec<String>,
}

/** A typed relation
 * Fields
    - relation_id: String - stable identifier (type, endpoints, status)
    - relation_type: RelationType - type
    - source: NodeRef - from
    - target: NodeRef - to
    - status: RelationStatus - observed, declared, inferred, or reviewed
    - provenance: RelationProvenance - how it was produced
    - valid_from: Option<u64> - start of temporal validity (Unix ms), None when unbounded/unknown
    - valid_to: Option<u64> - end of temporal validity
    - version_scope: Option<String> - versions it holds for ("zone:payments@v3")
    - vocabulary_version: u32 - VOCABULARY_VERSION
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Relation {
    pub(crate) relation_id: String,
    pub(crate) relation_type: RelationType,
    pub(crate) source: NodeRef,
    pub(crate) target: NodeRef,
    pub(crate) status: RelationStatus,
    pub(crate) provenance: RelationProvenance,
    pub(crate) valid_from: Option<u64>,
    pub(crate) valid_to: Option<u64>,
    pub(crate) version_scope: Option<String>,
    pub(crate) vocabulary_version: u32,
}

impl Relation {
    /** Build a relation with its stable identifier
     * Input
        - relation_type: RelationType - type
        - source: NodeRef - from
        - target: NodeRef - to
        - status: RelationStatus - how it is known
        - method: &str - rule or source that produced it
        - evidence: Vec<String> - supporting evidence
     * Output
        - Relation
    */
    pub(crate) fn new(
        relation_type: RelationType,
        source: NodeRef,
        target: NodeRef,
        status: RelationStatus,
        method: &str,
        evidence: Vec<String>,
    ) -> Self {
        let type_name = format!("{relation_type:?}");
        let status_name = format!("{status:?}");
        Self {
            relation_id: stable_id("rel", &[&type_name, source.id(), target.id(), &status_name]),
            relation_type,
            source,
            target,
            status,
            provenance: RelationProvenance {
                method: method.into(),
                method_version: "1".into(),
                evidence,
            },
            valid_from: None,
            valid_to: None,
            version_scope: None,
            vocabulary_version: VOCABULARY_VERSION,
        }
    }
}

/** The kind of a node for endpoint validation
 * Variants
    - Entity(EntityKind) - an entity of a kind
    - Evidence - an evidence record
    - Assertion - an assertion
    - Contribution - a contribution
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Entity(EntityKind),
    Evidence,
    Assertion,
    Contribution,
}

/** Check that a relation type allows its endpoint kinds
 * Input
    - relation_type: RelationType - type
    - source: NodeKind - from
    - target: NodeKind - to
 * Output
    - CaptureResult<()>
    - Error (InvalidRelation) when the endpoints are not allowed
*/
pub(crate) fn check_endpoints(
    relation_type: RelationType,
    source: NodeKind,
    target: NodeKind,
) -> CaptureResult<()> {
    use NodeKind::{Assertion, Contribution, Entity, Evidence};
    use RelationType as T;
    let allowed = match relation_type {
        T::Contains
        | T::DependsOn
        | T::Calls
        | T::Reads
        | T::Writes
        | T::Triggered
        | T::DeployedTo
        | T::ExecutedDuring
        | T::ResultedIn => {
            matches!((source, target), (Entity(_), Entity(_)))
        }
        T::BelongsToZone => matches!((source, target), (Entity(_), Entity(EntityKind::Zone))),
        T::BelongsToFlow => matches!((source, target), (Entity(_), Entity(EntityKind::Flow))),
        T::GovernedBy => matches!((source, target), (Entity(_), Entity(EntityKind::Policy))),
        T::Supports | T::Contradicts => {
            matches!((source, target), (Evidence | Assertion, Assertion))
        }
        T::DerivedFrom => matches!(
            (source, target),
            (Assertion | Contribution | Evidence, Evidence | Assertion)
        ),
        T::ObservedOn => matches!((source, target), (Evidence, Entity(_))),
        T::ProposedFor | T::AppliesTo => {
            matches!((source, target), (Contribution | Assertion, Entity(_)))
        }
        T::Supersedes => matches!(
            (source, target),
            (Evidence, Evidence) | (Assertion, Assertion) | (Contribution, Contribution)
        ),
    };
    if allowed {
        Ok(())
    } else {
        Err(CaptureError::new(
            ErrorCode::InvalidRelation,
            format!("{relation_type:?} cannot connect {source:?} to {target:?}"),
        ))
    }
}
