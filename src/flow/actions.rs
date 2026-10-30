// The actions of a flow's subagents that call a service (flow/subagent.rs runs them): reading
// Datadog monitors and Grafana alerts of the flow, opening a Google Meet through Google Calendar,
// paging on-call (PagerDuty, Opsgenie, or a webhook), and opening or reviewing the pull request of
// the incident's designated branch on GitHub. Each uses the integration this machine keeps (its
// settings, and credentials that never leave the machine) and returns exactly what the service
// answered. Nothing is pretended: a service that is not connected, or that refuses, says so.

use serde_json::{json, Map, Value};

use super::subagent::SubagentConfig;
use super::watch::Incident;
use crate::governance::workspace::Workspace;
use crate::platform::http::request;

/** What one action did
 * Fields
    - result: &'static str - READ, ALERT, CREATED, PAGED, OPENED, EXISTS, REVIEWED, SKIPPED,
      NOT_CONNECTED, NOT_ALLOWED, or FAILED
    - detail: String - what the service answered
    - link: Option<String> - a link people should see (a meeting, a pull request)
*/
pub(crate) struct Outcome {
    pub(crate) result: &'static str,
    pub(crate) detail: String,
    pub(crate) link: Option<String>,
}

/** Build an outcome
 * Input
    - result: &'static str - result
    - detail: impl Into<String> - detail
 * Output
    - Outcome
*/
fn outcome(result: &'static str, detail: impl Into<String>) -> Outcome {
    Outcome {
        result,
        detail: detail.into(),
        link: None,
    }
}

/** What an action works with
 * Fields
    - workspace: &Workspace - repository
    - flow: &str - flow
    - watcher: &str - subagent id
    - headline: &str - one line about why it runs
    - incident: Option<&Incident> - the incident, when one started it
    - config: &SubagentConfig - the subagent's configuration (parameters, branch)
    - trace_files: &[String] - the files of the flow's approved trace
*/
pub(crate) struct Job<'a> {
    pub(crate) workspace: &'a Workspace,
    pub(crate) flow: &'a str,
    pub(crate) watcher: &'a str,
    pub(crate) headline: &'a str,
    pub(crate) incident: Option<&'a Incident>,
    pub(crate) config: &'a SubagentConfig,
    pub(crate) trace_files: &'a [String],
}

impl Job<'_> {
    /** A text parameter of the subagent
     * Input
        - key: &str - parameter
     * Output
        - Option<String>
    */
    fn parameter(&self, key: &str) -> Option<String> {
        self.config
            .parameters
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|text| !text.is_empty())
    }

    /** A key that names this incident (or this run) for de-duplication
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn dedup(&self) -> String {
        self.incident.map_or_else(
            || format!("crane-{}-{}", self.flow, self.watcher),
            |incident| format!("crane-{}", incident.incident_id),
        )
    }
}

/** An integration's settings and its credentials */
type Connection = (Map<String, Value>, Map<String, Value>);

/** An integration this machine keeps, with its credentials, when it is connected
 * Input
    - workspace: &Workspace - repository
    - name: &str - integration
 * Output
    - Result<Connection, Outcome> - settings and credentials, or why not
*/
fn connected(workspace: &Workspace, name: &str) -> Result<Connection, Outcome> {
    let Some(integration) = crate::integrations::load(workspace, name) else {
        return Err(outcome(
            "NOT_CONNECTED",
            format!("connect {name} on DataSea or with 'crane integrate {name}'"),
        ));
    };
    let secrets = crate::integrations::secrets(workspace, name);
    if let Some(schema) = crate::integrations::schema::schema(name) {
        let missing =
            crate::integrations::schema::missing_secrets(schema, &integration.settings, &secrets);
        if !missing.is_empty() {
            return Err(outcome("NOT_CONNECTED", format!("{name} has no {} on this machine; add it with 'crane integrate {name} --secrets-stdin' or in Landscape", missing.iter().map(|secret| secret.label).collect::<Vec<_>>().join(", "))));
        }
    }
    Ok((integration.settings, secrets))
}

