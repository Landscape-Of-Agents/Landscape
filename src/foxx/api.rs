// The read-only Foxx API. Every route reads the governance state and the runtime stores; there is
// no route that changes anything. Lists are bounded (limit at most 500) and paginated by event
// sequence; secrets are never returned (integrations expose non-secret settings only).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::views::{agents, metric_map, session_view, task_view, Snapshot};
use crate::github;
use crate::governance::state::{integrity, State};
use crate::governance::verify::{evaluate, Options, Outcome};
use crate::governance::workspace::Workspace;
use crate::hooks::adapter::AgentKind;
use crate::hooks::install;
use crate::integrations;
use crate::platform::now_millis;
use crate::platform::render::iso_time;
use crate::selection::registry::Operation;
use crate::telemetry::alerts;
use crate::telemetry::compliance::{self, Inputs};
use crate::telemetry::events::{verify_chain, Event};
use crate::telemetry::ledger::{self, Ledger};
use crate::telemetry::metrics::{self, DEFINITIONS};
use crate::telemetry::model::{Decision, Enforcement};
use crate::trust::crypto::{canonical_json, digest};

/** Largest page of events */
const MAX_LIMIT: usize = 500;

/** Default page of events */
const DEFAULT_LIMIT: usize = 100;

/** A route's answer: status, body, and a download file name for attachments */
type Answer = (u16, Value, Option<String>);

/** Build an error answer
 * Input
    - status: u16 - HTTP status
    - message: String - explanation
 * Output
    - Answer
*/
fn error(status: u16, message: String) -> Answer {
    (status, json!({"error": message}), None)
}

/** Route a request
 * Input
    - workspace: &Workspace - repository
    - path: &str - request path
    - query: &BTreeMap<String, String> - query parameters
 * Output
    - Answer
*/
pub(crate) fn route(workspace: &Workspace, path: &str, query: &BTreeMap<String, String>) -> Answer {
    let snapshot = match Snapshot::load_for_dashboard(workspace) {
        Ok(snapshot) => snapshot,
        Err(message) => return error(500, message),
    };
    let segments = path
        .trim_start_matches("/api/")
        .trim_end_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    match segments.as_slice() {
        ["health"] => (200, health(workspace, &snapshot), None),
        ["overview"] => (200, overview(workspace, &snapshot, query), None),
        ["agents"] => (200, json!({"agents": agents(&snapshot)}), None),
        ["sessions"] => (200, sessions(&snapshot, query), None),
        ["sessions", id] => match snapshot.session(id) {
            Some(session) => {
                let mut view = session_view(&snapshot, session);
                view["tasks"] = json!(snapshot.tasks.iter().filter(|task| task.foxx_session_id == session.foxx_session_id).map(|task| task_view(&snapshot, task)).collect::<Vec<_>>());
                (200, view, None)
            }
            None => error(404, format!("no session '{id}'")),
        },
        ["tasks"] => (200, json!({"tasks": snapshot.tasks.iter().filter(|task| query.get("session").is_none_or(|session| &task.foxx_session_id == session)).take(MAX_LIMIT).map(|task| task_view(&snapshot, task)).collect::<Vec<_>>()}), None),
        ["tasks", id] => match snapshot.task(id) {
            Some(task) => (200, task_view(&snapshot, task), None),
            None => error(404, format!("no task '{id}'")),
        },
        ["events"] => (200, events(&snapshot, query), None),
        ["graph", id] => match snapshot.session(id) {
            Some(session) => (200, graph(&snapshot.session_events(&session.foxx_session_id)), None),
            None => error(404, format!("no session '{id}'")),
        },
        ["governance"] => governance(workspace),
        ["ledger"] => (200, ledger_view(&snapshot, query), None),
        ["metrics"] => metrics_view(&snapshot, query),
        ["definitions"] => (200, json!({"metrics": DEFINITIONS, "controls": compliance::CONTROLS, "domains": crate::telemetry::model::Domain::ALL}), None),
        ["repo", "insights"] => match query.get("privilege") {
            Some(privilege) => match github::fetch(workspace, privilege) {
                Ok(data) => (200, json!({"privilege": privilege, "data": data}), None),
                Err(message) => error(403, message),
            },
            None => error(400, "give ?privilege=rf|codeowners|contributors|ci-cd|gh-actions|gh-branches|gh-insights".into()),
        },
        ["compliance"] => (200, compliance_view(workspace, &snapshot), None),
        ["alerts"] => (200, json!({"alerts": snapshot.alerts.clone().into_iter().take(MAX_LIMIT).collect::<Vec<_>>()}), None),
        ["integrations"] => (200, json!({"integrations": integrations::configured(workspace)}), None),
        ["repo"] => (200, repo_view(workspace), None),
        ["reports", "session", id] => match snapshot.session(id) {
            Some(session) => (200, session_report(workspace, &snapshot, &session.foxx_session_id), None),
            None => error(404, format!("no session '{id}'")),
        },
        ["evidence", "session", id] => match snapshot.session(id) {
            Some(session) => {
                let bundle = evidence_bundle(workspace, &snapshot, &session.foxx_session_id);
                let text = serde_json::to_string_pretty(&bundle).unwrap_or_default();
                (200, Value::String(text), Some(format!("crane-evidence-{}.json", session.foxx_session_id)))
            }
            None => error(404, format!("no session '{id}'")),
        },
        _ => error(404, format!("no API route {path}")),
    }
}

