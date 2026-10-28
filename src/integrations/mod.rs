// Integrations: Jira, WhatsApp (Cloud API), Slack, and GitHub. 'crane integrate NAME' asks for the
// parameters each service's API needs, validates them, and stores them in the trust directory:
// non-secret settings in integrations/NAME.json and credentials in secrets/NAME.json (owner-only),
// never in the repository, events, logs, or dashboard responses.
//
// Each service is a Provider (one module each): what to ask, how to verify the saved settings, and
// whether and how it takes a security alert. Adding a service means adding one module and one entry
// in PROVIDERS; storage, prompting, confirmation, and alert routing are shared.

mod github;
mod jira;
pub(crate) mod prompt;
mod slack;
mod whatsapp;

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::governance::workspace::Workspace;
use crate::platform::files::{restrict_to_owner, write_atomic};
use crate::platform::http::post_detached;
use crate::platform::{actor, io_error, now_unix};
use crate::trust::require_human;
use prompt::Prompter;

/** An alert ready to deliver
 * Fields
    - text: &'a str - the message
    - severity: u8 - severity rank (severity_rank)
    - issue: Option<&'a str> - the tracker issue of the session's task, when known
*/
pub(crate) struct Alert<'a> {
    pub(crate) text: &'a str,
    pub(crate) severity: u8,
    pub(crate) issue: Option<&'a str>,
}

/** A service Crane connects to */
pub(crate) trait Provider: Sync {
    /** The integration's name, as typed in 'crane integrate NAME'
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str;

    /** Ask for the settings and credentials, validating every answer
     * Input
        - prompter: &mut Prompter - prompts
        - settings: &mut Map<String, Value> - non-secret settings (filled)
        - secrets: &mut Map<String, Value> - credentials (filled)
        - workspace: &Workspace - repository
     * Output
        - Result<(), String>
        - Error if an answer is invalid or input ends
    */
    fn ask(
        &self,
        prompter: &mut Prompter,
        settings: &mut Map<String, Value>,
        secrets: &mut Map<String, Value>,
        workspace: &Workspace,
    ) -> Result<(), String>;

    /** Check saved settings against the real service
     * Input
        - integration: &Integration - saved settings
        - secrets: &Map<String, Value> - credentials
     * Output
        - Result<String, String> what was verified, or why it failed
    */
    fn verify(
        &self,
        integration: &Integration,
        secrets: &Map<String, Value>,
    ) -> Result<String, String>;

    /** Deliver a security alert, when this service takes it
     * Input
        - workspace: &Workspace - repository
        - integration: &Integration - saved settings
        - alert: &Alert - the alert
     * Output
        - Option<Value> the delivery record, None when this service does not take the alert
    */
    fn deliver(
        &self,
        workspace: &Workspace,
        integration: &Integration,
        alert: &Alert,
    ) -> Option<Value> {
        let _ = (workspace, integration, alert);
        None
    }
}

/** Every supported service, in the order alerts are delivered */
pub(crate) static PROVIDERS: &[&dyn Provider] = &[
    &slack::Slack,
    &jira::Jira,
    &whatsapp::WhatsApp,
    &github::GitHub,
];

/** Find a service by name
 * Input
    - name: &str - integration name (lowercase)
 * Output
    - Option<&'static dyn Provider>
*/
fn provider(name: &str) -> Option<&'static dyn Provider> {
    PROVIDERS
        .iter()
        .copied()
        .find(|provider| provider.name() == name)
}

/** Rank a severity name for comparisons: low 1, medium 2, high 3, critical 4, anything else 0
 * Input
    - name: &str - severity, any case
 * Output
    - u8
*/
pub(crate) fn severity_rank(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

/** Read a credential of an integration
 * Input
    - integration: &Integration - integration
    - secrets: &Map<String, Value> - its credentials
    - key: &str - credential name
 * Output
    - Result<String, String>
    - Error naming the missing credential
*/
fn secret(
    integration: &Integration,
    secrets: &Map<String, Value>,
    key: &str,
) -> Result<String, String> {
    secrets
        .get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| format!("the {} integration has no {key}", integration.name))
}

/** Describe a failed HTTP answer
 * Input
    - status: u16 - HTTP status
    - body: &str - answer body
 * Output
    - String
*/
fn failed(status: u16, body: &str) -> String {
    format!(
        "HTTP {status}: {}",
        body.chars().take(300).collect::<String>()
    )
}

