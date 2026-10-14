// The global filter every view shares (telemetry/query.rs: any attribute, any operator, combined
// freely), and the aggregations the dashboard's graphs draw: events over time grouped by any
// facet, the count of each facet value, and the catalog of every attribute with its values.

use super::*;
use crate::telemetry::query::{facet_path, values, Query, FACETS};
use std::collections::{BTreeMap, BTreeSet};

/** Most series a graph draws; the rest are summed as OTHER */
const MAX_SERIES: usize = 8;

/** Most values listed per facet or attribute */
const MAX_VALUES: usize = 25;

/** Attributes that identify one event or chain link; listed for exists/missing, never by value */
const IDENTIFIERS: [&str; 5] = [
    "event_id",
    "digest",
    "previous_digest",
    "sequence",
    "occurred_at",
];

/** The events of the repository that pass the query, oldest first
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - skip: Option<&str> - an attribute path whose clauses are ignored
 * Output
    - Vec<&Value>
*/
pub(super) fn filtered<'a>(
    context: &'a Context,
    query: &Query,
    skip: Option<&str>,
) -> Vec<&'a Value> {
    context
        .specs
        .iter()
        .filter(|spec| query.matches(spec, skip))
        .collect()
}

/** Choose a bucket size that gives at most about sixty buckets over a range
 * Input
    - span: u64 - range in milliseconds
 * Output
    - u64 bucket size in milliseconds
*/
pub(super) fn bucket_size(span: u64) -> u64 {
    const SECOND: u64 = 1000;
    [
        1, 5, 15, 30, 60, 300, 900, 1800, 3600, 10_800, 21_600, 43_200, 86_400, 259_200, 604_800,
    ]
    .into_iter()
    .map(|seconds| seconds * SECOND)
    .find(|size| span / size <= 60)
    .unwrap_or(2_592_000 * SECOND)
}

/** The time range a graph covers: the query's, or the events' own
 * Input
    - query: &Query - filter
    - events: &[&Value] - matching events
 * Output
    - (u64, u64) first and last Unix milliseconds
*/
pub(super) fn range(query: &Query, events: &[&Value]) -> (u64, u64) {
    let times = events
        .iter()
        .filter_map(|spec| spec["occurred_at"].as_u64());
    let (low, high) = times.fold((u64::MAX, 0), |(low, high), at| (low.min(at), high.max(at)));
    let first = query.since.unwrap_or(if low == u64::MAX {
        crate::platform::now_millis()
    } else {
        low
    });
    let last = query
        .until
        .unwrap_or_else(|| high.max(crate::platform::now_millis().min(first + 30 * 86_400_000)));
    (first, last.max(first + 1))
}

/** How a series turns the events of one bucket into one number
 * Variants
    - Count - how many events
    - Sum, Avg, Min, Max - of a numeric attribute
    - P50, P95, P99 - a percentile of a numeric attribute
    - Distinct - how many different values an attribute takes (sessions, agents, tools, ...)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Agg {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    P50,
    P95,
    P99,
    Distinct,
}

impl Agg {
    /** Read an aggregation name; without a metric only Count applies
     * Input
        - name: Option<&str> - count, sum, avg, min, max, p50, p95, p99, or distinct
        - metric: bool - a metric attribute was given
     * Output
        - Agg
    */
    pub(super) fn parse(name: Option<&str>, metric: bool) -> Self {
        if !metric {
            return Self::Count;
        }
        match name.unwrap_or("avg") {
            "count" => Self::Count,
            "sum" => Self::Sum,
            "min" => Self::Min,
            "max" => Self::Max,
            "p50" | "median" => Self::P50,
            "p95" => Self::P95,
            "p99" => Self::P99,
            "distinct" => Self::Distinct,
            _ => Self::Avg,
        }
    }

    /** The aggregation's name
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::Min => "min",
            Self::Max => "max",
            Self::P50 => "p50",
            Self::P95 => "p95",
            Self::P99 => "p99",
            Self::Distinct => "distinct",
        }
    }

    /** Reduce the samples of one bucket (or a whole series) to one number
     * Input
        - samples: &[String] - one item per event (count), the attribute's values (distinct), or
          its first numeric value (the others)
     * Output
        - Option<f64>, None when a numeric aggregation has nothing to aggregate
    */
    pub(super) fn reduce(self, samples: &[String]) -> Option<f64> {
        match self {
            Self::Count => Some(samples.len() as f64),
            Self::Distinct => Some(samples.iter().collect::<BTreeSet<_>>().len() as f64),
            _ => {
                let mut numbers = samples
                    .iter()
                    .filter_map(|sample| sample.parse::<f64>().ok())
                    .collect::<Vec<_>>();
                if numbers.is_empty() {
                    return None;
                }
                numbers.sort_by(|a, b| a.total_cmp(b));
                let at =
                    |share: f64| numbers[((numbers.len() - 1) as f64 * share).round() as usize];
                Some(match self {
                    Self::Sum => numbers.iter().sum(),
                    Self::Avg => numbers.iter().sum::<f64>() / numbers.len() as f64,
                    Self::Min => numbers[0],
                    Self::Max => numbers[numbers.len() - 1],
                    Self::P50 => at(0.5),
                    Self::P95 => at(0.95),
                    _ => at(0.99),
                })
            }
        }
    }
}

