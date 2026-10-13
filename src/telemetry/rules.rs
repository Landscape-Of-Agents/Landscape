// Alert rules: a person builds them with 'crane alert add' (never the dashboard, never an agent),
// each naming a metric (any eval of DASHBOARD.md, such as denied or bypass_attempts), a comparison
// with a threshold over a time window, optional filters on the events counted, and the channels to
// notify (Slack, WhatsApp). The background sync evaluates every rule over this machine's events: a
// rule whose value crosses its threshold fires and notifies (waiting for each channel's answer, so
// delivery is recorded as delivered or failed with the HTTP status), reminds after its cooldown
// while still firing, and resolves when the value returns. Every evaluation is logged, firings and
// deliveries are recorded as alerts and uploaded, and the rules and their recent evaluations are
// sent in the status report, so the dashboard can chart each rule against its threshold.
//
// Rules live in the trust directory (alert-rules.json), outside the repository and out of an
// agent's reach; their state and evaluation log live in the runtime directory.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::evals::summarize;
use super::query::{Clause, Query};
use super::Stores;
use crate::governance::workspace::Workspace;
use crate::integrations;
use crate::platform::files::{append_line, read_lines, write_atomic, Lock};
use crate::platform::{io_error, now_millis};

/** Most evaluations kept in the log */
const MAX_LOG: usize = 1_000;

/** Channels a rule may notify */
pub(crate) const CHANNELS: [&str; 2] = ["slack", "whatsapp"];

/** Comparisons a rule may use, with their symbols */
pub(crate) const COMPARISONS: [(&str, &str); 5] = [
    ("above", ">"),
    ("at_least", ">="),
    ("below", "<"),
    ("at_most", "<="),
    ("equals", "="),
];

/** Metrics a rule may watch: every number the evals give (nested ones by dotted path) */
pub(crate) const METRICS: &[(&str, &str)] = &[
    ("events", "events matching the rule's filter"),
    (
        "tool_calls_decided",
        "tool calls decided by the pre-tool hook",
    ),
    ("allowed", "tool calls allowed"),
    ("denied", "tool calls denied, quarantined, or errored"),
    ("approvals", "tool calls that needed approval"),
    ("denial_share", "denied ÷ decided (0 to 1)"),
    (
        "bypass_attempts",
        "denied attempts at Crane's own protections",
    ),
    (
        "bypasses_confirmed",
        "protected code changed outside the checks",
    ),
    ("violations_after_effect", "rules broken by allowed actions"),
    ("coverage_gaps", "actions without a decision or observation"),
    (
        "missing_decisions",
        "tool calls that ran without a decision",
    ),
    (
        "unattributed_changes",
        "changes no recorded action explains",
    ),
    ("scripts_blocked", "risky scripts blocked"),
    ("quarantines", "sessions quarantined"),
    ("verified", "tool calls verified after running"),
    (
        "evidence_completeness",
        "allowed calls that were verified (0 to 1)",
    ),
    ("tasks_failed", "tasks that ended unverified or failed"),
    (
        "suspicious_prompts",
        "prompts that matched injection patterns",
    ),
    ("secrets_redacted", "secrets removed from evidence"),
    ("latency_ms.p95", "95th percentile hook latency (ms)"),
    ("tool_duration_ms.p95", "95th percentile tool duration (ms)"),
    ("credits.available", "credits available across sessions"),
    ("credits.consumed", "credits consumed"),
    ("credits.revoked", "credits revoked as penalties"),
];

/** An alert rule
 * Fields
    - name: String - unique name
    - metric: String - eval name or dotted path (METRICS)
    - op: String - above, at_least, below, at_most, equals
    - threshold: f64 - value compared with
    - window_minutes: u64 - how far back events are counted
    - channels: Vec<String> - slack and/or whatsapp
    - severity: String - LOW, MEDIUM, HIGH, CRITICAL (shown in the message and on the dashboard)
    - cooldown_minutes: u64 - least time between two notifications of a firing rule
    - filter: Vec<Clause> - conditions on the events counted (telemetry/query.rs)
    - enabled: bool - evaluated at all
    - notify_resolved: bool - also notify when the rule resolves
    - created_at: u64 - Unix milliseconds
    - created_by: String - Git user name
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Rule {
    pub(crate) name: String,
    pub(crate) metric: String,
    pub(crate) op: String,
    pub(crate) threshold: f64,
    pub(crate) window_minutes: u64,
    pub(crate) channels: Vec<String>,
    pub(crate) severity: String,
    pub(crate) cooldown_minutes: u64,
    #[serde(default)]
    pub(crate) filter: Vec<Clause>,
    #[serde(default = "enabled")]
    pub(crate) enabled: bool,
    #[serde(default = "enabled")]
    pub(crate) notify_resolved: bool,
    pub(crate) created_at: u64,
    pub(crate) created_by: String,
}

/** Default for enabled flags
 * Input
    - None
 * Output
    - bool (true)
*/
fn enabled() -> bool {
    true
}

