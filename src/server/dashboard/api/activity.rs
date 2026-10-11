// Activity views: health (versions, per-member chains, sizes), the overview (activity, outcomes,
// integrity, recent violations and alerts), the members of a repository, and alerts.

use super::*;
use crate::foxx::views::metric_map;
use crate::platform::now_millis;
use crate::platform::render::iso_time;
use crate::server::dashboard::data::Context;
use crate::telemetry::metrics::{self};
use crate::telemetry::model::Enforcement;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/** Health: versions, chain status per member, store sizes, and the members' status reports
 * Input
    - context: &Context - repository data
 * Output
    - Value
*/
pub(super) fn health(context: &Context) -> Value {
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
pub(super) fn overview(context: &Context, query: &BTreeMap<String, String>) -> Value {
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
pub(super) fn members(context: &Context) -> Value {
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

/** Alerts, newest first, with their member
 * Input
    - context: &Context - repository data
 * Output
    - Value {alerts}
*/
pub(super) fn alerts(context: &Context) -> Value {
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
