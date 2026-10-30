// What each integration a flow's subagents use keeps, declared so it can be set without prompts:
// from DataSea (the non-secret settings, through a request a person applies on a machine) and from
// Landscape or the command line ('crane integrate NAME --settings JSON', credentials on standard
// input). Every setting has a kind that is checked (an address must be an https URL and answer
// before it is kept, platform/probe.rs); credentials are only ever entered on the machine.

use serde_json::{json, Map, Value};

/** What a setting holds
 * Variants
    - Text - any non-empty text
    - Url - an address that must answer before it is kept
    - Choice - one of the listed values
    - Digits - digits only
    - ProjectKey - a Jira project key (PAY)
    - Channel - a Slack channel (#name or an id)
    - Repository - OWNER/NAME
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    Url,
    Choice(&'static [&'static str]),
    Digits,
    ProjectKey,
    Channel,
    Repository,
}

/** A non-secret setting
 * Fields
    - key: &'static str - name in the settings
    - label: &'static str - what it is
    - kind: Kind - what it holds
    - required: bool - it must be set (when it applies)
    - default: Option<&'static str> - used when not given
    - when: Option<(&'static str, &'static [&'static str])> - it applies only when that setting
      has one of these values
*/
pub(crate) struct Setting {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) kind: Kind,
    pub(crate) required: bool,
    pub(crate) default: Option<&'static str>,
    pub(crate) when: Option<(&'static str, &'static [&'static str])>,
}

/** A credential, entered only on the machine
 * Fields
    - key: &'static str - name in the secrets
    - label: &'static str - what it is
    - prefixes: &'static [&'static str] - how a valid one starts (empty: anything)
    - optional: bool - it may be left out
    - when: Option<(&'static str, &'static [&'static str])> - as for Setting
*/
pub(crate) struct Secret {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) prefixes: &'static [&'static str],
    pub(crate) optional: bool,
    pub(crate) when: Option<(&'static str, &'static [&'static str])>,
}

/** One integration's settings and credentials
 * Fields
    - name: &'static str - integration name
    - label: &'static str - shown name
    - purpose: &'static str - what subagents use it for
    - settings: &'static [Setting] - non-secret settings
    - secrets: &'static [Secret] - credentials
*/
pub(crate) struct Schema {
    pub(crate) name: &'static str,
    pub(crate) label: &'static str,
    pub(crate) purpose: &'static str,
    pub(crate) settings: &'static [Setting],
    pub(crate) secrets: &'static [Secret],
}

/** A setting, briefly */
const fn setting(
    key: &'static str,
    label: &'static str,
    kind: Kind,
    required: bool,
    default: Option<&'static str>,
    when: Option<(&'static str, &'static [&'static str])>,
) -> Setting {
    Setting {
        key,
        label,
        kind,
        required,
        default,
        when,
    }
}

/** A credential, briefly */
const fn secret(
    key: &'static str,
    label: &'static str,
    prefixes: &'static [&'static str],
    optional: bool,
    when: Option<(&'static str, &'static [&'static str])>,
) -> Secret {
    Secret {
        key,
        label,
        prefixes,
        optional,
        when,
    }
}

/** The integrations that can be set without prompts */
pub(crate) static SCHEMAS: &[Schema] = &[
    Schema {
        name: "slack",
        label: "Slack",
        purpose: "Messages to the flow's people (notify_people)",
        settings: &[
            setting(
                "mode",
                "Delivery",
                Kind::Choice(&["webhook", "bot"]),
                true,
                Some("webhook"),
                None,
            ),
            setting(
                "channel",
                "Channel (#name or channel ID)",
                Kind::Channel,
                true,
                None,
                Some(("mode", &["bot"])),
            ),
            setting(
                "min_severity",
                "Minimum alert severity",
                Kind::Choice(&["critical", "high", "medium", "low"]),
                true,
                Some("high"),
                None,
            ),
            setting(
                "dashboard_url",
                "DataSea URL for alert links",
                Kind::Url,
                false,
                None,
                None,
            ),
        ],
        secrets: &[
            secret(
                "webhook_url",
                "Incoming webhook URL",
                &["https://hooks.slack.com/services/"],
                false,
                Some(("mode", &["webhook"])),
            ),
            secret(
                "bot_token",
                "Bot token",
                &["xoxb-"],
                false,
                Some(("mode", &["bot"])),
            ),
            secret(
                "signing_secret",
                "Signing secret (interactive requests)",
                &[],
                true,
                None,
            ),
        ],
    },
    Schema {
        name: "jira",
        label: "Jira",
        purpose: "The task's ticket (read_jira_ticket) and incident issues",
        settings: &[
            setting(
                "deployment",
                "Deployment",
                Kind::Choice(&["cloud", "data_center"]),
                true,
                Some("cloud"),
                None,
            ),
            setting("site_url", "Jira site URL", Kind::Url, true, None, None),
            setting(
                "auth",
                "Authentication",
                Kind::Choice(&["api_token", "oauth2", "personal_access_token", "basic"]),
                true,
                Some("api_token"),
                None,
            ),
            setting(
                "user",
                "Account email or username",
                Kind::Text,
                true,
                None,
                Some(("auth", &["api_token", "basic"])),
            ),
            setting(
                "project_key",
                "Project key",
                Kind::ProjectKey,
                true,
                None,
                None,
            ),
            setting(
                "issue_type",
                "Issue type",
                Kind::Choice(&["Bug", "Task", "Story", "Security"]),
                true,
                Some("Bug"),
                None,
            ),
            setting(
                "api_version",
                "REST API version",
                Kind::Choice(&["3", "2"]),
                true,
                Some("3"),
                None,
            ),
        ],
        secrets: &[
            secret(
                "token",
                "API token, access token, or password",
                &[],
                false,
                None,
            ),
            secret(
                "webhook_secret",
                "Webhook secret for incoming events",
                &[],
                true,
                None,
            ),
        ],
    },
    Schema {
        name: "github",
        label: "GitHub",
        purpose:
            "Pull requests and reviews on the incident branch (open_pull_request, review_edits)",
        settings: &[
            setting(
                "auth",
                "Authentication",
                Kind::Choice(&["gh_cli", "fine_grained_pat", "github_app"]),
                true,
                Some("fine_grained_pat"),
                None,
            ),
            setting(
                "repository",
                "Repository (OWNER/NAME)",
                Kind::Repository,
                true,
                None,
                None,
            ),
            setting(
                "api_url",
                "API base URL",
                Kind::Url,
                true,
                Some("https://api.github.com"),
                None,
            ),
            setting(
                "app_id",
                "App ID",
                Kind::Digits,
                true,
                None,
                Some(("auth", &["github_app"])),
            ),
            setting(
                "installation_id",
                "Installation ID",
                Kind::Digits,
                true,
                None,
                Some(("auth", &["github_app"])),
            ),
            setting(
                "features",
                "What Crane may do",
                Kind::Choice(&["read", "comments", "statuses", "pull_requests"]),
                true,
                Some("read"),
                None,
            ),
        ],
        secrets: &[secret(
            "token",
            "Fine-grained token",
            &["github_pat_", "ghp_"],
            false,
            Some(("auth", &["fine_grained_pat"])),
        )],
    },
    Schema {
        name: "datadog",
        label: "Datadog",
        purpose: "Monitors of the flow (check_metrics)",
        settings: &[setting(
            "site",
            "Datadog API site",
            Kind::Url,
            true,
            Some("https://api.datadoghq.com"),
            None,
        )],
        secrets: &[
            secret("api_key", "API key", &[], false, None),
            secret("app_key", "Application key", &[], false, None),
        ],
    },
    Schema {
        name: "grafana",
        label: "Grafana",
        purpose: "Firing alerts of the flow (check_metrics)",
        settings: &[setting("url", "Grafana URL", Kind::Url, true, None, None)],
        secrets: &[secret("token", "Service account token", &[], false, None)],
    },
    Schema {
        name: "google_meet",
        label: "Google Meet",
        purpose: "A meeting for the incident (create_meeting), through Google Calendar",
        settings: &[setting(
            "calendar",
            "Calendar id",
            Kind::Text,
            true,
            Some("primary"),
            None,
        )],
        secrets: &[secret(
            "token",
            "OAuth access token (calendar events scope)",
            &[],
            false,
            None,
        )],
    },
    Schema {
        name: "oncall",
        label: "On-call paging",
        purpose: "Trigger the on-call status (page_oncall): PagerDuty, Opsgenie, or a webhook",
        settings: &[setting(
            "url",
            "Paging API or webhook URL",
            Kind::Url,
            true,
            Some("https://events.pagerduty.com/v2/enqueue"),
            None,
        )],
        secrets: &[secret(
            "token",
            "Routing key, API key, or webhook token",
            &[],
            false,
            None,
        )],
    },
];

/** Find an integration's schema
 * Input
    - name: &str - integration name
 * Output
    - Option<&'static Schema>
*/
pub(crate) fn schema(name: &str) -> Option<&'static Schema> {
    SCHEMAS.iter().find(|schema| schema.name == name)
}

/** Report whether a setting or credential applies, given the settings
 * Input
    - when: Option<(&str, &[&str])> - the condition
    - settings: &Map<String, Value> - settings
 * Output
    - bool
*/
pub(crate) fn applies(when: Option<(&str, &[&str])>, settings: &Map<String, Value>) -> bool {
    when.is_none_or(|(key, values)| {
        settings
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| values.contains(&value))
    })
}

/** Check one setting's value
 * Input
    - setting: &Setting - its declaration
    - value: &str - the value
 * Output
    - Result<(), String>
*/
fn check_value(setting: &Setting, value: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("{} ({}): {why}", setting.label, setting.key));
    match setting.kind {
        Kind::Text if value.trim().is_empty() => bad("a value is required"),
        Kind::Url => crate::platform::probe::syntax(value).or_else(|error| bad(&error)),
        Kind::Choice(values) if !values.contains(&value) => {
            bad(&format!("one of {}", values.join(", ")))
        }
        Kind::Digits
            if value.is_empty() || !value.chars().all(|character| character.is_ascii_digit()) =>
        {
            bad("digits only")
        }
        Kind::ProjectKey
            if !(value.len() >= 2
                && value.len() <= 10
                && value.starts_with(|character: char| character.is_ascii_uppercase())
                && value.chars().all(|character| {
                    character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
                })) =>
        {
            bad("2-10 uppercase letters, digits, or '_' (such as PAY)")
        }
        Kind::Channel
            if !(value.starts_with('#') || value.starts_with('C') || value.starts_with('G')) =>
        {
            bad("#channel-name or a channel ID such as C0123456789")
        }
        Kind::Repository
            if value.split('/').filter(|part| !part.is_empty()).count() != 2
                || value.matches('/').count() != 1 =>
        {
            bad("OWNER/NAME")
        }
        _ => Ok(()),
    }
}

/** Merge given settings into the current ones, fill defaults, and check everything: unknown keys,
 * values, required settings that apply, and that no credential is among them
 * Input
    - schema: &Schema - integration
    - current: &Map<String, Value> - settings kept now
    - given: &Map<String, Value> - settings to set ("" or null removes one)
 * Output
    - Result<Map<String, Value>, String> - the merged settings, or every problem
*/
pub(crate) fn merge_settings(
    schema: &Schema,
    current: &Map<String, Value>,
    given: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let mut problems = Vec::new();
    let mut merged = current.clone();
    for (key, value) in given {
        if schema.secrets.iter().any(|secret| secret.key == key) {
            problems.push(format!(
                "{key} is a credential; enter it on the machine, never in the settings"
            ));
            continue;
        }
        if !schema.settings.iter().any(|setting| setting.key == key) {
            problems.push(format!(
                "{key} is not a setting of {} (use {})",
                schema.name,
                schema
                    .settings
                    .iter()
                    .map(|setting| setting.key)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            continue;
        }
        match value {
            Value::Null => {
                merged.remove(key);
            }
            Value::String(text) if text.trim().is_empty() => {
                merged.remove(key);
            }
            Value::String(text) => {
                merged.insert(
                    key.clone(),
                    json!(text.trim().trim_end_matches('/').to_string()),
                );
            }
            Value::Number(number) => {
                merged.insert(key.clone(), json!(number.to_string()));
            }
            _ => problems.push(format!("{key} must be text")),
        }
    }
    for setting in schema.settings {
        if let (Some(default), false) = (setting.default, merged.contains_key(setting.key)) {
            merged.insert(setting.key.into(), json!(default));
        }
    }
    for setting in schema.settings {
        let applies = applies(setting.when, &merged);
        match merged.get(setting.key).and_then(Value::as_str) {
            Some(value) if applies => {
                if let Err(error) = check_value(setting, value) {
                    problems.push(error);
                }
            }
            None if applies && setting.required => {
                problems.push(format!("{} ({}) is required", setting.label, setting.key))
            }
            _ => {}
        }
    }
    if problems.is_empty() {
        Ok(merged)
    } else {
        Err(format!(
            "the {} settings are not valid:\n- {}",
            schema.label,
            problems.join("\n- ")
        ))
    }
}

/** Check credentials given on the machine against the schema
 * Input
    - schema: &Schema - integration
    - settings: &Map<String, Value> - settings (they decide which credentials apply)
    - given: &Map<String, Value> - credentials
 * Output
    - Result<(), String>
*/
pub(crate) fn check_secrets(
    schema: &Schema,
    settings: &Map<String, Value>,
    given: &Map<String, Value>,
) -> Result<(), String> {
    for (key, value) in given {
        let secret = schema
            .secrets
            .iter()
            .find(|secret| secret.key == key)
            .ok_or_else(|| format!("{key} is not a credential of {}", schema.name))?;
        if !applies(secret.when, settings) {
            return Err(format!("{key} does not apply with these settings"));
        }
        let text = value.as_str().unwrap_or_default();
        if !text.is_empty()
            && !secret.prefixes.is_empty()
            && !secret
                .prefixes
                .iter()
                .any(|prefix| text.starts_with(prefix))
        {
            return Err(format!(
                "{} must start with {}",
                secret.label,
                secret.prefixes.join(" or ")
            ));
        }
    }
    Ok(())
}

/** The credentials that apply and are not stored yet
 * Input
    - schema: &Schema - integration
    - settings: &Map<String, Value> - settings
    - secrets: &Map<String, Value> - credentials stored
 * Output
    - Vec<&'static Secret>
*/
pub(crate) fn missing_secrets(
    schema: &'static Schema,
    settings: &Map<String, Value>,
    secrets: &Map<String, Value>,
) -> Vec<&'static Secret> {
    schema
        .secrets
        .iter()
        .filter(|secret| !secret.optional && applies(secret.when, settings))
        .filter(|secret| {
            secrets
                .get(secret.key)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        })
        .collect()
}

/** Every schema as the dashboard and Landscape read it (no values)
 * Input
    - None
 * Output
    - Value
*/
pub(crate) fn schemas_json() -> Value {
    let when = |when: Option<(&str, &[&str])>| {
        when.map(|(key, values)| json!({"key": key, "values": values}))
    };
    json!(SCHEMAS
        .iter()
        .map(|schema| json!({
            "name": schema.name,
            "label": schema.label,
            "purpose": schema.purpose,
            "settings": schema.settings.iter().map(|setting| json!({
                "key": setting.key, "label": setting.label, "required": setting.required, "default": setting.default, "when": when(setting.when),
                "kind": match setting.kind { Kind::Choice(_) => "choice", Kind::Url => "url", Kind::Digits => "digits", Kind::ProjectKey => "project_key", Kind::Channel => "channel", Kind::Repository => "repository", Kind::Text => "text" },
                "choices": match setting.kind { Kind::Choice(values) => json!(values), _ => Value::Null },
            })).collect::<Vec<_>>(),
            "secrets": schema.secrets.iter().map(|secret| json!({"key": secret.key, "label": secret.label, "optional": secret.optional, "when": when(secret.when)})).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Settings merge with defaults and are checked; credentials are refused among them and listed
     * as missing until they are stored
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn integration_settings_are_checked_before_they_are_kept() {
        let jira = schema("jira").unwrap();
        let given = json!({"site_url": "https://acme.atlassian.net/", "user": "ana@acme.com", "project_key": "PAY"});
        let merged = merge_settings(jira, &Map::new(), given.as_object().unwrap()).unwrap();
        assert_eq!(merged["site_url"], "https://acme.atlassian.net");
        assert_eq!(
            merged["issue_type"], "Bug",
            "defaults fill what is not given"
        );
        let error = merge_settings(
            jira,
            &Map::new(),
            json!({"site_url": "http://jira", "token": "x", "project_key": "pay"})
                .as_object()
                .unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("is a credential"), "{error}");
        assert!(
            error.contains("must start with https://") || error.contains("https URL"),
            "{error}"
        );
        assert!(error.contains("2-10 uppercase"), "{error}");
        assert!(error.contains("(user) is required"), "{error}");
        let missing = missing_secrets(jira, &merged, &Map::new());
        assert_eq!(
            missing.iter().map(|secret| secret.key).collect::<Vec<_>>(),
            ["token"]
        );
        let slack = schema("slack").unwrap();
        let bot = merge_settings(
            slack,
            &Map::new(),
            json!({"mode": "bot", "channel": "#incidents"})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            missing_secrets(slack, &bot, &Map::new())
                .iter()
                .map(|secret| secret.key)
                .collect::<Vec<_>>(),
            ["bot_token"]
        );
        assert!(check_secrets(
            slack,
            &bot,
            json!({"bot_token": "nope"}).as_object().unwrap()
        )
        .is_err());
        assert!(check_secrets(
            slack,
            &bot,
            json!({"webhook_url": "https://hooks.slack.com/services/x"})
                .as_object()
                .unwrap()
        )
        .unwrap_err()
        .contains("does not apply"));
        assert!(check_secrets(
            slack,
            &bot,
            json!({"bot_token": "xoxb-1"}).as_object().unwrap()
        )
        .is_ok());
        assert_eq!(schemas_json().as_array().unwrap().len(), SCHEMAS.len());
    }
}