/** Send an alert request without waiting for the answer, unless CRANE_ALERTS_DRY_RUN=1
 * Input
    - url: &str - endpoint
    - headers: &[String] - headers (may contain credentials)
    - body: &Value - JSON body
 * Output
    - String, "queued", "dry-run", or why it could not be sent
*/
fn send_alert(url: &str, headers: &[String], body: &Value) -> String {
    send(url, headers, body, false)
}

/** Send an alert request, waiting for the service's answer when asked (alert rules run in the
 * background sync, so they can wait and record whether the message was really accepted)
 * Input
    - url: &str - endpoint
    - headers: &[String] - headers (may contain credentials)
    - body: &Value - JSON body
    - wait: bool - wait for the answer
 * Output
    - String: "delivered (HTTP 200)", "queued", "dry-run", or "HTTP 4xx: …" and other failures
*/
fn send(url: &str, headers: &[String], body: &Value, wait: bool) -> String {
    if std::env::var("CRANE_ALERTS_DRY_RUN").is_ok_and(|value| value == "1") {
        return "dry-run".into();
    }
    if !wait {
        return match post_detached(url, headers, &body.to_string()) {
            Ok(()) => "queued".into(),
            Err(error) => error,
        };
    }
    let mut all = vec!["Content-Type: application/json".to_string()];
    all.extend(headers.iter().cloned());
    match crate::platform::http::request("POST", url, &all, Some(&body.to_string())) {
        // Slack's API answers 200 with "ok": false on errors
        Ok((status, answer))
            if (200..300).contains(&status) && !answer.contains("\"ok\":false") =>
        {
            format!("delivered (HTTP {status})")
        }
        Ok((status, answer)) => failed(status, &answer),
        Err(error) => error,
    }
}

/** Deliver a message to one channel now and record the outcome (alert rules)
 * Input
    - workspace: &Workspace - repository
    - channel: &str - slack or whatsapp
    - text: &str - message
 * Output
    - Value {channel, result, status, ms}
*/
pub(crate) fn deliver_now(workspace: &Workspace, channel: &str, text: &str) -> Value {
    let started = std::time::Instant::now();
    let result = match (channel, load(workspace, channel)) {
        (_, None) => format!("not configured; run 'crane integrate {channel}'"),
        ("slack", Some(integration)) => slack::deliver_text(workspace, &integration, text, true),
        ("whatsapp", Some(integration)) => {
            whatsapp::deliver_text(workspace, &integration, text, true)
        }
        (other, Some(_)) => format!("{other} does not take alerts; use slack or whatsapp"),
    };
    let status = result.split("HTTP ").nth(1).and_then(|rest| {
        rest.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u64>()
            .ok()
    });
    serde_json::json!({"channel": channel, "result": result, "status": status, "ms": started.elapsed().as_millis() as u64})
}

/** Read a string setting
 * Input
    - integration: &Integration - integration
    - key: &str - setting
 * Output
    - Result<String, String>
    - Error naming the missing setting
*/
fn setting(integration: &Integration, key: &str) -> Result<String, String> {
    integration
        .settings
        .get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| format!("the {} integration has no {key}", integration.name))
}

/** Verify a saved integration against the real service
 * Input
    - workspace: &Workspace - repository
    - integration: &Integration - saved settings
 * Output
    - Result<String, String> what was verified, or why it failed
*/
pub(crate) fn verify(workspace: &Workspace, integration: &Integration) -> Result<String, String> {
    let provider = provider(&integration.name)
        .ok_or_else(|| format!("unknown integration '{}'", integration.name))?;
    let result = provider.verify(integration, &secrets(workspace, &integration.name));
    crate::platform::notes::note(
        "INTEGRATION_TESTED",
        serde_json::json!({"integration": integration.name, "result": if result.is_ok() { "OK" } else { "FAILED" }}),
    );
    result
}

/** Deliver an alert to every configured service that takes it
 * Input
    - workspace: &Workspace - repository
    - alert: &Alert - the alert
 * Output
    - Vec<Value> delivery records
*/
pub(crate) fn deliver(workspace: &Workspace, alert: &Alert) -> Vec<Value> {
    PROVIDERS
        .iter()
        .filter_map(|provider| {
            let integration = load(workspace, provider.name())?;
            provider.deliver(workspace, &integration, alert)
        })
        .collect()
}