impl Rule {
    /** The comparison's symbol
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(crate) fn symbol(&self) -> &'static str {
        COMPARISONS
            .iter()
            .find(|(name, _)| *name == self.op)
            .map_or("?", |(_, symbol)| symbol)
    }

    /** Report whether a value breaches the rule
     * Input
        - value: f64 - observed value
     * Output
        - bool
    */
    pub(crate) fn breached(&self, value: f64) -> bool {
        match self.op.as_str() {
            "above" => value > self.threshold,
            "at_least" => value >= self.threshold,
            "below" => value < self.threshold,
            "at_most" => value <= self.threshold,
            _ => (value - self.threshold).abs() < f64::EPSILON,
        }
    }

    /** Describe the rule in one line
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn describe(&self) -> String {
        let filter = if self.filter.is_empty() {
            String::new()
        } else {
            format!(
                " where {}",
                self.filter
                    .iter()
                    .map(|clause| format!(
                        "{} {:?} {}",
                        clause.path,
                        clause.op,
                        clause.values.join(",")
                    )
                    .to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join(" and ")
            )
        };
        format!(
            "{} {} {} over {} min{filter}",
            self.metric,
            self.symbol(),
            self.threshold,
            self.window_minutes
        )
    }
}

/** The rule file
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<PathBuf, String>
*/
fn rules_path(workspace: &Workspace) -> Result<PathBuf, String> {
    Ok(workspace.trust()?.repo_dir.join("alert-rules.json"))
}

/** Read the rules
 * Input
    - workspace: &Workspace - repository
 * Output
    - Vec<Rule> (empty when none or unreadable)
*/
pub(crate) fn load(workspace: &Workspace) -> Vec<Rule> {
    rules_path(workspace)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/** Report whether any rule exists, cheaply (the hooks ask before starting a background sync)
 * Input
    - workspace: &Workspace - repository
 * Output
    - bool
*/
pub(crate) fn exist(workspace: &Workspace) -> bool {
    rules_path(workspace).is_ok_and(|path| path.exists())
}

/** Write the rules
 * Input
    - workspace: &Workspace - repository
    - rules: &[Rule] - rules
 * Output
    - Result<(), String>
*/
pub(crate) fn save(workspace: &Workspace, rules: &[Rule]) -> Result<(), String> {
    let path = rules_path(workspace)?;
    if rules.is_empty() {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        };
    }
    write_atomic(
        &path,
        serde_json::to_string_pretty(rules)
            .map_err(io_error)?
            .as_bytes(),
    )
}

/** Check a rule before saving it
 * Input
    - rule: &Rule - rule
 * Output
    - Result<(), String>
    - Error describing the first problem
*/
pub(crate) fn check(rule: &Rule) -> Result<(), String> {
    if rule.name.is_empty()
        || rule.name.len() > 64
        || !rule.name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return Err("a rule name uses letters, digits, '-' or '_' (at most 64)".into());
    }
    if !METRICS.iter().any(|(name, _)| *name == rule.metric) {
        return Err(format!(
            "unknown metric '{}'; run 'crane alert metrics'",
            rule.metric
        ));
    }
    if !COMPARISONS.iter().any(|(name, _)| *name == rule.op) {
        return Err("compare with above, at_least, below, at_most, or equals".into());
    }
    if !(1..=10_080).contains(&rule.window_minutes) {
        return Err("the window is 1 to 10080 minutes".into());
    }
    if rule.channels.is_empty()
        || rule
            .channels
            .iter()
            .any(|channel| !CHANNELS.contains(&channel.as_str()))
    {
        return Err("notify slack, whatsapp, or both".into());
    }
    if !["LOW", "MEDIUM", "HIGH", "CRITICAL"].contains(&rule.severity.as_str()) {
        return Err("severity is low, medium, high, or critical".into());
    }
    if !rule.threshold.is_finite() {
        return Err("the threshold must be a number".into());
    }
    Ok(())
}

