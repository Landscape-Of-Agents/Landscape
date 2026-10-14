// Aggregations the dashboard draws as heatmaps, under the global query: a matrix counting the
// events of every pair of values of two attributes (agent by tool, member by agent, control flow
// by severity, ...), and a calendar of when events happen (day of week by hour, and per day).

use super::filter::{filtered, label};
use super::*;
use crate::telemetry::evals::field;
use crate::telemetry::query::{facet_path, values, Query};
use std::collections::BTreeMap;

/** Most rows or columns a matrix returns; the rest are summed as OTHER */
const MAX_AXIS: usize = 40;

/** The values of an attribute on one axis of a matrix: every non-empty value (each item of a
 * list), or NONE
 * Input
    - spec: &Value - event
    - path: &str - attribute path
 * Output
    - Vec<String>
*/
fn axis_values(spec: &Value, path: &str) -> Vec<String> {
    let found = values(spec, path)
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if found.is_empty() {
        vec!["NONE".into()]
    } else {
        found
    }
}

/** Keep the largest values of an axis and fold the others into OTHER
 * Input
    - totals: &BTreeMap<String, u64> - events per value
    - limit: usize - most values kept
 * Output
    - (Vec<String>, bool) kept values (largest first) and whether any were folded
*/
fn top(totals: &BTreeMap<String, u64>, limit: usize) -> (Vec<String>, bool) {
    let mut ranked = totals.iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let kept = ranked
        .iter()
        .take(limit)
        .map(|(key, _)| (*key).clone())
        .collect::<Vec<_>>();
    (kept, ranked.len() > limit)
}

/** A matrix of event counts by two attributes: each cell counts the events that carry both
 * values (a list attribute counts once per item), with how many of them were denied
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - rows and cols (facet names or attribute paths; agent and
      tool by default), limit (most rows and columns, 12 by default), hide_none (1 to leave out
      events without a value)
 * Output
    - Value {rows_by, cols_by, rows: [{key, label, total}], cols: [...], cells: [[count]],
      denied: [[count]], max, total}
*/
pub(super) fn matrix(context: &Context, query: &Query, params: &BTreeMap<String, String>) -> Value {
    let pick = |key: &str, default: &'static str| {
        params
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(default)
            .to_string()
    };
    let (rows_by, cols_by) = (pick("rows", "agent"), pick("cols", "tool"));
    let (row_path, col_path) = (
        facet_path(&rows_by).unwrap_or(&rows_by).to_string(),
        facet_path(&cols_by).unwrap_or(&cols_by).to_string(),
    );
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(12)
        .clamp(1, MAX_AXIS);
    let hide_none = params.get("hide_none").is_some_and(|value| value == "1");
    let events = filtered(context, query, None);
    let mut cells: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    let (mut row_totals, mut col_totals) = (BTreeMap::new(), BTreeMap::new());
    for spec in &events {
        let denied = u64::from(matches!(
            field(spec, "decision", "decision"),
            "DENY" | "QUARANTINE" | "ERROR"
        ));
        for row in axis_values(spec, &row_path) {
            for col in axis_values(spec, &col_path) {
                if hide_none && (row == "NONE" || col == "NONE") {
                    continue;
                }
                let cell = cells.entry((row.clone(), col.clone())).or_default();
                cell.0 += 1;
                cell.1 += denied;
                *row_totals.entry(row.clone()).or_insert(0u64) += 1;
                *col_totals.entry(col).or_insert(0u64) += 1;
            }
        }
    }
    let (mut rows, rows_folded) = top(&row_totals, limit);
    let (mut cols, cols_folded) = top(&col_totals, limit);
    if rows_folded {
        rows.push("OTHER".into());
    }
    if cols_folded {
        cols.push("OTHER".into());
    }
    let index = |list: &[String], key: &str| {
        list.iter()
            .position(|item| item == key)
            .unwrap_or(list.len() - 1)
    };
    let mut counts = vec![vec![0u64; cols.len()]; rows.len()];
    let mut denied = vec![vec![0u64; cols.len()]; rows.len()];
    for ((row, col), (count, refused)) in &cells {
        let (r, c) = (index(&rows, row), index(&cols, col));
        counts[r][c] += count;
        denied[r][c] += refused;
    }
    let axis = |list: &[String], by: &str| {
        list.iter()
            .enumerate()
            .map(|(position, key)| {
                let total = if key == "OTHER" {
                    if by == "rows" {
                        counts[position].iter().sum::<u64>()
                    } else {
                        counts.iter().map(|row| row[position]).sum::<u64>()
                    }
                } else if by == "rows" {
                    row_totals[key]
                } else {
                    col_totals[key]
                };
                let name = if by == "rows" { &rows_by } else { &cols_by };
                json!({"key": key, "label": label(context, Some(name), key), "total": total})
            })
            .collect::<Vec<_>>()
    };
    json!({
        "rows_by": rows_by,
        "cols_by": cols_by,
        "rows": axis(&rows, "rows"),
        "cols": axis(&cols, "cols"),
        "max": counts.iter().flatten().copied().max().unwrap_or(0),
        "cells": counts,
        "denied": denied,
        "total": events.len(),
    })
}

/** When events happen: counts by day of the week and hour of the day, and per day, in the
 * viewer's time zone
 * Input
    - context: &Context - repository data
    - query: &Query - filter
    - params: &BTreeMap<String, String> - tz_offset: minutes to add to local time to get UTC (as a
      browser reports it), 0 by default
 * Output
    - Value {tz_offset, week: [[count; 24]; 7] (0 is Sunday), days: [{day, count}] (day is the
      UTC milliseconds of local midnight), max_hour, max_day, total}
*/
pub(super) fn calendar(
    context: &Context,
    query: &Query,
    params: &BTreeMap<String, String>,
) -> Value {
    const DAY: i64 = 86_400_000;
    let offset = params
        .get("tz_offset")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .clamp(-840, 840)
        * 60_000;
    let mut week = [[0u64; 24]; 7];
    let mut days: BTreeMap<i64, u64> = BTreeMap::new();
    let events = filtered(context, query, None);
    for spec in &events {
        let Some(at) = spec["occurred_at"].as_i64() else {
            continue;
        };
        let local = at - offset;
        let day = local.div_euclid(DAY);
        // 1 January 1970 was a Thursday (4)
        let weekday = (day + 4).rem_euclid(7) as usize;
        let hour = (local.rem_euclid(DAY) / 3_600_000) as usize;
        week[weekday][hour] += 1;
        *days.entry(day * DAY + offset).or_default() += 1;
    }
    json!({
        "tz_offset": offset / 60_000,
        "week": week,
        "days": days.iter().map(|(day, count)| json!({"day": day, "count": count})).collect::<Vec<_>>(),
        "max_hour": week.iter().flatten().copied().max().unwrap_or(0),
        "max_day": days.values().copied().max().unwrap_or(0),
        "total": events.len(),
    })
}
