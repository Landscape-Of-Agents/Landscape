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
            let measured = signals(agent)
                .into_iter()
                .map(|signal| {
                    json!({
                        "metric": signal.metric,
                        "threshold": signal.threshold,
                        "window_minutes": signal.window_minutes,
                        "observed": matching(agent, &signal, &context.snapshot.events, now).len(),
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "agent_id": agent["agent_id"],
                "name": agent["name"],
                "metric": agent["metric"],
                "observed": measured.first().map_or(json!(0), |signal| signal["observed"].clone()),
                "threshold": agent["threshold"],
                "signals": measured,
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

/** One threshold an agent watches
 * Fields
    - metric: String - which events count (critical_events, denials, ...)
    - threshold: u64 - matching events that open an incident
    - window_minutes: u64 - lookback window
*/
pub(crate) struct Signal {
    pub(crate) metric: String,
    pub(crate) threshold: u64,
    pub(crate) window_minutes: u64,
}

/** The signals of an agent: its signals list, or for agents saved before signals existed, the one
 * signal described by its top-level metric, threshold, and window
 * Input
    - agent: &Value - stored dashboard agent
 * Output
    - Vec<Signal> - empty when the agent has no complete signal
*/
pub(crate) fn signals(agent: &Value) -> Vec<Signal> {
    let read = |item: &Value| {
        Some(Signal {
            metric: item["metric"].as_str()?.to_string(),
            threshold: item["threshold"].as_u64()?,
            window_minutes: item["window_minutes"].as_u64()?,
        })
    };
    match agent["signals"].as_array() {
        Some(items) if !items.is_empty() => items.iter().filter_map(read).collect(),
        _ => read(agent).into_iter().collect(),
    }
}

/** Return threshold incidents detected from the selected repository's current event evidence,
 * one per agent signal that meets its threshold
 * Input
    - context: &Context - repository data
    - now: u64 - evaluation time in milliseconds
 * Output
    - Vec<Value> - incident candidates
*/
pub(crate) fn evaluate(context: &Context, now: u64) -> Vec<Value> {
    context
        .dashboard_agents
        .iter()
        .filter(|agent| matches!(agent["state"].as_str(), Some("active" | "idle")))
        .flat_map(|agent| {
            signals(agent)
                .into_iter()
                .enumerate()
                .filter_map(|(index, signal)| {
                    candidate(agent, index, &signal, &context.snapshot.events, now)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/** Per-agent observations recorded by every background or manual evaluation. The headline
 * observation is the signal closest to (or furthest past) its threshold; every signal is listed
 * Input
    - context: &Context - repository data
    - now: u64 - evaluation time in milliseconds
 * Output
    - Vec<Value> - {agent_id, observed, threshold, triggered, signals}
*/
pub(crate) fn measurements(context: &Context, now: u64) -> Vec<Value> {
    context
        .dashboard_agents
        .iter()
        .filter(|agent| matches!(agent["state"].as_str(), Some("active" | "idle")))
        .map(|agent| {
            let measured = signals(agent)
                .into_iter()
                .map(|signal| {
                    let observed =
                        matching(agent, &signal, &context.snapshot.events, now).len() as u64;
                    (signal, observed)
                })
                .collect::<Vec<_>>();
            // Compare observed / threshold ratios without dividing
            let headline = measured.iter().max_by(|(left, a), (right, b)| {
                (u128::from(*a) * u128::from(right.threshold.max(1)))
                    .cmp(&(u128::from(*b) * u128::from(left.threshold.max(1))))
            });
            json!({
                "agent_id": agent["agent_id"],
                "observed": headline.map_or(0, |(_, observed)| *observed),
                "threshold": headline.map_or(0, |(signal, _)| signal.threshold),
                "triggered": measured.iter().any(|(signal, observed)| *observed >= signal.threshold),
                "signals": measured.iter().map(|(signal, observed)| json!({
                    "metric": signal.metric,
                    "threshold": signal.threshold,
                    "window_minutes": signal.window_minutes,
                    "observed": observed,
                    "triggered": *observed >= signal.threshold,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/** The incident an agent's signal opens when its matching events meet the threshold
 * Input
    - agent: &Value - stored dashboard agent
    - index: usize - position of the signal (the first keeps the key of single-signal agents)
    - signal: &Signal - the threshold
    - events: &[Event] - repository events
    - now: u64 - evaluation time in milliseconds
 * Output
    - Option<Value> - None when the agent is not monitoring or stays below the threshold
*/
fn candidate(
    agent: &Value,
    index: usize,
    signal: &Signal,
    events: &[Event],
    now: u64,
) -> Option<Value> {
    if !matches!(agent["state"].as_str(), Some("active" | "idle")) {
        return None;
    }
    let assignments = agent["assignments"].as_array()?;
    let matched = matching(agent, signal, events, now);
    let observed = matched.len() as u64;
    let threshold = signal.threshold;
    if observed < threshold {
        return None;
    }
    let window_minutes = signal.window_minutes;
    let metric = signal.metric.as_str();
    let bucket = now / window_minutes.saturating_mul(60_000).max(1);
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
    let event_ids = matched
        .iter()
        .take(100)
        .map(|event| event.event_id.clone())
        .collect::<Vec<_>>();
    let critical = matched
        .iter()
        .any(|event| spec(event)["severity"]["severity_status"].as_str() == Some("CRITICAL"));
    let updated_at = agent["updated_at"].as_u64().unwrap_or_default();
    let incident_key = if index == 0 {
        format!("{agent_id}:{updated_at}:{bucket}")
    } else {
        format!("{agent_id}:{updated_at}:{metric}:{bucket}")
    };
    Some(json!({
        "agent_id": agent_id,
        "agent_name": name,
        "incident_key": incident_key,
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

/** The events a signal counts: inside its window, covered by one of the agent's assignments,
 * CRITICAL or HIGH only for an idle agent, and matching the signal's metric
 * Input
    - agent: &Value - stored dashboard agent
    - signal: &Signal - the threshold
    - events: &[Event] - repository events
    - now: u64 - evaluation time in milliseconds
 * Output
    - Vec<&Event> - matching events (empty when the agent has no assignments)
*/
fn matching<'a>(agent: &Value, signal: &Signal, events: &'a [Event], now: u64) -> Vec<&'a Event> {
    let Some(assignments) = agent["assignments"].as_array() else {
        return Vec::new();
    };
    let start = now.saturating_sub(signal.window_minutes.saturating_mul(60_000));
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
            !idle
                || matches!(
                    spec(event)["severity"]["severity_status"].as_str(),
                    Some("CRITICAL" | "HIGH")
                )
        })
        .filter(|event| metric_matches(event, &signal.metric))
        .collect()
}

fn spec(event: &Event) -> &Value {
    static EMPTY: Value = Value::Null;
    event.spec.first().unwrap_or(&EMPTY)
}

/** Whether an event is covered by an assignment. A flow assignment names either a telemetry flow
 * (F05) or a repository flow from governance.yaml, whose id the event carries among the zones and
 * flows covering the files it wrote
 * Input
    - event: &Event - event
    - assignment: &Value - {kind: all|flow|zone|selection, id}
 * Output
    - bool
*/
fn assignment_matches(event: &Event, assignment: &Value) -> bool {
    let kind = assignment["kind"].as_str().unwrap_or_default();
    let id = assignment["id"].as_str().unwrap_or_default();
    match kind {
        "all" => id == "*",
        "flow" => spec(event)["flow"].as_str() == Some(id) || bound(event, "flow", id),
        "zone" => bound(event, "zone", id),
        "selection" => event
            .bindings
            .selections
            .iter()
            .any(|selection| selection == id),
        _ => false,
    }
}

/** Whether an event's bindings name a zone or flow. Hooks record the entities covering the files
 * a call writes as "zone:ID" and "flow:ID"; a bare ID is accepted too
 * Input
    - event: &Event - event
    - kind: &str - zone or flow
    - id: &str - entity id
 * Output
    - bool
*/
fn bound(event: &Event, kind: &str, id: &str) -> bool {
    event.bindings.zones.iter().any(|entity| {
        entity == id
            || entity
                .strip_prefix(kind)
                .and_then(|rest| rest.strip_prefix(':'))
                == Some(id)
    })
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
    use super::{assignment_matches, candidate, matching, metric_matches, signals};
    use crate::telemetry::events::Event;
    use crate::telemetry::model::Domain;
    use serde_json::{json, Value};

    /** An event bound to zone payments, repository flow checkout, and selection A1B2C3D4
     * Input
        - name: &str - event name (TOOL_DENIED denies, BYPASS_ATTEMPT is critical)
        - flow: &str - telemetry flow
     * Output
        - Event
    */
    fn event(name: &str, flow: &str) -> Event {
        let mut event = Event::new(name, Domain::PolicyIntegrity, "repo", "key");
        // As hooks record them
        event.bindings.zones = vec!["zone:payments".into(), "flow:checkout".into()];
        event.bindings.selections = vec!["A1B2C3D4".into()];
        event.spec = vec![json!({
            "event_name": name,
            "flow": flow,
            "decision": {"decision": if name == "TOOL_DENIED" { "DENY" } else { "ALLOW" }},
            "severity": {"severity_status": if name == "BYPASS_ATTEMPT" { "CRITICAL" } else { "LOW" }, "alert_kind": name},
        })];
        event
    }

    /** How many events an agent's first signal counts
     * Input
        - agent: &Value - agent
        - events: &[Event] - events
        - now: u64 - evaluation time
     * Output
        - usize
    */
    fn observed(agent: &Value, events: &[Event], now: u64) -> usize {
        matching(agent, &signals(agent)[0], events, now).len()
    }

    /** The incident of an agent's first signal
     * Input
        - agent: &Value - agent
        - events: &[Event] - events
        - now: u64 - evaluation time
     * Output
        - Option<Value>
    */
    fn first_incident(agent: &Value, events: &[Event], now: u64) -> Option<Value> {
        candidate(agent, 0, &signals(agent)[0], events, now)
    }

    #[test]
    fn assignments_and_metrics_use_event_evidence() {
        let denied = event("TOOL_DENIED", "F05");
        assert!(assignment_matches(
            &denied,
            &json!({"kind": "flow", "id": "F05"})
        ));
        // A repository flow from governance.yaml matches through the event's bindings
        assert!(assignment_matches(
            &denied,
            &json!({"kind": "flow", "id": "checkout"})
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
        assert!(first_incident(&agent, &[denied], now).is_none());
        assert_eq!(observed(&agent, &[bypass.clone()], now), 1);
        let incident = first_incident(&agent, &[bypass.clone()], now).unwrap();
        assert_eq!(incident["severity"], "CRITICAL");
        assert_eq!(incident["event_ids"].as_array().unwrap().len(), 1);
        agent["state"] = json!("sleep");
        assert!(first_incident(&agent, &[bypass], now).is_none());
    }

    #[test]
    fn every_signal_has_its_own_threshold_window_and_incident() {
        let now = 10_000_000;
        let mut denied = event("TOOL_DENIED", "F05");
        denied.occurred_at = now;
        // Ten minutes old: inside a 15-minute window, outside a 5-minute one
        let mut older = event("TOOL_DENIED", "F05");
        older.occurred_at = now - 10 * 60_000;
        let events = [denied, older];
        let agent = json!({
            "agent_id": "agent1234",
            "name": "Checkout sentinel",
            "state": "active",
            "metric": "denials",
            "threshold": 2,
            "window_minutes": 5,
            "signals": [
                {"metric": "denials", "threshold": 2, "window_minutes": 5},
                {"metric": "denials", "threshold": 2, "window_minutes": 15},
                {"metric": "event_volume", "threshold": 1, "window_minutes": 60},
            ],
            "updated_at": now,
            "assignments": [{"kind": "flow", "id": "checkout"}],
        });
        let list = signals(&agent);
        assert_eq!(list.len(), 3);
        assert_eq!(matching(&agent, &list[0], &events, now).len(), 1);
        assert_eq!(matching(&agent, &list[1], &events, now).len(), 2);
        assert!(candidate(&agent, 0, &list[0], &events, now).is_none());
        let wide = candidate(&agent, 1, &list[1], &events, now).unwrap();
        let volume = candidate(&agent, 2, &list[2], &events, now).unwrap();
        assert_eq!(wide["metric"], "denials");
        assert_eq!(volume["metric"], "event_volume");
        assert_ne!(wide["incident_key"], volume["incident_key"]);
        // An agent saved before signals existed reads as its one top-level signal
        let legacy = json!({"metric": "denials", "threshold": 3, "window_minutes": 15});
        let legacy = signals(&legacy);
        assert_eq!(legacy.len(), 1);
        assert_eq!((legacy[0].threshold, legacy[0].window_minutes), (3, 15));
    }
}
