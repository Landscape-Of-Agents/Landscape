// Datadog, Grafana, Google Meet, and on-call paging: connection settings for the actions of flow
// subagents (flow/subagent.rs). Crane stores the settings and credentials like any integration, but
// does not call these services yet: verification checks that the settings are complete, and a
// subagent action that needs one of them is recorded as prepared, not run. None of them takes
// security alerts.

use serde_json::{json, Map, Value};

use super::*;
use crate::governance::workspace::Workspace;

/** One setting a connection asks for
 * Fields
    - key: &'static str - name in the settings or secrets
    - label: &'static str - prompt
    - default: Option<&'static str> - suggested value
    - secret: bool - stored with the credentials
    - url: bool - must be an https URL
*/
struct Field {
    key: &'static str,
    label: &'static str,
    default: Option<&'static str>,
    secret: bool,
    url: bool,
}

/** A connection Crane keeps settings for without calling the service yet
 * Fields
    - name: &'static str - integration name
    - fields: &'static [Field] - what it asks for
*/
pub(crate) struct Connection {
    name: &'static str,
    fields: &'static [Field],
}

/** Datadog: metrics and monitors a Monitor subagent reads */
pub(crate) static DATADOG: Connection = Connection {
    name: "datadog",
    fields: &[
        Field {
            key: "site",
            label: "Datadog site URL",
            default: Some("https://api.datadoghq.com"),
            secret: false,
            url: true,
        },
        Field {
            key: "api_key",
            label: "API key",
            default: None,
            secret: true,
            url: false,
        },
        Field {
            key: "app_key",
            label: "Application key",
            default: None,
            secret: true,
            url: false,
        },
    ],
};

/** Grafana: dashboards and alerts a Monitor subagent reads */
pub(crate) static GRAFANA: Connection = Connection {
    name: "grafana",
    fields: &[
        Field {
            key: "url",
            label: "Grafana URL",
            default: None,
            secret: false,
            url: true,
        },
        Field {
            key: "token",
            label: "Service account token",
            default: None,
            secret: true,
            url: false,
        },
    ],
};

/** Google Meet: the meeting a Monitor subagent opens for an incident */
pub(crate) static GOOGLE_MEET: Connection = Connection {
    name: "google_meet",
    fields: &[
        Field {
            key: "calendar",
            label: "Calendar id (the incident calendar)",
            default: Some("primary"),
            secret: false,
            url: false,
        },
        Field {
            key: "token",
            label: "OAuth access token with calendar events scope",
            default: None,
            secret: true,
            url: false,
        },
    ],
};

/** On-call paging (PagerDuty, Opsgenie, or a webhook): who a Monitor subagent pages */
pub(crate) static ONCALL: Connection = Connection {
    name: "oncall",
    fields: &[
        Field {
            key: "url",
            label: "Paging API or webhook URL",
            default: Some("https://events.pagerduty.com/v2/enqueue"),
            secret: false,
            url: true,
        },
        Field {
            key: "token",
            label: "Routing or integration key",
            default: None,
            secret: true,
            url: false,
        },
    ],
};

impl Provider for Connection {
    /** Return the integration's name
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str {
        self.name
    }

    /** Ask for every setting, validating each answer
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
        println!(
            "Crane keeps these settings for flow subagents; it does not call {} yet.",
            self.name
        );
        for field in self.fields {
            if field.secret {
                secrets.insert(
                    field.key.into(),
                    json!(prompter.secret(field.label, false, &required)?),
                );
            } else {
                let check: &dyn Fn(&str) -> Result<(), String> =
                    if field.url { &https_url } else { &required };
                let value = prompter.text(field.label, field.default, check)?;
                settings.insert(field.key.into(), json!(value.trim_end_matches('/')));
            }
        }
        Ok(())
    }

    /** Check that every setting and credential is present (the service is not called)
     * Input
        - integration: &Integration - saved settings
        - secrets: &Map<String, Value> - credentials
     * Output
        - Result<String, String>
    */
    fn verify(
        &self,
        integration: &Integration,
        secrets: &Map<String, Value>,
    ) -> Result<String, String> {
        for field in self.fields {
            let present = if field.secret {
                secrets.get(field.key)
            } else {
                integration.settings.get(field.key)
            }
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
            if !present {
                return Err(format!("{} is missing", field.label));
            }
        }
        Ok(format!(
            "settings complete; Crane does not call {} yet, so subagent actions that need it are recorded as prepared",
            self.name
        ))
    }
}
