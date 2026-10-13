// Timeline views over the repository's event stream (DASHBOARD.md): served events with their
// member, sessions with their evals, the event timeline with filters and pagination, and the action
// graph of a session.

use super::*;
use crate::foxx::views::session_view_with;
use crate::server::dashboard::data::Context;
use crate::telemetry::evals::field;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/** An event as served: with the repository's kind (personal or organization) and its member
 * Input
    - context: &Context - repository data
    - spec: &Value - event from the stream
    - kind: &str - PERSONAL or ORGANIZATION
 * Output
    - Value
*/
pub(super) fn served(context: &Context, spec: &Value, kind: &str) -> Value {
    let mut value = spec.clone();
    if value["repository"].is_object() {
        value["repository"]["repo"] = json!(kind);
    }
    value["member"] = context.member(spec["member_key"].as_str().map(String::from).as_ref());
    value
}

/** The events of one session in the stream
 * Input
    - context: &Context - repository data
    - session: &str - canonical session
 * Output
    - Vec<&Value>
*/
pub(super) fn of_session<'a>(context: &'a Context, session: &str) -> Vec<&'a Value> {
    context
        .specs
        .iter()
        .filter(|spec| spec["session"]["session_id"].as_str() == Some(session))
        .collect()
}

/** List sessions with their evals, filtered by agent, provider, or safety
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
            let mut view = session_view_with(snapshot, session, &context.specs);
            view["member"] = context.member(context.session_member.get(&session.foxx_session_id));
            view["stale"] = json!(context.stale(session));
            view
        })
        .collect::<Vec<_>>();
    json!({"sessions": list})
}

/** The event timeline, newest first, filtered by any attribute and paginated
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - the global filter (filter.rs), cursor, limit
    - kind: &str - PERSONAL or ORGANIZATION
 * Output
    - Value {events, next_cursor, total}
*/
pub(super) fn events(context: &Context, query: &BTreeMap<String, String>, kind: &str) -> Value {
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let cursor = query
        .get("cursor")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let matching = super::filter::filtered(context, query, None)
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let page = matching
        .iter()
        .skip(cursor)
        .take(limit)
        .map(|spec| served(context, spec, kind))
        .collect::<Vec<_>>();
    let more = matching.len() > cursor + limit;
    json!({
        "events": page,
        "total": matching.len(),
        "next_cursor": if more { Some(cursor + limit) } else { None },
    })
}

/** The action graph of a session: its events in order; "verified_by" edges join a decision to the
 * verification of the same tool call (by its action_id), and "followed_by" edges only record order,
 * never a claimed cause
 * Input
    - context: &Context - repository data
    - session: &str - canonical session
    - kind: &str - PERSONAL or ORGANIZATION
 * Output
    - Value {nodes, edges}
*/
pub(super) fn graph(context: &Context, session: &str, kind: &str) -> Value {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut previous: Option<String> = None;
    let mut decided: BTreeMap<String, String> = BTreeMap::new();
    for spec in of_session(context, session) {
        let name = spec["event_name"].as_str().unwrap_or_default();
        let flow = spec["flow"].as_str().unwrap_or_default();
        let id = spec["event_id"].as_str().unwrap_or_default().to_string();
        let gap = field(spec, "decision", "enforcement") == "COVERAGE_GAP" || flow == "F11";
        let node_kind = match flow {
            _ if gap => "gap",
            "F05" | "F16" => "decision",
            "F06" => "verification",
            "F04" if name == "PROMPT_SUSPICIOUS" => "signal",
            "F04" | "F07" => "task",
            "F03" | "F08" | "F17" | "F10" => "session",
            "F27" if name == "CREDITS_LOW" => "signal",
            "F27" => "credit",
            "F14" => "alert",
            _ => "other",
        };
        let flags = [
            (
                matches!(
                    field(spec, "decision", "decision"),
                    "DENY" | "QUARANTINE" | "ERROR"
                ),
                "denied",
            ),
            (
                field(spec, "severity", "alert_kind") == "BYPASS_ATTEMPT",
                "bypass_attempt",
            ),
            (name == "BYPASS_CONFIRMED", "bypass_confirmed"),
            (name == "VIOLATION_AFTER_EFFECT", "violation"),
            (
                field(spec, "decision", "enforcement") == "COVERAGE_GAP",
                "coverage_gap",
            ),
            (
                field(spec, "assessment", "assessment") == "UNRESOLVED",
                "unresolved",
            ),
        ]
        .into_iter()
        .filter_map(|(present, flag)| present.then_some(flag))
        .collect::<Vec<_>>();
        let tool = field(spec, "action", "action_tool");
        nodes.push(json!({
            "id": id,
            "kind": node_kind,
            "label": if tool.is_empty() { name.to_string() } else { format!("{name} {tool}") },
            "event": served(context, spec, kind),
            "flags": flags,
        }));
        if let Some(previous) = &previous {
            edges.push(json!({"from": previous, "to": id, "kind": "followed_by", "evidence": "event order"}));
        }
        let call = field(spec, "action", "action_id").to_string();
        if !call.is_empty() {
            if flow == "F05" && !decided.contains_key(&call) {
                decided.insert(call.clone(), id.clone());
            } else if flow == "F06" {
                if let Some(decision) = decided.get(&call) {
                    edges.push(json!({"from": decision, "to": id, "kind": "verified_by", "evidence": format!("tool call {call}")}));
                }
            }
        }
        previous = Some(id);
    }
    json!({"nodes": nodes, "edges": edges})
}