/** Parse a time filter given as Unix milliseconds
 * Input
    - query: &BTreeMap<String, String> - query
    - key: &str - since or until
 * Output
    - Option<u64>
*/
fn time_filter(query: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    query.get(key).and_then(|value| value.parse::<u64>().ok())
}

/** Health of the runtime: versions, chain status, store sizes, and storage backends
 * Input
    - workspace: &Workspace - repository
    - snapshot: &Snapshot - stores
 * Output
    - Value
*/
fn health(workspace: &Workspace, snapshot: &Snapshot) -> Value {
    let state = State::load(workspace).ok();
    json!({
        "crane_version": env!("CARGO_PKG_VERSION"),
        "schema_version": crate::telemetry::events::SCHEMA_VERSION,
        "metric_formula_version": metrics::FORMULA_VERSION,
        "repository_id": workspace.repository_id,
        "registry_generation": state.as_ref().map(State::generation),
        "events": snapshot.events.len(),
        "event_chain": verify_chain(&snapshot.events),
        "ledger_entries": snapshot.ledger.len(),
        "ledger_reconciles": ledger::verify(&snapshot.ledger).is_ok(),
        "sessions": snapshot.sessions.len(),
        "storage": {
            "local": "trust-directory runtime folder (append-only JSON lines)",
            "source": snapshot.source,
            "source_error": snapshot.source_error,
            "service": if crate::telemetry::sync::configured() {
                crate::telemetry::ingest::endpoint().unwrap_or_default()
            } else {
                "off".into()
            },
            "sync": crate::telemetry::ingest::state(&snapshot.stores),
        },
        "mode": "local read-only",
        "now": iso_time(now_millis()),
    })
}

/** The overview page: activity, outcomes, autonomy, integrity, coverage, and recent alerts
 * Input
    - workspace: &Workspace - repository
    - snapshot: &Snapshot - stores
    - query: &BTreeMap<String, String> - since/until filters
 * Output
    - Value
*/
fn overview(workspace: &Workspace, snapshot: &Snapshot, query: &BTreeMap<String, String>) -> Value {
    let since = time_filter(query, "since").unwrap_or(0);
    let until = time_filter(query, "until").unwrap_or(u64::MAX);
    let events = snapshot
        .events
        .iter()
        .filter(|event| event.occurred_at >= since && event.occurred_at <= until)
        .collect::<Vec<_>>();
    let sessions = snapshot
        .sessions
        .iter()
        .map(|session| serde_json::to_value(session).unwrap_or_default())
        .collect::<Vec<_>>();
    let ledger = snapshot.ledger.iter().collect::<Vec<_>>();
    let computed = metrics::compute(&events, &ledger, &sessions);
    let integrity_findings = match (State::load(workspace), workspace.trust()) {
        (Ok(state), Ok(trust)) => integrity(workspace, &state, &trust),
        (Err(error), _) | (_, Err(error)) => vec![crate::governance::Finding::new(
            "governance_unreadable",
            crate::governance::Severity::Critical,
            error,
        )],
    };
    json!({
        "active_sessions": snapshot.sessions.iter().filter(|session| session.ended_at.is_none()).count(),
        "sessions": snapshot.sessions.len(),
        "tasks": snapshot.tasks.len(),
        "tasks_by_status": snapshot.tasks.iter().fold(BTreeMap::<String, usize>::new(), |mut counts, task| { *counts.entry(task.status.clone()).or_default() += 1; counts }),
        "agents": snapshot.sessions.iter().map(|session| session.agent_id.clone()).collect::<BTreeSet<_>>().len(),
        "metrics": metric_map(&computed),
        "integrity": {
            "ok": !integrity_findings.iter().any(super::super::governance::Finding::blocking),
            "findings": integrity_findings,
        },
        "event_chain": verify_chain(&snapshot.events),
        "recent_alerts": snapshot.alerts.clone().into_iter().take(10).collect::<Vec<_>>(),
        "recent_violations": snapshot.events.iter().rev().filter(|event| matches!(event.enforcement, Enforcement::BypassConfirmed | Enforcement::DetectedAfterEffect)).take(10).map(compact).collect::<Vec<_>>(),
    })
}

