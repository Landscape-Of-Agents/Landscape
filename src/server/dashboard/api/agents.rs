use serde_json::{json, Value};
use std::collections::BTreeSet;

use crate::server::dashboard::data::Context;
use crate::telemetry::events::Event;
use crate::telemetry::model::Execution;

/** Read-only view for the Dashboard Agents page. */
pub(super) fn view(context: &Context) -> Value {
    let agents = context.dashboard_agents.clone();
    let mut incidents = context.dashboard_incidents.clone();
    for incident in &mut incidents {
        let incident_id = incident["incident_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if let Some(entries) = incident["three_a"].as_array_mut() {
            for action in context.dashboard_agent_activity.iter().filter(|activity| {
                activity["details"]["incident_id"].as_str() == Some(incident_id.as_str())
            }) {
                let details = &action["details"];
                let description = match action["action"].as_str().unwrap_or_default() {
                    "quarantine_requested" => format!(
                        "Requested {} quarantine for {}.",
                        details["kind"].as_str().unwrap_or("target"),
                        details["target_id"].as_str().unwrap_or("unknown")
                    ),
                    "quarantine_activated" => "Repository admin activated the quarantine.".into(),
                    "quarantine_rejected" => {
                        "Repository admin rejected the quarantine request.".into()
                    }
                    "quarantine_release_requested" => "Requested release of the quarantine.".into(),
                    "quarantine_released" => "Repository admin released the quarantine.".into(),
                    "quarantine_release_rejected" => {
                        "Repository admin rejected the release; the quarantine remains active."
                            .into()
                    }
                    "flow_context_proposed" => format!(
                        "Proposed this incident context for flow {}.",
                        details["flow_id"].as_str().unwrap_or("unknown")
                    ),
                    "flow_context_approved" => {
                        "Repository admin approved the incident context attachment.".into()
                    }
                    "flow_context_rejected" => {
                        "Repository admin rejected the incident context attachment.".into()
                    }
                    _ => continue,
                };
                entries.push(json!({
                    "at": action["at"],
                    "kind": action["action"],
                    "actor": action["actor"],
                    "text": description,
                }));
            }
            entries.sort_by_key(|entry| entry["at"].as_u64().unwrap_or_default());
        }
        let event_ids = incident["event_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let mut subjects = BTreeSet::new();
        for event in context
            .snapshot
            .events
            .iter()
            .filter(|event| event_ids.contains(event.event_id.as_str()))
        {
            subjects.insert((
                event.scope.agent_id.clone().unwrap_or_default(),
                event.scope.foxx_session_id.clone().unwrap_or_default(),
                event.payload["tool"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
        incident["subjects"] = json!(subjects
            .into_iter()
            .filter(|(agent, session, tool)| !agent.is_empty()
                || !session.is_empty()
                || !tool.is_empty())
            .take(20)
            .map(|(agent_id, session_id, tool)| json!({
                "agent_id": agent_id,
                "session_id": session_id,
                "tool": tool,
            }))
            .collect::<Vec<_>>());
    }
    let quarantines = context.dashboard_quarantines.clone();
    let flow_context = context.dashboard_flow_context.clone();
    let now = crate::platform::now_millis();
    let metrics = context
        .dashboard_agents
        .iter()
        .map(|agent| {
            json!({
                "agent_id": agent["agent_id"],
                "name": agent["name"],
                "metric": agent["metric"],
                "observed": observed(agent, &context.snapshot.events, now),
                "threshold": agent["threshold"],
                "state": agent["state"],
                "metrics": agent["metrics"],
            })
        })
        .collect::<Vec<_>>();
    json!({
        "agents": agents,
        "incidents": incidents,
        "quarantines": quarantines,
        "flow_context": flow_context,
        "activity": context.dashboard_agent_activity,
        "metrics": metrics,
        "summary": {
            "total": context.dashboard_agents.len(),
            "active": context.dashboard_agents.iter().filter(|agent| agent["state"] == "active").count(),
            "idle": context.dashboard_agents.iter().filter(|agent| agent["state"] == "idle").count(),
            "sleeping": context.dashboard_agents.iter().filter(|agent| agent["state"] == "sleep").count(),
            "open_incidents": context.dashboard_incidents.iter().filter(|incident| incident["status"] == "OPEN").count(),
            "resolved_incidents": context.dashboard_incidents.iter().filter(|incident| incident["status"] == "RESOLVED").count(),
            "active_quarantines": context.dashboard_quarantines.iter().filter(|quarantine| quarantine["state"] == "ACTIVE").count(),
            "pending_quarantines": context.dashboard_quarantines.iter().filter(|quarantine| quarantine["state"] == "PENDING").count(),
        }
    })
}

/** Return threshold incidents detected from the selected repository's current event evidence. */
pub(crate) fn evaluate(context: &Context, now: u64) -> Vec<Value> {
    context
        .dashboard_agents
        .iter()
        .filter(|agent| matches!(agent["state"].as_str(), Some("active" | "idle")))
        .filter_map(|agent| candidate(agent, &context.snapshot.events, now))
        .collect()
}

/** Per-agent observations recorded by every background or manual evaluation. */
pub(crate) fn measurements(context: &Context, now: u64) -> Vec<Value> {
    context
        .dashboard_agents
        .iter()
        .filter(|agent| matches!(agent["state"].as_str(), Some("active" | "idle")))
        .map(|agent| {
            let observed = observed(agent, &context.snapshot.events, now);
            let threshold = agent["threshold"].as_u64().unwrap_or_default();
            json!({
                "agent_id": agent["agent_id"],
                "observed": observed,
                "threshold": threshold,
                "triggered": observed >= threshold,
            })
        })
        .collect()
}

fn candidate(agent: &Value, events: &[Event], now: u64) -> Option<Value> {
    if !matches!(agent["state"].as_str(), Some("active" | "idle")) {
        return None;
    }
    let window_minutes = agent["window_minutes"].as_u64()?;
    let threshold = agent["threshold"].as_u64()?;
    let window_ms = window_minutes.saturating_mul(60_000);
    let start = now.saturating_sub(window_ms);
    let metric = agent["metric"].as_str()?;
    let observed = observed(agent, events, now);
    if observed < threshold {
        return None;
    }
    let assignments = agent["assignments"].as_array()?;
    let idle = agent["state"] == "idle";
    let matching = events
        .iter()
        .filter(|event| event.occurred_at >= start && event.occurred_at <= now)
        .filter(|event| {
            assignments
                .iter()
                .any(|assignment| assignment_matches(event, assignment))
        })
        .filter(|event| {
            !idle || {
                let severity = spec(event)["severity"]["severity_status"]
                    .as_str()
                    .unwrap_or("");
                matches!(severity, "CRITICAL" | "HIGH")
            }
        })
        .filter(|event| metric_matches(event, metric))
        .collect::<Vec<_>>();
    let bucket = now / window_ms.max(1);
    let agent_id = agent["agent_id"].as_str()?;
    let name = agent["name"].as_str().unwrap_or("Dashboard agent");
    let metric_label = metric.replace('_', " ");
    let reason = format!(
        "{name} observed {observed} {metric_label} events in the last {window_minutes} minute(s), meeting its threshold of {threshold}."
    );
    let assignment_ids = assignments
        .iter()
        .map(|assignment| {
            format!(
                "{}:{}",
                assignment["kind"].as_str().unwrap_or("unknown"),
                assignment["id"].as_str().unwrap_or("*")
            )
        })
        .collect::<Vec<_>>();
    let event_ids = matching
        .iter()
        .take(100)
        .map(|event| event.event_id.clone())
        .collect::<Vec<_>>();
    let critical = matching
        .iter()
        .any(|event| spec(event)["severity"]["severity_status"].as_str() == Some("CRITICAL"));
    Some(json!({
        "agent_id": agent_id,
        "agent_name": name,
        "incident_key": format!("{agent_id}:{}:{bucket}", agent["updated_at"].as_u64().unwrap_or_default()),
        "severity": if critical || metric == "bypass_attempts" || metric == "prompt_injections" { "CRITICAL" } else { "HIGH" },
        "reason": reason,
        "metric": metric,
        "observed": observed,
        "threshold": threshold,
        "assignments": assignments,
        "event_ids": event_ids,
        "three_a": [
            {"at": now, "kind": "monitoring_action", "actor": name, "text": format!("Evaluated {} against a threshold of {threshold} over {window_minutes} minute(s).", metric_label)},
            {"at": now, "kind": "incident_identified", "actor": name, "text": reason},
        ],
        "assignment_labels": assignment_ids,
    }))
}

fn observed(agent: &Value, events: &[Event], now: u64) -> u64 {
    let Some(window_ms) = agent["window_minutes"]
        .as_u64()
        .map(|minutes| minutes.saturating_mul(60_000))
    else {
        return 0;
    };
    let Some(assignments) = agent["assignments"].as_array() else {
        return 0;
    };
    let Some(metric) = agent["metric"].as_str() else {
        return 0;
    };
    let start = now.saturating_sub(window_ms);
    let idle = agent["state"] == "idle";
    events
        .iter()
        .filter(|event| event.occurred_at >= start && event.occurred_at <= now)
        .filter(|event| {
            assignments
                .iter()
                .any(|assignment| assignment_matches(event, assignment))
        })
        .filter(|event| {
            !idle || {
                let severity = spec(event)["severity"]["severity_status"]
                    .as_str()
                    .unwrap_or("");
                matches!(severity, "CRITICAL" | "HIGH")
            }
        })
        .filter(|event| metric_matches(event, metric))
        .count() as u64
}

fn spec(event: &Event) -> &Value {
    static EMPTY: Value = Value::Null;
    event.spec.first().unwrap_or(&EMPTY)
}

fn assignment_matches(event: &Event, assignment: &Value) -> bool {
    let kind = assignment["kind"].as_str().unwrap_or_default();
    let id = assignment["id"].as_str().unwrap_or_default();
    match kind {
        "all" => id == "*",
        "flow" => spec(event)["flow"].as_str() == Some(id),
        "zone" => event.bindings.zones.iter().any(|zone| zone == id),
        "selection" => event
            .bindings
            .selections
            .iter()
            .any(|selection| selection == id),
        _ => false,
    }
}

fn metric_matches(event: &Event, metric: &str) -> bool {
    let item = spec(event);
    match metric {
        "critical_events" => item["severity"]["severity_status"] == "CRITICAL",
        "denials" => item["decision"]["decision"] == "DENY",
        "prompt_injections" => {
            let name = item["event_name"].as_str().unwrap_or(&event.event_type);
            name.contains("PROMPT") || name.contains("INJECTION")
        }
        "bypass_attempts" => {
            item["severity"]["alert_kind"] == "BYPASS_ATTEMPT"
                || item["event_name"]
                    .as_str()
                    .is_some_and(|name| name.contains("BYPASS"))
        }
        "tool_failures" => event.execution == Some(Execution::Failure),
        "event_volume" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{assignment_matches, candidate, metric_matches, observed};
    use crate::telemetry::events::Event;
    use crate::telemetry::model::Domain;
    use serde_json::json;

    fn event(name: &str, flow: &str) -> Event {
        let mut event = Event::new(name, Domain::PolicyIntegrity, "repo", "key");
        event.bindings.zones = vec!["payments".into()];
        event.bindings.selections = vec!["A1B2C3D4".into()];
        event.spec = vec![json!({
            "event_name": name,
            "flow": flow,
            "decision": {"decision": if name == "TOOL_DENIED" { "DENY" } else { "ALLOW" }},
            "severity": {"severity_status": if name == "BYPASS_ATTEMPT" { "CRITICAL" } else { "LOW" }, "alert_kind": name},
        })];
        event
    }

    #[test]
    fn assignments_and_metrics_use_event_evidence() {
        let denied = event("TOOL_DENIED", "F05");
        assert!(assignment_matches(
            &denied,
            &json!({"kind": "flow", "id": "F05"})
        ));
        assert!(assignment_matches(
            &denied,
            &json!({"kind": "zone", "id": "payments"})
        ));
        assert!(assignment_matches(
            &denied,
            &json!({"kind": "selection", "id": "A1B2C3D4"})
        ));
        assert!(!assignment_matches(
            &denied,
            &json!({"kind": "flow", "id": "F06"})
        ));
        assert!(metric_matches(&denied, "denials"));
        assert!(metric_matches(&denied, "event_volume"));
        assert!(!metric_matches(&denied, "bypass_attempts"));
        let bypass = event("BYPASS_ATTEMPT", "F05");
        assert!(metric_matches(&bypass, "critical_events"));
        assert!(metric_matches(&bypass, "bypass_attempts"));
        assert!(!metric_matches(&bypass, "prompt_injections"));
    }

    #[test]
    fn idle_agents_only_trigger_for_critical_events_and_incidents_are_windowed() {
        let now = 10_000_000;
        let mut denied = event("TOOL_DENIED", "F05");
        denied.occurred_at = now;
        let mut bypass = event("BYPASS_ATTEMPT", "F05");
        bypass.occurred_at = now;
        let mut agent = json!({
            "agent_id": "agent1234",
            "name": "Critical monitor",
            "state": "idle",
            "metric": "critical_events",
            "threshold": 1,
            "window_minutes": 5,
            "updated_at": now,
            "assignments": [{"kind": "flow", "id": "F05"}],
        });
        assert_eq!(observed(&agent, &[denied.clone()], now), 0);
        assert!(candidate(&agent, &[denied], now).is_none());
        assert_eq!(observed(&agent, &[bypass.clone()], now), 1);
        let incident = candidate(&agent, &[bypass.clone()], now).unwrap();
        assert_eq!(incident["severity"], "CRITICAL");
        assert_eq!(incident["event_ids"].as_array().unwrap().len(), 1);
        agent["state"] = json!("sleep");
        assert!(candidate(&agent, &[bypass], now).is_none());
    }
}
