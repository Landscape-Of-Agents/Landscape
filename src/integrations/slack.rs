// Slack: security alerts posted through an incoming webhook or a bot token, with a minimum
// severity, a cooldown, and an optional dashboard link. Verification posts a test message.

use serde_json::{json, Map, Value};

use super::*;
use crate::governance::workspace::Workspace;
use crate::platform::http::request;

/** The Slack provider */
pub(crate) struct Slack;

impl Provider for Slack {
    /** Return the integration's name
     * Input
        - None (uses self)
     * Output
        - &'static str "slack"
    */
    fn name(&self) -> &'static str {
        "slack"
    }

    /** Ask for the Slack settings and credentials (see ask below)
     * Input
        - prompter, settings, secrets, workspace - see Provider::ask
     * Output
        - Result<(), String>
    */
    fn ask(
        &self,
        prompter: &mut Prompter,
        settings: &mut Map<String, Value>,
        secrets: &mut Map<String, Value>,
        _workspace: &Workspace,
    ) -> Result<(), String> {
        ask(prompter, settings, secrets)
    }

    /** Check the saved Slack settings against the real service (see verify below)
     * Input
        - integration: &Integration - saved settings
        - secrets: &Map<String, Value> - credentials
     * Output
        - Result<String, String> what was verified
    */
    fn verify(
        &self,
        integration: &Integration,
        secrets: &Map<String, Value>,
    ) -> Result<String, String> {
        verify(integration, secrets)
    }

    /** Deliver an alert when it passes this channel's rule: at least the configured minimum severity (default high)
     * Input
        - workspace: &Workspace - repository
        - integration: &Integration - saved settings
        - alert: &Alert - the alert
     * Output
        - Option<Value> the delivery record, None when the rule declines it
    */
    fn deliver(
        &self,
        workspace: &Workspace,
        integration: &Integration,
        alert: &Alert,
    ) -> Option<Value> {
        let minimum = integration
            .settings
            .get("min_severity")
            .and_then(Value::as_str)
            .unwrap_or("high");
        (alert.severity >= severity_rank(minimum))
            .then(|| json!({"channel": "slack", "result": deliver_text(workspace, integration, alert.text, false)}))
    }
}

/** Ask for Slack parameters
 * Input
    - prompter: &mut Prompter - prompts
    - settings: &mut Map<String, Value> - non-secret settings
    - secrets: &mut Map<String, Value> - secrets
 * Output
    - Result<(), String>
*/
fn ask(
    prompter: &mut Prompter,
    settings: &mut Map<String, Value>,
    secrets: &mut Map<String, Value>,
) -> Result<(), String> {
    let mode = prompter.choice(
        "Delivery method",
        &["Incoming webhook", "Bot token (chat.postMessage)"],
        0,
    )?;
    if mode == 0 {
        settings.insert("mode".into(), json!("webhook"));
        secrets.insert(
            "webhook_url".into(),
            json!(prompter.secret("Incoming webhook URL", false, &|value| {
                if value.starts_with("https://hooks.slack.com/services/") {
                    Ok(())
                } else {
                    Err("a Slack incoming webhook URL starts with https://hooks.slack.com/services/".into())
                }
            })?),
        );
    } else {
        settings.insert("mode".into(), json!("bot"));
        secrets.insert(
            "bot_token".into(),
            json!(prompter.secret("Bot token", false, &|value| {
                if value.starts_with("xoxb-") {
                    Ok(())
                } else {
                    Err("a bot token starts with xoxb-".into())
                }
            })?),
        );
        settings.insert(
            "channel".into(),
            json!(
                prompter.text("Channel (#name or channel ID)", None, &|value| {
                    if value.starts_with('#') || value.starts_with('C') || value.starts_with('G') {
                        Ok(())
                    } else {
                        Err("use #channel-name or a channel ID such as C0123456789".into())
                    }
                })?
            ),
        );
    }
    let signing = prompter.secret(
        "Signing secret (verifies interactive requests)",
        true,
        &anything,
    )?;
    if !signing.is_empty() {
        secrets.insert("signing_secret".into(), json!(signing));
    }
    let severities = ["critical", "high", "medium", "low"];
    let severity = prompter.choice(
        "Minimum alert severity",
        &["Critical", "High", "Medium", "Low"],
        1,
    )?;
    settings.insert("min_severity".into(), json!(severities[severity]));
    let dashboard = prompter.text(
        "Foxx dashboard URL for alert links (optional)",
        Some(""),
        &|value| {
            if value.is_empty() {
                Ok(())
            } else {
                https_url(value).or_else(|_| {
                    if value.starts_with("http://127.0.0.1")
                        || value.starts_with("http://localhost")
                    {
                        Ok(())
                    } else {
                        Err("use an https URL or a localhost URL".into())
                    }
                })
            }
        },
    )?;
    if !dashboard.is_empty() {
        settings.insert("dashboard_url".into(), json!(dashboard));
    }
    settings.insert("cooldown_seconds".into(), json!(600));
    Ok(())
}

/** Post a test message to the configured channel
 * Input
    - integration: &Integration - saved settings
    - secrets: &Map<String, Value> - credentials
 * Output
    - Result<String, String> what was verified
*/
fn verify(integration: &Integration, secrets: &Map<String, Value>) -> Result<String, String> {
    let text = "Crane is connected: this channel will receive Crane security alerts.";
    let (status, body) = match setting(integration, "mode")?.as_str() {
        "bot" => request(
            "POST",
            "https://slack.com/api/chat.postMessage",
            &[
                format!(
                    "Authorization: Bearer {}",
                    secret(integration, secrets, "bot_token")?
                ),
                "Content-Type: application/json; charset=utf-8".into(),
            ],
            Some(&json!({"channel": setting(integration, "channel")?, "text": text}).to_string()),
        )?,
        _ => request(
            "POST",
            &secret(integration, secrets, "webhook_url")?,
            &["Content-Type: application/json".into()],
            Some(&json!({ "text": text }).to_string()),
        )?,
    };
    let ok_body = body.trim() == "ok"
        || serde_json::from_str::<Value>(&body).is_ok_and(|value| value["ok"] == true);
    if status == 200 && ok_body {
        Ok("a test message was posted to Slack".into())
    } else {
        Err(failed(status, &body))
    }
}

/** Deliver an alert to Slack
 * Input
    - workspace: &Workspace - repository
    - slack: &Integration - Slack settings
    - text: &str - alert text
 * Output
    - String delivery result
*/
pub(super) fn deliver_text(
    workspace: &Workspace,
    slack: &Integration,
    text: &str,
    wait: bool,
) -> String {
    let secrets = secrets(workspace, "slack");
    match slack.settings.get("mode").and_then(Value::as_str) {
        Some("bot") => match (
            secrets.get("bot_token").and_then(Value::as_str),
            slack.settings.get("channel").and_then(Value::as_str),
        ) {
            (Some(token), Some(channel)) => send(
                "https://slack.com/api/chat.postMessage",
                &[format!("Authorization: Bearer {token}")],
                &json!({"channel": channel, "text": text}),
                wait,
            ),
            _ => "missing bot token or channel".into(),
        },
        _ => match secrets.get("webhook_url").and_then(Value::as_str) {
            Some(url) => send(url, &[], &json!({"text": text}), wait),
            None => "missing webhook URL".into(),
        },
    }
}