/** A compact event for lists and timelines
 * Input
    - event: &Event - event
 * Output
    - Value
*/
fn compact(event: &Event) -> Value {
    json!({
        "event_id": event.event_id,
        "sequence": event.sequence,
        "type": event.event_type,
        "at": iso_time(event.occurred_at),
        "occurred_at": event.occurred_at,
        "session": event.scope.foxx_session_id,
        "task": event.scope.foxx_task_id,
        "tool_call": event.scope.tool_call_id,
        "provider": event.scope.provider,
        "model": event.scope.model,
        "domain": event.domain,
        "severity": event.severity,
        "decision": event.decision,
        "execution": event.execution,
        "assessment": event.assessment,
        "enforcement": event.enforcement,
        "telemetry": event.telemetry,
        "policies": event.bindings.policies,
        "selections": event.bindings.selections,
        "registry_generation": event.bindings.registry_generation,
        "resources": event.resources,
        "reasons": event.reasons,
        "measurements": event.measurements,
        "evidence": event.evidence,
        "digest": event.digest,
    })
}

/** List sessions with filters (agent, provider, safety)
 * Input
    - snapshot: &Snapshot - stores
    - query: &BTreeMap<String, String> - filters
 * Output
    - Value
*/
fn sessions(snapshot: &Snapshot, query: &BTreeMap<String, String>) -> Value {
    let list = snapshot
        .sessions
        .iter()
        .filter(|session| {
            query
                .get("agent")
                .is_none_or(|agent| &session.agent_id == agent)
        })
        .filter(|session| {
            query
                .get("provider")
                .is_none_or(|provider| &session.provider == provider)
        })
        .filter(|session| {
            query
                .get("safety")
                .is_none_or(|safety| format!("{:?}", session.safety).eq_ignore_ascii_case(safety))
        })
        .take(MAX_LIMIT)
        .map(|session| session_view(snapshot, session))
        .collect::<Vec<_>>();
    json!({"sessions": list})
}

