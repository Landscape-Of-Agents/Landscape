// WhatsApp (Cloud API): high-severity alerts sent as text or template messages to the configured
// recipients. Verification reads the phone number with the access token.

use serde_json::{json, Map, Value};

use super::*;
use crate::governance::workspace::Workspace;
use crate::platform::http::request;

/** The WhatsApp provider */
pub(crate) struct WhatsApp;

impl Provider for WhatsApp {
    /** Return the integration's name
     * Input
        - None (uses self)
     * Output
        - &'static str "whatsapp"
    */
    fn name(&self) -> &'static str {
        "whatsapp"
    }

    /** Ask for the WhatsApp settings and credentials (see ask below)
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

    /** Check the saved WhatsApp settings against the real service (see verify below)
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

    /** Deliver an alert when it passes this channel's rule: high severity or above
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
        (alert.severity >= severity_rank("high"))
            .then(|| json!({"channel": "whatsapp", "result": deliver_text(workspace, integration, alert.text, false)}))
    }
}

/** Ask for WhatsApp Cloud API parameters
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
    let versions = ["v21.0", "v20.0", "v19.0"];
    let version = prompter.choice("Graph API version", &versions, 0)?;
    settings.insert("api_version".into(), json!(versions[version]));
    settings.insert(
        "phone_number_id".into(),
        json!(prompter.text(
            "Phone number ID (WhatsApp Manager > API setup)",
            None,
            &digits
        )?),
    );
    settings.insert(
        "business_account_id".into(),
        json!(prompter.text("WhatsApp Business Account ID", None, &digits)?),
    );
    secrets.insert(
        "access_token".into(),
        json!(prompter.secret("Access token (system user, permanent)", false, &required)?),
    );
    let recipients = prompter.text(
        "Alert recipients (E.164, comma separated, such as +15551234567)",
        None,
        &|value| {
            let numbers = value.split(',').map(str::trim).collect::<Vec<_>>();
            if numbers.iter().all(|number| {
                number.starts_with('+')
                    && number.len() >= 8
                    && number[1..]
                        .chars()
                        .all(|character| character.is_ascii_digit())
            }) {
                Ok(())
            } else {
                Err("use international E.164 numbers such as +15551234567".into())
            }
        },
    )?;
    settings.insert(
        "recipients".into(),
        json!(recipients
            .split(',')
            .map(|number| number.trim().to_string())
            .collect::<Vec<_>>()),
    );
    let kind = prompter.choice(
        "Message type",
        &[
            "Template message (required outside the 24-hour customer window)",
            "Text message (only inside an open 24-hour window)",
        ],
        0,
    )?;
    settings.insert(
        "message_type".into(),
        json!(if kind == 0 { "template" } else { "text" }),
    );
    if kind == 0 {
        settings.insert(
            "template_name".into(),
            json!(prompter.text("Approved template name", None, &required)?),
        );
        settings.insert(
            "template_language".into(),
            json!(prompter.text("Template language code", Some("en_US"), &required)?),
        );
    }
    let verify = prompter.secret("Webhook verify token", true, &anything)?;
    if !verify.is_empty() {
        secrets.insert("webhook_verify_token".into(), json!(verify));
    }
    let app_secret = prompter.secret(
        "App secret (validates X-Hub-Signature-256)",
        true,
        &anything,
    )?;
    if !app_secret.is_empty() {
        secrets.insert("app_secret".into(), json!(app_secret));
    }
    Ok(())
}

/** Read the phone number with the access token
 * Input
    - integration: &Integration - saved settings
    - secrets: &Map<String, Value> - credentials
 * Output
    - Result<String, String> what was verified
*/
fn verify(integration: &Integration, secrets: &Map<String, Value>) -> Result<String, String> {
    let (status, body) = request(
        "GET",
        &format!(
            "https://graph.facebook.com/{}/{}?fields=display_phone_number",
            setting(integration, "api_version")?,
            setting(integration, "phone_number_id")?
        ),
        &[format!(
            "Authorization: Bearer {}",
            secret(integration, secrets, "access_token")?
        )],
        None,
    )?;
    if status == 200 {
        Ok("the WhatsApp Cloud API accepted the token for this phone number".into())
    } else {
        Err(failed(status, &body))
    }
}

/** Deliver an alert to WhatsApp recipients through the Cloud API
 * Input
    - workspace: &Workspace - repository
    - whatsapp: &Integration - WhatsApp settings
    - text: &str - alert text
 * Output
    - String delivery result
*/
pub(super) fn deliver_text(
    workspace: &Workspace,
    whatsapp: &Integration,
    text: &str,
    wait: bool,
) -> String {
    let secrets = secrets(workspace, "whatsapp");
    let (Some(token), Some(version), Some(phone)) = (
        secrets.get("access_token").and_then(Value::as_str),
        whatsapp.settings.get("api_version").and_then(Value::as_str),
        whatsapp
            .settings
            .get("phone_number_id")
            .and_then(Value::as_str),
    ) else {
        return "incomplete WhatsApp configuration".into();
    };
    let url = format!("https://graph.facebook.com/{version}/{phone}/messages");
    let mut results = Vec::new();
    for recipient in whatsapp
        .settings
        .get("recipients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let body = if whatsapp
            .settings
            .get("message_type")
            .and_then(Value::as_str)
            == Some("template")
        {
            json!({
                "messaging_product": "whatsapp",
                "to": recipient,
                "type": "template",
                "template": {
                    "name": whatsapp.settings.get("template_name"),
                    "language": {"code": whatsapp.settings.get("template_language")},
                    "components": [{"type": "body", "parameters": [{"type": "text", "text": text.chars().take(1000).collect::<String>()}]}],
                },
            })
        } else {
            json!({"messaging_product": "whatsapp", "to": recipient, "type": "text", "text": {"body": text}})
        };
        results.push(send(
            &url,
            &[format!("Authorization: Bearer {token}")],
            &body,
            wait,
        ));
    }
    results.join(", ")
}
