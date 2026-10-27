// The resident part of the repository status a machine reports to the DataSea dashboard
// (foxx::status, key "residents"): definitions, assignments with their lifecycle, derived runtime
// status, and health, recent activations with their timelines, the review inbox (approvals,
// questions, reconciliation, RCA proposals), idle cost and health counters, recent test reports,
// and the step and trigger catalogs. Only identifiers, states, digests, counts, reason codes, and
// bounded plan parameters leave the machine; context text and answers stay in the trust directory.

use serde_json::{json, Value};

use super::authority::{self, Authority, Decision};
use super::model::{Assignment, Lifecycle, TriggerSource};
use super::plan;
use super::store::{Activation, ActivationState, ResidentStore, StepState};
use crate::governance::workspace::Workspace;

/** Most activations reported */
const MAX_ACTIVATIONS: usize = 60;

/** The derived runtime status of an assignment: its lifecycle when it is not active, otherwise
 * what its latest unfinished activation is doing, DEGRADED when Crane would not admit it now (a
 * revoked signed assignment, an unverified state, an inactive target), else ACTIVE_IDLE
 * Input
    - store: &ResidentStore - store
    - assignment: &Assignment - assignment
    - admission: Option<&authority::Authorization> - Crane's answer now (None: unknown)
 * Output
    - String
*/
pub(crate) fn status(
    store: &ResidentStore,
    assignment: &Assignment,
    admission: Option<&authority::Authorization>,
) -> String {
    match assignment.lifecycle {
        Lifecycle::Draft => return "DRAFT".into(),
        Lifecycle::Validated => return "VALIDATED".into(),
        Lifecycle::Paused => return "PAUSED".into(),
        Lifecycle::Expired => return "EXPIRED".into(),
        Lifecycle::Revoked => return "REVOKED".into(),
        Lifecycle::Active => {}
    }
    let open = store
        .state
        .activations
        .values()
        .filter(|activation| {
            activation.assignment_id == assignment.assignment_id && !activation.state.ended()
        })
        .map(|activation| activation.state)
        .collect::<Vec<_>>();
    if open.contains(&ActivationState::AwaitingApproval) {
        return "AWAITING_APPROVAL".into();
    }
    if open.contains(&ActivationState::ReconciliationRequired) {
        return "RECONCILIATION_REQUIRED".into();
    }
    if open.contains(&ActivationState::AwaitingInput) {
        return "AWAITING_INPUT".into();
    }
    if open.iter().any(|state| {
        matches!(
            state,
            ActivationState::Executing | ActivationState::Verifying
        )
    }) {
        return "EXECUTING".into();
    }
    if open.iter().any(|state| {
        matches!(
            state,
            ActivationState::ContextLoading
                | ActivationState::Investigating
                | ActivationState::PlanReady
        )
    }) {
        return "INVESTIGATING".into();
    }
    if open.contains(&ActivationState::WaitingForEvent) {
        return "WAITING_FOR_EVENT".into();
    }
    if admission.is_none_or(|admission| admission.decision != Decision::Allow) {
        return "DEGRADED".into();
    }
    if open.contains(&ActivationState::Queued) {
        return "QUEUED".into();
    }
    "ACTIVE_IDLE".into()
}

