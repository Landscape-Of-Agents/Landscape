// Alerts: deterministic, rule-driven notifications raised by the hook runtime (low autonomy
// credits, bypass attempts and confirmed bypasses, violations, quarantine, coverage gaps, failed
// verification, suspicious prompts). Each alert is deduplicated with a cooldown, recorded in the
// runtime folder, and delivered to Slack and WhatsApp when configured, by a detached curl process
// that reads its URL, headers, and body from stdin, so credentials never appear in a command line.
// Alert text carries identifiers, the trigger, values, and the action taken, never secrets, raw
// prompts, or tool payloads. Delivery is best effort and never changes a decision.

use serde_json::{json, Value};

use super::events::Event;
use super::identity::SessionRecord;
use super::Stores;
use crate::governance::workspace::Workspace;
use crate::integrations;
use crate::platform::files::{append_line, read_lines, Lock};
use crate::platform::now_unix;

/** Default cooldown between two alerts with the same key, in seconds */
const DEFAULT_COOLDOWN: u64 = 600;

/** Describe an alert kind
 * Input
    - kind: &str - alert kind
 * Output
    - (&'static str, &'static str) title and the action Crane took
*/
fn describe(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "low_credits" => (
            "Low autonomy credits",
            "changes will need human approval once credits run out",
        ),
        "bypass_attempt" => (
            "Attempt to bypass Crane",
            "the action was denied before it ran",
        ),
        "bypass_confirmed" => (
            "Confirmed bypass of Crane",
            "the session was quarantined; the agent was told to restore the code",
        ),
        "violation" => (
            "Policy violation detected after the effect",
            "the session was degraded; the agent was told to repair",
        ),
        "quarantine" => (
            "Agent session quarantined",
            "mutating actions are denied for the rest of the session",
        ),
        "coverage_gap" => (
            "Enforcement coverage gap",
            "the action is recorded as unobserved; verify the hooks",
        ),
        "verification_failed" => (
            "Task verification failed",
            "the agent was asked to repair before finishing",
        ),
        "prompt_injection" => (
            "Suspicious prompt",
            "recorded for review; decisions are unaffected",
        ),
        _ => ("Crane alert", "recorded"),
    }
}

/** Raise an alert for an event: deduplicate it, record it, and deliver it to the configured
 * channels; failures are recorded, never propagated
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
    - kind: &str - alert kind
    - session: &SessionRecord - session
    - event: &Event - triggering event
 * Output
    - None
*/
pub(crate) fn raise(
    workspace: &Workspace,
    stores: &Stores,
    kind: &str,
    session: &SessionRecord,
    event: &Event,
) {
    let _ = try_raise(workspace, stores, kind, session, event);
}

/** Implementation of raise
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
    - kind: &str - alert kind
    - session: &SessionRecord - session
    - event: &Event - triggering event
 * Output
    - Result<(), String>
*/
fn try_raise(
    workspace: &Workspace,
    stores: &Stores,
    kind: &str,
    session: &SessionRecord,
    event: &Event,
) -> Result<(), String> {
    let path = stores.directory.join("alerts.jsonl");
    let _lock = Lock::acquire(&path.with_extension("lock"))?;
    let key = format!("{kind}:{}", session.foxx_session_id);
    let slack = integrations::load(workspace, "slack");
    let cooldown = slack
        .as_ref()
        .and_then(|integration| {
            integration
                .settings
                .get("cooldown_seconds")
                .and_then(Value::as_u64)
        })
        .unwrap_or(DEFAULT_COOLDOWN);
    let now = now_unix();
    let suppressed = read_lines(&path)?
        .iter()
        .rev()
        .find(|alert| alert["key"] == key.as_str())
        .is_some_and(|alert| now.saturating_sub(alert["at"].as_u64().unwrap_or(0)) < cooldown);
    let (title, action) = describe(kind);
    let severity = format!("{:?}", event.severity).to_ascii_uppercase();
    let observed = event
        .measurements
        .iter()
        .find(|measurement| measurement.name == "credits_available")
        .and_then(|measurement| measurement.value);
    let text = format!(
        "[{severity}] {title}\nAgent: {} ({}) session {} task {}\nTrigger: {}\nObserved: {}  Threshold: {}\nAction taken: {action}{}",
        session.provider,
        session.model.clone().unwrap_or_else(|| "model unknown".into()),
        session.foxx_session_id,
        event.scope.foxx_task_id.clone().unwrap_or_else(|| "-".into()),
        event.reasons.first().cloned().unwrap_or_else(|| event.event_type.clone()),
        observed.map_or("-".into(), |value| value.to_string()),
        if kind == "low_credits" { "low_credit_threshold" } else { "policy" },
        slack
            .as_ref()
            .and_then(|integration| integration.settings.get("dashboard_url").and_then(Value::as_str))
            .map(|url| format!("\nDashboard: {url}/#/sessions/{}", session.foxx_session_id))
            .unwrap_or_default()
    );
    let deliveries = if suppressed {
        Vec::new()
    } else {
        integrations::deliver(
            workspace,
            &integrations::Alert {
                text: &text,
                severity: integrations::severity_rank(&severity),
                issue: event.scope.external_task_id.as_deref(),
            },
        )
    };
    let record = json!({
        "alert_id": format!("alr_{}_{}", now, event.sequence),
        "key": key,
        "kind": kind,
        "title": title,
        "severity": severity,
        "at": now,
        "session": session.foxx_session_id,
        "event_id": event.event_id,
        "suppressed": suppressed,
        "state": "OPEN",
        "deliveries": deliveries,
    });
    append_line(&path, &record.to_string())
}

/** Read the recorded alerts, newest first
 * Input
    - stores: &Stores - runtime stores
 * Output
    - Vec<Value>
*/
pub(crate) fn list(stores: &Stores) -> Vec<Value> {
    let mut alerts = read_lines(&stores.directory.join("alerts.jsonl")).unwrap_or_default();
    alerts.reverse();
    alerts
}

/** Report whether alert delivery is configured, for compliance evidence
 * Input
    - workspace: &Workspace - repository
 * Output
    - bool
*/
pub(crate) fn configured(workspace: &Workspace) -> bool {
    integrations::load(workspace, "slack").is_some()
        || integrations::load(workspace, "whatsapp").is_some()
}
