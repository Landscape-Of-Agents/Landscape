// Deeper views over the event stream (DASHBOARD.md), every one under the global query: the Gantt
// chart of agent actions, the event timeline, traces (every event of one tool call, command, or
// audit entry), the topology of who used what, the dependency graph between flows, histograms of
// any number, and the texts (statements, reasons, rules, failure states) events carry.

use super::filter::{filtered, label, range};
use super::*;
use crate::telemetry::evals::field;
use crate::telemetry::query::{facet_path, values, Query};
use std::collections::{BTreeMap, BTreeSet};

/** A node of the topology: its layer and name */
type Node = (usize, String);

/** Most bars a Gantt chart returns (the newest) */
const MAX_BARS: usize = 1_500;

/** Most points the timeline returns (the newest) */
const MAX_POINTS: usize = 5_000;

/** Read the first value of a facet or attribute
 * Input
    - spec: &Value - event
    - by: &str - facet name or attribute path
 * Output
    - String ("NONE" when absent)
*/
fn first(spec: &Value, by: &str) -> String {
    values(spec, facet_path(by).unwrap_or(by))
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "NONE".into())
}

/** The worst of two severities
 * Input
    - a, b: &str - severities
 * Output
    - String
*/
fn worst(a: &str, b: &str) -> String {
    let rank = |severity: &str| {
        ["INFO", "LOW", "MEDIUM", "HIGH", "CRITICAL"]
            .iter()
            .position(|item| *item == severity)
            .unwrap_or(0)
    };
    if rank(b) > rank(a) {
        b.to_string()
    } else {
        a.to_string()
    }
}

/** The trace an event belongs to: its tool call or command (action_id), else itself
 * Input
    - spec: &Value - event
 * Output
    - String
*/
fn trace_id(spec: &Value) -> String {
    let action = field(spec, "action", "action_id");
    if action.is_empty() {
        spec["event_id"].as_str().unwrap_or_default().to_string()
    } else {
        action.to_string()
    }
}

/** One trace: its events and summary
 * Fields
    - id: String - trace identifier
    - spans: Vec<&'a Value> - its events, oldest first
*/
struct Trace<'a> {
    id: String,
    spans: Vec<&'a Value>,
}