/** Read a metric from evals by its dotted path
 * Input
    - evals: &Value - evals::summarize
    - metric: &str - path
 * Output
    - Option<f64> (None when the evals have no value for it)
*/
fn metric_value(evals: &Value, metric: &str) -> Option<f64> {
    let mut node = evals;
    for part in metric.split('.') {
        node = &node[part];
    }
    node.as_f64()
}

/** The value of a rule's metric over its window, from this machine's events
 * Input
    - rule: &Rule - rule
    - specs: &[Value] - every specification event of the machine
    - now: u64 - Unix milliseconds
 * Output
    - (Option<f64>, usize) value (None when it cannot be computed) and events counted
*/
pub(crate) fn measure(rule: &Rule, specs: &[Value], now: u64) -> (Option<f64>, usize) {
    let query = Query {
        since: Some(now.saturating_sub(rule.window_minutes * 60_000)),
        until: None,
        text: None,
        clauses: rule.filter.clone(),
    };
    let matching = specs
        .iter()
        .filter(|spec| query.matches(spec, None))
        .collect::<Vec<_>>();
    let evals = summarize(&matching);
    let value = metric_value(&evals, &rule.metric).or_else(|| {
        // A count with nothing to count is zero, but a ratio or percentile with nothing behind it
        // stays unknown, so a "below" rule never fires on missing data
        let counted = !rule.metric.contains('.')
            && !matches!(
                rule.metric.as_str(),
                "denial_share" | "evidence_completeness"
            );
        counted.then_some(0.0)
    });
    (value, matching.len())
}

/** One rule's state between evaluations
 * Fields
    - state: String - OK or FIRING
    - since: u64 - when it entered the state (Unix milliseconds)
    - last_notified: u64 - when it last notified
    - last_value: Option<f64> - last measured value
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    state: String,
    since: u64,
    last_notified: u64,
    last_value: Option<f64>,
}

/** Format Unix milliseconds as DD:MM:YYYY:HH:MM:SS (UTC), as the dashboard shows times
 * Input
    - millis: u64 - Unix milliseconds
 * Output
    - String
*/
pub(crate) fn stamp(millis: u64) -> String {
    let seconds = millis / 1000;
    let days = (seconds / 86_400) as i64;
    let (hour, minute, second) = (
        (seconds % 86_400) / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
    );
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day:02}:{month:02}:{year:04}:{hour:02}:{minute:02}:{second:02}")
}

/** Build the notification text of a rule
 * Input
    - workspace: &Workspace - repository
    - rule: &Rule - rule
    - value: f64 - observed value
    - state: &str - FIRING, RESOLVED, REMINDER, or TEST
    - now: u64 - Unix milliseconds
 * Output
    - String
*/
fn message(workspace: &Workspace, rule: &Rule, value: f64, state: &str, now: u64) -> String {
    let headline = match state {
        "RESOLVED" => "resolved",
        "REMINDER" => "is still firing",
        "TEST" => "test notification",
        _ => "is firing",
    };
    format!(
        "[{}] Crane alert rule '{}' {headline}\nRepository: {}\nMetric: {} = {} (alert when {} {} over the last {} min)\n{}When: {} UTC\nMachine user: {}",
        rule.severity,
        rule.name,
        workspace.remote.clone().unwrap_or_else(|| workspace.root.display().to_string()),
        rule.metric,
        (value * 1000.0).round() / 1000.0,
        rule.symbol(),
        rule.threshold,
        rule.window_minutes,
        if rule.filter.is_empty() { String::new() } else { format!("Counting: {}\n", rule.describe().split(" where ").nth(1).unwrap_or_default()) },
        stamp(now),
        workspace.user.clone().unwrap_or_else(crate::platform::actor),
    )
}

