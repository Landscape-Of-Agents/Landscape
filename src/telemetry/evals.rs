// The dashboard's numbers (DASHBOARD.md "eval"), computed only from specification events: counts by
// event, flow, decision, severity, enforcement, and alert; the outcomes the security views need
// (allowed, approvals, denials, bypass attempts and confirmed bypasses, violations, coverage gaps,
// unattributed changes, verified calls, evidence completeness); latency and duration percentiles;
// the credit balance; and the distinct models, identity provenances, and network destinations
// seen. A number with no events behind it is null, never zero.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

/** Names of events that are decisions before a tool call (F05, F16) */
const DECISIONS: &[&str] = &[
    "TOOL_CALL_ALLOWED",
    "TOOL_CALL_APPROVAL_REQUIRED",
    "TOOL_CALL_DENIED_RULE",
    "TOOL_CALL_DENIED_BYPASS_ATTEMPT",
    "TOOL_CALL_DENIED_QUARANTINE",
    "TOOL_CALL_DENIED_FAIL_CLOSED",
    "SCRIPT_BLOCKED",
    "SCRIPT_APPROVAL_REQUIRED",
    "SCRIPT_WRITE_APPROVAL_REQUIRED",
    "PERMISSION_PRE_DENIED",
    "PERMISSION_LEFT_TO_PERSON",
];

/** Read a string at a group and key
 * Input
    - spec: &Value - event
    - group: &str - group
    - key: &str - sub-attribute
 * Output
    - &str (empty when absent)
*/
pub(crate) fn field<'a>(spec: &'a Value, group: &str, key: &str) -> &'a str {
    spec[group][key].as_str().unwrap_or_default()
}

/** Percentiles of a list of numbers
 * Input
    - values: Vec<f64> - values
 * Output
    - Value {p50, p95, max, count}, null when empty
*/
fn percentiles(mut values: Vec<f64>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let at = |fraction: f64| values[((values.len() - 1) as f64 * fraction).round() as usize];
    json!({"p50": at(0.5), "p95": at(0.95), "max": values[values.len() - 1], "count": values.len()})
}

