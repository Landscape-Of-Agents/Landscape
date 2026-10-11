// Jira: the issue tracker an agent session's task (CRANE_TASK_ID) belongs to. Configuration asks
// for the site, API version, authentication, and project; verification reads the account and the
// project; high-severity alerts become comments on the session's issue when it belongs to the
// configured project.

use serde_json::{json, Map, Value};

use super::*;
use crate::governance::workspace::Workspace;
use crate::platform::http::request;

/** The Jira provider */
pub(crate) struct Jira;

impl Provider for Jira {
    /** Return the integration's name
     * Input
        - None (uses self)
     * Output
        - &'static str "jira"
    */
    fn name(&self) -> &'static str {
        "jira"
    }

    /** Ask for the Jira settings and credentials (see ask below)
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

    /** Check the saved Jira settings against the real service (see verify below)
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

    /** Deliver an alert when it passes this channel's rule: high severity or above, for a session linked to an issue
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
        let issue = alert.issue?;
        (alert.severity >= severity_rank("high"))
            .then(|| json!({"channel": "jira", "issue": issue, "result": deliver_text(workspace, integration, issue, alert.text)}))
    }
}

/** Ask for Jira Cloud or Data Center parameters
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
    let deployment = prompter.choice(
        "Jira deployment",
        &["Jira Cloud (*.atlassian.net)", "Jira Data Center / Server"],
        0,
    )?;
    let cloud = deployment == 0;
    settings.insert(
        "deployment".into(),
        json!(if cloud { "cloud" } else { "data_center" }),
    );
    let url = prompter.text("Jira site URL", None, &|value| {
        https_url(value)?;
        if cloud && !value.trim_end_matches('/').ends_with(".atlassian.net") {
            return Err("a Jira Cloud site ends with .atlassian.net".into());
        }
        Ok(())
    })?;
    settings.insert("site_url".into(), json!(url.trim_end_matches('/')));
    let auth_options: &[&str] = if cloud {
        &[
            "API token (account email + token)",
            "OAuth 2.0 (3LO) access token",
        ]
    } else {
        &[
            "Personal access token (bearer)",
            "Username and password (basic)",
        ]
    };
    let auth = prompter.choice("Authentication", auth_options, 0)?;
    let auth_name = match (cloud, auth) {
        (true, 0) => "api_token",
        (true, _) => "oauth2",
        (false, 0) => "personal_access_token",
        (false, _) => "basic",
    };
    settings.insert("auth".into(), json!(auth_name));
    if matches!(auth_name, "api_token" | "basic") {
        let user = prompter.text(
            if cloud { "Account email" } else { "Username" },
            None,
            &|value| {
                required(value)?;
                if cloud && !value.contains('@') {
                    return Err("enter the Atlassian account email".into());
                }
                Ok(())
            },
        )?;
        settings.insert("user".into(), json!(user));
    }
    let secret_label = match auth_name {
        "api_token" => "API token",
        "oauth2" => "OAuth access token",
        "personal_access_token" => "Personal access token",
        _ => "Password",
    };
    secrets.insert(
        "token".into(),
        json!(prompter.secret(secret_label, false, &required)?),
    );
    let project = prompter.text("Project key", None, &|value| {
        let valid = value.len() >= 2
            && value.len() <= 10
            && value.starts_with(|character: char| character.is_ascii_uppercase())
            && value.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            });
        if valid {
            Ok(())
        } else {
            Err("a project key is 2-10 uppercase letters, digits, or '_' (such as PAY)".into())
        }
    })?;
    settings.insert("project_key".into(), json!(project));
    let issue_types = ["Bug", "Task", "Story", "Security"];
    let issue = prompter.choice("Issue type for Crane findings", &issue_types, 0)?;
    settings.insert("issue_type".into(), json!(issue_types[issue]));
    let version = prompter.choice(
        "REST API version",
        &["3 (Cloud)", "2 (Data Center / Server)"],
        if cloud { 0 } else { 1 },
    )?;
    settings.insert(
        "api_version".into(),
        json!(if version == 0 { "3" } else { "2" }),
    );
    let webhook = prompter.secret("Webhook secret for incoming Jira events", true, &anything)?;
    if !webhook.is_empty() {
        secrets.insert("webhook_secret".into(), json!(webhook));
    }
    Ok(())
}

/** Check the account and the project
 * Input
    - integration: &Integration - saved settings
    - secrets: &Map<String, Value> - credentials
 * Output
    - Result<String, String> what was verified
*/
fn verify(integration: &Integration, secrets: &Map<String, Value>) -> Result<String, String> {
    let site = setting(integration, "site_url")?;
    let version = setting(integration, "api_version")?;
    let authorization =
        authorization(integration, secrets).ok_or("the Jira credentials are incomplete")?;
    let headers = [authorization, "Accept: application/json".to_string()];
    let (status, body) = request(
        "GET",
        &format!("{site}/rest/api/{version}/myself"),
        &headers,
        None,
    )?;
    if status != 200 {
        return Err(failed(status, &body));
    }
    let account = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value["displayName"].as_str().map(String::from))
        .unwrap_or_else(|| "the account".into());
    let project = setting(integration, "project_key")?;
    let (status, body) = request(
        "GET",
        &format!("{site}/rest/api/{version}/project/{project}"),
        &headers,
        None,
    )?;
    if status == 200 {
        Ok(format!(
            "authenticated to Jira as {account}; project {project} is accessible"
        ))
    } else {
        Err(format!(
            "authenticated as {account}, but project {project}: {}",
            failed(status, &body)
        ))
    }
}

