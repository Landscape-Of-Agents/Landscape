// The dashboard's JSON API for one repository (and the selected members): every view the old local
// dashboard had (overview, agents and sessions, the action graph, the event timeline, governance,
// the autonomy ledger, metrics, compliance, alerts, health, the repository connection, integrations,
// session reports, and evidence bundles), built from the uploaded records and the members' latest
// repository status. No route changes anything. Lists are bounded and paginated.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::data::Context;
use crate::foxx::views::{agents, metric_map, session_view, task_view};
use crate::platform::now_millis;
use crate::platform::render::iso_time;
use crate::telemetry::compliance::{self, Inputs};
use crate::telemetry::events::Event;
use crate::telemetry::ledger::Ledger;
use crate::telemetry::metrics::{self, DEFINITIONS};
use crate::telemetry::model::{Decision, Enforcement};
use crate::trust::crypto::{canonical_json, digest};

/** Largest page of events */
const MAX_LIMIT: usize = 500;

/** Default page of events */
const DEFAULT_LIMIT: usize = 100;

/** A route's answer: status, body, and a download file name for attachments */
pub(crate) type Answer = (u16, Value, Option<String>);

/** Build an error answer
 * Input
    - status: u16 - HTTP status
    - message: String - explanation
 * Output
    - Answer
*/
pub(crate) fn error(status: u16, message: String) -> Answer {
    (status, json!({"error": message}), None)
}

