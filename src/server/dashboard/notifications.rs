use serde_json::{json, Value};
use std::time::Duration;

const SLACK_WEBHOOK: &str = "CRANE_DASHBOARD_AGENT_SLACK_WEBHOOK_URL";
const RESEND_API_KEY: &str = "CRANE_DASHBOARD_AGENT_RESEND_API_KEY";
const EMAIL_FROM: &str = "CRANE_DASHBOARD_AGENT_EMAIL_FROM";
const EMAIL_TO: &str = "CRANE_DASHBOARD_AGENT_EMAIL_TO";

pub(crate) fn deliver(repository: &str, remote: &str, incident: &Value) -> Vec<Value> {
    let text = format!(
        "Crane Dashboard Agent incident: {} [{}]\nRepository: {} ({})\nReason: {}",
        incident["agent_name"].as_str().unwrap_or("Dashboard Agent"),
        incident["severity"].as_str().unwrap_or("UNKNOWN"),
        remote,
        repository,
        incident["reason"].as_str().unwrap_or("No reason supplied")
    );
    let subject = format!(
        "Crane incident [{}] - {}",
        incident["severity"].as_str().unwrap_or("UNKNOWN"),
        remote
    );
    let mut outcomes = Vec::new();
    outcomes.push(match std::env::var(SLACK_WEBHOOK) {
        Ok(url) if valid_slack_webhook(&url) => json!({
            "channel": "slack",
            "status": send_slack(&url, &text),
        }),
        Ok(_) => json!({"channel": "slack", "status": "failed", "error": "configured webhook URL is invalid"}),
        Err(_) => json!({"channel": "slack", "status": "not_configured"}),
    });
    outcomes.push(match (
        std::env::var(RESEND_API_KEY),
        std::env::var(EMAIL_FROM),
        std::env::var(EMAIL_TO),
    ) {
        (Ok(key), Ok(from), Ok(to)) if valid_email(&from) => {
            let recipients = to
                .split(',')
                .map(str::trim)
                .filter(|address| valid_email(address))
                .collect::<Vec<_>>();
            if recipients.is_empty() {
                json!({"channel": "email", "status": "failed", "error": "no valid recipient addresses configured"})
            } else {
                json!({
                    "channel": "email",
                    "status": send_email(&key, &from, &recipients, &subject, &text),
                })
            }
        }
        (Err(_), Err(_), Err(_)) => json!({"channel": "email", "status": "not_configured"}),
        _ => json!({"channel": "email", "status": "failed", "error": "Resend key, sender, and recipient settings must all be configured"}),
    });
    outcomes
}

fn valid_slack_webhook(url: &str) -> bool {
    url.starts_with("https://hooks.slack.com/services/")
        && url.len() <= 2048
        && !url.chars().any(char::is_whitespace)
}

fn valid_email(address: &str) -> bool {
    let address = address.trim();
    if address.len() > 254 || address.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = address.split('@');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(local), Some(domain), None)
            if !local.is_empty()
                && !domain.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
    )
}

fn send_slack(url: &str, text: &str) -> &'static str {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .redirects(0)
        .build();
    match agent
        .post(url)
        .set("Content-Type", "application/json")
        .send_json(json!({"text": text}))
    {
        Ok(response) if response.status() == 200 => "sent",
        Ok(_) | Err(_) => "failed",
    }
}

fn send_email(
    key: &str,
    from: &str,
    recipients: &[&str],
    subject: &str,
    text: &str,
) -> &'static str {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .redirects(0)
        .build();
    match agent
        .post("https://api.resend.com/emails")
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .send_json(json!({
            "from": from,
            "to": recipients,
            "subject": subject,
            "text": text,
        })) {
        Ok(response) if (200..300).contains(&response.status()) => "sent",
        Ok(_) | Err(_) => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::{valid_email, valid_slack_webhook};

    #[test]
    fn validates_notification_destinations_without_exposing_credentials() {
        assert!(valid_slack_webhook(
            "https://hooks.slack.com/services/T000/B000/secret"
        ));
        assert!(!valid_slack_webhook(
            "http://hooks.slack.com/services/T/B/C"
        ));
        assert!(valid_email("alerts@example.com"));
        assert!(!valid_email("not-an-email"));
        assert!(!valid_email("a@b"));
    }
}