/** Read a text value
 * Input
    - map: &Map<String, Value> - settings or credentials
    - key: &str - key
 * Output
    - String (empty when missing)
*/
fn text(map: &Map<String, Value>, key: &str) -> String {
    map.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/** Percent-encode a query value
 * Input
    - value: &str - text
 * Output
    - String
*/
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/** A service's refusal, briefly
 * Input
    - service: &str - service name
    - status: u16 - HTTP status
    - body: &str - answer
 * Output
    - Outcome (FAILED)
*/
fn refused(service: &str, status: u16, body: &str) -> Outcome {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            ["message", "error", "errors", "detail"]
                .iter()
                .find_map(|key| {
                    value.get(*key).map(|found| {
                        found
                            .as_str()
                            .map_or_else(|| found.to_string(), str::to_string)
                    })
                })
        })
        .unwrap_or_else(|| body.chars().take(200).collect());
    outcome(
        "FAILED",
        format!("{service} answered HTTP {status}: {message}"),
    )
}

/** Read the flow's observability: Datadog monitors tagged for it and Grafana alerts firing for it
 * (parameters: datadog_monitor_tags, default "flow:FLOW"; grafana_matcher, default flow="FLOW")
 * Input
    - job: &Job - the run
 * Output
    - Outcome - READ, or ALERT when a monitor alerts or an alert fires
*/
pub(crate) fn check_metrics(job: &Job) -> Outcome {
    let mut parts = Vec::new();
    let mut alerting = false;
    let mut reached = false;
    match connected(job.workspace, "datadog") {
        Ok((settings, secrets)) => {
            let tags = job
                .parameter("datadog_monitor_tags")
                .unwrap_or_else(|| format!("flow:{}", job.flow));
            let url = format!(
                "{}/api/v1/monitor?monitor_tags={}",
                text(&settings, "site"),
                encode(&tags)
            );
            match request(
                "GET",
                &url,
                &[
                    format!("DD-API-KEY: {}", text(&secrets, "api_key")),
                    format!("DD-APPLICATION-KEY: {}", text(&secrets, "app_key")),
                    "Accept: application/json".into(),
                ],
                None,
            ) {
                Ok((200, body)) => {
                    reached = true;
                    let monitors = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|value| value.as_array().cloned())
                        .unwrap_or_default();
                    let state = |name: &str| {
                        monitors
                            .iter()
                            .filter(|monitor| monitor["overall_state"] == name)
                            .count()
                    };
                    alerting |= state("Alert") > 0;
                    parts.push(format!(
                        "Datadog: {} monitor(s) tagged {tags}: {} alert, {} warn, {} ok",
                        monitors.len(),
                        state("Alert"),
                        state("Warn"),
                        state("OK")
                    ));
                }
                Ok((status, body)) => parts.push(refused("Datadog", status, &body).detail),
                Err(error) => parts.push(format!("Datadog did not answer: {error}")),
            }
        }
        Err(not) => parts.push(format!("Datadog: {}", not.detail)),
    }
    match connected(job.workspace, "grafana") {
        Ok((settings, secrets)) => {
            let matcher = job
                .parameter("grafana_matcher")
                .unwrap_or_else(|| format!("flow=\"{}\"", job.flow));
            let url = format!(
                "{}/api/alertmanager/grafana/api/v2/alerts?active=true&filter={}",
                text(&settings, "url"),
                encode(&matcher)
            );
            match request(
                "GET",
                &url,
                &[
                    format!("Authorization: Bearer {}", text(&secrets, "token")),
                    "Accept: application/json".into(),
                ],
                None,
            ) {
                Ok((200, body)) => {
                    reached = true;
                    let alerts = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|value| value.as_array().cloned())
                        .unwrap_or_default();
                    alerting |= !alerts.is_empty();
                    let names = alerts
                        .iter()
                        .filter_map(|alert| alert["labels"]["alertname"].as_str())
                        .take(5)
                        .collect::<Vec<_>>();
                    parts.push(format!(
                        "Grafana: {} alert(s) firing for {matcher}{}",
                        alerts.len(),
                        if names.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", names.join(", "))
                        }
                    ));
                }
                Ok((status, body)) => parts.push(refused("Grafana", status, &body).detail),
                Err(error) => parts.push(format!("Grafana did not answer: {error}")),
            }
        }
        Err(not) => parts.push(format!("Grafana: {}", not.detail)),
    }
    let detail = parts.join("; ");
    if !reached {
        return outcome(
            if detail.contains("answered HTTP") || detail.contains("did not answer") {
                "FAILED"
            } else {
                "NOT_CONNECTED"
            },
            detail,
        );
    }
    outcome(if alerting { "ALERT" } else { "READ" }, detail)
}