/** List events with filters and cursor pagination by sequence
 * Input
    - snapshot: &Snapshot - stores
    - query: &BTreeMap<String, String> - session, task, agent, provider, model, type, decision,
      severity, domain, enforcement, tool_call, selection, policy, since, until, cursor, limit
 * Output
    - Value {events, next_cursor}
*/
fn events(snapshot: &Snapshot, query: &BTreeMap<String, String>) -> Value {
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let cursor = query
        .get("cursor")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let since = time_filter(query, "since").unwrap_or(0);
    let until = time_filter(query, "until").unwrap_or(u64::MAX);
    let agent_sessions = query.get("agent").map(|agent| {
        snapshot
            .sessions
            .iter()
            .filter(|session| &session.agent_id == agent)
            .map(|session| session.foxx_session_id.clone())
            .collect::<BTreeSet<_>>()
    });
    let matches = |event: &&Event| {
        let text = |value: Value| {
            value
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| value.to_string())
        };
        event.sequence > cursor
            && event.occurred_at >= since
            && event.occurred_at <= until
            && query
                .get("session")
                .is_none_or(|value| event.scope.foxx_session_id.as_deref() == Some(value.as_str()))
            && query
                .get("task")
                .is_none_or(|value| event.scope.foxx_task_id.as_deref() == Some(value.as_str()))
            && query
                .get("provider")
                .is_none_or(|value| event.scope.provider.as_deref() == Some(value.as_str()))
            && query
                .get("model")
                .is_none_or(|value| event.scope.model.as_deref() == Some(value.as_str()))
            && query
                .get("tool_call")
                .is_none_or(|value| event.scope.tool_call_id.as_deref() == Some(value.as_str()))
            && query
                .get("type")
                .is_none_or(|value| event.event_type.starts_with(value.as_str()))
            && query.get("decision").is_none_or(|value| {
                event
                    .decision
                    .is_some_and(|decision| text(json!(decision)).eq_ignore_ascii_case(value))
            })
            && query
                .get("severity")
                .is_none_or(|value| text(json!(event.severity)).eq_ignore_ascii_case(value))
            && query
                .get("domain")
                .is_none_or(|value| text(json!(event.domain)) == *value)
            && query
                .get("enforcement")
                .is_none_or(|value| text(json!(event.enforcement)).eq_ignore_ascii_case(value))
            && query
                .get("selection")
                .is_none_or(|value| event.bindings.selections.contains(value))
            && query
                .get("policy")
                .is_none_or(|value| event.bindings.policies.contains(value))
            && agent_sessions.as_ref().is_none_or(|ids| {
                event
                    .scope
                    .foxx_session_id
                    .as_ref()
                    .is_some_and(|id| ids.contains(id))
            })
    };
    let page = snapshot
        .events
        .iter()
        .filter(matches)
        .take(limit + 1)
        .collect::<Vec<_>>();
    let more = page.len() > limit;
    let page = &page[..page.len().min(limit)];
    json!({
        "events": page.iter().map(|event| compact(event)).collect::<Vec<_>>(),
        "next_cursor": if more { page.last().map(|event| event.sequence) } else { None },
    })
}

/** Build the action graph of a session: tool calls in order, each with its decision and its
 * post-action verification; "verified_by" edges join a decision to the verification of the same
 * tool call (established by the tool call identifier), and "followed_by" edges only record order,
 * never a claimed cause
 * Input
    - events: &[&Event] - session events
 * Output
    - Value {nodes, edges}
*/
fn graph(events: &[&Event]) -> Value {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut previous: Option<String> = None;
    let mut decided: BTreeMap<String, String> = BTreeMap::new();
    for event in events {
        let kind = match event.event_type.as_str() {
            "tool_call.decided" => "decision",
            "tool_call.verified" => "verification",
            "task.started" | "task.verified" | "task.verification_failed" => "task",
            "session.started" | "session.attested" | "session.quarantined" | "session.created" => {
                "session"
            }
            "hook.missing_decision" | "tool_call.unsettled" | "hook.failed" => "gap",
            "credits.low" | "prompt.suspicious" => "signal",
            _ => "other",
        };
        let flags = [
            (
                event
                    .decision
                    .is_some_and(|decision| decision != Decision::Allow),
                "denied",
            ),
            (
                event.measurement("bypass_attempt") == Some(1.0),
                "bypass_attempt",
            ),
            (
                event.enforcement == Enforcement::BypassConfirmed,
                "bypass_confirmed",
            ),
            (
                event.enforcement == Enforcement::DetectedAfterEffect,
                "violation",
            ),
            (
                event.enforcement == Enforcement::CoverageGap,
                "coverage_gap",
            ),
            (
                event.assessment == Some(crate::telemetry::model::Assessment::Unresolved),
                "unresolved",
            ),
        ]
        .into_iter()
        .filter_map(|(present, flag)| present.then_some(flag))
        .collect::<Vec<_>>();
        nodes.push(json!({
            "id": event.event_id,
            "kind": kind,
            "label": event.payload.get("tool").and_then(Value::as_str).map(String::from).unwrap_or_else(|| event.event_type.clone()),
            "event": compact(event),
            "flags": flags,
        }));
        if let Some(previous) = &previous {
            edges.push(json!({"from": previous, "to": event.event_id, "kind": "followed_by", "evidence": "event order"}));
        }
        if let Some(call) = &event.scope.tool_call_id {
            match event.event_type.as_str() {
                "tool_call.decided" => {
                    decided.insert(call.clone(), event.event_id.clone());
                }
                "tool_call.verified" | "hook.missing_decision" => {
                    if let Some(decision) = decided.get(call) {
                        edges.push(json!({"from": decision, "to": event.event_id, "kind": "verified_by", "evidence": format!("tool call {call}")}));
                    }
                }
                _ => {}
            }
        }
        previous = Some(event.event_id.clone());
    }
    json!({"nodes": nodes, "edges": edges})
}