/** Summarize an activation (no context text, no answers)
 * Input
    - activation: &Activation - activation
 * Output
    - Value
*/
pub(crate) fn activation_summary(activation: &Activation) -> Value {
    let plan = activation.plan.as_ref();
    json!({
        "activation_id": activation.activation_id,
        "assignment": activation.assignment_id,
        "definition": format!("{}@{}", activation.definition_id, activation.definition_version),
        "mode": activation.mode,
        "state": activation.state,
        "outcome": activation.outcome,
        "trigger": {"id": activation.trigger.trigger_id, "source": activation.trigger.source, "kind": activation.trigger.kind, "occurred_at": activation.trigger.occurred_at, "received_at": activation.trigger.received_at, "origin": activation.trigger.origin, "targets": activation.trigger.targets},
        "later_triggers": activation.triggers,
        "context": activation.context.as_ref().map(|context| json!({"pack": context.pack, "cache": context.cache, "dependencies": context.dependencies, "sections": context.sections.iter().map(|(kind, source, reference, bytes)| json!({"kind": kind, "source": source, "reference": reference, "bytes": bytes})).collect::<Vec<_>>(), "missing": context.missing, "previous": context.previous})),
        "investigation": activation.investigation.as_ref().map(|investigation| json!({
            "rca": investigation.rca,
            "rca_state": investigation.rca_state,
            "conclusion": investigation.conclusion,
            "cause": investigation.cause,
            "signals": investigation.signals,
            "candidates": investigation.candidates,
            "gaps": investigation.gaps,
            "consulted": investigation.consulted,
            "reasoner": investigation.reasoner,
        })),
        "plan": plan.map(|plan| json!({
            "digest": plan.digest,
            "steps": plan.steps.iter().map(|step| {
                let record = activation.steps.get(&step.step_id);
                json!({
                    "step": step.step_id,
                    "type": step.step_type.name(),
                    "effect": step.step_type.info().effect,
                    "depends_on": step.depends_on,
                    "basis": step.basis,
                    "check": step.check.as_ref().map(super::rca::CheckSpec::text),
                    "proposed_steps": step.proposal.len(),
                    "state": record.map(|record| record.state),
                    "attempts": record.map(|record| record.attempts),
                    "detail": record.map(|record| record.detail.clone()),
                    "decision": record.and_then(|record| record.authorization.as_ref()).map(|authorization| json!({"decision": authorization.decision, "reasons": authorization.codes(), "generation": authorization.generation})),
                    "evidence": record.map(|record| record.evidence.clone()),
                    "external_ref": record.and_then(|record| record.external_ref.clone()),
                    "deadline": record.and_then(|record| record.deadline),
                })
            }).collect::<Vec<_>>(),
        })),
        "approvals": activation.approvals.iter().map(|approval| json!({"step": approval.step, "approved": approval.approved, "approver": approval.approver.name, "digest": approval.digest, "at": approval.at, "expires_at": approval.expires_at, "used": approval.used, "reason": approval.reason})).collect::<Vec<_>>(),
        "usage": activation.usage,
        "timeline": activation.timeline,
        "created_at": activation.created_at,
        "updated_at": activation.updated_at,
    })
}

/** The review inbox: steps waiting for approval, questions, uncertain effects, RCA proposals,
 * and unhealthy or expired assignments
 * Input
    - store: &ResidentStore - store
    - statuses: &[(String, String)] - assignment ids with their derived status
 * Output
    - Value
*/
fn inbox(store: &ResidentStore, statuses: &[(String, String)]) -> Value {
    let mut approvals = Vec::new();
    let mut questions = Vec::new();
    let mut reconciliation = Vec::new();
    for activation in store
        .state
        .activations
        .values()
        .filter(|activation| activation.mode == super::store::Mode::Live)
    {
        let Some(plan) = &activation.plan else {
            continue;
        };
        for step in &plan.steps {
            let Some(record) = activation.steps.get(&step.step_id) else {
                continue;
            };
            let row = json!({
                "activation": activation.activation_id,
                "assignment": activation.assignment_id,
                "step": step.step_id,
                "type": step.step_type.name(),
                "summary": step.params.get("summary").or_else(|| step.params.get("question")).or_else(|| step.params.get("text")).cloned(),
                "proposed_steps": step.proposal.iter().map(|item| json!({"step": item.step_id, "type": item.step_type.name(), "basis": item.basis})).collect::<Vec<_>>(),
                "reasons": record.authorization.as_ref().map(super::authority::Authorization::codes),
                "detail": record.detail,
                "deadline": record.deadline,
                "since": record.updated_at,
                "plan": plan.digest,
            });
            match record.state {
                StepState::AwaitingApproval => approvals.push(row),
                StepState::AwaitingInput => questions.push(row),
                StepState::Uncertain => reconciliation.push(row),
                _ => {}
            }
        }
    }
    let proposals = store
        .state
        .rcas
        .values()
        .flat_map(|versions| versions.values())
        .filter(|record| record.state == super::rca::RcaState::Proposed)
        .map(|record| json!({"rca": format!("{}@{}", record.graph.rca_id, record.graph.version), "name": record.graph.name, "target": record.graph.target, "origin": record.origin, "nodes": record.graph.nodes.len(), "edges": record.graph.edges.len(), "digest": record.digest}))
        .collect::<Vec<_>>();
    let attention = statuses
        .iter()
        .filter(|(_, status)| matches!(status.as_str(), "DEGRADED" | "EXPIRED"))
        .map(|(id, status)| json!({"assignment": id, "status": status}))
        .collect::<Vec<_>>();
    json!({"approvals": approvals, "questions": questions, "reconciliation": reconciliation, "rca_proposals": proposals, "assignments": attention})
}