impl Trace<'_> {
    fn tool_call_id(&self) -> Option<&str> {
        let has_tool = self
            .spans
            .iter()
            .any(|spec| !field(spec, "action", "action_tool").is_empty());
        has_tool.then_some(self.id.as_str())
    }

    /** Summarize the trace for tables and charts
     * Input
        - context: &Context - repository data
     * Output
        - Value {id, kind, start, end, duration_ms, spans, root, outcome, severity, decision, tool,
          session, member, flows, events, agent, agent_id, model, resources, targets, tool_ms,
          latency_ms, tokens: {total, input, output, cache_read, cache_creation, turn, turn_calls}}
    */
    fn summary(&self, context: &Context) -> Value {
        let first_span = self.spans[0];
        let start = self
            .spans
            .iter()
            .filter_map(|spec| spec["occurred_at"].as_u64())
            .min()
            .unwrap_or(0);
        let mut end = self
            .spans
            .iter()
            .filter_map(|spec| spec["occurred_at"].as_u64())
            .max()
            .unwrap_or(start);
        // A measured tool duration extends the bar past the last event
        if let Some(duration) = self
            .spans
            .iter()
            .find_map(|spec| spec["eval"]["tool_duration_ms"].as_u64())
        {
            end = end.max(start + duration);
        }
        let severity = self.spans.iter().fold("INFO".to_string(), |acc, spec| {
            worst(&acc, field(spec, "severity", "severity_status"))
        });
        let decision = self
            .spans
            .iter()
            .map(|spec| field(spec, "decision", "decision"))
            .find(|value| !value.is_empty() && *value != "NOT_REQUIRED")
            .unwrap_or("NOT_REQUIRED");
        let failed = self.spans.iter().any(|spec| {
            matches!(
                field(spec, "response", "response_status"),
                "FAILED" | "FAILURE" | "BLOCKED" | "REFUSED" | "FORBIDDEN"
            ) || matches!(
                field(spec, "decision", "decision"),
                "DENY" | "QUARANTINE" | "ERROR"
            ) || matches!(
                spec["event_name"].as_str(),
                Some("BYPASS_CONFIRMED" | "VIOLATION_AFTER_EFFECT")
            )
        });
        // The trace is named after its decision (F05) when it has one, else its first event
        let root = self
            .spans
            .iter()
            .find(|spec| spec["flow"] == "F05")
            .unwrap_or(&first_span);
        let kind = match field(first_span, "action", "action_type") {
            "" => "EVENT".to_string(),
            other => other.to_string(),
        };
        let flows = self
            .spans
            .iter()
            .filter_map(|spec| spec["flow"].as_str())
            .collect::<BTreeSet<_>>();
        let text = |group: &str, key: &str| {
            self.spans
                .iter()
                .map(|spec| field(spec, group, key))
                .find(|value| !value.is_empty() && *value != "NOT_AN_AGENT")
        };
        let eval = |key: &str| {
            self.spans
                .iter()
                .find_map(|spec| spec["eval"][key].as_f64())
        };
        let listed = |path: &str| {
            self.spans
                .iter()
                .flat_map(|spec| values(spec, path))
                .filter(|value| !value.is_empty() && value != "NOT_CONSUMED")
                .collect::<BTreeSet<_>>()
        };
        let tokens = eval("model_tokens").map(|total| {
            json!({
                "total": total,
                "input": eval("model_input_tokens"),
                "output": eval("model_output_tokens"),
                "cache_read": eval("cache_read_tokens"),
                "cache_creation": eval("cache_creation_tokens"),
                "turn": eval("turn_tokens"),
                "turn_calls": eval("turn_tool_calls"),
            })
        });
        json!({
            "agent": text("agent", "agent_type"),
            "agent_id": text("agent", "agent_id"),
            "model": text("agent", "model"),
            "resources": listed("resource.resource"),
            "targets": listed("resource.resource_ids").into_iter().take(8).collect::<Vec<_>>(),
            "tool_ms": eval("tool_duration_ms"),
            "latency_ms": eval("latency_ms"),
            "tokens": tokens,
            "id": self.id,
            "kind": kind,
            "start": start,
            "end": end,
            "duration_ms": end - start,
            "spans": self.spans.len(),
            "root": root["event_name"],
            "outcome": if failed { "FAILED" } else { "OK" },
            "severity": severity,
            "decision": decision,
            "tool": self.spans.iter().map(|spec| field(spec, "action", "action_tool")).find(|tool| !tool.is_empty()),
            "tool_call_id": self.tool_call_id(),
            "session": self.spans.iter().map(|spec| field(spec, "session", "session_id")).find(|session| !session.is_empty()),
            "member": context.member(first_span["member_key"].as_str().map(String::from).as_ref()),
            "flows": flows,
            "events": self.spans.iter().map(|spec| spec["event_name"].as_str().unwrap_or_default()).collect::<Vec<_>>(),
        })
    }
}

/** Group events into traces, keeping only traces with at least one event that passes the query
 * Input
    - context: &Context - repository data
    - query: &Query - filter
 * Output
    - Vec<Trace> oldest first
*/
fn traces<'a>(context: &'a Context, query: &Query) -> Vec<Trace<'a>> {
    let wanted = filtered(context, query, None)
        .into_iter()
        .map(trace_id)
        .collect::<BTreeSet<_>>();
    let mut groups: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for spec in &context.specs {
        let id = trace_id(spec);
        if wanted.contains(&id) {
            groups.entry(id).or_default().push(spec);
        }
    }
    let mut list = groups
        .into_iter()
        .map(|(id, spans)| Trace { id, spans })
        .collect::<Vec<_>>();
    list.sort_by_key(|trace| trace.spans[0]["occurred_at"].as_u64().unwrap_or(0));
    list
}

