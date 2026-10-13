// Timeline views: compact events with their member, sessions, the event timeline with filters and
// pagination, and the action graph of a session.

use super::*;
use crate::foxx::views::session_view;
use crate::platform::render::iso_time;
use crate::server::dashboard::data::Context;
use crate::telemetry::events::Event;
use crate::telemetry::model::{Decision, Enforcement};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/** A compact event for lists and timelines, with its member
 * Input
    - context: &Context - repository data
    - event: &Event - event
 * Output
    - Value
*/
pub(super) fn compact(context: &Context, event: &Event) -> Value {
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
pub(super) fn sessions(context: &Context, query: &BTreeMap<String, String>) -> Value {
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
            view["stale"] = json!(context.stale(session));
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
pub(super) fn events(context: &Context, query: &BTreeMap<String, String>) -> Value {
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
pub(super) fn graph(context: &Context, events: &[&Event]) -> Value {
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