/** Notify a rule's channels and record the alert (uploaded with the next sync)
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
    - rule: &Rule - rule
    - value: f64 - observed value
    - state: &str - FIRING, RESOLVED, REMINDER, or TEST
    - now: u64 - Unix milliseconds
 * Output
    - Result<Value, String> the alert record
*/
pub(crate) fn notify(
    workspace: &Workspace,
    stores: &Stores,
    rule: &Rule,
    value: f64,
    state: &str,
    now: u64,
) -> Result<Value, String> {
    let text = message(workspace, rule, value, state, now);
    let deliveries = rule
        .channels
        .iter()
        .map(|channel| integrations::deliver_now(workspace, channel, &text))
        .collect::<Vec<_>>();
    let record = json!({
        "alert_id": format!("alr_rule_{}_{}_{}", rule.name, now, state.to_ascii_lowercase()),
        "key": format!("rule:{}", rule.name),
        "kind": "rule",
        "title": format!("Rule {}: {}", rule.name, rule.describe()),
        "severity": if state == "RESOLVED" || state == "TEST" { "INFO".to_string() } else { rule.severity.clone() },
        "at": now / 1000,
        "suppressed": false,
        "state": if state == "REMINDER" { "FIRING" } else { state },
        "reminder": state == "REMINDER",
        "value": value,
        "rule": rule,
        "deliveries": deliveries,
    });
    append_line(&stores.directory.join("alerts.jsonl"), &record.to_string())?;
    Ok(record)
}

/** Evaluate every enabled rule now, notifying on changes; skipped when another evaluation runs
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
    - trigger: &str - SYNC or COMMAND
 * Output
    - Result<Vec<Value>, String> one evaluation record per rule
*/
pub(crate) fn evaluate(
    workspace: &Workspace,
    stores: &Stores,
    trigger: &str,
) -> Result<Vec<Value>, String> {
    let rules = load(workspace);
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let Some(_lock) = Lock::try_acquire(&stores.directory.join("alert-rules.lock"))? else {
        return Ok(Vec::new());
    };
    let specs = crate::foxx::views::Snapshot::load(workspace)?.specs();
    let state_path = stores.directory.join("alert-rules-state.json");
    let mut states: Map<String, Value> = fs::read_to_string(&state_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let now = now_millis();
    let mut records = Vec::new();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        let mut state: State = states
            .get(&rule.name)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        if state.state.is_empty() {
            state.state = "OK".into();
            state.since = now;
        }
        let (value, counted) = measure(rule, &specs, now);
        let mut transition = None;
        let mut delivery = Value::Null;
        if let Some(value) = value {
            let breached = rule.breached(value);
            let cooldown = rule.cooldown_minutes * 60_000;
            let next = match (state.state.as_str(), breached) {
                ("OK", true) => Some("FIRING"),
                ("FIRING", false) => Some("RESOLVED"),
                ("FIRING", true)
                    if now.saturating_sub(state.last_notified) >= cooldown.max(60_000) =>
                {
                    Some("REMINDER")
                }
                _ => None,
            };
            if let Some(next) = next {
                let notify_now = next != "RESOLVED" || rule.notify_resolved;
                if notify_now {
                    delivery = notify(workspace, stores, rule, value, next, now)
                        .map(|record| record["deliveries"].clone())
                        .unwrap_or(Value::Null);
                    state.last_notified = now;
                }
                if next != "REMINDER" {
                    state.state = if next == "FIRING" {
                        "FIRING".into()
                    } else {
                        "OK".into()
                    };
                    state.since = now;
                }
                transition = Some(next);
            }
            state.last_value = Some(value);
        }
        let record = json!({
            "at": now,
            "rule": rule.name,
            "metric": rule.metric,
            "op": rule.op,
            "threshold": rule.threshold,
            "value": value,
            "events": counted,
            "state": state.state,
            "transition": transition,
            "deliveries": delivery,
            "trigger": trigger,
        });
        append_line(
            &stores.directory.join("alert-evaluations.jsonl"),
            &record.to_string(),
        )?;
        states.insert(
            rule.name.clone(),
            serde_json::to_value(&state).map_err(io_error)?,
        );
        records.push(record);
    }
    write_atomic(&state_path, Value::Object(states).to_string().as_bytes())?;
    trim(&stores.directory.join("alert-evaluations.jsonl"))?;
    Ok(records)
}