/** A number as JSON: whole numbers as integers, others rounded to three decimals, none as null
 * Input
    - value: Option<f64> - number
 * Output
    - Value
*/
fn number_json(value: Option<f64>) -> Value {
    match value {
        Some(value) if value.fract() == 0.0 && value.abs() < 9.0e15 => json!(value as i64),
        Some(value) => json!((value * 1000.0).round() / 1000.0),
        None => Value::Null,
    }
}

/** Events over time, per bucket, grouped by a facet or any attribute (the largest groups drawn on
 * their own, the rest as OTHER). Each bucket counts its events, or aggregates a metric: the sum,
 * average, smallest, largest, or a percentile of a numeric attribute, or how many different values
 * an attribute takes (for instance distinct sessions: how many were active)
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - by (facet or path), metric (attribute path), agg
 * Output
    - Value {by, metric, agg, bucket_ms, start, buckets, series: [{key, label, total, events, points}], total}
*/
pub(super) fn series(context: &Context, query: &Query, params: &BTreeMap<String, String>) -> Value {
    let events = filtered(context, query, None);
    let by = params
        .get("by")
        .map(String::as_str)
        .filter(|by| !by.is_empty());
    let path = by.map(|by| facet_path(by).unwrap_or(by).to_string());
    let metric = params
        .get("metric")
        .map(String::as_str)
        .filter(|metric| !metric.is_empty())
        .map(|metric| facet_path(metric).unwrap_or(metric).to_string());
    let agg = Agg::parse(params.get("agg").map(String::as_str), metric.is_some());
    let key_of = |spec: &Value| match &path {
        Some(path) => values(spec, path)
            .into_iter()
            .next()
            .unwrap_or_else(|| "NONE".into()),
        None => "events".into(),
    };
    // What one event adds to its bucket
    let samples_of = |spec: &Value| -> Vec<String> {
        match (&metric, agg) {
            (None, _) | (Some(_), Agg::Count) => vec![String::new()],
            (Some(metric), Agg::Distinct) => values(spec, metric)
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
            (Some(metric), _) => values(spec, metric)
                .into_iter()
                .find(|value| value.parse::<f64>().is_ok())
                .into_iter()
                .collect(),
        }
    };
    let (first, last) = range(query, &events);
    let size = bucket_size(last - first);
    let start = first / size * size;
    let buckets = ((last - start) / size + 1) as usize;
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    for spec in &events {
        *totals.entry(key_of(spec)).or_default() += 1;
    }
    let mut ranked = totals.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let kept = ranked
        .iter()
        .take(MAX_SERIES)
        .map(|(key, _)| key.clone())
        .collect::<BTreeSet<_>>();
    let mut samples: BTreeMap<String, (u64, Vec<Vec<String>>)> = BTreeMap::new();
    for spec in &events {
        let at = spec["occurred_at"].as_u64().unwrap_or(start).max(start);
        let index = (((at - start) / size) as usize).min(buckets - 1);
        let key = key_of(spec);
        let key = if kept.contains(&key) {
            key
        } else {
            "OTHER".to_string()
        };
        let entry = samples
            .entry(key)
            .or_insert_with(|| (0, vec![Vec::new(); buckets]));
        entry.0 += 1;
        entry.1[index].extend(samples_of(spec));
    }
    let mut keys = ranked
        .iter()
        .take(MAX_SERIES)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    if samples.contains_key("OTHER") && !keys.iter().any(|key| key == "OTHER") {
        keys.push("OTHER".into());
    }
    let list = keys
        .iter()
        .filter(|key| {
            // A metric's series without one value (its events lack the attribute) says nothing
            agg == Agg::Count
                || samples
                    .get(*key)
                    .is_some_and(|(_, buckets)| buckets.iter().any(|bucket| !bucket.is_empty()))
        })
        .filter_map(|key| {
            samples.get(key).map(|(count, buckets)| {
                let all = buckets.iter().flatten().cloned().collect::<Vec<_>>();
                json!({
                    "key": key,
                    "label": label(context, by, key),
                    "total": number_json(agg.reduce(&all)),
                    "events": count,
                    "points": buckets.iter().map(|bucket| number_json(agg.reduce(bucket))).collect::<Vec<_>>(),
                })
            })
        })
        .collect::<Vec<_>>();
    json!({"by": by, "metric": metric, "agg": agg.name(), "bucket_ms": size, "start": start, "buckets": buckets, "series": list, "total": events.len()})
}