/** The trace table: every trace with at least one matching event, newest first, filtered by
 * trace kind, outcome, duration, and span count, and paginated
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - kind, outcome, min_ms, max_ms, min_spans, cursor, limit
 * Output
    - Value {traces, total, next_cursor, kinds}
*/
pub(super) fn trace_table(
    context: &Context,
    query: &Query,
    params: &BTreeMap<String, String>,
) -> Value {
    let number = |key: &str| params.get(key).and_then(|value| value.parse::<u64>().ok());
    let list = |key: &str| {
        params
            .get(key)
            .map(|text| {
                text.split(',')
                    .map(|item| item.trim().to_ascii_uppercase())
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let kinds = list("kind");
    let outcomes = list("outcome");
    let all = traces(context, query)
        .iter()
        .rev()
        .map(|trace| trace.summary(context))
        .collect::<Vec<_>>();
    let available = all
        .iter()
        .filter_map(|trace| trace["kind"].as_str())
        .collect::<BTreeSet<_>>();
    let matching = all
        .iter()
        .filter(|trace| kinds.is_empty() || kinds.iter().any(|kind| trace["kind"] == kind.as_str()))
        .filter(|trace| {
            outcomes.is_empty()
                || outcomes
                    .iter()
                    .any(|outcome| trace["outcome"] == outcome.as_str())
        })
        .filter(|trace| {
            number("min_ms").is_none_or(|min| trace["duration_ms"].as_u64().unwrap_or(0) >= min)
        })
        .filter(|trace| {
            number("max_ms").is_none_or(|max| trace["duration_ms"].as_u64().unwrap_or(0) <= max)
        })
        .filter(|trace| {
            number("min_spans").is_none_or(|min| trace["spans"].as_u64().unwrap_or(0) >= min)
        })
        .collect::<Vec<_>>();
    let cursor = number("cursor").unwrap_or(0) as usize;
    let limit = (number("limit").unwrap_or(DEFAULT_LIMIT as u64) as usize).clamp(1, MAX_LIMIT);
    json!({
        "traces": matching.iter().skip(cursor).take(limit).collect::<Vec<_>>(),
        "total": matching.len(),
        "next_cursor": (matching.len() > cursor + limit).then_some(cursor + limit),
        "kinds": available,
    })
}

/** The metrics of tool calls, grouped by tool (or agent, agent identity, session, model, member,
 * or any attribute): calls, outcomes, decisions, tool time and hook latency percentiles, and model
 * tokens. Only traces that are tool calls count
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - by: tool (default), agent, agent_id, session, model,
      member, or a summary field
 * Output
    - Value {by, groups: [{key, calls, denied, approvals, failed, failure_share, sessions, agents,
      tool_ms: {p50, p95, max, total}, latency_ms: {p50, p95}, tokens: {total, input, output,
      cache_read, cache_creation, per_call, measured_calls}, first, last}], totals}
*/
pub(super) fn usage(context: &Context, query: &Query, params: &BTreeMap<String, String>) -> Value {
    let by = params
        .get("by")
        .map(String::as_str)
        .filter(|by| !by.is_empty())
        .unwrap_or("tool")
        .to_string();
    let summaries = traces(context, query)
        .iter()
        .map(|trace| trace.summary(context))
        .filter(|summary| summary["tool"].is_string())
        .collect::<Vec<_>>();
    let key_of = |summary: &Value| -> String {
        let value = match by.as_str() {
            "member" => &summary["member"]["actor"],
            other => &summary[other],
        };
        value
            .as_str()
            .map(String::from)
            .or_else(|| value.as_u64().map(|number| number.to_string()))
            .unwrap_or_else(|| "NONE".into())
    };
    let mut groups: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for summary in &summaries {
        groups.entry(key_of(summary)).or_default().push(summary);
    }
    let percentile = |numbers: &mut Vec<f64>, share: f64| {
        if numbers.is_empty() {
            return Value::Null;
        }
        numbers.sort_by(|a, b| a.total_cmp(b));
        json!(numbers[((numbers.len() - 1) as f64 * share).round() as usize])
    };
    let describe = |key: &str, list: &[&Value]| {
        let calls = list.len();
        let count =
            |test: &dyn Fn(&Value) -> bool| list.iter().filter(|summary| test(summary)).count();
        let denied = count(&|summary| {
            matches!(
                summary["decision"].as_str(),
                Some("DENY" | "QUARANTINE" | "ERROR")
            )
        });
        let failed = count(&|summary| summary["outcome"] == "FAILED");
        let mut tool = list
            .iter()
            .filter_map(|summary| summary["tool_ms"].as_f64())
            .collect::<Vec<_>>();
        let mut latency = list
            .iter()
            .filter_map(|summary| summary["latency_ms"].as_f64())
            .collect::<Vec<_>>();
        let measured = list
            .iter()
            .filter(|summary| summary["tokens"].is_object())
            .collect::<Vec<_>>();
        let sum = |key: &str| {
            measured
                .iter()
                .filter_map(|summary| summary["tokens"][key].as_f64())
                .sum::<f64>()
        };
        let distinct = |key: &str| {
            list.iter()
                .filter_map(|summary| summary[key].as_str())
                .collect::<BTreeSet<_>>()
                .len()
        };
        let total_tokens = sum("total");
        json!({
            "key": key,
            "calls": calls,
            "denied": denied,
            "approvals": count(&|summary| summary["decision"] == "REQUIRE_APPROVAL"),
            "failed": failed,
            "failure_share": failed as f64 / calls.max(1) as f64,
            "sessions": distinct("session"),
            "agents": distinct("agent_id"),
            "tools": distinct("tool"),
            "tool_ms": {"p50": percentile(&mut tool.clone(), 0.5), "p95": percentile(&mut tool.clone(), 0.95), "max": percentile(&mut tool, 1.0), "total": list.iter().filter_map(|summary| summary["tool_ms"].as_f64()).sum::<f64>()},
            "latency_ms": {"p50": percentile(&mut latency.clone(), 0.5), "p95": percentile(&mut latency, 0.95)},
            "tokens": {
                "total": total_tokens,
                "input": sum("input"),
                "output": sum("output"),
                "cache_read": sum("cache_read"),
                "cache_creation": sum("cache_creation"),
                "per_call": if measured.is_empty() { Value::Null } else { json!(total_tokens / measured.len() as f64) },
                "measured_calls": measured.len(),
            },
            "first": list.iter().filter_map(|summary| summary["start"].as_u64()).min(),
            "last": list.iter().filter_map(|summary| summary["end"].as_u64()).max(),
        })
    };
    let mut list = groups
        .iter()
        .map(|(key, list)| describe(key, list))
        .collect::<Vec<_>>();
    list.sort_by(|a, b| b["calls"].as_u64().cmp(&a["calls"].as_u64()));
    json!({
        "by": by,
        "groups": list,
        "totals": describe("all", &summaries.iter().collect::<Vec<_>>()),
    })
}

/** One trace with every event, for the trace view
 * Input
    - context: &Context - repository data
    - id: &str - trace identifier
    - kind: &str - PERSONAL or ORGANIZATION
 * Output
    - Option<Value> {trace, spans}
*/
pub(super) fn trace_detail(context: &Context, id: &str, kind: &str) -> Option<Value> {
    let spans = context
        .specs
        .iter()
        .filter(|spec| trace_id(spec) == id)
        .collect::<Vec<_>>();
    if spans.is_empty() {
        return None;
    }
    let trace = Trace {
        id: id.into(),
        spans,
    };
    Some(json!({
        "trace": trace.summary(context),
        "spans": trace.spans.iter().map(|spec| super::timeline::served(context, spec, kind)).collect::<Vec<_>>(),
    }))
}

/** The Gantt chart: one bar per trace (a tool call from its decision to its verification and
 * credit settlement, a command, an alert), on rows grouped by session, agent, member, tool, flow,
 * or any attribute, with each session's own span behind its row
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - rows (facet or attribute; default session), color
      (facet or attribute; default decision)
 * Output
    - Value {start, end, rows: [{key, label, span, bars: [{id, start, end, label, color, severity,
      outcome, spans, session, tool}]}], total, truncated}
*/
pub(super) fn gantt(context: &Context, query: &Query, params: &BTreeMap<String, String>) -> Value {
    let rows_by = params.get("rows").map(String::as_str).unwrap_or("session");
    let color_by = params
        .get("color")
        .map(String::as_str)
        .unwrap_or("decision");
    let all = traces(context, query);
    let truncated = all.len() > MAX_BARS;
    let mut rows: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let (mut low, mut high) = (u64::MAX, 0);
    for trace in all.iter().rev().take(MAX_BARS) {
        let summary = trace.summary(context);
        // A trace's row is the first value any of its events has (a reservation has no tool, its
        // decision does)
        let row = if rows_by == "session" {
            summary["session"]
                .as_str()
                .unwrap_or("NOT_IN_SESSION")
                .to_string()
        } else {
            let value = trace
                .spans
                .iter()
                .map(|spec| first(spec, rows_by))
                .find(|value| value != "NONE")
                .unwrap_or_else(|| "NONE".into());
            if rows_by == "tool" {
                crate::telemetry::spec::canonical_tool_type(&value)
                    .unwrap_or(&value)
                    .to_string()
            } else {
                value
            }
        };
        let color = trace
            .spans
            .iter()
            .map(|spec| first(spec, color_by))
            .find(|value| value != "NONE" && value != "NOT_REQUIRED")
            .unwrap_or_else(|| first(trace.spans[0], color_by));
        let start = summary["start"].as_u64().unwrap_or(0);
        let end = summary["end"].as_u64().unwrap_or(start);
        low = low.min(start);
        high = high.max(end);
        rows.entry(row).or_default().push(json!({
            "id": summary["id"],
            "start": start,
            "end": end,
            "label": format!("{}{}", summary["root"].as_str().unwrap_or_default(), summary["tool"].as_str().map(|tool| format!(" · {tool}")).unwrap_or_default()),
            "color": color,
            "severity": summary["severity"],
            "outcome": summary["outcome"],
            "decision": summary["decision"],
            "spans": summary["spans"],
            "session": summary["session"],
            "tool": summary["tool"],
            "events": summary["events"],
        }));
    }
    let spans = context
        .snapshot
        .sessions
        .iter()
        .map(|session| {
            (
                session.foxx_session_id.clone(),
                (
                    session.started_at,
                    session.ended_at.unwrap_or(session.last_seen_at),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut list = rows
        .into_iter()
        .map(|(key, mut bars)| {
            bars.sort_by_key(|bar| bar["start"].as_u64().unwrap_or(0));
            let span = if rows_by == "session" { spans.get(&key).map(|(start, end)| json!({"start": start, "end": end})) } else { None };
            let first_bar = bars.first().and_then(|bar| bar["start"].as_u64()).unwrap_or(0);
            json!({"key": key, "label": label(context, Some(rows_by), &key), "span": span, "bars": bars, "first": first_bar})
        })
        .collect::<Vec<_>>();
    list.sort_by_key(|row| row["first"].as_u64().unwrap_or(0));
    let (start, end) = if low == u64::MAX {
        range(query, &[])
    } else {
        (
            query.since.unwrap_or(low),
            query.until.unwrap_or(high).max(low + 1),
        )
    };
    json!({"start": start, "end": end, "rows": list, "rows_by": rows_by, "color_by": color_by, "total": all.len(), "truncated": truncated})
}

/** The event timeline: every matching event as a point on lanes grouped by a facet or attribute
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - lanes (facet or attribute; default flow), color (default
      severity)
 * Output
    - Value {start, end, lanes: [{key, label, count}], points: [{t, lane, color, name, id, statement}],
      total, truncated}
*/
pub(super) fn timeline(
    context: &Context,
    query: &Query,
    params: &BTreeMap<String, String>,
) -> Value {
    let lanes_by = params.get("lanes").map(String::as_str).unwrap_or("flow");
    let color_by = params
        .get("color")
        .map(String::as_str)
        .unwrap_or("severity");
    let events = filtered(context, query, None);
    let truncated = events.len() > MAX_POINTS;
    let shown = &events[events.len().saturating_sub(MAX_POINTS)..];
    let mut lanes: BTreeMap<String, u64> = BTreeMap::new();
    let points = shown
        .iter()
        .map(|spec| {
            let lane = first(spec, lanes_by);
            *lanes.entry(lane.clone()).or_default() += 1;
            json!({
                "t": spec["occurred_at"],
                "lane": lane,
                "color": first(spec, color_by),
                "name": spec["event_name"],
                "id": spec["event_id"],
                "statement": field(spec, "response", "response_statement"),
                "session": field(spec, "session", "session_id"),
            })
        })
        .collect::<Vec<_>>();
    let (start, end) = range(query, shown);
    json!({
        "start": start,
        "end": end,
        "lanes": lanes.iter().map(|(key, count)| json!({"key": key, "label": label(context, Some(lanes_by), key), "count": count})).collect::<Vec<_>>(),
        "points": points,
        "lanes_by": lanes_by,
        "color_by": color_by,
        "total": events.len(),
        "truncated": truncated,
    })
}

/** The default layers of the topology and their names */
const LAYERS: [(&str, &str); 4] = [
    ("member", "Members"),
    ("agent", "Agents"),
    ("tool", "Tools"),
    ("resource.resource", "Resources"),
];

/** Values that end a path through the topology (an event without that layer) */
const NO_VALUE: [&str; 5] = ["", "NONE", "NA", "NOT_CONSUMED", "NOT_REQUIRED"];

/** The topology of activity: by default members use agents, agents call tools, tools touch
 * resources; any 2 to 5 attributes can be chained instead (layers=flow,decision,tool, ...). Each
 * event walks the layers in order until one has no value; the last layer counts every item of a
 * list. Edges carry how many events joined two nodes and how many of those were denied
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - layers: comma-separated facet names or attribute paths
 * Output
    - Value {layers: [name], layer_keys: [facet or path], nodes: [{id, layer, label, count, denied}],
      edges: [{from, to, count, denied}]}
*/
pub(super) fn topology(
    context: &Context,
    query: &Query,
    params: &BTreeMap<String, String>,
) -> Value {
    let chosen = params
        .get("layers")
        .map(|text| {
            text.split(',')
                .map(str::trim)
                .filter(|layer| !layer.is_empty())
                .take(5)
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .filter(|layers| layers.len() >= 2);
    let keys = chosen.unwrap_or_else(|| LAYERS.iter().map(|(key, _)| key.to_string()).collect());
    let names = keys
        .iter()
        .map(|key| {
            LAYERS
                .iter()
                .find(|(known, _)| known == key)
                .map_or_else(|| key.clone(), |(_, name)| name.to_string())
        })
        .collect::<Vec<_>>();
    let paths = keys
        .iter()
        .map(|key| facet_path(key).unwrap_or(key).to_string())
        .collect::<Vec<_>>();
    let mut nodes: BTreeMap<Node, (u64, u64)> = BTreeMap::new();
    let mut edges: BTreeMap<(Node, Node), (u64, u64)> = BTreeMap::new();
    for spec in filtered(context, query, None) {
        let denied = u64::from(matches!(
            field(spec, "decision", "decision"),
            "DENY" | "QUARANTINE" | "ERROR"
        ));
        let read = |layer: usize| {
            let found = values(spec, &paths[layer])
                .into_iter()
                .filter(|value| !NO_VALUE.contains(&value.as_str()))
                .map(|value| {
                    if paths[layer] == "member_key" {
                        label(context, Some("member"), &value)
                    } else {
                        value
                    }
                })
                .collect::<Vec<_>>();
            match (layer, found.is_empty()) {
                // The first layer always has a node: events of the service carry no member
                (0, true) if paths[0] == "member_key" => vec!["SERVICE".to_string()],
                (0, true) => vec!["NONE".to_string()],
                _ => found,
            }
        };
        let mut path: Vec<Node> = Vec::new();
        let mut last: Vec<Node> = Vec::new();
        for layer in 0..paths.len() {
            let found = read(layer);
            if found.is_empty() {
                break;
            }
            if layer + 1 == paths.len() {
                last = found.into_iter().map(|value| (layer, value)).collect();
            } else {
                path.push((layer, found[0].clone()));
            }
        }
        let mut add = |from: Option<&Node>, node: &Node| {
            let entry = nodes.entry(node.clone()).or_default();
            entry.0 += 1;
            entry.1 += denied;
            if let Some(from) = from {
                let entry = edges.entry((from.clone(), node.clone())).or_default();
                entry.0 += 1;
                entry.1 += denied;
            }
        };
        for (position, node) in path.iter().enumerate() {
            add(position.checked_sub(1).map(|before| &path[before]), node);
        }
        for node in &last {
            add(path.last(), node);
        }
    }
    let id = |(layer, name): &(usize, String)| format!("{layer}:{name}");
    json!({
        "layers": names,
        "layer_keys": keys,
        "nodes": nodes.iter().map(|(node, (count, denied))| json!({"id": id(node), "layer": node.0, "label": node.1, "count": count, "denied": denied})).collect::<Vec<_>>(),
        "edges": edges.iter().map(|((from, to), (count, denied))| json!({"from": id(from), "to": id(to), "count": count, "denied": denied})).collect::<Vec<_>>(),
    })
}

/** The dependency graph between flows: within each session, how often one flow's event is
 * followed by another flow's event (order only, never a claimed cause)
 * Input
    - context: &Context - repository data
    - query: &Query - filter
 * Output
    - Value {nodes: [{id, count}], edges: [{from, to, count}]}
*/
pub(super) fn dependencies(context: &Context, query: &Query) -> Value {
    let mut by_session: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for spec in filtered(context, query, None) {
        let flow = spec["flow"].as_str().unwrap_or("NONE").to_string();
        *counts.entry(flow).or_default() += 1;
        let session = field(spec, "session", "session_id");
        let session = if session.is_empty() {
            field(spec, "action", "action_user")
        } else {
            session
        };
        by_session
            .entry(session.to_string())
            .or_default()
            .push(spec);
    }
    let mut edges: BTreeMap<(String, String), u64> = BTreeMap::new();
    for events in by_session.values() {
        for pair in events.windows(2) {
            let (from, to) = (
                pair[0]["flow"].as_str().unwrap_or("NONE"),
                pair[1]["flow"].as_str().unwrap_or("NONE"),
            );
            if from != to {
                *edges.entry((from.to_string(), to.to_string())).or_default() += 1;
            }
        }
    }
    json!({
        "nodes": counts.iter().map(|(id, count)| json!({"id": id, "count": count})).collect::<Vec<_>>(),
        "edges": edges.iter().map(|((from, to), count)| json!({"from": from, "to": to, "count": count})).collect::<Vec<_>>(),
    })
}

/** A histogram of a number: any numeric attribute (eval.latency_ms, eval.tool_duration_ms, …),
 * or a derived one: trace duration, the gap between a session's events, events per session
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - field, bins (default 20)
 * Output
    - Value {field, count, min, max, mean, p50, p95, bins: [{from, to, count}], fields}
*/
pub(super) fn histogram(
    context: &Context,
    query: &Query,
    params: &BTreeMap<String, String>,
) -> Value {
    let field_name = params
        .get("field")
        .cloned()
        .unwrap_or_else(|| "trace_duration_ms".into());
    let bins = params
        .get("bins")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(2, 100);
    let events = filtered(context, query, None);
    let mut numeric: BTreeSet<String> = BTreeSet::new();
    for spec in &events {
        if let Some(object) = spec["eval"].as_object() {
            numeric.extend(
                object
                    .iter()
                    .filter(|(_, value)| value.is_number())
                    .map(|(key, _)| format!("eval.{key}")),
            );
        }
    }
    let numbers: Vec<f64> = match field_name.as_str() {
        "trace_duration_ms" => traces(context, query)
            .iter()
            .map(|trace| {
                trace.summary(context)["duration_ms"]
                    .as_f64()
                    .unwrap_or(0.0)
            })
            .collect(),
        "session_gap_ms" => {
            let mut last: BTreeMap<&str, u64> = BTreeMap::new();
            let mut gaps = Vec::new();
            for spec in &events {
                let session = field(spec, "session", "session_id");
                let at = spec["occurred_at"].as_u64().unwrap_or(0);
                if session.is_empty() {
                    continue;
                }
                if let Some(previous) = last.insert(session, at) {
                    gaps.push(at.saturating_sub(previous) as f64);
                }
            }
            gaps
        }
        "events_per_session" => {
            let mut counts: BTreeMap<&str, f64> = BTreeMap::new();
            for spec in &events {
                let session = field(spec, "session", "session_id");
                if !session.is_empty() {
                    *counts.entry(session).or_default() += 1.0;
                }
            }
            counts.into_values().collect()
        }
        path => events
            .iter()
            .flat_map(|spec| values(spec, path))
            .filter_map(|value| value.parse::<f64>().ok())
            .collect(),
    };
    let mut fields = vec![
        "trace_duration_ms".to_string(),
        "session_gap_ms".to_string(),
        "events_per_session".to_string(),
    ];
    fields.extend(numeric);
    if numbers.is_empty() {
        return json!({"field": field_name, "count": 0, "bins": [], "fields": fields});
    }
    let mut sorted = numbers.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let (min, max) = (sorted[0], sorted[sorted.len() - 1]);
    let width = ((max - min) / bins as f64).max(f64::EPSILON);
    let mut counts = vec![0u64; bins];
    for value in &numbers {
        let index = (((value - min) / width) as usize).min(bins - 1);
        counts[index] += 1;
    }
    let quantile = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    json!({
        "field": field_name,
        "count": numbers.len(),
        "min": min,
        "max": max,
        "mean": numbers.iter().sum::<f64>() / numbers.len() as f64,
        "p50": quantile(0.5),
        "p95": quantile(0.95),
        "bins": counts.iter().enumerate().map(|(index, count)| json!({"from": min + width * index as f64, "to": min + width * (index + 1) as f64, "count": count})).collect::<Vec<_>>(),
        "fields": fields,
    })
}

/** The texts events carry: the most frequent statements, decision reasons and rules, failure
 * states, end reasons, and notes, the most frequent words of the statements, and the newest log
 * lines, all under the query
 * Input
    - context: &Context - repository data
    - query: &Query - filter
 * Output
    - Value {total, groups: {name: [{value, count}]}, words: [{value, count}], lines: [{t, severity,
      name, text, id}]}
*/
pub(super) fn semantics(context: &Context, query: &Query) -> Value {
    const STOP: [&str; 24] = [
        "the", "a", "an", "of", "to", "and", "or", "is", "in", "on", "for", "was", "be", "by",
        "it", "its", "this", "that", "with", "as", "at", "no", "not", "from",
    ];
    let events = filtered(context, query, None);
    let groups = [
        ("statements", "response.response_statement"),
        ("reasons", "decision.decision_reasons"),
        ("rules", "decision.decision_rule"),
        ("failure_states", "response.failure_state"),
        ("assessments", "assessment.assessment"),
        ("end_reasons", "other.end_reason"),
        ("finding_codes", "other.finding_codes"),
        ("risk_codes", "other.risk_codes"),
    ];
    let mut out = serde_json::Map::new();
    for (name, path) in groups {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for spec in &events {
            for value in values(spec, path)
                .into_iter()
                .filter(|value| !value.is_empty() && value != "NA" && value != "NOT_REQUIRED")
            {
                *counts.entry(value).or_default() += 1;
            }
        }
        let mut ranked = counts.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.insert(
            name.into(),
            json!(ranked
                .into_iter()
                .take(30)
                .map(|(value, count)| json!({"value": value, "count": count, "path": path}))
                .collect::<Vec<_>>()),
        );
    }
    let mut words: BTreeMap<String, u64> = BTreeMap::new();
    for spec in &events {
        for word in field(spec, "response", "response_statement")
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .map(str::to_ascii_lowercase)
        {
            if word.len() > 2 && !STOP.contains(&word.as_str()) {
                *words.entry(word).or_default() += 1;
            }
        }
    }
    let mut ranked = words.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    json!({
        "total": events.len(),
        "groups": out,
        "words": ranked.into_iter().take(60).map(|(value, count)| json!({"value": value, "count": count})).collect::<Vec<_>>(),
        "lines": events.iter().rev().take(300).map(|spec| json!({
            "t": spec["occurred_at"],
            "severity": field(spec, "severity", "severity_status"),
            "name": spec["event_name"],
            "flow": spec["flow"],
            "text": field(spec, "response", "response_statement"),
            "reasons": spec["decision"]["decision_reasons"],
            "id": spec["event_id"],
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_trace_exposes_its_stable_group_key_as_call_id() {
        let span = json!({"action": {"action_tool": "BASH"}});
        let trace = Trace {
            id: "call-1".into(),
            spans: vec![&span],
        };
        assert_eq!(trace.tool_call_id(), Some("call-1"));
    }

    #[test]
    fn non_tool_trace_has_no_tool_call_id() {
        let span = json!({"action": {"action_type": "COMMAND"}});
        let trace = Trace {
            id: "command-1".into(),
            spans: vec![&span],
        };
        assert_eq!(trace.tool_call_id(), None);
    }
}
