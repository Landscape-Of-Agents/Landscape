// The global filter every view shares, and the aggregations the dashboard's graphs draw: events
// over time grouped by any facet, and the count of each facet value. A filter names facets
// (severity, flow, decision, ...) with one or more comma-separated values, plus a time range, a
// session or task, a policy or selection, and free text.

use super::*;
use crate::telemetry::evals::field;
use std::collections::BTreeMap;

/** The facets an event can be filtered and grouped by, in the order the dashboard lists them */
pub(super) const FACETS: [&str; 12] = [
    "severity",
    "flow",
    "decision",
    "enforcement",
    "alert",
    "origin",
    "agent",
    "source",
    "event",
    "response",
    "tool",
    "member",
];

/** Most series a graph draws; the rest are summed as OTHER */
const MAX_SERIES: usize = 8;

/** Most values listed per facet */
const MAX_VALUES: usize = 20;

/** Read a facet of an event
 * Input
    - spec: &Value - event
    - facet: &str - one of FACETS
 * Output
    - &str (empty when the event has none)
*/
pub(super) fn facet<'a>(spec: &'a Value, facet: &str) -> &'a str {
    match facet {
        "severity" => field(spec, "severity", "severity_status"),
        "flow" => spec["flow"].as_str().unwrap_or_default(),
        "decision" => field(spec, "decision", "decision"),
        "enforcement" => field(spec, "decision", "enforcement"),
        "alert" => field(spec, "severity", "alert_kind"),
        "origin" => spec["origin"].as_str().unwrap_or_default(),
        "agent" => field(spec, "agent", "agent_type"),
        "source" => field(spec, "action", "action_source"),
        "event" => spec["event_name"].as_str().unwrap_or_default(),
        "response" => field(spec, "response", "response_status"),
        "tool" => field(spec, "action", "action_tool"),
        "member" => spec["member_key"].as_str().unwrap_or_default(),
        _ => "",
    }
}

/** Report whether an event passes the filter
 * Input
    - spec: &Value - event
    - query: &BTreeMap<String, String> - facets (comma-separated values), since, until, name (event
      name prefix), session, task, policy, selection, q (free text)
    - skip: Option<&str> - a facet to ignore (so a facet's own counts show its other values)
 * Output
    - bool
*/
pub(super) fn matches(spec: &Value, query: &BTreeMap<String, String>, skip: Option<&str>) -> bool {
    let at = spec["occurred_at"].as_u64().unwrap_or(0);
    if time_filter(query, "since").is_some_and(|since| at < since)
        || time_filter(query, "until").is_some_and(|until| at > until)
    {
        return false;
    }
    for name in FACETS {
        if Some(name) == skip {
            continue;
        }
        if let Some(wanted) = query.get(name).filter(|wanted| !wanted.is_empty()) {
            let value = facet(spec, name);
            if !wanted
                .split(',')
                .any(|item| item.trim().eq_ignore_ascii_case(value))
            {
                return false;
            }
        }
    }
    let listed = |group: &str, key: &str, wanted: &str| {
        spec[group][key]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(wanted)))
    };
    let name = spec["event_name"].as_str().unwrap_or_default();
    query
        .get("name")
        .or_else(|| query.get("type"))
        .is_none_or(|prefix| name.starts_with(&prefix.to_ascii_uppercase()))
        && query
            .get("session")
            .is_none_or(|session| field(spec, "session", "session_id") == session)
        && query
            .get("task")
            .is_none_or(|task| field(spec, "session", "task_id") == task)
        && query.get("policy").is_none_or(|policy| {
            listed("other", "policies", policy) || spec["other"]["policy"].as_str() == Some(policy)
        })
        && query.get("selection").is_none_or(|selection| {
            listed("other", "selections", selection)
                || listed("resource", "resource_ids", selection)
                || spec["other"]["selection_id"].as_str() == Some(selection)
        })
        && query
            .get("q")
            .map(|text| text.trim().to_ascii_lowercase())
            .filter(|text| !text.is_empty())
            .is_none_or(|text| spec.to_string().to_ascii_lowercase().contains(&text))
}

/** The events of the repository that pass the filter, oldest first
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - filter
    - skip: Option<&str> - a facet to ignore
 * Output
    - Vec<&Value>
*/
pub(super) fn filtered<'a>(
    context: &'a Context,
    query: &BTreeMap<String, String>,
    skip: Option<&str>,
) -> Vec<&'a Value> {
    context
        .specs
        .iter()
        .filter(|spec| matches(spec, query, skip))
        .collect()
}