/** The repository governance view: policies, selections with their resolution, checkpoints,
 * defaults, context bindings, findings, and relocations
 * Input
    - workspace: &Workspace - repository
 * Output
    - Answer
*/
fn governance(workspace: &Workspace) -> Answer {
    let (report, state) = match evaluate(workspace, &Options::default()) {
        Ok(result) => result,
        Err(message) => return error(500, message),
    };
    let selections = state
        .registry
        .selections
        .iter()
        .map(|record| {
            json!({
                "id": record.id,
                "name": record.name,
                "operation": record.operation,
                "change_type": record.change_type,
                "policy": record.policy,
                "language": record.language,
                "status": record.status,
                "generation": record.generation,
                "origin": {"checkpoint": record.origin.checkpoint, "commit": record.origin.commit, "path": record.origin.span.path, "lines": [record.origin.span.start_line, record.origin.span.end_line], "binding_digest": record.origin.binding_digest},
                "resolved": report.resolutions.get(&record.id).cloned().flatten(),
                "created_at": iso_time(record.created_at * 1000),
                "created_by": record.created_by,
            })
        })
        .collect::<Vec<_>>();
    let contexts = state
        .context
        .bindings
        .iter()
        .map(|binding| {
            let current = std::fs::read(workspace.root.join(&binding.path)).ok().map(|bytes| crate::trust::crypto::sha512_hex(&bytes));
            json!({"policy": binding.policy, "path": binding.path, "state": match current { None => "missing", Some(digest) if digest == binding.digest => "current", Some(_) => "stale" }})
        })
        .collect::<Vec<_>>();
    (
        200,
        json!({
            "generation": report.generation,
            "public_key": state.registry.manifest.as_ref().map(|manifest| manifest.public_key.clone()),
            "defaults": {"policy": state.config.default_policy, "checkpoint": state.config.default_checkpoint},
            "autonomy": state.config.autonomy,
            "checkpoints": state.checkpoints.checkpoints.iter().map(|checkpoint| json!({"name": checkpoint.name, "commit": checkpoint.commit, "branch": checkpoint.branch, "created_at": iso_time(checkpoint.created_at * 1000), "created_by": checkpoint.created_by})).collect::<Vec<_>>(),
            "policies": report.policies,
            "selections": selections,
            "contexts": contexts,
            "findings": report.findings,
            "relocations": report.relocations,
            "passed": report.passed,
            "governance_config": crate::governance::zones::load(workspace).ok(),
            "zones_and_flows": report.zones,
            "audit": state.audit.iter().rev().take(50).map(|entry| json!({"seq": entry.seq, "at": iso_time(entry.at * 1000), "actor": entry.actor, "action": entry.action, "detail": entry.detail})).collect::<Vec<_>>(),
        }),
        None,
    )
}

/** The ledger view of one session or all sessions
 * Input
    - snapshot: &Snapshot - stores
    - query: &BTreeMap<String, String> - optional session
 * Output
    - Value
*/
fn ledger_view(snapshot: &Snapshot, query: &BTreeMap<String, String>) -> Value {
    let entries = snapshot
        .ledger
        .iter()
        .filter(|entry| {
            query
                .get("session")
                .is_none_or(|session| &entry.session == session)
        })
        .collect::<Vec<_>>();
    let sessions = entries
        .iter()
        .map(|entry| entry.session.clone())
        .collect::<BTreeSet<_>>();
    json!({
        "reconciles": ledger::verify(&snapshot.ledger).is_ok(),
        "balances": sessions.iter().map(|session| json!({"session": session, "balance": Ledger::balance_of(&snapshot.ledger, session)})).collect::<Vec<_>>(),
        "entries": entries.iter().rev().take(MAX_LIMIT).collect::<Vec<_>>(),
    })
}