/** Summarize specification events
 * Input
    - specs: &[&Value] - events
 * Output
    - Value with by_event, by_flow, by_decision, by_severity, by_enforcement, by_alert, and the
      named outcomes (null when nothing was recorded)
*/
pub(crate) fn summarize(specs: &[&Value]) -> Value {
    let count = |key: &str| {
        let mut map: BTreeMap<String, u64> = BTreeMap::new();
        for spec in specs {
            let value = match key {
                "event_name" | "flow" => spec[key].as_str().unwrap_or_default().to_string(),
                "decision" => field(spec, "decision", "decision").to_string(),
                "severity" => field(spec, "severity", "severity_status").to_string(),
                "enforcement" => field(spec, "decision", "enforcement").to_string(),
                _ => field(spec, "severity", "alert_kind").to_string(),
            };
            if !value.is_empty()
                && value != "NONE"
                && value != "NOT_REQUIRED"
                && value != "NOT_APPLICABLE"
            {
                *map.entry(value).or_default() += 1;
            }
        }
        map
    };
    let by_event = count("event_name");
    let by_flow = count("flow");
    let by_severity = count("severity");
    let by_enforcement = count("enforcement");
    let by_alert = count("alert");
    let named = |name: &str| by_event.get(name).copied().unwrap_or(0);
    let decisions = specs
        .iter()
        .filter(|spec| DECISIONS.contains(&spec["event_name"].as_str().unwrap_or_default()))
        .collect::<Vec<_>>();
    let mut by_decision: BTreeMap<String, u64> = BTreeMap::new();
    for spec in &decisions {
        *by_decision
            .entry(field(spec, "decision", "decision").to_string())
            .or_default() += 1;
    }
    let decided = decisions.len() as u64;
    let some = |value: u64, present: bool| if present { json!(value) } else { Value::Null };
    let has_decisions = decided > 0;
    let allowed_calls = decisions
        .iter()
        .filter(|spec| field(spec, "decision", "decision") == "ALLOW")
        .map(|spec| field(spec, "action", "action_id").to_string())
        .collect::<BTreeSet<_>>();
    let verified_calls = specs
        .iter()
        .filter(|spec| spec["flow"] == "F06")
        .map(|spec| field(spec, "action", "action_id").to_string())
        .collect::<BTreeSet<_>>();
    let completeness = if allowed_calls.is_empty() {
        Value::Null
    } else {
        json!(
            allowed_calls.intersection(&verified_calls).count() as f64 / allowed_calls.len() as f64
        )
    };
    let numbers = |name: &str| {
        specs
            .iter()
            .filter_map(|spec| spec["eval"][name].as_f64())
            .collect::<Vec<_>>()
    };
    // The latest balance of each session, from its credit events
    let mut balances: BTreeMap<String, &Value> = BTreeMap::new();
    for spec in specs
        .iter()
        .filter(|spec| spec["flow"] == "F27" && spec["eval"]["available"].is_number())
    {
        let session = field(spec, "session", "session_id").to_string();
        let newer = balances
            .get(&session)
            .is_none_or(|earlier| spec["occurred_at"].as_u64() >= earlier["occurred_at"].as_u64());
        if newer {
            balances.insert(session, spec);
        }
    }
    let credit = |key: &str| {
        if balances.is_empty() {
            Value::Null
        } else {
            json!(balances
                .values()
                .filter_map(|spec| spec["eval"][key].as_u64())
                .sum::<u64>())
        }
    };
    let distinct = |group: &str, key: &str| {
        specs
            .iter()
            .filter_map(|spec| spec[group][key].as_str())
            .filter(|value| !value.is_empty() && *value != "NA")
            .map(String::from)
            .collect::<BTreeSet<_>>()
    };
    let destinations = specs
        .iter()
        .filter(|spec| spec["event_name"] == "NETWORK_DESTINATION_INFERRED")
        .flat_map(|spec| {
            spec["resource"]["resource_ids"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|host| host.as_str().map(String::from))
        .collect::<BTreeSet<_>>();
    let denied = by_decision.get("DENY").copied().unwrap_or(0)
        + by_decision.get("QUARANTINE").copied().unwrap_or(0)
        + by_decision.get("ERROR").copied().unwrap_or(0);
    let bypass_attempts = decisions
        .iter()
        .filter(|spec| field(spec, "severity", "alert_kind") == "BYPASS_ATTEMPT")
        .count() as u64;
    let present = !specs.is_empty();
    json!({
        "events": specs.len(),
        "by_event": by_event,
        "by_flow": by_flow,
        "by_decision": by_decision,
        "by_severity": by_severity,
        "by_enforcement": by_enforcement,
        "by_alert": by_alert,
        "tool_calls_decided": some(decided, present),
        "allowed": some(by_decision.get("ALLOW").copied().unwrap_or(0), has_decisions),
        "approvals": some(by_decision.get("REQUIRE_APPROVAL").copied().unwrap_or(0), has_decisions),
        "denied": some(denied, has_decisions),
        "denial_share": if has_decisions { json!(denied as f64 / decided as f64) } else { Value::Null },
        "bypass_attempts": some(bypass_attempts, has_decisions),
        "bypasses_confirmed": some(named("BYPASS_CONFIRMED"), present),
        "violations_after_effect": some(named("VIOLATION_AFTER_EFFECT"), present),
        "coverage_gaps": some(by_enforcement.get("COVERAGE_GAP").copied().unwrap_or(0), present),
        "missing_decisions": some(named("TOOL_CALL_MISSING_DECISION"), present),
        "unattributed_changes": some(named("UNATTRIBUTED_CHANGES_FOUND"), present),
        "scripts_blocked": some(named("SCRIPT_BLOCKED"), present),
        "verified": some(named("TOOL_CALL_VERIFIED"), present),
        "evidence_completeness": completeness,
        "quarantines": some(named("SESSION_QUARANTINED_ATTEMPTS") + named("BYPASS_CONFIRMED"), present),
        "tasks_verified": some(named("TASK_VERIFIED"), present),
        "tasks_failed": some(named("TASK_VERIFICATION_FAILED") + named("TASK_ENDED_UNVERIFIED"), present),
        "suspicious_prompts": some(named("PROMPT_SUSPICIOUS"), present),
        "attestations_signed": some(specs.iter().filter(|spec| spec["event_name"] == "SESSION_ATTESTED" && field(spec, "response", "response_status") == "SIGNED").count() as u64, present),
        "secrets_redacted": if present { json!(numbers("secrets_redacted").iter().sum::<f64>()) } else { Value::Null },
        "latency_ms": percentiles(numbers("latency_ms")),
        "tool_duration_ms": percentiles(numbers("tool_duration_ms")),
        "credits": {
            "granted": credit("granted"),
            "consumed": credit("consumed"),
            "reserved": credit("reserved"),
            "revoked": credit("revoked"),
            "expired": credit("expired"),
            "available": credit("available"),
        },
        "models": distinct("agent", "model"),
        "identity_provenance": distinct("session", "id_provenance"),
        "network_destinations": destinations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check that outcomes are counted from events, and missing data is null rather than zero
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn outcomes_are_counted_and_missing_is_null() {
        let empty = summarize(&[]);
        assert!(empty["denied"].is_null());
        assert!(empty["evidence_completeness"].is_null());
        let event = |name: &str, flow: &str, decision: &str, id: &str, alert: &str| json!({"event_name": name, "flow": flow, "occurred_at": 1, "action": {"action_id": id}, "decision": {"decision": decision, "enforcement": "NOT_APPLICABLE"}, "severity": {"severity_status": "INFO", "alert_kind": alert}, "eval": {"latency_ms": 10.0}});
        let allowed = event("TOOL_CALL_ALLOWED", "F05", "ALLOW", "c1", "NONE");
        let verified = event("TOOL_CALL_VERIFIED", "F06", "NOT_REQUIRED", "c1", "NONE");
        let bypass = event(
            "TOOL_CALL_DENIED_BYPASS_ATTEMPT",
            "F05",
            "DENY",
            "c2",
            "BYPASS_ATTEMPT",
        );
        let summary = summarize(&[&allowed, &verified, &bypass]);
        assert_eq!(summary["allowed"], 1);
        assert_eq!(summary["denied"], 1);
        assert_eq!(summary["bypass_attempts"], 1);
        assert_eq!(summary["evidence_completeness"], 1.0);
        assert_eq!(summary["denial_share"], 0.5);
        assert_eq!(summary["latency_ms"]["count"], 3);
        assert_eq!(summary["by_flow"]["F05"], 2);
    }
}