/** Build the Authorization header of a Jira integration: Basic for API tokens and passwords,
 * Bearer for personal access tokens and OAuth tokens
 * Input
    - integration: &Integration - Jira settings
    - secrets: &Map<String, Value> - Jira secrets
 * Output
    - Option<String> header, None when incomplete
*/
pub(crate) fn authorization(
    integration: &Integration,
    secrets: &Map<String, Value>,
) -> Option<String> {
    let token = secrets.get("token")?.as_str()?;
    match integration.settings.get("auth").and_then(Value::as_str)? {
        "api_token" | "basic" => {
            let user = integration.settings.get("user")?.as_str()?;
            Some(format!(
                "Authorization: Basic {}",
                crate::platform::http::base64(format!("{user}:{token}").as_bytes())
            ))
        }
        _ => Some(format!("Authorization: Bearer {token}")),
    }
}

/** Deliver an alert as a comment on the Jira issue the session works on (CRANE_TASK_ID), when the
 * issue belongs to the configured project
 * Input
    - workspace: &Workspace - repository
    - jira: &Integration - Jira settings
    - issue: &str - issue key such as PAY-1821
    - text: &str - alert text
 * Output
    - String delivery result
*/
pub(super) fn deliver_text(
    workspace: &Workspace,
    jira: &Integration,
    issue: &str,
    text: &str,
) -> String {
    let project = jira
        .settings
        .get("project_key")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let valid = issue
        .strip_prefix(project)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|number| {
            !number.is_empty() && number.chars().all(|character| character.is_ascii_digit())
        });
    if !valid {
        return format!("skipped: {issue} is not an issue of project {project}");
    }
    let secrets = secrets(workspace, "jira");
    let (Some(authorization), Some(site), Some(version)) = (
        authorization(jira, &secrets),
        jira.settings.get("site_url").and_then(Value::as_str),
        jira.settings.get("api_version").and_then(Value::as_str),
    ) else {
        return "incomplete Jira configuration".into();
    };
    let body = if version == "3" {
        json!({"body": {"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]}})
    } else {
        json!({ "body": text })
    };
    send_alert(
        &format!("{site}/rest/api/{version}/issue/{issue}/comment"),
        &[authorization],
        &body,
    )
}