/** Metrics of a scope: repository (default), session, task, agent, tool_call, or hook (a hook
 * event name)
 * Input
    - snapshot: &Snapshot - stores
    - query: &BTreeMap<String, String> - scope and id
 * Output
    - Answer
*/
fn metrics_view(snapshot: &Snapshot, query: &BTreeMap<String, String>) -> Answer {
    let scope = query
        .get("scope")
        .map(String::as_str)
        .unwrap_or("repository");
    let id = query.get("id").cloned().unwrap_or_default();
    let session_ids: BTreeSet<String> = match scope {
        "session" => BTreeSet::from([id.clone()]),
        "agent" => snapshot
            .sessions
            .iter()
            .filter(|session| session.agent_id == id)
            .map(|session| session.foxx_session_id.clone())
            .collect(),
        _ => snapshot
            .sessions
            .iter()
            .map(|session| session.foxx_session_id.clone())
            .collect(),
    };
    let events = snapshot
        .events
        .iter()
        .filter(|event| match scope {
            "repository" => true,
            "session" | "agent" => event
                .scope
                .foxx_session_id
                .as_ref()
                .is_some_and(|session| session_ids.contains(session)),
            "task" => event.scope.foxx_task_id.as_deref() == Some(id.as_str()),
            "tool_call" => event.scope.tool_call_id.as_deref() == Some(id.as_str()),
            "hook" => event.payload.get("hook_event").and_then(Value::as_str) == Some(id.as_str()),
            _ => false,
        })
        .collect::<Vec<_>>();
    if !matches!(
        scope,
        "repository" | "session" | "agent" | "task" | "tool_call" | "hook"
    ) {
        return error(
            400,
            format!(
                "unknown scope '{scope}'; use repository, session, task, agent, tool_call, or hook"
            ),
        );
    }
    let ledger = match scope {
        "task" => snapshot
            .ledger
            .iter()
            .filter(|entry| entry.task.as_deref() == Some(id.as_str()))
            .collect(),
        "tool_call" => snapshot
            .ledger
            .iter()
            .filter(|entry| entry.tool_call_id.as_deref() == Some(id.as_str()))
            .collect(),
        _ => metrics::ledger_of(&snapshot.ledger, &session_ids),
    };
    let sessions = snapshot
        .sessions
        .iter()
        .filter(|session| {
            matches!(scope, "repository") || session_ids.contains(&session.foxx_session_id)
        })
        .map(|session| serde_json::to_value(session).unwrap_or_default())
        .collect::<Vec<_>>();
    (
        200,
        json!({"scope": scope, "id": id, "metrics": metrics::compute(&events, &ledger, &sessions)}),
        None,
    )
}

/** Inputs of the compliance evaluation, from fresh verification
 * Input
    - workspace: &Workspace - repository
    - snapshot: &Snapshot - stores
 * Output
    - Inputs
*/
fn compliance_inputs(workspace: &Workspace, snapshot: &Snapshot) -> Inputs {
    let mut inputs = Inputs {
        events_verified: verify_chain(&snapshot.events).status != "broken",
        ledger_verified: ledger::verify(&snapshot.ledger).is_ok(),
        alerts_configured: alerts::configured(workspace),
        hooks_valid: [AgentKind::Claude, AgentKind::Codex]
            .into_iter()
            .any(|kind| install::validate(kind).is_ok_and(|report| report["valid"] == true)),
        ..Inputs::default()
    };
    if let Ok((report, state)) = evaluate(
        workspace,
        &Options {
            candidates: false,
            ..Options::default()
        },
    ) {
        let tally = |operation: Operation| {
            let commands = report
                .policies
                .iter()
                .flat_map(|policy| policy.commands.iter())
                .filter(|command| command.operation == operation)
                .collect::<Vec<_>>();
            Some((
                commands
                    .iter()
                    .filter(|command| command.outcome == Outcome::Pass)
                    .count(),
                commands.len(),
            ))
        };
        inputs.preserve = tally(Operation::Preserve);
        inputs.target = tally(Operation::Target);
        if let Ok(trust) = workspace.trust() {
            let findings = integrity(workspace, &state, &trust);
            inputs.integrity_findings =
                findings.iter().filter(|finding| finding.blocking()).count();
            inputs.audit_verified = !findings
                .iter()
                .any(|finding| finding.code == "audit_tampered");
            inputs.anchor = trust.anchor().ok().flatten().is_some();
        }
    }
    inputs
}

