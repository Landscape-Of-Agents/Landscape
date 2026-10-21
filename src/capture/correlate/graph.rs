// The evidence graph: entities and validated relations with adjacency indexes, bounded
// breadth-first traversal, and Context Views. The graph is the canonical relationship model;
// trees and paths are traversal results. Every traversal names the relation types it may follow
// and is capped in depth, result count, and payload size (requests are clamped to hard limits), and
// a visibility check (scope and authorization) hides nodes the requester may not see. Traversal
// never retrieves everything connected by default.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::model::{
    check_endpoints, Entity, NodeKind, NodeRef, Relation, RelationStatus, RelationType,
};
use crate::capture::{stable_id, CaptureError, CaptureResult, ErrorCode};

/** Deepest traversal allowed, whatever is requested */
pub(crate) const HARD_MAX_DEPTH: usize = 6;

/** Most nodes a traversal may return, whatever is requested */
pub(crate) const HARD_MAX_RESULTS: usize = 1000;

/** Largest serialized traversal result, whatever is requested */
pub(crate) const HARD_MAX_BYTES: usize = 1024 * 1024;

/** Version of the Context View layout */
pub(crate) const VIEW_VERSION: u32 = 1;

/** Which way relations are followed
 * Variants
    - Outgoing - from source to target
    - Incoming - from target to source
    - Both - either way
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Direction {
    Outgoing,
    Incoming,
    Both,
}

/** A bounded traversal request
 * Fields
    - start: Vec<NodeRef> - starting nodes
    - relation_types: Vec<RelationType> - relation types that may be followed (required)
    - statuses: Vec<RelationStatus> - relation statuses that may be followed, empty for all
    - direction: Direction - direction
    - max_depth: usize - deepest hop count
    - max_results: usize - most nodes returned
    - max_bytes: usize - largest serialized result
    - at: Option<u64> - follow only relations valid at this time (Unix ms)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TraversalRequest {
    pub(crate) start: Vec<NodeRef>,
    pub(crate) relation_types: Vec<RelationType>,
    pub(crate) statuses: Vec<RelationStatus>,
    pub(crate) direction: Direction,
    pub(crate) max_depth: usize,
    pub(crate) max_results: usize,
    pub(crate) max_bytes: usize,
    pub(crate) at: Option<u64>,
}

/** A limit a traversal reached
 * Variants
    - Depth - nodes existed beyond the depth limit
    - Results - the result count limit was reached
    - Bytes - the payload limit was reached
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LimitHit {
    Depth,
    Results,
    Bytes,
}

/** The path that reached a node
 * Fields
    - node: NodeRef - node reached
    - depth: usize - hops from the start
    - via: Vec<String> - relation identifiers followed, in order
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NodePath {
    pub(crate) node: NodeRef,
    pub(crate) depth: usize,
    pub(crate) via: Vec<String>,
}

/** A traversal result, with the provenance-carrying relations it followed
 * Fields
    - nodes: Vec<NodePath> - nodes reached (starting nodes at depth 0), with their paths
    - relations: Vec<Relation> - relations followed
    - depth_reached: usize - deepest depth reached
    - hidden: usize - nodes not shown because of scope or authorization
    - limits: Vec<LimitHit> - limits reached (the result is truncated when not empty)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TraversalResult {
    pub(crate) nodes: Vec<NodePath>,
    pub(crate) relations: Vec<Relation>,
    pub(crate) depth_reached: usize,
    pub(crate) hidden: usize,
    pub(crate) limits: Vec<LimitHit>,
}

/** A bounded, versioned selection of entities, relations, evidence, and assertions relevant to a
 * question
 * Fields
    - view_id: String - stable identifier (question and content)
    - version: u32 - VIEW_VERSION
    - question: String - question or task it was built for
    - roots: Vec<NodeRef> - starting nodes
    - entities: Vec<Entity> - entities included
    - relations: Vec<Relation> - relations included
    - evidence: Vec<String> - evidence records included
    - assertions: Vec<String> - assertions included
    - limits: Vec<LimitHit> - limits reached while building it
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ContextView {
    pub(crate) view_id: String,
    pub(crate) version: u32,
    pub(crate) question: String,
    pub(crate) roots: Vec<NodeRef>,
    pub(crate) entities: Vec<Entity>,
    pub(crate) relations: Vec<Relation>,
    pub(crate) evidence: Vec<String>,
    pub(crate) assertions: Vec<String>,
    pub(crate) limits: Vec<LimitHit>,
    pub(crate) created_at: u64,
}

/** The evidence graph
 * Fields
    - entities: BTreeMap<String, Entity> - entities by id
    - relations: BTreeMap<String, Relation> - relations by id
    - nodes: BTreeSet<NodeRef> - evidence, assertion, and contribution nodes known to the graph
    - outgoing: BTreeMap<NodeRef, BTreeSet<String>> - relation ids by source
    - incoming: BTreeMap<NodeRef, BTreeSet<String>> - relation ids by target
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct EvidenceGraph {
    pub(crate) entities: BTreeMap<String, Entity>,
    pub(crate) relations: BTreeMap<String, Relation>,
    nodes: BTreeSet<NodeRef>,
    outgoing: BTreeMap<NodeRef, BTreeSet<String>>,
    incoming: BTreeMap<NodeRef, BTreeSet<String>>,
}

impl EvidenceGraph {
    /** Add an entity or merge it into the existing one (attributes are added, the first evidence is
     * kept, and a version only fills one that is unknown: a known version changes only through
     * observe_version, from evidence)
     * Input
        - entity: Entity - entity
     * Output
        - String entity id
    */
    pub(crate) fn add_entity(&mut self, entity: Entity) -> String {
        let id = entity.entity_id.clone();
        match self.entities.get_mut(&id) {
            Some(existing) => {
                for (key, value) in entity.attributes {
                    existing.attributes.entry(key).or_insert(value);
                }
                if existing.version.is_none() {
                    existing.version = entity.version;
                }
                if existing.first_evidence.is_none() {
                    existing.first_evidence = entity.first_evidence;
                }
            }
            None => {
                self.entities.insert(id.clone(), entity);
            }
        }
        id
    }

    /** Record the version of an entity that evidence observed (a zone or flow revision, a policy
     * revision). This is the only way a known version changes; contributions never call it
     * Input
        - entity_id: &str - entity
        - version: String - observed version
     * Output
        - None
    */
    pub(crate) fn observe_version(&mut self, entity_id: &str, version: String) {
        if let Some(entity) = self.entities.get_mut(entity_id) {
            entity.version = Some(version);
        }
    }

    /** Make an evidence, assertion, or contribution node known to the graph
     * Input
        - node: NodeRef - node (entities are added with add_entity)
     * Output
        - None
    */
    pub(crate) fn register(&mut self, node: NodeRef) {
        if !matches!(node, NodeRef::Entity(_)) {
            self.nodes.insert(node);
        }
    }

    /** The kind of a known node
     * Input
        - node: &NodeRef - node
     * Output
        - Option<NodeKind>, None when the node is unknown
    */
    fn kind(&self, node: &NodeRef) -> Option<NodeKind> {
        match node {
            NodeRef::Entity(id) => self
                .entities
                .get(id)
                .map(|entity| NodeKind::Entity(entity.kind)),
            other if self.nodes.contains(other) => Some(match other {
                NodeRef::Evidence(_) => NodeKind::Evidence,
                NodeRef::Assertion(_) => NodeKind::Assertion,
                _ => NodeKind::Contribution,
            }),
            _ => None,
        }
    }

    /** Add a validated relation: both endpoints must exist and the type must allow them; adding
     * the same relation again merges its evidence
     * Input
        - relation: Relation - relation
     * Output
        - CaptureResult<String> relation id
        - Error (InvalidRelation) for unknown endpoints or disallowed endpoint kinds
    */
    pub(crate) fn add_relation(&mut self, relation: Relation) -> CaptureResult<String> {
        let unknown = |node: &NodeRef| {
            CaptureError::new(ErrorCode::InvalidRelation, format!("unknown node {node:?}"))
        };
        let source = self
            .kind(&relation.source)
            .ok_or_else(|| unknown(&relation.source))?;
        let target = self
            .kind(&relation.target)
            .ok_or_else(|| unknown(&relation.target))?;
        check_endpoints(relation.relation_type, source, target)?;
        let id = relation.relation_id.clone();
        if let Some(existing) = self.relations.get_mut(&id) {
            for evidence in relation.provenance.evidence {
                if !existing.provenance.evidence.contains(&evidence) {
                    existing.provenance.evidence.push(evidence);
                }
            }
            return Ok(id);
        }
        self.outgoing
            .entry(relation.source.clone())
            .or_default()
            .insert(id.clone());
        self.incoming
            .entry(relation.target.clone())
            .or_default()
            .insert(id.clone());
        self.relations.insert(id.clone(), relation);
        Ok(id)
    }

    /** List relations touching a node, of the given types, in a direction
     * Input
        - node: &NodeRef - node
        - types: &[RelationType] - relation types (empty for all)
        - direction: Direction - direction
     * Output
        - Vec<&Relation>
    */
    pub(crate) fn relations_of(
        &self,
        node: &NodeRef,
        types: &[RelationType],
        direction: Direction,
    ) -> Vec<&Relation> {
        let mut ids = BTreeSet::new();
        if matches!(direction, Direction::Outgoing | Direction::Both) {
            ids.extend(self.outgoing.get(node).into_iter().flatten().cloned());
        }
        if matches!(direction, Direction::Incoming | Direction::Both) {
            ids.extend(self.incoming.get(node).into_iter().flatten().cloned());
        }
        ids.iter()
            .filter_map(|id| self.relations.get(id))
            .filter(|relation| types.is_empty() || types.contains(&relation.relation_type))
            .collect()
    }

    /** Traverse breadth first within the request's limits (clamped to the hard limits), following
     * only the requested relation types and statuses, and hiding nodes the visibility check
     * rejects (a hidden node is not expanded either)
     * Input
        - request: &TraversalRequest - traversal
        - visible: &dyn Fn(&NodeRef) -> bool - scope and authorization check
     * Output
        - CaptureResult<TraversalResult>
        - Error (InvalidQuery) when no relation type is named
    */
    pub(crate) fn traverse(
        &self,
        request: &TraversalRequest,
        visible: &dyn Fn(&NodeRef) -> bool,
    ) -> CaptureResult<TraversalResult> {
        if request.relation_types.is_empty() {
            return Err(CaptureError::new(
                ErrorCode::InvalidQuery,
                "a traversal must name the relation types it follows",
            ));
        }
        let max_depth = request.max_depth.min(HARD_MAX_DEPTH);
        let max_results = request.max_results.clamp(1, HARD_MAX_RESULTS);
        let max_bytes = request.max_bytes.min(HARD_MAX_BYTES);
        let mut result = TraversalResult::default();
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::new();
        let mut bytes = 0usize;
        let mut hidden = BTreeSet::new();
        for start in &request.start {
            if !visible(start) {
                hidden.insert(start.clone());
                continue;
            }
            if seen.insert(start.clone()) {
                queue.push_back(NodePath {
                    node: start.clone(),
                    depth: 0,
                    via: Vec::new(),
                });
            }
        }
        'search: while let Some(path) = queue.pop_front() {
            if result.nodes.len() >= max_results {
                result.limits.push(LimitHit::Results);
                break;
            }
            let size = serde_json::to_string(&path.node).map_or(0, |text| text.len())
                + match &path.node {
                    NodeRef::Entity(id) => self
                        .entities
                        .get(id)
                        .and_then(|entity| serde_json::to_string(entity).ok())
                        .map_or(0, |text| text.len()),
                    _ => 0,
                };
            if bytes + size > max_bytes {
                result.limits.push(LimitHit::Bytes);
                break;
            }
            bytes += size;
            result.depth_reached = result.depth_reached.max(path.depth);
            let depth = path.depth;
            let node = path.node.clone();
            let via = path.via.clone();
            result.nodes.push(path);
            for relation in self.relations_of(&node, &request.relation_types, request.direction) {
                if !request.statuses.is_empty() && !request.statuses.contains(&relation.status) {
                    continue;
                }
                if let Some(at) = request.at {
                    if relation.valid_from.is_some_and(|from| at < from)
                        || relation.valid_to.is_some_and(|to| at > to)
                    {
                        continue;
                    }
                }
                let next = if relation.source == node {
                    &relation.target
                } else {
                    &relation.source
                };
                if seen.contains(next) {
                    continue;
                }
                if depth >= max_depth {
                    if !result.limits.contains(&LimitHit::Depth) {
                        result.limits.push(LimitHit::Depth);
                    }
                    continue;
                }
                if !visible(next) {
                    hidden.insert(next.clone());
                    continue;
                }
                let relation_size = serde_json::to_string(relation).map_or(0, |text| text.len());
                if bytes + relation_size > max_bytes {
                    result.limits.push(LimitHit::Bytes);
                    break 'search;
                }
                bytes += relation_size;
                seen.insert(next.clone());
                result.relations.push(relation.clone());
                let mut path = via.clone();
                path.push(relation.relation_id.clone());
                queue.push_back(NodePath {
                    node: next.clone(),
                    depth: depth + 1,
                    via: path,
                });
            }
        }
        result.hidden = hidden.len();
        Ok(result)
    }

    /** Build a Context View from a traversal
     * Input
        - question: &str - question or task
        - request: &TraversalRequest - traversal
        - visible: &dyn Fn(&NodeRef) -> bool - scope and authorization check
        - created_at: u64 - Unix milliseconds
     * Output
        - CaptureResult<(ContextView, TraversalResult)>
        - Error when the traversal is invalid
    */
    pub(crate) fn context_view(
        &self,
        question: &str,
        request: &TraversalRequest,
        visible: &dyn Fn(&NodeRef) -> bool,
        created_at: u64,
    ) -> CaptureResult<(ContextView, TraversalResult)> {
        let traversal = self.traverse(request, visible)?;
        let mut view = ContextView {
            version: VIEW_VERSION,
            question: question.into(),
            roots: request.start.clone(),
            relations: traversal.relations.clone(),
            limits: traversal.limits.clone(),
            created_at,
            ..ContextView::default()
        };
        for path in &traversal.nodes {
            match &path.node {
                NodeRef::Entity(id) => view.entities.extend(self.entities.get(id).cloned()),
                NodeRef::Evidence(id) => view.evidence.push(id.clone()),
                NodeRef::Assertion(id) => view.assertions.push(id.clone()),
                NodeRef::Contribution(_) => {}
            }
        }
        view.view_id = view_identifier(&view);
        Ok((view, traversal))
    }
}

/** The stable identifier of a context view: its question and every member (entities, evidence,
 * relations), so a view changed by sealing gets a new identifier
 * Input
    - view: &ContextView - view
 * Output
    - String
*/
pub(crate) fn view_identifier(view: &ContextView) -> String {
    let mut members = view
        .entities
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect::<Vec<_>>();
    members.extend(view.evidence.iter().cloned());
    members.extend(
        view.relations
            .iter()
            .map(|relation| relation.relation_id.clone()),
    );
    let parts = std::iter::once(view.question.as_str())
        .chain(members.iter().map(String::as_str))
        .collect::<Vec<_>>();
    stable_id("ctx", &parts)
}