/** Open a Google Meet for the incident: a 30-minute event on the configured calendar with a Meet
 * conference (parameter meeting_minutes changes the length)
 * Input
    - job: &Job - the run
 * Output
    - Outcome - CREATED with the meeting link
*/
pub(crate) fn create_meeting(job: &Job) -> Outcome {
    let (settings, secrets) = match connected(job.workspace, "google_meet") {
        Ok(found) => found,
        Err(not) => return not,
    };
    let minutes = job
        .parameter("meeting_minutes")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30)
        .clamp(5, 240);
    let now = crate::platform::now_millis();
    let body = json!({
        "summary": format!("Incident: {}", job.headline.chars().take(120).collect::<String>()),
        "description": format!("Opened by Crane subagent {} of flow {}.", job.watcher, job.flow),
        "start": {"dateTime": crate::platform::render::iso_time(now)},
        "end": {"dateTime": crate::platform::render::iso_time(now + minutes * 60_000)},
        "conferenceData": {"createRequest": {"requestId": job.dedup(), "conferenceSolutionKey": {"type": "hangoutsMeet"}}},
    });
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events?conferenceDataVersion=1",
        encode(&text(&settings, "calendar"))
    );
    match request(
        "POST",
        &url,
        &[
            format!("Authorization: Bearer {}", text(&secrets, "token")),
            "Content-Type: application/json".into(),
        ],
        Some(&body.to_string()),
    ) {
        Ok((200, answer)) => {
            let event = serde_json::from_str::<Value>(&answer).unwrap_or(Value::Null);
            let link = event["hangoutLink"]
                .as_str()
                .or_else(|| event["htmlLink"].as_str())
                .map(str::to_string);
            Outcome {
                result: "CREATED",
                detail: format!(
                    "meeting for {minutes} minutes{}",
                    link.as_ref()
                        .map(|link| format!(": {link}"))
                        .unwrap_or_default()
                ),
                link,
            }
        }
        Ok((status, answer)) => refused("Google Calendar", status, &answer),
        Err(error) => outcome("FAILED", format!("Google Calendar did not answer: {error}")),
    }
}

/** Trigger the on-call status: PagerDuty Events v2, Opsgenie alerts, or any webhook, chosen by the
 * configured URL; one page per incident (de-duplicated by its id)
 * Input
    - job: &Job - the run
 * Output
    - Outcome - PAGED
*/
pub(crate) fn page_oncall(job: &Job) -> Outcome {
    let (settings, secrets) = match connected(job.workspace, "oncall") {
        Ok(found) => found,
        Err(not) => return not,
    };
    let url = text(&settings, "url");
    let token = text(&secrets, "token");
    let summary = format!("[{}] {}", job.flow, job.headline)
        .chars()
        .take(1000)
        .collect::<String>();
    let severity = job.incident.map_or("error", |incident| {
        if incident.code == "TRACE_DRIFT" {
            "warning"
        } else {
            "critical"
        }
    });
    let host = crate::platform::probe::host_and_port(&url)
        .map(|(host, _)| host)
        .unwrap_or_default();
    let (service, headers, body) = if host.ends_with("pagerduty.com") {
        (
            "PagerDuty",
            vec!["Content-Type: application/json".to_string()],
            json!({
                "routing_key": token, "event_action": "trigger", "dedup_key": job.dedup(),
                "payload": {"summary": summary, "source": "crane", "severity": severity, "component": job.flow, "custom_details": {"subagent": job.watcher}},
            }),
        )
    } else if host.ends_with("opsgenie.com") {
        (
            "Opsgenie",
            vec![
                "Content-Type: application/json".to_string(),
                format!("Authorization: GenieKey {token}"),
            ],
            json!({
                "message": summary.chars().take(130).collect::<String>(), "alias": job.dedup(), "description": summary,
                "priority": if severity == "critical" { "P1" } else { "P3" }, "tags": ["crane", job.flow], "source": "crane",
            }),
        )
    } else {
        (
            "the on-call webhook",
            vec![
                "Content-Type: application/json".to_string(),
                format!("Authorization: Bearer {token}"),
            ],
            json!({
                "event": "crane.page", "flow": job.flow, "subagent": job.watcher, "summary": summary, "severity": severity, "dedup_key": job.dedup(),
            }),
        )
    };
    match request("POST", &url, &headers, Some(&body.to_string())) {
        Ok((status, _)) if (200..300).contains(&status) => outcome(
            "PAGED",
            format!(
                "{service} accepted the page (HTTP {status}), key {}",
                job.dedup()
            ),
        ),
        Ok((status, answer)) => refused(service, status, &answer),
        Err(error) => outcome("FAILED", format!("{service} did not answer: {error}")),
    }
}

