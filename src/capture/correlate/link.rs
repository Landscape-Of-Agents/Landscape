// Deterministic correlation rules: from a normalized Evidence Record to the entities it names and
// the typed relations its source reports or states. A relation a source reports about its own
// object (a CI run naming its commit) is observed; a reference one record makes to another object
// (a pull request naming its task) is declared; a relation derived by a rule (a branch name that
// contains the task key, a Landscape selector matching a file) is declared when the governing
// configuration states it and inferred otherwise. Nothing is linked by temporal proximity.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::graph::EvidenceGraph;
use super::model::{Entity, EntityKind, NodeRef, Relation, RelationStatus, RelationType};
use crate::capture::format::record::{EvidenceRecord, EvidenceType, Payload};
use crate::platform::glob;

/** What correlation knows about the investigation
 * Fields
    - task: Option<&'a str> - task key of the work episode
    - episode: Option<&'a str> - work episode entity id
*/
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LinkContext<'a> {
    pub(crate) task: Option<&'a str>,
    pub(crate) episode: Option<&'a str>,
}

/** What correlation produced
 * Fields
    - entities: Vec<String> - entities the record named
    - relations: usize - relations added or merged
    - failures: Vec<String> - relations rejected by validation
*/
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LinkOutcome {
    pub(crate) entities: Vec<String>,
    pub(crate) relations: usize,
    pub(crate) failures: Vec<String>,
}

/** Correlation of one record in progress
 * Fields
    - graph: &'g mut EvidenceGraph - graph
    - evidence: String - evidence id
    - outcome: LinkOutcome - what was produced
*/
struct Linker<'g> {
    graph: &'g mut EvidenceGraph,
    evidence: String,
    outcome: LinkOutcome,
}

impl Linker<'_> {
    /** Add an entity named by the record
     * Input
        - kind: EntityKind - kind
        - key: &str - natural key
        - version: Option<String> - version
        - attributes: Value - attributes (an object)
     * Output
        - String entity id
    */
    fn entity(
        &mut self,
        kind: EntityKind,
        key: &str,
        version: Option<String>,
        attributes: Value,
    ) -> String {
        let mut entity = Entity::new(kind, key);
        entity.first_evidence = Some(self.evidence.clone());
        if let Value::Object(map) = attributes {
            entity.attributes = map;
        }
        let id = self.graph.add_entity(entity);
        // The version comes from the evidence being linked, so it may update a known one
        if let Some(version) = version {
            self.graph.observe_version(&id, version);
        }
        if !self.outcome.entities.contains(&id) {
            self.outcome.entities.push(id.clone());
        }
        id
    }

    /** Add a relation between two entities, supported by the record
     * Input
        - relation_type: RelationType - type
        - source: &str - source entity id
        - target: &str - target entity id
        - status: RelationStatus - how it is known
        - method: &str - rule
     * Output
        - None (a rejected relation is recorded as a failure)
    */
    fn relate(
        &mut self,
        relation_type: RelationType,
        source: &str,
        target: &str,
        status: RelationStatus,
        method: &str,
    ) {
        let relation = Relation::new(
            relation_type,
            NodeRef::Entity(source.into()),
            NodeRef::Entity(target.into()),
            status,
            method,
            vec![self.evidence.clone()],
        );
        match self.graph.add_relation(relation) {
            Ok(_) => self.outcome.relations += 1,
            Err(error) => self.outcome.failures.push(error.to_string()),
        }
    }
}