/** A stored integration (non-secret part)
 * Fields
    - name: String - integration name
    - settings: Map<String, Value> - non-secret settings
    - secret_fields: Vec<String> - names of the secrets stored separately
    - configured_at: u64 - Unix seconds
    - configured_by: String - actor
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Integration {
    pub(crate) name: String,
    pub(crate) settings: Map<String, Value>,
    pub(crate) secret_fields: Vec<String>,
    pub(crate) configured_at: u64,
    pub(crate) configured_by: String,
}

/** Paths of an integration's settings and secrets
 * Input
    - workspace: &Workspace - repository
    - name: &str - integration name
 * Output
    - Result<(PathBuf, PathBuf), String>
*/
fn paths(workspace: &Workspace, name: &str) -> Result<(PathBuf, PathBuf), String> {
    let directory = workspace.trust()?.repo_dir;
    Ok((
        directory.join("integrations").join(format!("{name}.json")),
        directory.join("secrets").join(format!("{name}.json")),
    ))
}

/** Load an integration's non-secret settings
 * Input
    - workspace: &Workspace - repository
    - name: &str - integration name
 * Output
    - Option<Integration>
*/
pub(crate) fn load(workspace: &Workspace, name: &str) -> Option<Integration> {
    let (settings, _) = paths(workspace, name).ok()?;
    serde_json::from_str(&fs::read_to_string(settings).ok()?).ok()
}

/** Load an integration's secrets (for delivery only; never returned by any API)
 * Input
    - workspace: &Workspace - repository
    - name: &str - integration name
 * Output
    - Map<String, Value>, empty when none
*/
pub(crate) fn secrets(workspace: &Workspace, name: &str) -> Map<String, Value> {
    let Ok((_, secrets)) = paths(workspace, name) else {
        return Map::new();
    };
    let mut found: Map<String, Value> = fs::read_to_string(secrets)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    // A credential an Environment Brick names for this integration, from this machine's vault
    if let Some(token) = crate::trust::vault::for_operation(workspace, name) {
        found.insert("token".into(), Value::String(token));
    }
    found
}

/** List the configured integrations (non-secret settings)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Vec<Integration>
*/
pub(crate) fn configured(workspace: &Workspace) -> Vec<Integration> {
    PROVIDERS
        .iter()
        .filter_map(|provider| load(workspace, provider.name()))
        .collect()
}

/** Require an https URL
 * Input
    - value: &str - candidate
 * Output
    - Result<(), String>
*/
fn https_url(value: &str) -> Result<(), String> {
    let rest = value
        .strip_prefix("https://")
        .ok_or("must start with https://")?;
    if rest.is_empty() || rest.starts_with('/') || rest.contains(char::is_whitespace) {
        return Err("must be a full https URL".into());
    }
    Ok(())
}

/** Require digits only
 * Input
    - value: &str - candidate
 * Output
    - Result<(), String>
*/
fn digits(value: &str) -> Result<(), String> {
    if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
        Ok(())
    } else {
        Err("must contain digits only".into())
    }
}

/** Require a non-empty value
 * Input
    - value: &str - candidate
 * Output
    - Result<(), String>
*/
fn required(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err("a value is required".into())
    } else {
        Ok(())
    }
}

/** Accept anything
 * Input
    - _value: &str - candidate
 * Output
    - Result<(), String>, always Ok
*/
fn anything(_value: &str) -> Result<(), String> {
    Ok(())
}

/** Configure an integration interactively and store it
 * Input
    - name: &str - jira, whatsapp, slack, or github
 * Output
    - Result<Integration, String>
    - Error if run by an agent, the name is unknown, an answer is invalid, or the user cancels
*/
pub(crate) fn configure(name: &str) -> Result<Integration, String> {
    let name = name.to_ascii_lowercase();
    let Some(provider) = provider(&name) else {
        let names = PROVIDERS
            .iter()
            .map(|provider| provider.name())
            .collect::<Vec<_>>();
        return Err(format!(
            "unknown integration '{name}'; use {}",
            names.join(", ")
        ));
    };
    require_human(&format!("crane integrate {name}"))?;
    let workspace = Workspace::discover()?;
    let mut prompter = Prompter::new();
    let mut settings = Map::new();
    let mut secrets = Map::new();
    println!(
        "Configure the {name} integration for {} (credentials are stored outside the repository)",
        workspace.root.display()
    );
    provider.ask(&mut prompter, &mut settings, &mut secrets, &workspace)?;
    println!("Summary:");
    for (key, value) in &settings {
        println!("  {key}: {value}");
    }
    for key in secrets.keys() {
        println!("  {key}: ******** (secret)");
    }
    if !prompter.confirm("Save this integration?", true)? {
        return Err("cancelled; nothing was saved".into());
    }
    let integration = Integration {
        name: name.clone(),
        settings,
        secret_fields: secrets.keys().cloned().collect(),
        configured_at: now_unix(),
        configured_by: actor(),
    };
    let (settings_path, secrets_path) = paths(&workspace, &name)?;
    write_atomic(
        &settings_path,
        serde_json::to_string_pretty(&integration)
            .map_err(io_error)?
            .as_bytes(),
    )?;
    write_atomic(
        &secrets_path,
        serde_json::to_string(&Value::Object(secrets))
            .map_err(io_error)?
            .as_bytes(),
    )?;
    restrict_to_owner(&secrets_path)?;
    if prompter
        .confirm("Check the connection now?", true)
        .unwrap_or(false)
    {
        match verify(&workspace, &integration) {
            Ok(message) => println!("Connection verified: {message}"),
            Err(error) => println!(
                "Connection check failed: {error}. The settings were saved; fix them with 'crane integrate {name}'."
            ),
        }
    }
    Ok(integration)
}