/** GitHub as this machine reaches it: the API, the repository, and an authorization header
 * Input
    - workspace: &Workspace - repository
    - needs_writing: bool - the action writes (a pull request or a comment)
 * Output
    - Result<(String, String, String, String), Outcome> - api, repository, header, features
*/
fn github(
    workspace: &Workspace,
    needs_writing: bool,
) -> Result<(String, String, String, String), Outcome> {
    let (settings, secrets) = connected(workspace, "github")?;
    let features = text(&settings, "features");
    if needs_writing && !matches!(features.as_str(), "pull_requests" | "comments") {
        return Err(outcome("NOT_ALLOWED", format!("the GitHub integration allows '{features}'; set 'What Crane may do' to pull_requests on DataSea or with 'crane integrate github'")));
    }
    let token = match text(&settings, "auth").as_str() {
        "fine_grained_pat" => text(&secrets, "token"),
        "gh_cli" => {
            let output = crate::platform::process::run(std::process::Command::new("gh").args(["auth", "token"]), std::time::Duration::from_secs(10)).map_err(|error| outcome("FAILED", error))?;
            if !output.success {
                return Err(outcome("NOT_CONNECTED", "the gh CLI is not logged in ('gh auth login')"));
            }
            output.stdout.trim().to_string()
        }
        other => return Err(outcome("NOT_ALLOWED", format!("GitHub authentication '{other}' cannot open pull requests from Crane yet; use a fine-grained token or the gh CLI"))),
    };
    Ok((
        text(&settings, "api_url"),
        text(&settings, "repository"),
        format!("Authorization: Bearer {token}"),
        features,
    ))
}

/** The open pull request of a branch, when there is one
 * Input
    - api, repository, auth: &str - GitHub as reached
    - branch: &str - head branch
 * Output
    - Result<Option<Value>, Outcome>
*/
fn open_pull(
    api: &str,
    repository: &str,
    auth: &str,
    branch: &str,
) -> Result<Option<Value>, Outcome> {
    let owner = repository.split('/').next().unwrap_or_default();
    let url = format!(
        "{api}/repos/{repository}/pulls?state=open&head={}",
        encode(&format!("{owner}:{branch}"))
    );
    match request(
        "GET",
        &url,
        &[
            auth.to_string(),
            "Accept: application/vnd.github+json".into(),
            "User-Agent: crane".into(),
        ],
        None,
    ) {
        Ok((200, body)) => Ok(serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value.as_array().and_then(|pulls| pulls.first().cloned()))),
        Ok((status, body)) => Err(refused("GitHub", status, &body)),
        Err(error) => Err(outcome("FAILED", format!("GitHub did not answer: {error}"))),
    }
}

/** Open the pull request of the incident's designated branch into its base (parameter
 * base_branch, default main); an open one is reported instead of a second
 * Input
    - job: &Job - the run
 * Output
    - Outcome - OPENED or EXISTS, with the link
*/
pub(crate) fn open_pull_request(job: &Job) -> Outcome {
    let Some(branch) = job.config.branch.clone() else {
        return outcome(
            "SKIPPED",
            "set the subagent's designated branch (configuration 'branch')",
        );
    };
    let (api, repository, auth, _) = match github(job.workspace, true) {
        Ok(found) => found,
        Err(not) => return not,
    };
    match open_pull(&api, &repository, &auth, &branch) {
        Ok(Some(pull)) => {
            let link = pull["html_url"].as_str().map(str::to_string);
            return Outcome {
                result: "EXISTS",
                detail: format!("pull request #{} of {branch} is open", pull["number"]),
                link,
            };
        }
        Ok(None) => {}
        Err(not) => return not,
    }
    let base = job
        .parameter("base_branch")
        .unwrap_or_else(|| "main".into());
    let body = json!({
        "title": format!("[{}] {}", job.flow, job.headline.chars().take(200).collect::<String>()),
        "head": branch, "base": base,
        "body": format!("Opened by Crane subagent {} of flow {}.\n\n{}", job.watcher, job.flow, job.incident.map(|incident| format!("Incident {} ({}): {}\n\n{}", incident.incident_id, incident.code, incident.summary, incident.detail)).unwrap_or_default()),
    });
    match request(
        "POST",
        &format!("{api}/repos/{repository}/pulls"),
        &[
            auth,
            "Accept: application/vnd.github+json".into(),
            "User-Agent: crane".into(),
            "Content-Type: application/json".into(),
        ],
        Some(&body.to_string()),
    ) {
        Ok((201, answer)) => {
            let pull = serde_json::from_str::<Value>(&answer).unwrap_or(Value::Null);
            let link = pull["html_url"].as_str().map(str::to_string);
            Outcome {
                result: "OPENED",
                detail: format!("pull request #{} from {branch} into {base}", pull["number"]),
                link,
            }
        }
        Ok((status, answer)) => refused("GitHub", status, &answer),
        Err(error) => outcome("FAILED", format!("GitHub did not answer: {error}")),
    }
}