/** Keep only the newest evaluations in the log
 * Input
    - path: &std::path::Path - log
 * Output
    - Result<(), String>
*/
fn trim(path: &std::path::Path) -> Result<(), String> {
    let lines = read_lines(path)?;
    if lines.len() <= MAX_LOG + MAX_LOG / 2 {
        return Ok(());
    }
    let kept = lines[lines.len() - MAX_LOG..]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    write_atomic(path, kept.as_bytes())
}

/** The alerting part of the status report: rules, their state, and recent evaluations
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
 * Output
    - Value {rules, state, history, channels}
*/
pub(crate) fn report(workspace: &Workspace, stores: &Stores) -> Value {
    let rules = load(workspace);
    let state: Value = fs::read_to_string(stores.directory.join("alert-rules-state.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let history = read_lines(&stores.directory.join("alert-evaluations.jsonl")).unwrap_or_default();
    json!({
        "rules": rules.iter().map(|rule| {
            let mut value = serde_json::to_value(rule).unwrap_or_default();
            value["description"] = json!(rule.describe());
            value
        }).collect::<Vec<_>>(),
        "state": state,
        "history": history[history.len().saturating_sub(300)..],
        "channels": CHANNELS.iter().map(|channel| json!({"channel": channel, "configured": integrations::load(workspace, channel).is_some()})).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::query::Op;

    /** A rule for the tests
     * Input
        - metric: &str - metric
        - op: &str - comparison
        - threshold: f64 - threshold
     * Output
        - Rule
    */
    fn rule(metric: &str, op: &str, threshold: f64) -> Rule {
        Rule {
            name: "r".into(),
            metric: metric.into(),
            op: op.into(),
            threshold,
            window_minutes: 10,
            channels: vec!["slack".into()],
            severity: "HIGH".into(),
            cooldown_minutes: 30,
            filter: Vec::new(),
            enabled: true,
            notify_resolved: true,
            created_at: 0,
            created_by: "t".into(),
        }
    }

    /** Rules measure their metric over their window and filter, compare as stated, and never fire
     * on missing ratios
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn rules_measure_and_compare() {
        let now = 10 * 60_000 + 1_000;
        let decided = |decision: &str, at: u64, severity: &str| json!({"event_name": if decision == "DENY" { "TOOL_CALL_DENIED_RULE" } else { "TOOL_CALL_ALLOWED" }, "flow": "F05", "occurred_at": at, "decision": {"decision": decision}, "severity": {"severity_status": severity}, "action": {"action_id": format!("c{at}")}});
        let specs = vec![
            decided("DENY", 0, "HIGH"),
            decided("DENY", now - 1_000, "HIGH"),
            decided("DENY", now - 2_000, "MEDIUM"),
            decided("ALLOW", now - 3_000, "INFO"),
        ];
        let mut denied = rule("denied", "at_least", 2.0);
        assert_eq!(
            measure(&denied, &specs, now),
            (Some(2.0), 3),
            "the event before the window is not counted"
        );
        assert!(denied.breached(2.0) && !denied.breached(1.0));
        denied.filter = vec![Clause {
            path: "severity.severity_status".into(),
            op: Op::Is,
            values: vec!["HIGH".into()],
            group: None,
        }];
        assert_eq!(
            measure(&denied, &specs, now).0,
            Some(1.0),
            "the filter narrows what is counted"
        );
        let share = rule("denial_share", "below", 0.5);
        assert_eq!(
            measure(&share, &[], now).0,
            None,
            "a ratio with no events is unknown, not zero"
        );
        assert_eq!(
            measure(&rule("bypass_attempts", "above", 0.0), &[], now).0,
            Some(0.0),
            "a count with no events is zero"
        );
        assert!(check(&rule("denied", "above", 1.0)).is_ok());
        assert!(check(&rule("nonsense", "above", 1.0)).is_err());
        assert!(check(&rule("denied", "sideways", 1.0)).is_err());
        assert_eq!(rule("denied", "at_most", 3.0).symbol(), "<=");
    }

    /** Times are written as DD:MM:YYYY:HH:MM:SS
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn stamps_are_day_first() {
        assert_eq!(stamp(0), "01:01:1970:00:00:00");
        assert_eq!(stamp(1_791_905_689_000), "13:10:2026:15:34:49");
        assert_eq!(stamp(951_782_400_000), "29:02:2000:00:00:00");
    }
}