/** Correlate one record: add the entities it names, the relations it reports or states, and an
 * OBSERVED_ON relation from the evidence to each entity
 * Input
    - graph: &mut EvidenceGraph - graph
    - record: &EvidenceRecord - record
    - context: &LinkContext - investigation context
 * Output
    - LinkOutcome
*/
pub(crate) fn link(
    graph: &mut EvidenceGraph,
    record: &EvidenceRecord,
    context: &LinkContext,
) -> LinkOutcome {
    graph.register(NodeRef::Evidence(record.evidence_id.clone()));
    let mut linker = Linker {
        graph,
        evidence: record.evidence_id.clone(),
        outcome: LinkOutcome::default(),
    };
    let refs = &record.refs;
    let observed = RelationStatus::Observed;
    let declared = RelationStatus::Declared;
    let repository = record
        .scope
        .repository
        .as_deref()
        .map(|name| linker.entity(EntityKind::Repository, name, None, json!({})));
    let commit = refs
        .commit
        .as_deref()
        .map(|sha| linker.entity(EntityKind::Commit, sha, None, json!({})));
    let environment = record
        .scope
        .environment
        .as_deref()
        .map(|name| linker.entity(EntityKind::Environment, name, None, json!({})));
    let field = |name: &str| record.field(name);
    match record.evidence_type {
        EvidenceType::Task => {
            let key = refs
                .task
                .clone()
                .unwrap_or_else(|| record.source_ref.record_id.clone());
            let task = linker.entity(
                EntityKind::Task,
                &key,
                None,
                json!({"title": field("title")}),
            );
            if let Some(number) = refs.pull_request.as_deref() {
                let pr = linker.entity(EntityKind::PullRequest, number, None, json!({}));
                linker.relate(
                    RelationType::ResultedIn,
                    &task,
                    &pr,
                    declared,
                    "task.pull_request",
                );
            }
        }
        EvidenceType::PullRequest => {
            let number = field("number")
                .or_else(|| refs.pull_request.clone())
                .unwrap_or_else(|| record.source_ref.record_id.clone());
            let pr = linker.entity(
                EntityKind::PullRequest,
                &number,
                None,
                json!({"title": field("title"), "branch": field("branch")}),
            );
            if let Some(repository) = &repository {
                linker.relate(
                    RelationType::Contains,
                    repository,
                    &pr,
                    observed,
                    "pull_request.repository",
                );
            }
            for sha in refs.commits.iter().chain(refs.commit.iter()) {
                let commit = linker.entity(EntityKind::Commit, sha, None, json!({}));
                linker.relate(
                    RelationType::Contains,
                    &pr,
                    &commit,
                    observed,
                    "pull_request.commits",
                );
            }
            match (refs.task.as_deref(), context.task, field("branch")) {
                (Some(task), _, _) => {
                    let task = linker.entity(EntityKind::Task, task, None, json!({}));
                    linker.relate(
                        RelationType::ResultedIn,
                        &task,
                        &pr,
                        declared,
                        "pull_request.task",
                    );
                }
                (None, Some(task), Some(branch))
                    if branch
                        .to_ascii_lowercase()
                        .contains(&task.to_ascii_lowercase()) =>
                {
                    let task = linker.entity(EntityKind::Task, task, None, json!({}));
                    linker.relate(
                        RelationType::ResultedIn,
                        &task,
                        &pr,
                        RelationStatus::Inferred,
                        "branch_name_match",
                    );
                }
                _ => {}
            }
        }
        EvidenceType::CodeChange => {
            if let (Some(repository), Some(commit)) = (&repository, &commit) {
                linker.relate(
                    RelationType::Contains,
                    repository,
                    commit,
                    observed,
                    "commit.repository",
                );
            }
            if let (Some(commit), Some(Payload::Code { files, .. })) = (&commit, &record.payload) {
                for file in files {
                    let file_entity = linker.entity(EntityKind::File, &file.path, None, json!({}));
                    linker.relate(
                        RelationType::Writes,
                        commit,
                        &file_entity,
                        observed,
                        "commit.files",
                    );
                }
            }
        }
        EvidenceType::CiRun => {
            let key = refs
                .ci_run
                .clone()
                .unwrap_or_else(|| record.source_ref.record_id.clone());
            let run = linker.entity(
                EntityKind::CiRun,
                &key,
                None,
                json!({"name": field("name")}),
            );
            if let Some(commit) = &commit {
                linker.relate(
                    RelationType::Triggered,
                    commit,
                    &run,
                    observed,
                    "ci_run.commit",
                );
            }
        }
        EvidenceType::TestReport => {
            if let Some(run) = refs.ci_run.as_deref() {
                linker.entity(EntityKind::CiRun, run, None, json!({}));
            }
        }
        EvidenceType::Deployment => {
            let key = refs
                .deployment
                .clone()
                .unwrap_or_else(|| record.source_ref.record_id.clone());
            let deployment = linker.entity(EntityKind::Deployment, &key, None, json!({}));
            if let Some(commit) = &commit {
                linker.relate(
                    RelationType::ResultedIn,
                    commit,
                    &deployment,
                    observed,
                    "deployment.commit",
                );
            }
            if let Some(environment) = &environment {
                linker.relate(
                    RelationType::DeployedTo,
                    &deployment,
                    environment,
                    observed,
                    "deployment.environment",
                );
            }
        }
        EvidenceType::Verification
        | EvidenceType::RuntimeLog
        | EvidenceType::Metric
        | EvidenceType::Configuration => {
            if let Some(deployment) = refs.deployment.as_deref() {
                linker.entity(EntityKind::Deployment, deployment, None, json!({}));
            }
        }
        EvidenceType::AgentSession | EvidenceType::ToolCall | EvidenceType::AgentEvent => {
            if let Some(session) = refs.session.as_deref() {
                let session = linker.entity(
                    EntityKind::AgentSession,
                    session,
                    None,
                    json!({"agent": refs.agent}),
                );
                if record.evidence_type == EvidenceType::ToolCall {
                    let call = linker.entity(
                        EntityKind::ToolCall,
                        &record.source_ref.record_id,
                        None,
                        json!({"tool": field("tool")}),
                    );
                    linker.relate(
                        RelationType::ExecutedDuring,
                        &call,
                        &session,
                        observed,
                        "datasea.scope",
                    );
                }
                if let (Some(task), Some(episode)) = (context.task, context.episode) {
                    // Either identity of the task ties the session to the episode
                    if refs.task.as_deref() == Some(task)
                        || field("external_task").as_deref() == Some(task)
                    {
                        linker.relate(
                            RelationType::ExecutedDuring,
                            &session,
                            episode,
                            observed,
                            "datasea.task",
                        );
                    }
                }
            }
        }
        EvidenceType::GraphState => {
            let kind = match field("kind").as_deref() {
                Some("flow") => EntityKind::Flow,
                _ => EntityKind::Zone,
            };
            if let Some(id) = field("id") {
                let attributes = match &record.payload {
                    Some(Payload::Graph { nodes, .. }) => {
                        nodes.first().cloned().unwrap_or(Value::Null)
                    }
                    _ => Value::Null,
                };
                let attributes = json!({
                    "selectors": attributes.get("selectors"),
                    "include": attributes.get("include"),
                    "policies": attributes.get("policies"),
                });
                linker.entity(
                    kind,
                    &id,
                    field("version").map(|version| format!("v{version}")),
                    attributes,
                );
            }
        }
        EvidenceType::Policy => {
            if let Some(name) = field("name") {
                linker.entity(
                    EntityKind::Policy,
                    &name,
                    record.source_ref.revision.clone(),
                    json!({}),
                );
            }
        }
        EvidenceType::Validation => {}
    }
    let Linker {
        graph,
        evidence,
        mut outcome,
    } = linker;
    for entity in outcome.entities.clone() {
        let relation = Relation::new(
            RelationType::ObservedOn,
            NodeRef::Evidence(evidence.clone()),
            NodeRef::Entity(entity),
            RelationStatus::Observed,
            "record.refs",
            vec![evidence.clone()],
        );
        match graph.add_relation(relation) {
            Ok(_) => outcome.relations += 1,
            Err(error) => outcome.failures.push(error.to_string()),
        }
    }
    outcome
}