/** The label that ties a Jira issue to one resident step: "crane-" and 24 hex characters of the
 * step's idempotency key, so a retry or a reconciliation finds the issue instead of opening another
 * Input
    - key: &str - idempotency key
 * Output
    - String
*/
pub(crate) fn issue_label(key: &str) -> String {
    format!(
        "crane-{}",
        &crate::trust::crypto::digest("integrations/jira/label/v1", &[key.as_bytes()])[..24]
    )
}

/** What a Jira issue call answered
 * Variants
    - Created - the issue key
    - Found - an issue with the label already exists (its key)
    - Absent - no issue has the label
    - Refused - Jira refused (HTTP 4xx), so nothing was created
    - Unknown - the outcome is not known (5xx, timeout, transport error)
    - NotConfigured - Jira is not configured or incomplete
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IssueAnswer {
    Created(String),
    Found(String),
    Absent,
    Refused(String),
    Unknown(String),
    NotConfigured(String),
}

/** The settings an issue call needs: site, API version, project, issue type, Authorization header
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<(String, String, String, String, String), String>
*/
fn issue_settings(
    workspace: &Workspace,
) -> Result<(String, String, String, String, String), String> {
    let jira = load(workspace, "jira").ok_or("Jira is not configured ('crane integrate jira')")?;
    let text = |key: &str| {
        jira.settings
            .get(key)
            .and_then(Value::as_str)
            .map(String::from)
    };
    let authorization = jira::authorization(&jira, &secrets(workspace, "jira"))
        .ok_or("the Jira credentials are incomplete")?;
    match (text("site_url"), text("api_version"), text("project_key")) {
        (Some(site), Some(version), Some(project)) => Ok((
            site,
            version,
            project,
            text("issue_type").unwrap_or_else(|| "Task".into()),
            authorization,
        )),
        _ => Err("the Jira configuration is incomplete".into()),
    }
}

/** Classify a Jira answer that is not a success
 * Input
    - result: Result<(u16, String), String> - the HTTP answer
 * Output
    - IssueAnswer (Refused or Unknown)
*/
fn issue_failure(result: Result<(u16, String), String>) -> IssueAnswer {
    match result {
        Ok((status, body)) if (400..500).contains(&status) => {
            IssueAnswer::Refused(failed(status, &body))
        }
        Ok((status, body)) => IssueAnswer::Unknown(failed(status, &body)),
        Err(error) => IssueAnswer::Unknown(error),
    }
}

/** Find the issue that carries a label in the configured project
 * Input
    - workspace: &Workspace - repository
    - label: &str - label (issue_label)
 * Output
    - IssueAnswer (Found, Absent, Refused, Unknown, or NotConfigured)
*/
pub(crate) fn find_issue(workspace: &Workspace, label: &str) -> IssueAnswer {
    let (site, version, project, _, authorization) = match issue_settings(workspace) {
        Ok(settings) => settings,
        Err(error) => return IssueAnswer::NotConfigured(error),
    };
    let path = if version == "3" {
        "search/jql"
    } else {
        "search"
    };
    let body = json!({"jql": format!("project = \"{project}\" AND labels = \"{label}\""), "maxResults": 1, "fields": ["key"]});
    let headers = ["Content-Type: application/json".to_string(), authorization];
    match crate::platform::http::request(
        "POST",
        &format!("{site}/rest/api/{version}/{path}"),
        &headers,
        Some(&body.to_string()),
    ) {
        Ok((status, answer)) if (200..300).contains(&status) => {
            let value: Value = serde_json::from_str(&answer).unwrap_or(Value::Null);
            match value["issues"][0]["key"].as_str() {
                Some(key) => IssueAnswer::Found(key.into()),
                None => IssueAnswer::Absent,
            }
        }
        other => issue_failure(other),
    }
}

