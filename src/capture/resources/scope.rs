// Scope resolution, done before any source is queried: which task, episode, repository, commits,
// pull requests, deployments, environment, sessions, zones, and flows the investigation is about,
// its time window, and which registered resources the requesting actor may use for it. Anything
// not known stays explicitly unknown, and a failed lookup never widens a query: a scoped resource
// is never asked without its scope. Facts learned from retrieved evidence (a pull request naming
// its head commit) are added with the evidence that established them.

use serde::{Deserialize, Serialize};

use super::registry::{ResourceRegistry, ResourceStatus};
use crate::capture::broker::run::EvidenceQueryRequest;
use crate::capture::format::record::{EvidenceRecord, EvidenceType};
use crate::capture::AccessPolicy;

/** A reference to an existing versioned object (a Landscape zone or flow)
 * Fields
    - id: String - identifier
    - version: Option<String> - version, None when it could not be resolved
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct VersionedRef {
    pub(crate) id: String,
    pub(crate) version: Option<String>,
}

/** A time window; None bounds are open
 * Fields
    - since: Option<u64> - start (Unix ms), inclusive
    - until: Option<u64> - end (Unix ms), inclusive
*/
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TimeWindow {
    pub(crate) since: Option<u64>,
    pub(crate) until: Option<u64>,
}