/** Read a list of strings from an entity attribute path
 * Input
    - entity: &Entity - entity
    - path: &[&str] - attribute path
 * Output
    - Vec<String>
*/
fn strings(entity: &Entity, path: &[&str]) -> Vec<String> {
    let mut node = entity.attributes.get(path[0]);
    for part in &path[1..] {
        node = node.and_then(|value| value.get(*part));
    }
    node.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/** Link files to the Landscape zones and flows whose declared selectors cover them, and zones and
 * flows to the Crane policies they declare (only policies already in the graph), for every file
 * and every zone and flow
 * Input
    - graph: &mut EvidenceGraph - graph
 * Output
    - LinkOutcome
*/
#[cfg_attr(not(test), allow(dead_code))] // the full linking; investigations link only what changed
pub(crate) fn link_governance(graph: &mut EvidenceGraph) -> LinkOutcome {
    link_governance_where(graph, &|_, _| true)
}

/** Link governance only where something changed: files first seen since the last linking against
 * every zone and flow, and every file against zones and flows that are new or changed version. The
 * links of unchanged pairs already exist (relation ids are deterministic), so the result equals a
 * full linking while the work stays proportional to what changed
 * Input
    - graph: &mut EvidenceGraph - graph
    - new_files: &BTreeSet<String> - file entities added since the last linking
    - changed_scopes: &BTreeSet<String> - zone and flow entities added or re-versioned since
 * Output
    - LinkOutcome
*/
pub(crate) fn link_governance_changes(
    graph: &mut EvidenceGraph,
    new_files: &BTreeSet<String>,
    changed_scopes: &BTreeSet<String>,
) -> LinkOutcome {
    link_governance_where(graph, &|file, scope| {
        new_files.contains(file) || changed_scopes.contains(scope)
    })
}

/** Link governance for the file and zone or flow pairs a filter admits (policy links of every zone
 * and flow are always refreshed: they are few)
 * Input
    - graph: &mut EvidenceGraph - graph
    - admit: &dyn Fn(&str, &str) -> bool - whether to link a file entity to a zone or flow entity
 * Output
    - LinkOutcome
*/
fn link_governance_where(
    graph: &mut EvidenceGraph,
    admit: &dyn Fn(&str, &str) -> bool,
) -> LinkOutcome {
    let mut outcome = LinkOutcome::default();
    let files = graph
        .entities
        .values()
        .filter(|entity| entity.kind == EntityKind::File)
        .map(|entity| (entity.entity_id.clone(), entity.key.clone()))
        .collect::<Vec<_>>();
    let governed = graph
        .entities
        .values()
        .filter(|entity| matches!(entity.kind, EntityKind::Zone | EntityKind::Flow))
        .cloned()
        .collect::<Vec<_>>();
    for scope in governed {
        let (relation_type, patterns, files_listed, directories) = if scope.kind == EntityKind::Zone
        {
            (
                RelationType::BelongsToZone,
                strings(&scope, &["selectors", "paths"]),
                strings(&scope, &["selectors", "files"]),
                strings(&scope, &["selectors", "directories"]),
            )
        } else {
            (
                RelationType::BelongsToFlow,
                strings(&scope, &["include"]),
                Vec::new(),
                Vec::new(),
            )
        };
        let evidence = scope.first_evidence.clone().into_iter().collect::<Vec<_>>();
        let version_scope = Some(format!(
            "{}:{}@{}",
            if scope.kind == EntityKind::Zone {
                "zone"
            } else {
                "flow"
            },
            scope.key,
            scope.version.clone().unwrap_or_else(|| "unknown".into())
        ));
        for (file_id, path) in &files {
            if !admit(file_id, &scope.entity_id) {
                continue;
            }
            let covered = patterns.iter().any(|pattern| glob::matches(pattern, path))
                || files_listed.iter().any(|file| file == path)
                || directories.iter().any(|directory| {
                    path.starts_with(&format!("{}/", directory.trim_end_matches('/')))
                });
            if covered {
                let mut relation = Relation::new(
                    relation_type,
                    NodeRef::Entity(file_id.clone()),
                    NodeRef::Entity(scope.entity_id.clone()),
                    RelationStatus::Declared,
                    "landscape.selectors",
                    evidence.clone(),
                );
                relation.version_scope = version_scope.clone();
                match graph.add_relation(relation) {
                    Ok(_) => outcome.relations += 1,
                    Err(error) => outcome.failures.push(error.to_string()),
                }
            }
        }
        for policy in strings(&scope, &["policies"]) {
            let policy_id = super::model::entity_id(EntityKind::Policy, &policy);
            if graph.entities.contains_key(&policy_id) {
                let mut relation = Relation::new(
                    RelationType::GovernedBy,
                    NodeRef::Entity(scope.entity_id.clone()),
                    NodeRef::Entity(policy_id),
                    RelationStatus::Declared,
                    "landscape.policies",
                    evidence.clone(),
                );
                relation.version_scope = version_scope.clone();
                match graph.add_relation(relation) {
                    Ok(_) => outcome.relations += 1,
                    Err(error) => outcome.failures.push(error.to_string()),
                }
            }
        }
    }
    outcome
}