/** How a value is shown: members by their Git user name, everything else as it is
 * Input
    - context: &Context - repository data
    - by: Option<&str> - facet or attribute
    - key: &str - value
 * Output
    - String
*/
pub(super) fn label(context: &Context, by: Option<&str>, key: &str) -> String {
    if matches!(by, Some("member" | "member_key"))
        && !key.is_empty()
        && key != "OTHER"
        && key != "NONE"
    {
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

/** The count of every value of every facet among the events that pass the query; each facet
 * ignores its own clauses, so its other values stay visible for choosing
 * Input
    - context: &Context - repository data
    - query: &Query - filter
 * Output
    - Value {total, facets: {name: [{value, label, count}]}}
*/
pub(super) fn facets(context: &Context, query: &Query) -> Value {
    let mut all = serde_json::Map::new();
    for (name, path) in FACETS {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for spec in context
            .specs
            .iter()
            .filter(|spec| query.matches(spec, Some(path)))
        {
            for value in values(spec, path)
                .into_iter()
                .filter(|value| !value.is_empty())
            {
                *counts.entry(value).or_default() += 1;
            }
        }
        let mut ranked = counts.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        all.insert(
            name.into(),
            json!(ranked
                .into_iter()
                .take(MAX_VALUES)
                .map(|(value, count)| json!({"value": &value, "label": label(context, Some(name), &value), "count": count}))
                .collect::<Vec<_>>()),
        );
    }
    json!({"total": filtered(context, query, None).len(), "facets": all})
}

/** The catalog of every attribute the matching events carry: its path, kind (text, number,
 * boolean, list), how many events carry it, and its most frequent values (numbers also give their
 * smallest and largest). The dashboard's filter builder offers every one of them.
 * Input
    - context: &Context - repository data
    - query: &Query - filter
 * Output
    - Value {total, attributes: [{path, group, kind, present, distinct, values: [{value, count}], min, max}]}
*/
pub(super) fn attributes(context: &Context, query: &Query) -> Value {
    #[derive(Default)]
    struct Seen {
        kind: &'static str,
        present: u64,
        counts: BTreeMap<String, u64>,
        min: Option<f64>,
        max: Option<f64>,
    }
    let events = filtered(context, query, None);
    let mut seen: BTreeMap<String, Seen> = BTreeMap::new();
    let mut visit = |path: String, value: &Value| {
        let entry = seen.entry(path).or_default();
        let (kind, items): (&'static str, Vec<String>) = match value {
            Value::String(text) => ("text", vec![text.clone()]),
            Value::Number(number) => {
                let float = number.as_f64().unwrap_or(0.0);
                entry.min = Some(entry.min.map_or(float, |min| min.min(float)));
                entry.max = Some(entry.max.map_or(float, |max| max.max(float)));
                ("number", vec![number.to_string()])
            }
            Value::Bool(flag) => ("boolean", vec![flag.to_string()]),
            Value::Array(list) => (
                "list",
                list.iter()
                    .map(|item| item.as_str().map_or_else(|| item.to_string(), String::from))
                    .collect(),
            ),
            _ => return,
        };
        if entry.kind.is_empty() || entry.kind == "text" {
            entry.kind = kind;
        }
        entry.present += 1;
        if entry.counts.len() < 2_000 {
            for item in items {
                *entry.counts.entry(item).or_default() += 1;
            }
        }
    };
    for spec in &events {
        let Some(object) = spec.as_object() else {
            continue;
        };
        for (key, value) in object {
            match value {
                Value::Object(group) => {
                    for (inner, value) in group {
                        if !value.is_object() {
                            visit(format!("{key}.{inner}"), value);
                        }
                    }
                }
                Value::Null => {}
                other if key != "member" => visit(key.clone(), other),
                _ => {}
            }
        }
    }
    let list = seen
        .into_iter()
        .map(|(path, seen)| {
            let mut ranked = seen.counts.into_iter().collect::<Vec<_>>();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let identifier = IDENTIFIERS.contains(&path.as_str()) || path.ends_with("_digest") || path == "other.signature";
            json!({
                "path": path,
                "group": path.split_once('.').map_or("envelope", |(group, _)| group),
                "kind": seen.kind,
                "present": seen.present,
                "distinct": ranked.len(),
                "values": if identifier { Vec::new() } else { ranked.into_iter().take(MAX_VALUES).map(|(value, count)| json!({"value": value, "label": label(context, Some(&path), &value), "count": count})).collect::<Vec<_>>() },
                "min": seen.min,
                "max": seen.max,
            })
        })
        .collect::<Vec<_>>();
    json!({"total": events.len(), "attributes": list})
}