/** The compliance view: every control with outcome, evidence, gaps, and mappings
 * Input
    - workspace: &Workspace - repository
    - snapshot: &Snapshot - stores
 * Output
    - Value
*/
fn compliance_view(workspace: &Workspace, snapshot: &Snapshot) -> Value {
    let events = snapshot.events.iter().collect::<Vec<_>>();
    let ledger = snapshot.ledger.iter().collect::<Vec<_>>();
    let sessions = snapshot
        .sessions
        .iter()
        .map(|session| serde_json::to_value(session).unwrap_or_default())
        .collect::<Vec<_>>();
    let computed = metrics::compute(&events, &ledger, &sessions);
    let controls = compliance::evaluate(
        &compliance_inputs(workspace, snapshot),
        &computed,
        &events,
        &ledger,
        &sessions,
    );
    json!({
        "disclaimer": "Framework mappings show which requirements this evidence supports; they are not a certification.",
        "controls": controls,
        "unsupported_telemetry": computed.iter().filter(|metric| metric.status == crate::telemetry::model::Telemetry::Unsupported).map(|metric| metric.name).collect::<Vec<_>>(),
    })
}

/** The repository connection without secrets
 * Input
    - workspace: &Workspace - repository
 * Output
    - Value
*/
fn repo_view(workspace: &Workspace) -> Value {
    let connection = github::load(workspace).ok().flatten();
    json!({
        "connection": connection.map(|connection| json!({"method": connection.method.name(), "repository": connection.normalized, "privileges": connection.privileges, "connected_at": iso_time(connection.connected_at * 1000)})),
        "github_grant_active": github::active_grant().is_some(),
    })
}