/** Collect the resident report of a repository from a store and the Crane view
 * Input
    - store: &ResidentStore - store
    - authority: Option<&Authority> - Crane view (None: could not be read)
    - now: u64 - Unix milliseconds
 * Output
    - Value
*/
pub(crate) fn build(store: &ResidentStore, authority: Option<&Authority>, now: u64) -> Value {
    let definitions = store
        .state
        .definitions
        .values()
        .flat_map(|versions| versions.values())
        .map(|record| json!({
            "ref": format!("{}@{}", record.definition.definition_id, record.definition.version),
            "definition_id": record.definition.definition_id,
            "version": record.definition.version,
            "name": record.definition.name,
            "owner": record.definition.owner,
            "state": record.state,
            "digest": record.digest,
            "triggers": record.definition.triggers,
            "actions": record.definition.actions,
            "bricks": record.definition.bricks,
            "limits": record.definition.limits,
            "verification_required": record.definition.verification_required,
            "on_missing_context": record.definition.on_missing_context,
            "purpose_bytes": record.definition.purpose.len(),
            "self_reviewed": record.self_reviewed,
            "created_by": record.definition.created_by.name,
            "changes": record.changes.iter().map(|change| json!({"to": change.to, "by": change.by.name, "at": change.at})).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>();
    let mut statuses = Vec::new();
    let assignments = store
        .state
        .assignments
        .values()
        .map(|assignment| {
            let definition = store.definition(&assignment.definition_id, Some(assignment.definition_version)).ok();
            let admission = authority.map(|authority| authority::admit(authority, assignment, definition, now));
            let status = status(store, assignment, admission.as_ref());
            statuses.push((assignment.assignment_id.clone(), status.clone()));
            let mine = store.state.activations.values().filter(|activation| activation.assignment_id == assignment.assignment_id && activation.mode == super::store::Mode::Live).collect::<Vec<_>>();
            let latest = mine.iter().max_by_key(|activation| activation.created_at);
            let verified = mine.iter().filter(|activation| activation.state == ActivationState::Completed).map(|activation| activation.updated_at).max();
            let failure = mine.iter().filter(|activation| matches!(activation.state, ActivationState::Failed | ActivationState::Blocked)).max_by_key(|activation| activation.updated_at);
            let model_calls = mine.iter().map(|activation| activation.usage.model_calls).sum::<u64>();
            let connectors = assignment
                .subscriptions
                .iter()
                .map(|subscription| json!({"source": subscription.source, "supported": subscription.source.delivery().is_some(), "delivery": subscription.source.delivery()}))
                .collect::<Vec<_>>();
            let last_event = store
                .state
                .ignored
                .iter()
                .filter(|item| item.assignment.as_ref() == Some(&assignment.assignment_id))
                .map(|item| item.at)
                .chain(mine.iter().map(|activation| activation.trigger.received_at))
                .max();
            json!({
                "assignment_id": assignment.assignment_id,
                "definition": format!("{}@{}", assignment.definition_id, assignment.definition_version),
                "agent": assignment.agent,
                "target": assignment.target.key(),
                "rca": assignment.rca.as_ref().map(|reference| format!("{}@{}", reference.rca_id, reference.version)),
                "lifecycle": assignment.lifecycle,
                "status": status,
                "health": admission.as_ref().map(|admission| json!({"decision": admission.decision, "reasons": admission.codes(), "generation": admission.generation})),
                "subscriptions": assignment.subscriptions,
                "timers": assignment.timers,
                "connectors": connectors,
                "lifetime_days": assignment.lifetime_days,
                "starts_at": assignment.starts_at,
                "expires_at": assignment.expires_at,
                "validation": assignment.validation,
                "last_event_at": last_event,
                "latest_activation": latest.map(|activation| json!({"id": activation.activation_id, "state": activation.state, "at": activation.created_at})),
                "last_verified_at": verified,
                "last_failure": failure.map(|activation| json!({"id": activation.activation_id, "state": activation.state, "reason": activation.outcome.as_ref().map(|outcome| outcome.reason.clone())})),
                "pending_approvals": mine.iter().filter(|activation| activation.state == ActivationState::AwaitingApproval).count(),
                "activations": mine.len(),
                "model_calls": model_calls,
                "created_by": assignment.created_by.name,
                "changes": assignment.changes.iter().map(|change| json!({"to": change.to, "by": change.by.name, "reason": change.reason, "at": change.at})).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let mut recent = store.state.activations.values().collect::<Vec<_>>();
    recent.sort_by_key(|activation| std::cmp::Reverse(activation.created_at));
    let activations = recent
        .iter()
        .take(MAX_ACTIVATIONS)
        .map(|activation| activation_summary(activation))
        .collect::<Vec<_>>();
    let mut ignored = std::collections::BTreeMap::<String, u64>::new();
    for item in &store.state.ignored {
        *ignored.entry(item.reason.clone()).or_default() += 1;
    }
    let queue = store
        .state
        .activations
        .values()
        .filter(|activation| activation.state == ActivationState::Queued)
        .count();
    let dead = store
        .state
        .activations
        .values()
        .filter(|activation| {
            matches!(
                activation.state,
                ActivationState::Failed | ActivationState::Blocked
            )
        })
        .count();
    let ended = store
        .state
        .activations
        .values()
        .filter(|activation| {
            activation.state.ended() && activation.mode == super::store::Mode::Live
        })
        .collect::<Vec<_>>();
    let outcomes = [
        "COMPLETED",
        "PARTIALLY_VERIFIED",
        "FAILED",
        "BLOCKED",
        "CANCELLED",
        "UNKNOWN",
    ]
    .iter()
    .map(|name| {
        (
            name.to_string(),
            ended
                .iter()
                .filter(|activation| crate::resident::word(&activation.state) == *name)
                .count(),
        )
    })
    .collect::<std::collections::BTreeMap<_, _>>();
    let counters = &store.state.counters;
    json!({
        "schema_version": super::SCHEMA_VERSION,
        "definitions": definitions,
        "assignments": assignments,
        "activations": activations,
        "inbox": inbox(store, &statuses),
        "rcas": store.state.rcas.values().flat_map(|versions| versions.values()).map(|record| json!({"rca": format!("{}@{}", record.graph.rca_id, record.graph.version), "name": record.graph.name, "target": record.graph.target, "state": record.state, "origin": record.origin, "digest": record.digest, "nodes": record.graph.nodes.iter().map(|node| json!({"id": node.id, "kind": node.kind, "label": node.label, "status": node.status, "basis": node.basis})).collect::<Vec<_>>(), "edges": record.graph.edges, "self_reviewed": record.self_reviewed})).collect::<Vec<_>>(),
        "health": {
            "counters": counters,
            "idle_model_calls": counters.idle_model_calls,
            "queue_depth": queue,
            "dead_letter": dead,
            "ignored": ignored,
            "outcomes": outcomes,
            "cache": {"hits": counters.cache_hits, "misses": counters.cache_misses, "invalidations": counters.cache_invalidations},
            "token_usage": "unmeasured: no model provider is configured, so no activation reports tokens",
            "worker": "background sync (started by hooks and commands) or 'crane resident tick'; no daemon",
        },
        "tests": store.state.tests.iter().rev().take(20).map(|report| json!({"test_id": report["test_id"], "assignment": report["assignment"], "result": report["result"], "tested_at": report["tested_at"], "mode": report["mode"], "matching": report["matching"], "plan": report["plan"], "context": {"pack": report["context"]["pack"], "cache": report["context"]["cache"], "missing": report["context"]["missing"]}, "investigation": report["investigation"].get("conclusion"), "notes": report["notes"]})).collect::<Vec<_>>(),
        "catalog": {
            "steps": plan::catalog(None),
            "triggers": TriggerSource::ALL.iter().map(|source| json!({"source": source, "supported": source.delivery().is_some(), "delivery": source.delivery()})).collect::<Vec<_>>(),
        },
        "crane": authority.map(crane_view),
    })
}

/** What the creation wizard may offer from the Crane view: governed targets with their limits,
 * signed Brick assignments, and which action connectors are configured
 * Input
    - authority: &Authority - Crane view
 * Output
    - Value
*/
fn crane_view(authority: &Authority) -> Value {
    json!({
        "trust": authority.trust,
        "generation": authority.generation,
        "targets": authority.targets.iter().map(|(key, facts)| json!({"target": key, "status": facts.status, "ceiling": facts.ceiling, "criticality": facts.criticality, "approval": facts.approval, "read_only": facts.read_only, "fail_closed": facts.fail_closed.len()})).collect::<Vec<_>>(),
        "signed_assignments": authority.assignments,
        "connectors": authority.connectors,
    })
}

/** Collect the resident report of a repository (an {error} object when the store cannot be read;
 * null when there is no resident store yet)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Value
*/
pub(crate) fn collect(workspace: &Workspace) -> Value {
    let runtime = match workspace.trust() {
        Ok(trust) => trust.runtime(),
        Err(error) => return json!({"error": error}),
    };
    if !runtime.join("resident").join("store.json").exists() {
        return json!({"empty": true, "catalog": {"steps": plan::catalog(None), "triggers": TriggerSource::ALL.iter().map(|source| json!({"source": source, "supported": source.delivery().is_some(), "delivery": source.delivery()})).collect::<Vec<_>>()}});
    }
    match ResidentStore::open(&runtime) {
        Ok(store) => build(
            &store,
            authority::load(workspace).ok().as_ref(),
            crate::platform::now_millis(),
        ),
        Err(error) => json!({"error": error}),
    }
}