/** Review the edits of the incident branch's open pull request against the flow: the files it
 * changes, and those outside the flow's approved trace, posted as a comment when the integration
 * may comment
 * Input
    - job: &Job - the run
 * Output
    - Outcome - REVIEWED
*/
pub(crate) fn review_edits(job: &Job) -> Outcome {
    let Some(branch) = job.config.branch.clone() else {
        return outcome(
            "SKIPPED",
            "set the subagent's designated branch (configuration 'branch')",
        );
    };
    let (api, repository, auth, features) = match github(job.workspace, false) {
        Ok(found) => found,
        Err(not) => return not,
    };
    let pull = match open_pull(&api, &repository, &auth, &branch) {
        Ok(Some(pull)) => pull,
        Ok(None) => return outcome("SKIPPED", format!("no pull request of {branch} is open")),
        Err(not) => return not,
    };
    let number = pull["number"].as_u64().unwrap_or_default();
    let files = match request(
        "GET",
        &format!("{api}/repos/{repository}/pulls/{number}/files?per_page=100"),
        &[
            auth.clone(),
            "Accept: application/vnd.github+json".into(),
            "User-Agent: crane".into(),
        ],
        None,
    ) {
        Ok((200, body)) => serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default(),
        Ok((status, body)) => return refused("GitHub", status, &body),
        Err(error) => return outcome("FAILED", format!("GitHub did not answer: {error}")),
    };
    let names = files
        .iter()
        .filter_map(|file| file["filename"].as_str())
        .collect::<Vec<_>>();
    let outside = if job.trace_files.is_empty() {
        Vec::new()
    } else {
        names
            .iter()
            .filter(|name| !job.trace_files.iter().any(|traced| traced == **name))
            .copied()
            .collect::<Vec<_>>()
    };
    let summary = format!(
        "{} file(s) changed in #{number}{}",
        names.len(),
        if job.trace_files.is_empty() {
            " (the flow has no approved trace to compare with)".to_string()
        } else if outside.is_empty() {
            ", all within the flow's traced files".to_string()
        } else {
            format!(
                ", {} outside the flow's traced files: {}",
                outside.len(),
                outside.join(", ")
            )
        }
    );
    let link = pull["html_url"].as_str().map(str::to_string);
    if matches!(features.as_str(), "comments" | "pull_requests") {
        let comment = json!({"body": format!("**Crane review of the edits (subagent {} of flow {})**\n\n{summary}.", job.watcher, job.flow)});
        if let Ok((status, answer)) = request(
            "POST",
            &format!("{api}/repos/{repository}/issues/{number}/comments"),
            &[
                auth,
                "Accept: application/vnd.github+json".into(),
                "User-Agent: crane".into(),
                "Content-Type: application/json".into(),
            ],
            Some(&comment.to_string()),
        ) {
            if status != 201 {
                return Outcome {
                    result: "REVIEWED",
                    detail: format!(
                        "{summary}; the comment was refused: {}",
                        refused("GitHub", status, &answer).detail
                    ),
                    link,
                };
            }
            return Outcome {
                result: "REVIEWED",
                detail: format!("{summary}; posted as a comment"),
                link,
            };
        }
    }
    Outcome {
        result: "REVIEWED",
        detail: summary,
        link,
    }
}

#[cfg(test)]
mod tests {
    use super::encode;

    /** Query values are percent-encoded
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn query_values_are_encoded() {
        assert_eq!(encode("flow:checkout"), "flow%3Acheckout");
        assert_eq!(encode("flow=\"pay\""), "flow%3D%22pay%22");
        assert_eq!(encode("acme:incident/pay"), "acme%3Aincident%2Fpay");
    }
}
