// Activity views: health (versions, per-member chains, sizes), the overview (activity, evals,
// integrity, recent violations and alerts), the members of a repository, and alerts.

use super::*;
use crate::platform::now_millis;
use crate::platform::render::iso_time;
use crate::server::dashboard::data::Context;
use crate::telemetry::evals::summarize;
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
        "spec_version": crate::telemetry::spec::SPEC_VERSION,
        "spec_events": context.specs.len(),
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
        "storage": {"service": "DataSea service (MongoDB, read-only for the dashboard)"},
        "mode": "hosted read-only",
        "now": iso_time(now_millis()),
    })
}

/** The overview page: activity, evals, integrity, coverage, and recent alerts and violations
 * Input
    - context: &Context - repository data
    - filter: &Query - the global filter
    - kind: &str - PERSONAL or ORGANIZATION
 * Output
    - Value
*/
pub(super) fn overview(
    context: &Context,
    filter: &crate::telemetry::query::Query,
    kind: &str,
) -> Value {
    let snapshot = &context.snapshot;
    let specs = super::filter::filtered(context, filter, None);
    json!({
        "active_sessions": snapshot.sessions.iter().filter(|session| session.ended_at.is_none() && !context.stale(session)).count(),
        "stale_sessions": snapshot.sessions.iter().filter(|session| context.stale(session)).count(),
        "sessions": snapshot.sessions.len(),
        "tasks": snapshot.tasks.len(),
        "tasks_by_status": snapshot.tasks.iter().fold(BTreeMap::<String, usize>::new(), |mut counts, task| { *counts.entry(task.status.clone()).or_default() += 1; counts }),
        "agents": snapshot.sessions.iter().map(|session| session.agent_id.clone()).collect::<BTreeSet<_>>().len(),
        "members": context.selected.len(),
        "evals": summarize(&specs),
        "integrity": integrity(context),
        "event_chain": chains(context).0,
        "recent_alerts": alerts(context)["alerts"].as_array().map(|alerts| alerts.iter().take(10).cloned().collect::<Vec<_>>()).unwrap_or_default(),
        "recent_violations": specs.iter().rev().filter(|spec| matches!(spec["event_name"].as_str(), Some("BYPASS_CONFIRMED" | "VIOLATION_AFTER_EFFECT"))).take(10).map(|spec| served(context, spec, kind)).collect::<Vec<_>>(),
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
                "linked_by": context.links.get(&key),
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

/** Alerts, newest first, with their member; stale sessions are added as computed alerts (the
 * dashboard cannot store anything, so they are derived on every read)
 * Input
    - context: &Context - repository data
 * Output
    - Value {alerts}
*/
pub(super) fn alerts(context: &Context) -> Value {
    let stale = context
        .snapshot
        .sessions
        .iter()
        .filter(|session| context.stale(session))
        .map(|session| {
            json!({
                "at": session.last_seen_at / 1000,
                "kind": "stale_session",
                "title": "Agent session stopped reporting",
                "severity": "HIGH",
                "session": session.foxx_session_id,
                "state": "STALE",
                "computed": true,
                "suppressed": false,
                "deliveries": [],
                "installation": context.session_member.get(&session.foxx_session_id),
                "detail": format!("no hook activity since {} and no signed end of session: the agent may have been stopped, its hooks removed, or its evidence is not reaching the service", iso_time(session.last_seen_at)),
            })
        });
    let mut all = context
        .snapshot
        .alerts
        .iter()
        .cloned()
        .chain(stale)
        .collect::<Vec<_>>();
    all.sort_by_key(|alert| std::cmp::Reverse(alert["at"].as_u64().unwrap_or(0)));
    let list = all
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

/** How long the repository's records live: the retention period, and for each kind of record
 * that expires how many are held, the oldest and newest, how many are stale (older than the
 * period less 7 days: deleted within a week) and how many are deleted within a day
 * Input
    - context: &Context - repository data
 * Output
    - Value {mode (EXPIRES or PERSIST), days, stale_after_days, now, collections: [{name, label,
      documents, oldest, newest, stale, expiring_24h}], kept: [name]}
*/
pub(super) fn retention(context: &Context) -> Value {
    use crate::server::database::{retention_from_environment, DAY_MS, STALE_WINDOW_DAYS};
    let retention = retention_from_environment();
    let days = retention.map(|retention| retention as f64 / DAY_MS as f64);
    let now = now_millis();
    let summary = |name: &str, label: &str, times: Vec<u64>| {
        // Records within this many days of their deletion
        let older = |within_days: i64| match retention {
            Some(retention) => {
                let limit = (retention - within_days * DAY_MS).max(0) as u64;
                times
                    .iter()
                    .filter(|at| now.saturating_sub(**at) > limit)
                    .count()
            }
            None => 0,
        };
        json!({
            "name": name,
            "label": label,
            "documents": times.len(),
            "oldest": times.iter().min(),
            "newest": times.iter().max(),
            "stale": older(STALE_WINDOW_DAYS),
            "expiring_24h": older(1),
        })
    };
    let snapshot = &context.snapshot;
    let service = context
        .specs
        .iter()
        .filter(|spec| spec["origin"] == "SERVICE")
        .filter_map(|spec| spec["occurred_at"].as_u64())
        .collect();
    json!({
        "mode": if days.is_some() { "EXPIRES" } else { "PERSIST" },
        "days": days,
        "stale_after_days": days.map(|days| (days - STALE_WINDOW_DAYS as f64).max(0.0)),
        "now": now,
        "collections": [
            summary("events", "Machine events", snapshot.events.iter().map(|event| event.occurred_at).collect()),
            summary("service_events", "Service events", service),
            summary("alerts", "Alerts", snapshot.alerts.iter().filter_map(|alert| alert["at"].as_u64()).map(|at| at * 1000).collect()),
            summary("sessions", "Sessions (by last activity)", snapshot.sessions.iter().map(|session| session.last_seen_at).collect()),
            summary("tasks", "Tasks", snapshot.tasks.iter().map(|task| task.ended_at.unwrap_or(task.started_at)).collect()),
        ],
        "kept": ["repositories", "chain_heads", "autonomy_ledger", "selection_registry", "policy_versions", "governance_changes", "repository_status", "credit_grants", "member_links", "owner_keys", "dashboard_agents", "dashboard_incidents", "dashboard_quarantines", "dashboard_flow_context"],
    })
}
