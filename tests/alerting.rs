// Alert rules built with 'crane alert': adding (people only), evaluating, firing with delivery
// results, staying quiet while firing within the cooldown, resolving, test notifications, the log,
// and the listing with state.

mod common;

use common::{text, Fixture};
use serde_json::{json, Value};

/** A rule fires once its metric crosses the threshold, records each channel's delivery, stays
 * quiet while still firing, resolves when the threshold is raised above the value, and every step
 * is visible in check, list, and log; agents cannot change rules
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn alert_rules_fire_notify_and_resolve() {
    let fixture = Fixture::ready();
    let slack = fixture.with_input(
        &["integrate", "slack"],
        "1\nhttps://hooks.slack.com/services/T0/B0/TOPSECRETVALUE\n\n2\n\ny\n",
    );
    assert!(slack.status.success(), "{}", text(&slack));
    let add = |threshold: &str| {
        fixture.ok(&[
            "alert",
            "add",
            "busy-tools",
            "metric",
            "events",
            "at_least",
            threshold,
            "window",
            "1h",
            "channels",
            "slack",
            "severity",
            "high",
            "cooldown",
            "30",
            "where",
            "flow",
            "is",
            "F05",
            "where",
            "action.action_tool",
            "exists",
        ])
    };
    let added = add("2");
    assert!(added.contains("Added alert rule 'busy-tools'"), "{added}");
    assert!(added.contains("events >= 2 over 60 min"), "{added}");
    // Agents cannot build or change rules
    let refused = fixture.as_agent(&["alert", "add", "sneaky", "metric", "denied", "above", "100"]);
    assert!(!refused.status.success(), "{}", text(&refused));
    let refused = fixture.as_agent(&["alert", "remove", "busy-tools"]);
    assert!(!refused.status.success(), "{}", text(&refused));
    // Bad rules are explained
    assert!(fixture
        .fails(&["alert", "add", "x", "metric", "nonsense", "above", "1"])
        .contains("unknown metric"));
    assert!(fixture
        .fails(&[
            "alert", "add", "x", "metric", "denied", "above", "1", "where", "flow", "sideways",
            "F05"
        ])
        .contains("unknown operator"));
    // Nothing yet: the rule stays OK (commands and hooks also start a background sync, which
    // evaluates rules too, so the tests wait for it and check states rather than who changed them)
    settle();
    let (ok, checked) = fixture.json(&["alert", "check", "--json"]);
    assert!(ok, "{checked}");
    assert_eq!(checked[0]["state"], "OK", "{checked}");
    assert_eq!(checked[0]["value"], 0.0);
    // Three decided tool calls cross the threshold
    hook_calls(&fixture, 3);
    settle();
    let (_, checked) = fixture.json(&["alert", "check", "--json"]);
    assert_eq!(checked[0]["state"], "FIRING", "{checked}");
    assert!(checked[0]["value"].as_f64().unwrap() >= 2.0, "{checked}");
    // Still firing within the cooldown: no new notification
    let (_, again) = fixture.json(&["alert", "check", "--json"]);
    assert!(again[0]["transition"].is_null(), "{again}");
    assert_eq!(again[0]["state"], "FIRING");
    // Raising the threshold above the value resolves it (the rule keeps its state by name)
    let replaced = add("1000");
    assert!(replaced.contains("Replaced"), "{replaced}");
    settle();
    let (_, resolved) = fixture.json(&["alert", "check", "--json"]);
    assert_eq!(resolved[0]["state"], "OK", "{resolved}");
    // A test notification, the log, and the listing
    let tested = fixture.ok(&["alert", "test", "busy-tools"]);
    assert!(tested.contains("slack dry-run"), "{tested}");
    let (_, log) = fixture.json(&["alert", "log", "--json"]);
    let states = log
        .as_array()
        .unwrap()
        .iter()
        .map(|alert| alert["state"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        ["TEST", "RESOLVED", "FIRING"],
        "exactly one firing and one resolution: {log}"
    );
    for alert in log.as_array().unwrap() {
        assert_eq!(alert["kind"], "rule");
        assert_eq!(alert["deliveries"][0]["channel"], "slack", "{alert}");
        assert_eq!(alert["deliveries"][0]["result"], "dry-run", "{alert}");
    }
    let (_, listed) = fixture.json(&["alert", "list", "--json"]);
    assert_eq!(listed["rules"][0]["name"], "busy-tools", "{listed}");
    assert_eq!(listed["state"]["busy-tools"]["state"], "OK");
    assert_eq!(
        listed["channels"][0],
        json!({"channel": "slack", "configured": true})
    );
    assert!(
        listed["history"].as_array().unwrap().len() >= 4,
        "every evaluation is logged: {listed}"
    );
    assert!(fixture
        .ok(&["alert", "metrics"])
        .contains("bypass_attempts"));
    // Disabled rules are not evaluated; removed rules are gone
    fixture.ok(&["alert", "disable", "busy-tools"]);
    settle();
    let (_, none) = fixture.json(&["alert", "check", "--json"]);
    assert_eq!(none, json!([]));
    fixture.ok(&["alert", "remove", "busy-tools"]);
    assert!(fixture.ok(&["alert", "list"]).contains("No alert rules"));
}

/** Run decided tool calls through the hooks
 * Input
    - fixture: &Fixture - repository
    - count: usize - calls
 * Output
    - None
*/
fn hook_calls(fixture: &Fixture, count: usize) {
    fixture.hook("claude", "SessionStart", &json!({"session_id": "alert-1"}));
    for index in 0..count {
        let payload: Value = json!({"session_id": "alert-1", "tool_use_id": format!("a{index}"), "cwd": fixture.work.to_string_lossy(), "tool_name": "Bash", "tool_input": {"command": "echo ok"}});
        fixture.hook("claude", "pre-tool-use", &payload);
        fixture.hook("claude", "post-tool-use", &payload);
    }
}

/** Wait for the background syncs that commands and hooks start
 * Input
    - None
 * Output
    - None
*/
fn settle() {
    std::thread::sleep(std::time::Duration::from_secs(3));
}
