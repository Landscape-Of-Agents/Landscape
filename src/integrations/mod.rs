// Integrations: Jira, WhatsApp (Cloud API), Slack, and GitHub. 'crane integrate NAME' asks for the
// parameters each service's API needs, validates them, and stores them in the trust directory:
// non-secret settings in integrations/NAME.json and credentials in secrets/NAME.json (owner-only),
// never in the repository, events, logs, or dashboard responses.

pub(crate) mod prompt;

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::governance::workspace::Workspace;
use crate::platform::files::{restrict_to_owner, write_atomic};
use crate::platform::{actor, io_error, now_unix};
use crate::trust::require_human;
use prompt::Prompter;

/** Supported integrations */
pub(crate) const NAMES: &[&str] = &["jira", "whatsapp", "slack", "github"];

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
    fs::read_to_string(secrets)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/** List the configured integrations (non-secret settings)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Vec<Integration>
*/
pub(crate) fn configured(workspace: &Workspace) -> Vec<Integration> {
    NAMES
        .iter()
        .filter_map(|name| load(workspace, name))
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

/** Ask for Jira Cloud or Data Center parameters
 * Input
    - prompter: &mut Prompter - prompts
    - settings: &mut Map<String, Value> - non-secret settings
    - secrets: &mut Map<String, Value> - secrets
 * Output
    - Result<(), String>
*/
fn jira(
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

/** Ask for WhatsApp Cloud API parameters
 * Input
    - prompter: &mut Prompter - prompts
    - settings: &mut Map<String, Value> - non-secret settings
    - secrets: &mut Map<String, Value> - secrets
 * Output
    - Result<(), String>
*/
fn whatsapp(
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

/** Ask for Slack parameters
 * Input
    - prompter: &mut Prompter - prompts
    - settings: &mut Map<String, Value> - non-secret settings
    - secrets: &mut Map<String, Value> - secrets
 * Output
    - Result<(), String>
*/
fn slack(
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

/** Ask for GitHub parameters
 * Input
    - prompter: &mut Prompter - prompts
    - settings: &mut Map<String, Value> - non-secret settings
    - secrets: &mut Map<String, Value> - secrets
    - workspace: &Workspace - repository (its connection supplies the default repository)
 * Output
    - Result<(), String>
*/
fn github(
    prompter: &mut Prompter,
    settings: &mut Map<String, Value>,
    secrets: &mut Map<String, Value>,
    workspace: &Workspace,
) -> Result<(), String> {
    let auth = prompter.choice(
        "Authentication",
        &[
            "GitHub CLI login (crane github)",
            "Fine-grained personal access token",
            "GitHub App installation",
        ],
        0,
    )?;
    settings.insert(
        "auth".into(),
        json!(["gh_cli", "fine_grained_pat", "github_app"][auth]),
    );
    let default_repository = crate::github::load(workspace)
        .ok()
        .flatten()
        .filter(|connection| !connection.normalized.starts_with("local:"))
        .and_then(|connection| {
            connection
                .normalized
                .split_once('/')
                .map(|(_, slug)| slug.to_string())
        });
    let repository = prompter.text(
        "Repository (OWNER/NAME)",
        default_repository.as_deref(),
        &|value| {
            let parts = value.split('/').collect::<Vec<_>>();
            if parts.len() == 2 && parts.iter().all(|part| !part.is_empty()) {
                Ok(())
            } else {
                Err("use OWNER/NAME".into())
            }
        },
    )?;
    settings.insert("repository".into(), json!(repository));
    let api = prompter.text(
        "API base URL (GitHub Enterprise: https://HOST/api/v3)",
        Some("https://api.github.com"),
        &https_url,
    )?;
    settings.insert("api_url".into(), json!(api.trim_end_matches('/')));
    match auth {
        1 => {
            secrets.insert(
                "token".into(),
                json!(prompter.secret("Fine-grained token", false, &|value| {
                    if value.starts_with("github_pat_") || value.starts_with("ghp_") {
                        Ok(())
                    } else {
                        Err("a GitHub token starts with github_pat_ or ghp_".into())
                    }
                })?),
            );
        }
        2 => {
            settings.insert(
                "app_id".into(),
                json!(prompter.text("App ID", None, &digits)?),
            );
            settings.insert(
                "installation_id".into(),
                json!(prompter.text("Installation ID", None, &digits)?),
            );
            settings.insert(
                "private_key_path".into(),
                json!(prompter.text(
                    "Path to the App private key (.pem, kept outside the repository)",
                    None,
                    &|value| {
                        let path = std::path::Path::new(value);
                        if !path.is_file() {
                            return Err("the file does not exist".into());
                        }
                        if path
                            .canonicalize()
                            .ok()
                            .zip(workspace.root.canonicalize().ok())
                            .is_some_and(|(key, root)| key.starts_with(root))
                        {
                            return Err("keep the private key outside the repository".into());
                        }
                        Ok(())
                    }
                )?),
            );
        }
        _ => {}
    }
    let features = prompter.choice(
        "What Crane may do",
        &[
            "Read only (pull requests, checks, workflows)",
            "Read and post pull-request comments with Crane reports",
            "Read and set commit statuses",
        ],
        0,
    )?;
    settings.insert(
        "features".into(),
        json!(["read", "comments", "statuses"][features]),
    );
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
    if !NAMES.contains(&name.as_str()) {
        return Err(format!(
            "unknown integration '{name}'; use jira, whatsapp, slack, or github"
        ));
    }
    require_human(&format!("crane integrate {name}"))?;
    let workspace = Workspace::discover()?;
    let mut prompter = Prompter::new();
    let mut settings = Map::new();
    let mut secrets = Map::new();
    println!(
        "Configure the {name} integration for {} (credentials are stored outside the repository)",
        workspace.root.display()
    );
    match name.as_str() {
        "jira" => jira(&mut prompter, &mut settings, &mut secrets)?,
        "whatsapp" => whatsapp(&mut prompter, &mut settings, &mut secrets)?,
        "slack" => slack(&mut prompter, &mut settings, &mut secrets)?,
        _ => github(&mut prompter, &mut settings, &mut secrets, &workspace)?,
    }
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

/** Build the Authorization header of a Jira integration: Basic for API tokens and passwords,
 * Bearer for personal access tokens and OAuth tokens
 * Input
    - integration: &Integration - Jira settings
    - secrets: &Map<String, Value> - Jira secrets
 * Output
    - Option<String> header, None when incomplete
*/
pub(crate) fn jira_authorization(
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

/** Verify a saved integration against the real service: Slack posts a test message, Jira checks
 * the account and the project, GitHub reads the repository, and WhatsApp reads the phone number
 * Input
    - workspace: &Workspace - repository
    - integration: &Integration - saved settings
 * Output
    - Result<String, String> what was verified, or why it failed
*/
pub(crate) fn verify(workspace: &Workspace, integration: &Integration) -> Result<String, String> {
    use crate::platform::http::request;
    let secrets = secrets(workspace, &integration.name);
    let secret = |key: &str| {
        secrets
            .get(key)
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| format!("the {} integration has no {key}", integration.name))
    };
    let failed = |status: u16, body: &str| {
        format!(
            "HTTP {status}: {}",
            body.chars().take(300).collect::<String>()
        )
    };
    match integration.name.as_str() {
        "slack" => {
            let text = "Crane is connected: this channel will receive Crane security alerts.";
            let (status, body) = match setting(integration, "mode")?.as_str() {
                "bot" => request(
                    "POST",
                    "https://slack.com/api/chat.postMessage",
                    &[
                        format!("Authorization: Bearer {}", secret("bot_token")?),
                        "Content-Type: application/json; charset=utf-8".into(),
                    ],
                    Some(
                        &json!({"channel": setting(integration, "channel")?, "text": text})
                            .to_string(),
                    ),
                )?,
                _ => request(
                    "POST",
                    &secret("webhook_url")?,
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
        "jira" => {
            let site = setting(integration, "site_url")?;
            let version = setting(integration, "api_version")?;
            let authorization = jira_authorization(integration, &secrets)
                .ok_or("the Jira credentials are incomplete")?;
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
        "github" => {
            let repository = setting(integration, "repository")?;
            match setting(integration, "auth")?.as_str() {
                "fine_grained_pat" => {
                    let (status, body) = request(
                        "GET",
                        &format!("{}/repos/{repository}", setting(integration, "api_url")?),
                        &[
                            format!("Authorization: Bearer {}", secret("token")?),
                            "Accept: application/vnd.github+json".into(),
                            "User-Agent: crane".into(),
                        ],
                        None,
                    )?;
                    if status == 200 {
                        Ok(format!("the token can read {repository}"))
                    } else {
                        Err(failed(status, &body))
                    }
                }
                "gh_cli" => {
                    let output = crate::platform::process::run(
                        std::process::Command::new("gh").args([
                            "api",
                            &format!("repos/{repository}"),
                            "--jq",
                            ".full_name",
                        ]),
                        std::time::Duration::from_secs(20),
                    )?;
                    if output.success {
                        Ok(format!("gh can read {}", output.stdout.trim()))
                    } else {
                        Err(output.stderr.trim().to_string())
                    }
                }
                _ => Ok("GitHub App credentials are stored; Crane does not mint installation tokens in 0.4".into()),
            }
        }
        _ => {
            let (status, body) = request(
                "GET",
                &format!(
                    "https://graph.facebook.com/{}/{}?fields=display_phone_number",
                    setting(integration, "api_version")?,
                    setting(integration, "phone_number_id")?
                ),
                &[format!("Authorization: Bearer {}", secret("access_token")?)],
                None,
            )?;
            if status == 200 {
                Ok("the WhatsApp Cloud API accepted the token for this phone number".into())
            } else {
                Err(failed(status, &body))
            }
        }
    }
}