/** Route a repository request
 * Input
    - context: &Context - the repository's data
    - path: &str - request path after /api/
    - query: &BTreeMap<String, String> - query parameters
 * Output
    - Answer
*/
pub(crate) fn route(context: &Context, path: &str, query: &BTreeMap<String, String>) -> Answer {
    let snapshot = &context.snapshot;
    let segments = path.trim_end_matches('/').split('/').collect::<Vec<_>>();
    let session_detail = |id: &str| {
        snapshot.session(id).map(|session| {
            let mut view = session_view(snapshot, session);
            view["member"] = context.member(context.session_member.get(&session.foxx_session_id));
            view["tasks"] = json!(snapshot
                .tasks
                .iter()
                .filter(|task| task.foxx_session_id == session.foxx_session_id)
                .map(|task| task_view(snapshot, task))
                .collect::<Vec<_>>());
            view
        })
    };
    match segments.as_slice() {
        ["health"] => (200, health(context), None),
        ["overview"] => (200, overview(context, query), None),
        ["members"] => (200, members(context), None),
        ["agents"] => (200, json!({"agents": agents(snapshot)}), None),
        ["sessions"] => (200, sessions(context, query), None),
        ["sessions", id] => match session_detail(id) {
            Some(view) => (200, view, None),
            None => error(404, format!("no session '{id}'")),
        },
        ["tasks"] => (
            200,
            json!({"tasks": snapshot.tasks.iter().filter(|task| query.get("session").is_none_or(|session| &task.foxx_session_id == session)).take(MAX_LIMIT).map(|task| task_view(snapshot, task)).collect::<Vec<_>>()}),
            None,
        ),
        ["tasks", id] => match snapshot.task(id) {
            Some(task) => (200, task_view(snapshot, task), None),
            None => error(404, format!("no task '{id}'")),
        },
        ["events"] => (200, events(context, query), None),
        ["graph", id] => match snapshot.session(id) {
            Some(session) => (
                200,
                graph(context, &snapshot.session_events(&session.foxx_session_id)),
                None,
            ),
            None => error(404, format!("no session '{id}'")),
        },
        ["governance"] => governance(context),
        ["ledger"] => (200, ledger_view(context, query), None),
        ["metrics"] => metrics_view(context, query),
        ["definitions"] => (
            200,
            json!({"metrics": DEFINITIONS, "controls": compliance::CONTROLS, "domains": crate::telemetry::model::Domain::ALL}),
            None,
        ),
        ["compliance"] => (200, compliance_view(context), None),
        ["alerts"] => (200, alerts(context), None),
        ["integrations"] => (200, integrations(context), None),
        ["repo"] => (200, repo_view(context), None),
        ["reports", "session", id] => match snapshot.session(id) {
            Some(session) => (200, session_report(context, &session.foxx_session_id), None),
            None => error(404, format!("no session '{id}'")),
        },
        ["evidence", "session", id] => match snapshot.session(id) {
            Some(session) => {
                let bundle = evidence_bundle(context, &session.foxx_session_id);
                let text = serde_json::to_string_pretty(&bundle).unwrap_or_default();
                (
                    200,
                    Value::String(text),
                    Some(format!("crane-evidence-{}.json", session.foxx_session_id)),
                )
            }
            None => error(404, format!("no session '{id}'")),
        },
        _ => error(404, format!("no API route /api/{path}")),
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

/** Combine the members' event chain and ledger results
 * Input
    - context: &Context - repository data
 * Output
    - (Value, bool) event chain summary {status, length, members} and whether every ledger reconciles
*/
fn chains(context: &Context) -> (Value, bool) {
    let statuses = context
        .chains
        .iter()
        .map(|chain| {
            chain["event_chain"]["status"]
                .as_str()
                .unwrap_or("unknown")
                .to_string()
        })
        .collect::<Vec<_>>();
    let status = if statuses.iter().any(|status| status == "broken") {
        "broken"
    } else if statuses.iter().all(|status| status == "verified") && !statuses.is_empty() {
        "verified"
    } else if statuses.is_empty() {
        "empty"
    } else {
        statuses.first().map_or("unknown", String::as_str)
    };
    let members = context
        .chains
        .iter()
        .map(|chain| {
            json!({
                "member": context.member(chain["member"].as_str().map(String::from).as_ref()),
                "event_chain": chain["event_chain"],
                "ledger_reconciles": chain["ledger_reconciles"],
            })
        })
        .collect::<Vec<_>>();
    (
        json!({"status": status, "length": context.snapshot.events.len(), "members": members}),
        context
            .chains
            .iter()
            .all(|chain| chain["ledger_reconciles"] == true),
    )
}

/** Combine the members' latest integrity reports
 * Input
    - context: &Context - repository data
 * Output
    - Value {ok, findings, reported}
*/
fn integrity(context: &Context) -> Value {
    let mut findings = Vec::new();
    let mut ok = true;
    for status in &context.statuses {
        let member = context.member(status["installation"].as_str().map(String::from).as_ref());
        ok &= status["integrity"]["ok"] != false;
        for finding in status["integrity"]["findings"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let mut finding = finding.clone();
            finding["member"] = member.clone();
            findings.push(finding);
        }
    }
    json!({"ok": ok, "findings": findings, "reported": !context.statuses.is_empty()})
}

/** Health: versions, chain status per member, store sizes, and the members' status reports
 * Input
    - context: &Context - repository data
 * Output
    - Value
*/
fn health(context: &Context) -> Value {
    let (chain, ledger_reconciles) = chains(context);
    let latest = context.latest_status();
    json!({
        "crane_version": env!("CARGO_PKG_VERSION"),
        "reported_crane_version": latest.map(|status| status["crane_version"].clone()),
        "schema_version": crate::telemetry::events::SCHEMA_VERSION,
        "metric_formula_version": metrics::FORMULA_VERSION,
        "repository_id": context.repository,
        "remote": context.remote,
        "registry_generation": latest.map(|status| status["registry_generation"].clone()),
        "events": context.snapshot.events.len(),
        "event_chain": chain,
        "ledger_entries": context.snapshot.ledger.len(),
        "ledger_reconciles": ledger_reconciles,
        "sessions": context.snapshot.sessions.len(),
        "members": context.members.len(),
        "selected_members": context.selected.len(),
        "truncated": context.truncated,
        "storage": {"service": "Foxx service (MongoDB, read-only for the dashboard)"},
        "mode": "hosted read-only",
        "now": iso_time(now_millis()),
    })
}

/** The overview page: activity, outcomes, autonomy, integrity, coverage, and recent alerts
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - since/until filters
 * Output
    - Value
*/
fn overview(context: &Context, query: &BTreeMap<String, String>) -> Value {
    let snapshot = &context.snapshot;
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
    json!({
        "active_sessions": snapshot.sessions.iter().filter(|session| session.ended_at.is_none()).count(),
        "sessions": snapshot.sessions.len(),
        "tasks": snapshot.tasks.len(),
        "tasks_by_status": snapshot.tasks.iter().fold(BTreeMap::<String, usize>::new(), |mut counts, task| { *counts.entry(task.status.clone()).or_default() += 1; counts }),
        "agents": snapshot.sessions.iter().map(|session| session.agent_id.clone()).collect::<BTreeSet<_>>().len(),
        "members": context.selected.len(),
        "metrics": metric_map(&computed),
        "integrity": integrity(context),
        "event_chain": chains(context).0,
        "recent_alerts": alerts(context)["alerts"].as_array().map(|alerts| alerts.iter().take(10).cloned().collect::<Vec<_>>()).unwrap_or_default(),
        "recent_violations": snapshot.events.iter().rev().filter(|event| matches!(event.enforcement, Enforcement::BypassConfirmed | Enforcement::DetectedAfterEffect)).take(10).map(|event| compact(context, event)).collect::<Vec<_>>(),
    })
}

/** Every member of the repository with its activity and status report
 * Input
    - context: &Context - repository data
 * Output
    - Value {members}
*/
fn members(context: &Context) -> Value {
    let list = context
        .members
        .iter()
        .map(|member| {
            let key = member["installation"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let events = context
                .event_member
                .values()
                .filter(|owner| **owner == key)
                .count();
            let sessions = context
                .session_member
                .values()
                .filter(|owner| **owner == key)
                .count();
            let status = context
                .statuses
                .iter()
                .find(|status| status["installation"] == key.as_str());
            json!({
                "member": context.member(Some(&key)),
                "selected": context.selected.contains(&key),
                "verified": member["verified"],
                "last_upload": member["updated_at"],
                "registry_generation": member["registry_generation"],
                "events": events,
                "sessions": sessions,
                "status_reported_at": status.map(|status| status["collected_at"].clone()),
                "crane_version": status.map(|status| status["crane_version"].clone()),
            })
        })
        .collect::<Vec<_>>();
    json!({"members": list})
}

/** A compact event for lists and timelines, with its member
 * Input
    - context: &Context - repository data
    - event: &Event - event
 * Output
    - Value
*/
fn compact(context: &Context, event: &Event) -> Value {
    json!({
        "event_id": event.event_id,
        "sequence": event.sequence,
        "type": event.event_type,
        "at": iso_time(event.occurred_at),
        "occurred_at": event.occurred_at,
        "member": context.member(context.event_member.get(&event.event_id)),
        "session": event.scope.foxx_session_id,
        "task": event.scope.foxx_task_id,
        "tool_call": event.scope.tool_call_id,
        "provider": event.scope.provider,
        "model": event.scope.model,
        "severity": event.severity,
        "domain": event.domain,
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
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - filters
 * Output
    - Value
*/
fn sessions(context: &Context, query: &BTreeMap<String, String>) -> Value {
    let snapshot = &context.snapshot;
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
        .map(|session| {
            let mut view = session_view(snapshot, session);
            view["member"] = context.member(context.session_member.get(&session.foxx_session_id));
            view
        })
        .collect::<Vec<_>>();
    json!({"sessions": list})
}

/** List events with filters, paginated by position (members' sequences are independent)
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - session, task, agent, provider, model, type, decision,
      severity, domain, enforcement, tool_call, selection, policy, since, until, cursor, limit
 * Output
    - Value {events, next_cursor}
*/
fn events(context: &Context, query: &BTreeMap<String, String>) -> Value {
    let snapshot = &context.snapshot;
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let cursor = query
        .get("cursor")
        .and_then(|value| value.parse::<usize>().ok())
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
        event.occurred_at >= since
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
        .skip(cursor)
        .take(limit + 1)
        .collect::<Vec<_>>();
    let more = page.len() > limit;
    let page = &page[..page.len().min(limit)];
    json!({
        "events": page.iter().map(|event| compact(context, event)).collect::<Vec<_>>(),
        "next_cursor": if more { Some(cursor + limit) } else { None },
    })
}

/** Build the action graph of a session: tool calls in order, each with its decision and its
 * post-action verification; "verified_by" edges join a decision to the verification of the same
 * tool call (established by the tool call identifier), and "followed_by" edges only record order,
 * never a claimed cause
 * Input
    - context: &Context - repository data
    - events: &[&Event] - session events
 * Output
    - Value {nodes, edges}
*/
fn graph(context: &Context, events: &[&Event]) -> Value {
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
            "event": compact(context, event),
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

/** The governance view, as last reported by a member's machine
 * Input
    - context: &Context - repository data
 * Output
    - Answer
*/
fn governance(context: &Context) -> Answer {
    let Some(status) = context.latest_status() else {
        return error(
            404,
            "no member has reported the repository's governance yet; it is sent with the next upload".into(),
        );
    };
    let mut view = status["governance"].clone();
    if view.get("error").is_some() {
        return error(
            500,
            format!(
                "the reporting machine could not evaluate governance: {}",
                view["error"]
            ),
        );
    }
    view["reported_by"] =
        context.member(status["installation"].as_str().map(String::from).as_ref());
    view["reported_at"] = status["collected_at"].clone();
    (200, view, None)
}

/** The ledger view of one session or all sessions: entries, balances, and whether every member's
 * ledger reconciles
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - optional session
 * Output
    - Value
*/
fn ledger_view(context: &Context, query: &BTreeMap<String, String>) -> Value {
    let snapshot = &context.snapshot;
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
        "reconciles": chains(context).1,
        "balances": sessions.iter().map(|session| json!({"session": session, "balance": Ledger::balance_of(&snapshot.ledger, session)})).collect::<Vec<_>>(),
        "entries": entries.iter().rev().take(MAX_LIMIT).collect::<Vec<_>>(),
    })
}

/** Metrics for a scope: repository (default), session, agent, task, tool call, or hook
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - scope and id
 * Output
    - Answer
*/
fn metrics_view(context: &Context, query: &BTreeMap<String, String>) -> Answer {
    let snapshot = &context.snapshot;
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

/** The compliance view: every control with outcome, evidence, gaps, and mappings, from the
 * uploaded records and the latest reported local verification
 * Input
    - context: &Context - repository data
 * Output
    - Value
*/
fn compliance_view(context: &Context) -> Value {
    let snapshot = &context.snapshot;
    let events = snapshot.events.iter().collect::<Vec<_>>();
    let ledger = snapshot.ledger.iter().collect::<Vec<_>>();
    let sessions = snapshot
        .sessions
        .iter()
        .map(|session| serde_json::to_value(session).unwrap_or_default())
        .collect::<Vec<_>>();
    let computed = metrics::compute(&events, &ledger, &sessions);
    let (chain, ledger_reconciles) = chains(context);
    let mut inputs = compliance_inputs(context);
    inputs.events_verified = chain["status"] != "broken";
    inputs.ledger_verified = ledger_reconciles;
    let controls = compliance::evaluate(&inputs, &computed, &events, &ledger, &sessions);
    json!({
        "disclaimer": "Framework mappings show which requirements this evidence supports; they are not a certification.",
        "controls": controls,
        "unsupported_telemetry": computed.iter().filter(|metric| metric.status == crate::telemetry::model::Telemetry::Unsupported).map(|metric| metric.name).collect::<Vec<_>>(),
    })
}

/** Combine the members' reported compliance inputs: policy results from the latest report, and for
 * each machine-level control the best evidence any member reported (alert delivery, valid hooks, a
 * rollback anchor) or the worst finding (integrity, audit chain)
 * Input
    - context: &Context - repository data
 * Output
    - Inputs
*/
fn compliance_inputs(context: &Context) -> Inputs {
    let reported = context
        .statuses
        .iter()
        .filter_map(|status| {
            serde_json::from_value::<Inputs>(status["compliance_inputs"].clone()).ok()
        })
        .collect::<Vec<_>>();
    let mut inputs: Inputs = context
        .latest_status()
        .and_then(|status| serde_json::from_value(status["compliance_inputs"].clone()).ok())
        .unwrap_or_default();
    if !reported.is_empty() {
        inputs.alerts_configured = reported.iter().any(|item| item.alerts_configured);
        inputs.hooks_valid = reported.iter().any(|item| item.hooks_valid);
        inputs.anchor = reported.iter().any(|item| item.anchor);
        inputs.audit_verified = reported.iter().all(|item| item.audit_verified);
        inputs.integrity_findings = reported
            .iter()
            .map(|item| item.integrity_findings)
            .max()
            .unwrap_or(0);
    }
    inputs
}

/** Every member's configured integrations (non-secret settings), with the member
 * Input
    - context: &Context - repository data
 * Output
    - Value {integrations}
*/
fn integrations(context: &Context) -> Value {
    let list = context
        .statuses
        .iter()
        .flat_map(|status| {
            let member = context.member(status["installation"].as_str().map(String::from).as_ref());
            status["integrations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |mut integration| {
                    integration["member"] = member.clone();
                    integration
                })
        })
        .collect::<Vec<_>>();
    json!({"integrations": list})
}

/** Alerts, newest first, with their member
 * Input
    - context: &Context - repository data
 * Output
    - Value {alerts}
*/
fn alerts(context: &Context) -> Value {
    let list = context
        .snapshot
        .alerts
        .iter()
        .take(MAX_LIMIT)
        .map(|alert| {
            let mut alert = alert.clone();
            alert["member"] =
                context.member(alert["installation"].as_str().map(String::from).as_ref());
            if let Some(object) = alert.as_object_mut() {
                object.remove("installation");
                object.remove("repository_id");
            }
            alert
        })
        .collect::<Vec<_>>();
    json!({"alerts": list})
}

/** The repository connection, as last reported
 * Input
    - context: &Context - repository data
 * Output
    - Value
*/
fn repo_view(context: &Context) -> Value {
    let mut view = context.latest_status().map_or_else(
        || json!({"connection": null}),
        |status| status["repo"].clone(),
    );
    view["remote"] = json!(context.remote);
    view
}

/** Build a session report: summary and outcome, timeline, action graph, resource effects,
 * decisions and violations, autonomy accounting, verification results, coverage, evidence and
 * integrity, remediation, and the telemetry's limitations
 * Input
    - context: &Context - repository data
    - id: &str - canonical session
 * Output
    - Value
*/
fn session_report(context: &Context, id: &str) -> Value {
    let snapshot = &context.snapshot;
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
        .map(|event| compact(context, event))
        .collect::<Vec<_>>();
    let gaps = events
        .iter()
        .filter(|event| event.enforcement == Enforcement::CoverageGap)
        .map(|event| compact(context, event))
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
    let member = context.session_member.get(id);
    let chain = context
        .chains
        .iter()
        .find(|chain| member.is_some_and(|member| chain["member"] == member.as_str()))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "report": "session",
        "generated_at": iso_time(now_millis()),
        "repository_id": context.repository,
        "remote": context.remote,
        "member": context.member(member),
        "summary": {
            "outcome": outcome,
            "session": view["session"],
            "credits": view["credits"],
            "decisions": {"allowed": metric("actions_allowed"), "denied": metric("actions_denied"), "approval": metric("actions_requiring_approval")},
            "bypass_attempts": metric("bypass_attempts"),
            "bypasses_confirmed": metric("bypasses_confirmed"),
            "violations_after_effect": metric("violations_after_effect"),
        },
        "timeline": events.iter().map(|event| compact(context, event)).collect::<Vec<_>>(),
        "action_graph": graph(context, &events),
        "resource_effects": effects,
        "violations": violations,
        "autonomy": snapshot.ledger.iter().filter(|entry| entry.session == id).collect::<Vec<_>>(),
        "verification": events.iter().filter(|event| event.event_type.starts_with("task.verif")).map(|event| compact(context, event)).collect::<Vec<_>>(),
        "coverage": {
            "gaps": gaps,
            "evidence_completeness": view["metrics"]["evidence_completeness"],
        },
        "metrics": view["metrics"],
        "integrity": {
            "event_chain": chain["event_chain"],
            "ledger_reconciles": chain["ledger_reconciles"],
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
    - context: &Context - repository data
    - id: &str - canonical session
 * Output
    - Value
*/
fn evidence_bundle(context: &Context, id: &str) -> Value {
    let snapshot = &context.snapshot;
    let report = session_report(context, id);
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
        "repository_id": context.repository,
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