/** Create an issue in the configured project with a label (the caller looks the label up first)
 * Input
    - workspace: &Workspace - repository
    - summary: &str - summary (at most 250 characters are sent)
    - description: &str - description
    - label: &str - label (issue_label)
 * Output
    - IssueAnswer (Created, Refused, Unknown, or NotConfigured)
*/
pub(crate) fn create_issue(
    workspace: &Workspace,
    summary: &str,
    description: &str,
    label: &str,
) -> IssueAnswer {
    if std::env::var("CRANE_ALERTS_DRY_RUN").is_ok_and(|value| value == "1") {
        return IssueAnswer::Refused("CRANE_ALERTS_DRY_RUN is set: nothing was created".into());
    }
    let (site, version, project, issue_type, authorization) = match issue_settings(workspace) {
        Ok(settings) => settings,
        Err(error) => return IssueAnswer::NotConfigured(error),
    };
    let summary = summary.chars().take(250).collect::<String>();
    let description = if version == "3" {
        json!({"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": [{"type": "text", "text": description}]}]})
    } else {
        json!(description)
    };
    let body = json!({"fields": {"project": {"key": project}, "summary": summary, "description": description, "issuetype": {"name": issue_type}, "labels": [label]}});
    let headers = ["Content-Type: application/json".to_string(), authorization];
    match crate::platform::http::request(
        "POST",
        &format!("{site}/rest/api/{version}/issue"),
        &headers,
        Some(&body.to_string()),
    ) {
        Ok((status, answer)) if (200..300).contains(&status) => {
            let value: Value = serde_json::from_str(&answer).unwrap_or(Value::Null);
            match value["key"].as_str() {
                Some(key) => IssueAnswer::Created(key.into()),
                None => IssueAnswer::Unknown("Jira answered without an issue key".into()),
            }
        }
        other => issue_failure(other),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{json, Map, Value};

    use super::{provider, severity_rank, Alert, Integration, PROVIDERS};
    use crate::governance::workspace::Workspace;

    /** Build a saved integration with settings
     * Input
        - name: &str - integration name
        - settings: Value - settings object
     * Output
        - Integration
    */
    fn saved(name: &str, settings: Value) -> Integration {
        Integration {
            name: name.into(),
            settings: settings.as_object().cloned().unwrap_or_else(Map::new),
            secret_fields: Vec::new(),
            configured_at: 0,
            configured_by: "test".into(),
        }
    }

    /** Check that every provider is registered once and is found by name
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn providers_are_registered_once() {
        let mut names = PROVIDERS
            .iter()
            .map(|provider| provider.name())
            .collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PROVIDERS.len());
        assert_eq!(names, ["github", "jira", "slack", "whatsapp"]);
        assert!(provider("slack").is_some());
        assert!(provider("teams").is_none());
        assert!(severity_rank("CRITICAL") > severity_rank("high"));
        assert_eq!(severity_rank("unknown"), 0);
    }

    /** Check the routing rules that decide without sending anything: Slack below its minimum
     * severity, Jira without an issue (or below high), and GitHub never take an alert
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn providers_decline_alerts_outside_their_rules() {
        let workspace = Workspace {
            root: PathBuf::from("."),
            crane: PathBuf::from(".crane"),
            repository_id: "r".into(),
            remote: None,
            root_commits: Vec::new(),
            user: None,
            email: None,
            identity_cache: "NONE",
        };
        let alert = |severity: &str, issue: Option<&'static str>| Alert {
            text: "t",
            severity: severity_rank(severity),
            issue,
        };
        let slack = provider("slack").unwrap();
        let strict = saved("slack", json!({"min_severity": "critical"}));
        assert!(slack
            .deliver(&workspace, &strict, &alert("high", None))
            .is_none());
        let jira = provider("jira").unwrap();
        let tracker = saved("jira", json!({"project_key": "PAY"}));
        assert!(jira
            .deliver(&workspace, &tracker, &alert("critical", None))
            .is_none());
        assert!(jira
            .deliver(&workspace, &tracker, &alert("medium", Some("PAY-1")))
            .is_none());
        let github = provider("github").unwrap();
        assert!(github
            .deliver(
                &workspace,
                &saved("github", json!({})),
                &alert("critical", Some("PAY-1"))
            )
            .is_none());
    }
}