/** Build a session report: summary and outcome, timeline, action graph, resource effects,
 * decisions and violations, autonomy accounting, verification results, coverage, evidence and
 * integrity, remediation, and the telemetry's limitations
 * Input
    - workspace: &Workspace - repository
    - snapshot: &Snapshot - stores
    - id: &str - canonical session
 * Output
    - Value
*/
pub(crate) fn session_report(workspace: &Workspace, snapshot: &Snapshot, id: &str) -> Value {
    let Some(session) = snapshot.session(id) else {
        return json!({"error": "no such session"});
    };
    let events = snapshot.session_events(id);
    let view = session_view(snapshot, session);
    let metric = |name: &str| view["metrics"][name]["value"].as_f64().unwrap_or(0.0);
    let violations = events
        .iter()
        .filter(|event| {
            matches!(
                event.enforcement,
                Enforcement::BypassConfirmed | Enforcement::DetectedAfterEffect
            )
        })
        .map(|event| compact(event))
        .collect::<Vec<_>>();
    let gaps = events
        .iter()
        .filter(|event| event.enforcement == Enforcement::CoverageGap)
        .map(|event| compact(event))
        .collect::<Vec<_>>();
    let outcome = if metric("bypasses_confirmed") > 0.0 {
        "BYPASS_CONFIRMED"
    } else if !violations.is_empty() {
        "VIOLATION"
    } else if !gaps.is_empty() {
        "INCOMPLETE_EVIDENCE"
    } else if metric("tasks_failed_verification") > 0.0 && metric("tasks_verified") == 0.0 {
        "NOT_VERIFIED"
    } else if metric("tasks_verified") > 0.0 {
        "COMPLIANT"
    } else {
        "NO_VERIFICATION_RECORDED"
    };
    let mut remediation = Vec::new();
    if metric("bypasses_confirmed") > 0.0 {
        remediation.push("Review the quarantined session's shell commands, restore the protected code from the checkpoint, and start a new session.");
    }
    if !violations.is_empty() {
        remediation.push("Run 'crane test .' and restore every failing preserved selection.");
    }
    if !gaps.is_empty() {
        remediation.push("Validate the hooks with 'crane agent hooks --profile ...'; actions without decisions are unobserved.");
    }
    if metric("bypass_attempts") > 0.0 {
        remediation.push("Review the bypass attempts; repeated attempts quarantine the session.");
    }
    let mut effects: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for event in &events {
        for resource in &event.resources {
            effects
                .entry(resource.access.clone())
                .or_default()
                .insert(resource.id.clone());
        }
    }
    let chain = verify_chain(&snapshot.events);
    json!({
        "report": "session",
        "generated_at": iso_time(now_millis()),
        "repository_id": workspace.repository_id,
        "summary": {
            "outcome": outcome,
            "session": view["session"],
            "credits": view["credits"],
            "decisions": {"allowed": metric("actions_allowed"), "denied": metric("actions_denied"), "approval": metric("actions_requiring_approval")},
            "bypass_attempts": metric("bypass_attempts"),
            "bypasses_confirmed": metric("bypasses_confirmed"),
            "violations_after_effect": metric("violations_after_effect"),
        },
        "timeline": events.iter().map(|event| compact(event)).collect::<Vec<_>>(),
        "action_graph": graph(&events),
        "resource_effects": effects,
        "violations": violations,
        "autonomy": snapshot.ledger.iter().filter(|entry| entry.session == id).collect::<Vec<_>>(),
        "verification": events.iter().filter(|event| event.event_type.starts_with("task.verif")).map(|event| compact(event)).collect::<Vec<_>>(),
        "coverage": {
            "gaps": gaps,
            "evidence_completeness": view["metrics"]["evidence_completeness"],
        },
        "metrics": view["metrics"],
        "integrity": {
            "event_chain": chain,
            "ledger_reconciles": ledger::verify(&snapshot.ledger).is_ok(),
            "attestation": events.iter().rev().find(|event| event.event_type == "session.attested").map(|event| event.payload["attestation"].clone()),
            "registry_generations": events.iter().map(|event| event.bindings.registry_generation).collect::<BTreeSet<_>>(),
        },
        "evidence_references": events.iter().flat_map(|event| event.evidence.clone()).collect::<Vec<_>>(),
        "remediation": remediation,
        "limitations": [
            "Facts marked INFERRED (network destinations, generated identifiers) were parsed, not observed.",
            "Network bytes, CPU, memory, and model tokens are UNSUPPORTED by hook instrumentation.",
            "Changes made outside agent hooks (other terminals, editors) are detected only when Crane next verifies.",
            "An agent with an unrestricted shell running as the same user can bypass local checks; detection after the effect is then the control.",
        ],
    })
}

/** Build a downloadable evidence bundle for a session: the report, the session's events and
 * ledger entries, and a manifest with the SHA-512 of every part and of the bundle
 * Input
    - workspace: &Workspace - repository
    - snapshot: &Snapshot - stores
    - id: &str - canonical session
 * Output
    - Value
*/
pub(crate) fn evidence_bundle(workspace: &Workspace, snapshot: &Snapshot, id: &str) -> Value {
    let report = session_report(workspace, snapshot, id);
    let events = snapshot
        .session_events(id)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let ledger = snapshot
        .ledger
        .iter()
        .filter(|entry| entry.session == id)
        .cloned()
        .collect::<Vec<_>>();
    let parts = [
        ("report", json!(report)),
        ("events", json!(events)),
        ("ledger", json!(ledger)),
    ];
    let files = parts
        .iter()
        .map(|(name, value)| json!({"name": name, "sha512": digest("evidence/part/v1", &[canonical_json(value).as_bytes()])}))
        .collect::<Vec<_>>();
    let manifest = json!({
        "format": 1,
        "created_at": iso_time(now_millis()),
        "repository_id": workspace.repository_id,
        "session": id,
        "parts": files,
        "verification": "recompute SHA-512 (domain evidence/part/v1) over the canonical JSON of each part; each event digest is SHA-512 over its canonical JSON and chains through previous_digest",
    });
    let bundle_digest = digest(
        "evidence/bundle/v1",
        &[canonical_json(&manifest).as_bytes()],
    );
    json!({
        "manifest": manifest,
        "bundle_digest": bundle_digest,
        "report": parts[0].1,
        "events": parts[1].1,
        "ledger": parts[2].1,
    })
}