/** Choose a bucket size that gives about fifty buckets over a range
 * Input
    - span: u64 - range in milliseconds
 * Output
    - u64 bucket size in milliseconds
*/
fn bucket_size(span: u64) -> u64 {
    const MINUTE: u64 = 60_000;
    [1, 5, 15, 30, 60, 180, 360, 720, 1440, 4320, 10_080]
        .into_iter()
        .map(|minutes| minutes * MINUTE)
        .find(|size| span / size <= 60)
        .unwrap_or(30 * 1440 * MINUTE)
}

/** Events over time, counted per bucket and grouped by a facet (the largest groups drawn on
 * their own, the rest as OTHER)
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - filter, plus by (a facet; none for one series)
 * Output
    - Value {bucket_ms, start, buckets, series: [{key, label, total, points}], total}
*/
pub(super) fn series(context: &Context, query: &BTreeMap<String, String>) -> Value {
    let events = filtered(context, query, None);
    let by = query
        .get("by")
        .map(String::as_str)
        .filter(|by| FACETS.contains(by));
    let times = events
        .iter()
        .filter_map(|spec| spec["occurred_at"].as_u64())
        .collect::<Vec<_>>();
    let first = time_filter(query, "since")
        .filter(|since| *since > 0)
        .or_else(|| times.iter().min().copied())
        .unwrap_or_else(crate::platform::now_millis);
    let last = time_filter(query, "until")
        .filter(|until| *until < u64::MAX)
        .unwrap_or_else(|| {
            times
                .iter()
                .max()
                .copied()
                .unwrap_or(first)
                .max(crate::platform::now_millis().min(first + 30 * 86_400_000))
        })
        .max(first + 1);
    let size = bucket_size(last - first);
    let start = first / size * size;
    let buckets = ((last - start) / size + 1) as usize;
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    for spec in &events {
        let key = by.map_or("events", |by| facet(spec, by));
        *totals.entry(key.to_string()).or_default() += 1;
    }
    let mut ranked = totals.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let kept = ranked
        .iter()
        .take(MAX_SERIES)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    let mut points: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for spec in &events {
        let at = spec["occurred_at"].as_u64().unwrap_or(start).max(start);
        let index = (((at - start) / size) as usize).min(buckets - 1);
        let key = by.map_or("events", |by| facet(spec, by));
        let key = if kept.iter().any(|kept| kept == key) {
            key.to_string()
        } else {
            "OTHER".to_string()
        };
        points.entry(key).or_insert_with(|| vec![0; buckets])[index] += 1;
    }
    let mut keys = kept.clone();
    if points.contains_key("OTHER") && !keys.iter().any(|key| key == "OTHER") {
        keys.push("OTHER".into());
    }
    let list = keys
        .iter()
        .filter_map(|key| {
            points.get(key).map(|points| {
                json!({
                    "key": key,
                    "label": label(context, by, key),
                    "total": points.iter().sum::<u64>(),
                    "points": points,
                })
            })
        })
        .collect::<Vec<_>>();
    json!({
        "by": by,
        "bucket_ms": size,
        "start": start,
        "buckets": buckets,
        "series": list,
        "total": events.len(),
    })
}

/** How a facet value is shown: members by their Git user name, everything else as it is
 * Input
    - context: &Context - repository data
    - by: Option<&str> - facet
    - key: &str - value
 * Output
    - String
*/
fn label(context: &Context, by: Option<&str>, key: &str) -> String {
    if by == Some("member") && !key.is_empty() && key != "OTHER" {
        let member = context.member(Some(&key.to_string()));
        return format!(
            "{} ({})",
            member["actor"].as_str().unwrap_or("unknown"),
            member["short"].as_str().unwrap_or_default()
        );
    }
    if key.is_empty() {
        "NONE".into()
    } else {
        key.into()
    }
}

/** The count of every value of every facet among the events that pass the filter; each facet
 * ignores its own filter, so its other values stay visible for choosing
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - filter
 * Output
    - Value {total, facets: {name: [{value, label, count}]}}
*/
pub(super) fn facets(context: &Context, query: &BTreeMap<String, String>) -> Value {
    let mut all = serde_json::Map::new();
    for name in FACETS {
        let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
        for spec in context
            .specs
            .iter()
            .filter(|spec| matches(spec, query, Some(name)))
        {
            let value = facet(spec, name);
            if !value.is_empty() {
                *counts.entry(value).or_default() += 1;
            }
        }
        let mut ranked = counts.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        all.insert(
            name.into(),
            json!(ranked
                .into_iter()
                .take(MAX_VALUES)
                .map(|(value, count)| json!({"value": value, "label": label(context, Some(name), value), "count": count}))
                .collect::<Vec<_>>()),
        );
    }
    json!({"total": filtered(context, query, None).len(), "facets": all})
}
