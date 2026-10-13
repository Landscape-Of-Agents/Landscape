// Operating views: alerting (the rules members built with 'crane alert add', their state, every
// evaluation against its threshold, every firing, and every delivery with its result) and credits
// (each session's balance, the grants admins approved on the dashboard, and what each machine did
// with them).

use super::*;
use crate::telemetry::ledger::Ledger;
use std::collections::BTreeMap;

/** The alerting view, combined across the selected members' status reports and alerts
 * Input
    - context: &Context - repository data
    - kind: &str - PERSONAL or ORGANIZATION
 * Output
    - Value {rules: [{…rule, member, state, last_value, since}], evaluations, firings, deliveries,
      channels, events}
*/
pub(super) fn alerting(context: &Context, kind: &str) -> Value {
    let mut rules = Vec::new();
    let mut evaluations = Vec::new();
    let mut channels = Vec::new();
    for status in &context.statuses {
        let member = context.member(status["installation"].as_str().map(String::from).as_ref());
        let alerting = &status["alerting"];
        for rule in alerting["rules"].as_array().into_iter().flatten() {
            let name = rule["name"].as_str().unwrap_or_default();
            let state = &alerting["state"][name];
            let mut rule = rule.clone();
            rule["member"] = member.clone();
            rule["state"] = if rule["enabled"] == false {
                json!("DISABLED")
            } else {
                state["state"].clone()
            };
            rule["last_value"] = state["last_value"].clone();
            rule["since"] = state["since"].clone();
            rule["last_notified"] = state["last_notified"].clone();
            rules.push(rule);
        }
        for evaluation in alerting["history"].as_array().into_iter().flatten() {
            let mut evaluation = evaluation.clone();
            evaluation["member"] = member.clone();
            evaluations.push(evaluation);
        }
        for channel in alerting["channels"].as_array().into_iter().flatten() {
            let mut channel = channel.clone();
            channel["member"] = member.clone();
            channels.push(channel);
        }
    }
    evaluations.sort_by_key(|evaluation| evaluation["at"].as_u64().unwrap_or(0));
    let mut firings = Vec::new();
    let mut deliveries = Vec::new();
    for alert in context
        .snapshot
        .alerts
        .iter()
        .filter(|alert| alert["kind"] == "rule")
    {
        let member = context.member(alert["installation"].as_str().map(String::from).as_ref());
        let at = alert["at"].as_u64().unwrap_or(0) * 1000;
        firings.push(json!({
            "alert_id": alert["alert_id"],
            "at": at,
            "rule": alert["rule"]["name"],
            "state": alert["state"],
            "reminder": alert["reminder"],
            "severity": alert["severity"],
            "value": alert["value"],
            "threshold": alert["rule"]["threshold"],
            "op": alert["rule"]["op"],
            "metric": alert["rule"]["metric"],
            "title": alert["title"],
            "member": member,
            "deliveries": alert["deliveries"],
        }));
        for delivery in alert["deliveries"].as_array().into_iter().flatten() {
            let result = delivery["result"].as_str().unwrap_or_default();
            deliveries.push(json!({
                "at": at,
                "alert_id": alert["alert_id"],
                "rule": alert["rule"]["name"],
                "state": alert["state"],
                "channel": delivery["channel"],
                "result": result,
                "status": delivery["status"],
                "ms": delivery["ms"],
                "outcome": if result.starts_with("delivered") { "DELIVERED" } else if result.starts_with("dry") { "DRY_RUN" } else if result.starts_with("queued") { "QUEUED" } else { "FAILED" },
                "member": member,
            }));
        }
    }
    firings.sort_by_key(|firing| std::cmp::Reverse(firing["at"].as_u64().unwrap_or(0)));
    deliveries.sort_by_key(|delivery| std::cmp::Reverse(delivery["at"].as_u64().unwrap_or(0)));
    // The alert events of the stream (F14), for the event table and timeline
    let events = context
        .specs
        .iter()
        .filter(|spec| spec["flow"] == "F14")
        .rev()
        .take(MAX_LIMIT)
        .map(|spec| super::timeline::served(context, spec, kind))
        .collect::<Vec<_>>();
    json!({
        "rules": rules,
        "evaluations": evaluations,
        "firings": firings,
        "deliveries": deliveries,
        "channels": channels,
        "events": events,
        "reported": !context.statuses.is_empty(),
    })
}

/** The credits view: each session's balance and agent, the grants approved on the dashboard with
 * what their machine did, and the ledger's grant entries
 * Input
    - context: &Context - repository data
 * Output
    - Value {sessions: [{session, agent, member, active, balance}], grants, ledger}
*/
pub(super) fn grants(context: &Context) -> Value {
    let snapshot = &context.snapshot;
    let sessions = snapshot
        .sessions
        .iter()
        .map(|session| {
            json!({
                "session": session.foxx_session_id,
                "agent": session.agent_id,
                "provider": session.provider,
                "model": session.model,
                "member": context.member(context.session_member.get(&session.foxx_session_id)),
                "active": session.ended_at.is_none() && !context.stale(session),
                "last_seen_at": session.last_seen_at,
                "balance": Ledger::balance_of(&snapshot.ledger, &session.foxx_session_id),
            })
        })
        .collect::<Vec<_>>();
    let mut by_status: BTreeMap<String, u64> = BTreeMap::new();
    for grant in &context.grants {
        *by_status
            .entry(grant["status"].as_str().unwrap_or("PENDING").to_string())
            .or_default() += 1;
    }
    let granted = snapshot
        .ledger
        .iter()
        .filter(|entry| entry.reason.starts_with("granted on the dashboard"))
        .rev()
        .take(MAX_LIMIT)
        .collect::<Vec<_>>();
    json!({
        "sessions": sessions,
        "grants": context.grants.iter().map(|grant| {
            let mut grant = grant.clone();
            grant["member"] = context.member(grant["installation"].as_str().map(String::from).as_ref());
            grant
        }).collect::<Vec<_>>(),
        "by_status": by_status,
        "ledger": granted,
    })
}