/** A scope fact learned from evidence
 * Fields
    - field: String - scope field ("commit", "deployment", ...)
    - value: String - value learned
    - evidence_id: String - evidence that established it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LearnedFact {
    pub(crate) field: String,
    pub(crate) value: String,
    pub(crate) evidence_id: String,
}

/** A resource the actor may not use, and why
 * Fields
    - resource_id: String - resource
    - reason: String - why
    - explicit: bool - whether the request named it explicitly
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Denial {
    pub(crate) resource_id: String,
    pub(crate) reason: String,
    pub(crate) explicit: bool,
}

/** The resolved scope of an investigation
 * Fields
    - episode_id: Option<String> - work episode
    - task_id: Option<String> - task
    - repository: Option<String> - repository, None when unknown
    - branch: Option<String> - branch
    - commits: Vec<String> - commits
    - pull_requests: Vec<String> - pull requests
    - deployments: Vec<String> - deployments
    - environment: Option<String> - environment
    - sessions: Vec<String> - agent sessions
    - zones: Vec<VersionedRef> - Landscape zones with resolved versions
    - flows: Vec<VersionedRef> - Landscape flows with resolved versions
    - time_window: TimeWindow - investigation window
    - permitted_resources: Vec<String> - resources the actor may use
    - denied: Vec<Denial> - resources refused, with reasons
    - unknown: Vec<String> - scope facts that are not known
    - learned: Vec<LearnedFact> - facts learned from evidence during the investigation
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScopeResolution {
    pub(crate) episode_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) commits: Vec<String>,
    pub(crate) pull_requests: Vec<String>,
    pub(crate) deployments: Vec<String>,
    pub(crate) environment: Option<String>,
    pub(crate) sessions: Vec<String>,
    pub(crate) zones: Vec<VersionedRef>,
    pub(crate) flows: Vec<VersionedRef>,
    pub(crate) time_window: TimeWindow,
    pub(crate) permitted_resources: Vec<String>,
    pub(crate) denied: Vec<Denial>,
    pub(crate) unknown: Vec<String>,
    pub(crate) learned: Vec<LearnedFact>,
}

impl ScopeResolution {
    /**
    Resolve a request's scope and the resources the actor may use. A repository or environment the actor may not see is denied, not dropped; a resource is permitted only when it is active, granted by the policy, named by the request (when the request names any), and within the repository and environment of the investigation
     * Input
        - request: &EvidenceQueryRequest - request
        - registry: &ResourceRegistry - registered resources
        - policy: &AccessPolicy - actor's permissions
     * Output
        - ScopeResolution
    */
    pub(crate) fn resolve(
        request: &EvidenceQueryRequest,
        registry: &ResourceRegistry,
        policy: &AccessPolicy,
    ) -> Self {
        let known = &request.known;
        let mut scope = Self {
            episode_id: request.episode_id.clone(),
            task_id: request.task_id.clone(),
            repository: known.repository.clone(),
            branch: known.branch.clone(),
            commits: known.commits.clone(),
            pull_requests: known.pull_requests.clone(),
            deployments: known.deployments.clone(),
            environment: known.environment.clone(),
            sessions: known.sessions.clone(),
            zones: known
                .zones
                .iter()
                .map(|id| VersionedRef {
                    id: id.clone(),
                    version: None,
                })
                .collect(),
            flows: known
                .flows
                .iter()
                .map(|id| VersionedRef {
                    id: id.clone(),
                    version: None,
                })
                .collect(),
            time_window: request.time_window,
            ..Self::default()
        };
        if let Some(repository) = &scope.repository {
            if !policy.permits_repository(Some(repository)) {
                scope.denied.push(Denial {
                    resource_id: format!("repository:{repository}"),
                    reason: "the actor may not see this repository".into(),
                    explicit: true,
                });
            }
        }
        if !policy.permits_environment(scope.environment.as_deref()) {
            let environment = scope.environment.clone().unwrap_or_default();
            scope.denied.push(Denial {
                resource_id: format!("environment:{environment}"),
                reason: "the actor may not see this environment".into(),
                explicit: true,
            });
        }
        for explicit in &request.resources {
            if registry.get(explicit).is_none() {
                scope.denied.push(Denial {
                    resource_id: explicit.clone(),
                    reason: "not a registered resource".into(),
                    explicit: true,
                });
            }
        }
        for definition in registry.definitions.values() {
            let id = &definition.resource_id;
            let explicit = request.resources.contains(id);
            if !request.resources.is_empty() && !explicit {
                continue;
            }
            let reason = if !policy.permits_resource(id) {
                Some("the actor's policy does not grant this resource")
            } else if definition.status != ResourceStatus::Active {
                Some("the resource is not active")
            } else if scope.repository.as_ref().is_some_and(|repository| {
                !definition.scope.repositories.is_empty()
                    && !definition.scope.repositories.contains(repository)
            }) {
                Some("the resource does not cover this repository")
            } else if scope.environment.as_ref().is_some_and(|environment| {
                !definition.scope.environments.is_empty()
                    && !definition.scope.environments.contains(environment)
            }) {
                Some("the resource does not cover this environment")
            } else {
                None
            };
            match reason {
                Some(reason) => scope.denied.push(Denial {
                    resource_id: id.clone(),
                    reason: reason.into(),
                    explicit,
                }),
                None => scope.permitted_resources.push(id.clone()),
            }
        }
        scope.refresh_unknown();
        scope
    }

    /** Recompute which scope facts are unknown
     * Input
        - None (uses self)
     * Output
        - None
    */
    pub(crate) fn refresh_unknown(&mut self) {
        let mut unknown = Vec::new();
        for (name, missing) in [
            ("task", self.task_id.is_none()),
            ("repository", self.repository.is_none()),
            ("branch", self.branch.is_none()),
            ("commit", self.commits.is_empty()),
            ("pull_request", self.pull_requests.is_empty()),
            ("deployment", self.deployments.is_empty()),
            ("environment", self.environment.is_none()),
            ("session", self.sessions.is_empty()),
        ] {
            if missing {
                unknown.push(name.to_string());
            }
        }
        for reference in self.zones.iter().filter(|zone| zone.version.is_none()) {
            unknown.push(format!("zone:{}.version", reference.id));
        }
        for reference in self.flows.iter().filter(|flow| flow.version.is_none()) {
            unknown.push(format!("flow:{}.version", reference.id));
        }
        self.unknown = unknown;
    }

    /** The value of a recipe template variable
     * Input
        - name: &str - task, repository, commit, pull_request, deployment, environment, session,
          or branch
     * Output
        - Option<String>, None when unknown (the first value of a list)
    */
    pub(crate) fn variable(&self, name: &str) -> Option<String> {
        match name {
            "task" => self.task_id.clone(),
            "repository" => self.repository.clone(),
            "branch" => self.branch.clone(),
            "commit" => self.commits.first().cloned(),
            "pull_request" => self.pull_requests.first().cloned(),
            "deployment" => self.deployments.first().cloned(),
            "environment" => self.environment.clone(),
            "session" => self.sessions.first().cloned(),
            _ => None,
        }
    }

    /** Every known value of a list-valued template variable (commit, pull_request, deployment,
     * session). A template takes the first; the broker warns when others exist, so a value known
     * but never checked is reported instead of silently skipped
     * Input
        - name: &str - variable
     * Output
        - &[String], empty for an unknown or single-valued variable
    */
    pub(crate) fn values(&self, name: &str) -> &[String] {
        match name {
            "commit" => &self.commits,
            "pull_request" => &self.pull_requests,
            "deployment" => &self.deployments,
            "session" => &self.sessions,
            _ => &[],
        }
    }

    /** Learn scope facts a retrieved record states: a pull request's or deployment's commit, a
     * deployment itself, and a session are added when not yet known (every distinct one, so a second
     * deployment is not hidden behind the first, and a deployment found by a discovery query becomes
     * part of the scope); a pull request's task, a task's pull request, and a deployment's
     * environment fill an unknown value (a known value is never overwritten)
     * Input
        - record: &EvidenceRecord - retrieved record
     * Output
        - None
    */
    pub(crate) fn learn(&mut self, record: &EvidenceRecord) {
        let mut facts = Vec::new();
        let refs = &record.refs;
        if matches!(
            record.evidence_type,
            EvidenceType::PullRequest | EvidenceType::Deployment
        ) {
            if let Some(commit) = &refs.commit {
                if !self.commits.contains(commit) {
                    self.commits.push(commit.clone());
                    facts.push(("commit", commit.clone()));
                }
            }
        }
        if record.evidence_type == EvidenceType::Deployment {
            let deployment = refs
                .deployment
                .clone()
                .unwrap_or_else(|| record.source_ref.record_id.clone());
            if !self.deployments.contains(&deployment) {
                self.deployments.push(deployment.clone());
                facts.push(("deployment", deployment));
            }
        }
        if record.evidence_type == EvidenceType::PullRequest && self.task_id.is_none() {
            if let Some(task) = &refs.task {
                self.task_id = Some(task.clone());
                facts.push(("task", task.clone()));
            }
        }
        if record.evidence_type == EvidenceType::Task && self.pull_requests.is_empty() {
            if let Some(pr) = &refs.pull_request {
                self.pull_requests.push(pr.clone());
                facts.push(("pull_request", pr.clone()));
            }
        }
        if record.evidence_type == EvidenceType::Deployment && self.environment.is_none() {
            if let Some(environment) = &record.scope.environment {
                self.environment = Some(environment.clone());
                facts.push(("environment", environment.clone()));
            }
        }
        if matches!(
            record.evidence_type,
            EvidenceType::AgentSession | EvidenceType::ToolCall | EvidenceType::AgentEvent
        ) {
            if let Some(session) = &refs.session {
                if !self.sessions.contains(session) {
                    self.sessions.push(session.clone());
                    facts.push(("session", session.clone()));
                }
            }
        }
        for (field, value) in facts {
            self.learned.push(LearnedFact {
                field: field.into(),
                value,
                evidence_id: record.evidence_id.clone(),
            });
        }
        self.refresh_unknown();
    }
}
