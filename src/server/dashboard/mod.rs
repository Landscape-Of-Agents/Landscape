// The hosted Foxx dashboard: one dashboard for individuals and teams, served by the ingestion
// service at its root. People sign in with GitHub; a personal account sees the repositories it
// owns, and an organization member sees the organization's repositories, with every teammate's
// activity and a filter by member. Telemetry reads use a read-only database handle (data.rs);
// dashboard-agent configuration, incidents, quarantines, flow-context attachments, and approvals
// use narrow write methods (approve.rs).
//
// Narrow writes in approve.rs cover GitHub-confirmed linking/owner approval, repository-scoped
// Dashboard Agent configuration and activity, and action proposals. Admin-only Dashboard Agent
// changes use a fresh GitHub role check in the OAuth callback; its token is used once and never kept.
//
// Routes: GET /, /app.js, /charts.js, /glossary.js, /logo.png, /favicon.png, /auth/login,
// /auth/approve, /auth/agent-action, /auth/callback, /auth/logout,
// /api/me (the viewer, accounts, and repositories), /api/accounts/NAME (a summary of an account's
// repositories), and /api/ROUTE?repository=ID[&members=KEY,...] (api.rs). Responses carry a
// strict Content-Security-Policy; the page loads only its own script.

pub(crate) mod api;
pub(crate) mod approve;
pub(crate) mod auth;
pub(crate) mod data;
mod notifications;
pub(crate) mod stream;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tiny_http::Request;

use super::http::{self, Reply};
use crate::server::service_events::{self, ServiceLog};
use crate::trust::crypto::{hex, is_digest, random_bytes, random_id};
use approve::Approvals;
use auth::{Auth, Intent, Viewer, SESSION_MILLIS};
use data::{Context, ReadOnly};

/** The page */
const PAGE: &str = include_str!("app.html");

/** The page's script */
const SCRIPT: &str = include_str!("app.js");

/** The page's chart library (SVG; the page loads no other script) */
const CHARTS: &str = include_str!("charts.js");

/** The glossary the info icons read (generated from DASHBOARD.md) */
const GLOSSARY: &str = include_str!("glossary.js");

/** The pages that watch agents (monitor, AI agents, investigate) */
const PAGES: &str = include_str!("pages.js");

/** The pages that operate and govern, and the page's start */
const GOVERN: &str = include_str!("govern.js");

/** The Foxx logo (256 px, transparent background) */
const LOGO: &[u8] = include_bytes!("logo.png");

/** The browser tab icon (64 px) */
const FAVICON: &[u8] = include_bytes!("favicon.png");
const MAX_DASHBOARD_AGENT_BODY: usize = 64 * 1024;

/** When each repository last received an upload, so its cached views are dropped at once */
static UPLOADS: std::sync::LazyLock<Mutex<HashMap<String, Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/** Record that a repository received an upload: the dashboard reloads it on the next request
 * Input
    - repository: &str - repository identity
 * Output
    - None
*/
pub(crate) fn uploaded(repository: &str) {
    if let Ok(mut uploads) = UPLOADS.lock() {
        if uploads.len() > 10_000 {
            uploads.clear();
        }
        uploads.insert(repository.to_string(), Instant::now());
    }
}

/** How long a loaded repository view is reused */
const CACHE_TTL: Duration = Duration::from_secs(20);

/** Most cached repository views */
const CACHE_SIZE: usize = 16;

/** The dashboard
 * Fields
    - read: ReadOnly - read-only database handle
    - approvals: Approvals - confirms approval requests (the only writes to records)
    - log: ServiceLog - records the dashboard's events (DASHBOARD.md F00, F02, F30)
    - auth: Option<Auth> - GitHub sign-in, None when not configured
    - cache: Mutex<HashMap<String, (Instant, Arc<Context>)>> - recently loaded repository views
*/
pub(crate) struct Dashboard {
    read: ReadOnly,
    approvals: Approvals,
    log: ServiceLog,
    auth: Option<Auth>,
    cache: Mutex<HashMap<String, (Instant, Arc<Context>)>>,
}

impl Dashboard {
    /** Create the dashboard
     * Input
        - read: ReadOnly - read-only database handle
        - approvals: Approvals - approval writer
        - log: ServiceLog - event log
        - auth: Option<Auth> - sign-in settings
     * Output
        - Dashboard
    */
    pub(crate) fn new(
        read: ReadOnly,
        approvals: Approvals,
        log: ServiceLog,
        auth: Option<Auth>,
    ) -> Self {
        Self {
            read,
            approvals,
            log,
            auth,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /** Report whether sign-in is configured
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn configured(&self) -> bool {
        self.auth.is_some()
    }

    pub(crate) fn codeowner(&self, token: &str, remote: &str) -> Result<String, String> {
        self.auth
            .as_ref()
            .ok_or_else(|| "GitHub sign-in must be configured on the service".to_string())?
            .codeowner(token, remote)
    }

    /** Evaluate the configured agents for a repository from recorded telemetry. */
    pub(crate) fn evaluate_dashboard_agents(
        &self,
        repository: &str,
        remote: &str,
        actor: &str,
        trigger: &str,
    ) -> Result<Value, String> {
        let configured_agents = self.read.find(
            "dashboard_agents",
            mongodb::bson::doc! {"repository_id": repository},
            mongodb::bson::doc! {},
            1,
        )?;
        if configured_agents.is_empty() {
            return Ok(json!({
                "skipped": true,
                "reason": "no dashboard agents are configured for this repository",
                "evaluated": 0,
                "incidents_opened": 0,
            }));
        }
        let now = crate::platform::now_millis();
        let lease_owner = random_id()?;
        if !self
            .approvals
            .acquire_dashboard_agent_lease(repository, &lease_owner, now)?
        {
            return Ok(
                json!({"skipped": true, "reason": "another service worker owns this repository evaluation"}),
            );
        }
        let result =
            self.evaluate_dashboard_agents_with_lease(repository, remote, actor, trigger, now);
        let result = match result {
            Ok(value) => Ok(value),
            Err(error) => match self
                .approvals
                .record_dashboard_agent_evaluation_failure(repository, actor, trigger, &error)
            {
                Ok(()) => Err(error),
                Err(log_error) => Err(format!(
                    "{error}; failed to persist the evaluation failure record: {log_error}"
                )),
            },
        };
        let release = self
            .approvals
            .release_dashboard_agent_lease(repository, &lease_owner);
        match (result, release) {
            (Ok(value), Ok(())) => {
                uploaded(repository);
                Ok(value)
            }
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(format!(
                "agent evaluation completed but its lease could not be released: {error}"
            )),
        }
    }

    fn evaluate_dashboard_agents_with_lease(
        &self,
        repository: &str,
        remote: &str,
        actor: &str,
        trigger: &str,
        now: u64,
    ) -> Result<Value, String> {
        let context = self.context(repository, remote, &BTreeSet::new())?;
        let candidates = api::agents::evaluate(&context, now);
        let measurements = api::agents::measurements(&context, now);
        for measurement in &measurements {
            let agent_id = measurement["agent_id"].as_str().unwrap_or_default();
            self.approvals.record_dashboard_agent_evaluation(
                repository,
                actor,
                trigger,
                approve::DashboardAgentEvaluation {
                    agent_id,
                    observed: measurement["observed"].as_u64().unwrap_or_default(),
                    threshold: measurement["threshold"].as_u64().unwrap_or_default(),
                    triggered: measurement["triggered"].as_bool().unwrap_or(false),
                    evaluated_at: now,
                },
            )?;
        }
        let mut opened = 0;
        for incident in &candidates {
            if self
                .approvals
                .record_dashboard_incident(repository, actor, incident)?
            {
                opened += 1;
                self.notify_dashboard_incident(repository, remote, incident);
            }
        }
        Ok(json!({
            "evaluated": measurements.len(),
            "triggered": candidates.len(),
            "opened": opened,
            "skipped": false,
            "trigger": trigger,
            "evaluated_at": now,
        }))
    }

    fn notify_dashboard_incident(&self, repository: &str, remote: &str, incident: &Value) {
        let outcomes = notifications::deliver(repository, remote, incident);
        let sent = outcomes
            .iter()
            .filter(|outcome| outcome["status"] == "sent")
            .count();
        let failed = outcomes
            .iter()
            .filter(|outcome| outcome["status"] == "failed")
            .count();
        let mut spec = service_events::dashboard(
            "DASHBOARD_AGENT_INCIDENT_NOTIFICATIONS",
            "F35",
            "INCIDENT_NOTIFY",
            Some("system:notifications"),
            Some((repository, remote)),
        );
        spec.respond(
            if failed > 0 {
                "PARTIAL"
            } else if sent > 0 {
                "OK"
            } else {
                "NOT_CONFIGURED"
            },
            if failed > 0 { "DELIVERY_FAILED" } else { "NA" },
            "Dashboard Agent incident notification delivery was attempted.",
        )
        .set("other", "notification_outcomes", &outcomes)
        .set("other", "incident_reason", &incident["reason"])
        .severity(if failed > 0 { "MEDIUM" } else { "LOW" }, "OUTCOME", "NONE");
        self.log.record(spec, Some(repository), None);
    }

    /** Run a periodic recovery sweep for repositories with configured agents. */
    pub(crate) fn sweep_dashboard_agents(&self, trigger: &str) {
        let agents = match self.read.find(
            "dashboard_agents",
            mongodb::bson::doc! {},
            mongodb::bson::doc! {"repository_id": 1},
            10_000,
        ) {
            Ok(agents) => agents,
            Err(error) => {
                super::log::write(
                    "error",
                    "dashboard_agents",
                    None,
                    "sweep_failed",
                    json!({"error": error}),
                );
                return;
            }
        };
        let configured = agents
            .iter()
            .filter_map(|agent| agent.get_str("repository_id").ok())
            .collect::<BTreeSet<_>>();
        let repositories = match data::verified_repositories(&self.read) {
            Ok(repositories) => repositories,
            Err(error) => {
                super::log::write(
                    "error",
                    "dashboard_agents",
                    None,
                    "sweep_failed",
                    json!({"error": error}),
                );
                return;
            }
        };
        for (repository, remote) in repositories {
            if !configured.contains(repository.as_str()) {
                continue;
            }
            if let Err(error) =
                self.evaluate_dashboard_agents(&repository, &remote, "system:monitor", trigger)
            {
                super::log::write(
                    "error",
                    "dashboard_agents",
                    None,
                    "evaluation_failed",
                    json!({"repository_id": repository, "trigger": trigger, "error": error}),
                );
            }
        }
    }

    /** Evaluate after one verified repository has received a telemetry batch. */
    pub(crate) fn evaluate_uploaded_agents(&self, repository: &str) -> Result<Value, String> {
        let document = self
            .read
            .find(
                "repositories",
                mongodb::bson::doc! {"repository_id": repository, "verified": true},
                mongodb::bson::doc! {},
                1,
            )?
            .into_iter()
            .next()
            .ok_or_else(|| "uploaded repository does not yet have a verified remote".to_string())?;
        let remote = document
            .get_str("remote")
            .map_err(|error| error.to_string())?;
        self.evaluate_dashboard_agents(repository, remote, "system:monitor", "telemetry_upload")
    }

    /** Answer a dashboard request
     * Input
        - request: &Request - request
     * Output
        - Reply
    */
    pub(crate) fn handle(&self, request: &mut Request) -> Reply {
        let (path, query) = http::split(request.url());
        if request.method().as_str().eq_ignore_ascii_case("POST") && path == "/api/dashboard-agents"
        {
            let content_type = http::header(request, "Content-Type").unwrap_or_default();
            if !content_type
                .to_ascii_lowercase()
                .starts_with("application/json")
            {
                return Reply::json(
                    415,
                    json!({"error": "Dashboard Agent changes require application/json"}),
                );
            }
            if request
                .body_length()
                .is_some_and(|length| length > MAX_DASHBOARD_AGENT_BODY)
            {
                return Reply::json(413, json!({"error": "request body too large"}));
            }
            let mut body = Vec::new();
            if request
                .as_reader()
                .take(MAX_DASHBOARD_AGENT_BODY as u64 + 1)
                .read_to_end(&mut body)
                .is_err()
                || body.len() > MAX_DASHBOARD_AGENT_BODY
            {
                return Reply::json(413, json!({"error": "request body could not be read"}));
            }
            return match self.dashboard_agents(request, &query, &body) {
                Ok(value) => Reply::json(200, value),
                Err((status, message)) => Reply::json(status, json!({"error": message})),
            };
        }
        if !request.method().as_str().eq_ignore_ascii_case("GET") {
            return Reply::json(
                405,
                json!({"error": "this dashboard route does not accept writes"}),
            );
        }
        match path.as_str() {
            "/" | "/index.html" => Reply::page(PAGE),
            "/app.js" => Reply::script(SCRIPT),
            "/charts.js" => Reply::script(CHARTS),
            "/glossary.js" => Reply::script(GLOSSARY),
            "/pages.js" => Reply::script(PAGES),
            "/govern.js" => Reply::script(GOVERN),
            "/logo.png" => Reply::image(LOGO, "image/png"),
            "/favicon.png" | "/favicon.ico" => Reply::image(FAVICON, "image/png"),
            "/auth/login" => self.login(request),
            "/auth/approve" => self.approve(request, &query),
            "/auth/agent-action" => self.start_agent_approval(request, &query),
            "/auth/grant" => self.start_grant(request, &query),
            "/auth/callback" => self.callback(request, &query),
            "/auth/logout" => {
                // F00.8
                let viewer = self.auth.as_ref().and_then(|auth| auth.viewer(request));
                let mut spec = service_events::dashboard(
                    "DASH_SIGNED_OUT",
                    "F00",
                    "SIGN_OUT",
                    viewer.as_ref().map(|viewer| viewer.login.as_str()),
                    None,
                );
                spec.respond("OK", "NA", "Signed out of the dashboard.")
                    .severity("INFO", "FIXED", "NONE");
                if let Some(viewer) = &viewer {
                    spec.set(
                        "eval",
                        "session_duration_ms",
                        (auth::SESSION_MILLIS).saturating_sub(
                            viewer.expires.saturating_sub(crate::platform::now_millis()),
                        ),
                    );
                }
                self.log.record(spec, None, None);
                Reply::redirect("/")
                    .with_header("Set-Cookie", &Auth::cookie(request, "foxx_session", "", 0))
            }
            _ if path.starts_with("/api/") => {
                let (status, body, download) = self.api(request, &path["/api/".len()..], &query);
                match (download, body) {
                    // A download is sent as the file itself
                    (Some(name), Value::String(text)) => Reply::download(text, &name),
                    (_, body) => Reply::json(status, body),
                }
            }
            _ => Reply::json(404, json!({"error": "not found"})),
        }
    }

    fn dashboard_agents(
        &self,
        request: &Request,
        query: &BTreeMap<String, String>,
        body: &[u8],
    ) -> Result<Value, (u16, String)> {
        let auth = self
            .auth
            .as_ref()
            .ok_or_else(|| (503, "dashboard sign-in is not configured".into()))?;
        let viewer = auth
            .viewer(request)
            .ok_or_else(|| (401, "sign in with GitHub".into()))?;
        let repository = query.get("repository").cloned().unwrap_or_default();
        let repositories = data::verified_repositories(&self.read)
            .map_err(|_| (503, "repository access could not be verified".into()))?;
        let (_, remote) = repositories
            .iter()
            .find(|(id, remote)| *id == repository && viewer.allowed(remote))
            .ok_or_else(|| {
                (
                    403,
                    "choose a repository visible to this GitHub account".into(),
                )
            })?;
        let input: Value = serde_json::from_slice(body)
            .map_err(|error| (400, format!("invalid Dashboard Agent request: {error}")))?;
        let action = input["action"].as_str().unwrap_or_default();
        let mut result = match action {
            "save-agent" => self
                .approvals
                .save_dashboard_agent(&repository, &viewer.login, &input)
                .map_err(|message| (400, message))?,
            "set-state" => {
                let agent_id = input["agent_id"]
                    .as_str()
                    .ok_or_else(|| (400, "agent_id is required".into()))?;
                let state = input["state"]
                    .as_str()
                    .ok_or_else(|| (400, "state is required".into()))?;
                self.approvals
                    .set_dashboard_agent_state(&repository, agent_id, &viewer.login, state)
                    .map_err(|message| (400, message))?;
                json!({"ok": true, "agent_id": agent_id, "state": state})
            }
            "evaluate" => self
                .evaluate_dashboard_agents(&repository, remote, &viewer.login, "manual")
                .map_err(|message| (503, message))?,
            "request-quarantine" => self
                .approvals
                .request_dashboard_quarantine(&repository, &viewer.login, &input)
                .map_err(|message| (400, message))?,
            "request-quarantine-release" => {
                let quarantine_id = input["quarantine_id"]
                    .as_str()
                    .ok_or_else(|| (400, "quarantine_id is required".into()))?;
                self.approvals
                    .request_dashboard_quarantine_release(&repository, quarantine_id, &viewer.login)
                    .map_err(|message| (404, message))?;
                json!({"ok": true, "state": "PENDING_RELEASE", "quarantine_id": quarantine_id})
            }
            "propose-flow-context" => {
                let incident_id = input["incident_id"]
                    .as_str()
                    .ok_or_else(|| (400, "incident_id is required".into()))?;
                let flow_id = input["flow_id"]
                    .as_str()
                    .ok_or_else(|| (400, "flow_id is required".into()))?;
                let context = self
                    .context(&repository, remote, &BTreeSet::new())
                    .map_err(|_| (503, "repository governance could not be loaded".into()))?;
                let flow_exists = context
                    .latest_status()
                    .and_then(|status| {
                        status["governance"]["governance_config"]["flows"].as_array()
                    })
                    .is_some_and(|flows| {
                        flows
                            .iter()
                            .any(|flow| flow["id"].as_str() == Some(flow_id))
                    });
                if !flow_exists {
                    return Err((
                        400,
                        "flow_id must identify a flow in the latest reported repository governance"
                            .into(),
                    ));
                }
                self.approvals
                    .propose_dashboard_flow_context(
                        &repository,
                        &viewer.login,
                        incident_id,
                        flow_id,
                    )
                    .map_err(|message| (400, message))?
            }
            "open-incident" => {
                let agent_id = input["agent_id"].as_str().unwrap_or_default();
                let context = self
                    .context(&repository, remote, &BTreeSet::new())
                    .map_err(|_| (503, "repository events could not be loaded".into()))?;
                let agent = context
                    .dashboard_agents
                    .iter()
                    .find(|agent| agent["agent_id"] == agent_id)
                    .cloned()
                    .ok_or_else(|| (404, "dashboard agent not found".into()))?;
                let reason = input["reason"]
                    .as_str()
                    .filter(|reason| !reason.trim().is_empty() && reason.len() <= 1000)
                    .ok_or_else(|| {
                        (400, "incident reason must contain 1-1000 characters".into())
                    })?;
                let now = crate::platform::now_millis();
                let incident = json!({
                    "agent_id": agent_id,
                    "agent_name": agent["name"],
                    "incident_key": format!("manual:{}:{}", agent_id, crate::trust::crypto::random_id().map_err(|error| (500, error))?),
                    "reason": reason,
                    "severity": "MEDIUM",
                    "metric": "manual_review",
                    "observed": 0,
                    "threshold": 0,
                    "assignments": agent["assignments"],
                    "event_ids": [],
                    "three_a": [
                        {"at": now, "kind": "incident_reported", "actor": viewer.login, "text": reason}
                    ],
                });
                let inserted = self
                    .approvals
                    .record_dashboard_incident(&repository, &viewer.login, &incident)
                    .map_err(|_| (500, "incident could not be recorded".into()))?;
                if inserted {
                    self.notify_dashboard_incident(&repository, remote, &incident);
                }
                json!({"ok": true})
            }
            "annotate-incident" | "resolve-incident" => {
                let incident_id = input["incident_id"]
                    .as_str()
                    .ok_or_else(|| (400, "incident_id is required".into()))?;
                let note = input["note"]
                    .as_str()
                    .ok_or_else(|| (400, "incident note is required".into()))?;
                self.approvals
                    .annotate_dashboard_incident(
                        &repository,
                        incident_id,
                        &viewer.login,
                        note,
                        action == "resolve-incident",
                    )
                    .map_err(|message| (404, message))?;
                json!({"ok": true, "incident_id": incident_id})
            }
            _ => return Err((400, "unknown Dashboard Agent action".into())),
        };
        if let Some(object) = result.as_object_mut() {
            object.insert("repository_id".into(), json!(repository));
        }
        uploaded(&repository);
        Ok(result)
    }

    /** Start GitHub sign-in with a random state bound to a short-lived cookie
     * Input
        - request: &Request - request
     * Output
        - Reply
    */
    fn login(&self, request: &Request) -> Reply {
        let Some(auth) = &self.auth else {
            return Reply::redirect("/");
        };
        let state = random_bytes(16)
            .map(|bytes| hex(&bytes))
            .unwrap_or_default();
        // F00.2
        let mut spec =
            service_events::dashboard("DASH_SIGN_IN_STARTED", "F00", "SIGN_IN", None, None);
        spec.respond("REDIRECTED", "NA", "Sent to GitHub to sign in.")
            .severity("LOW", "FIXED", "NONE")
            .set("other", "oauth_scope", "READ_ORG");
        self.log.record(spec, None, None);
        Reply::redirect(&auth.authorize_url(request, &state, false))
            .with_header(
                "Set-Cookie",
                &Auth::cookie(request, "foxx_oauth", &state, 600),
            )
            .with_header("Set-Cookie", &Auth::cookie(request, "foxx_intent", "", 0))
    }

    /** Start an approval: GitHub sign-in carrying what to approve in a signed cookie
     * Input
        - request: &Request - request
        - query: &BTreeMap<String, String> - action (link or owner), code, private=1 to also ask
          for repository access (private repositories)
     * Output
        - Reply
    */
    fn approve(&self, request: &Request, query: &BTreeMap<String, String>) -> Reply {
        let Some(auth) = &self.auth else {
            return Reply::redirect("/");
        };
        let action = query.get("action").cloned().unwrap_or_default();
        let code = query
            .get("code")
            .map(|code| code.trim().to_ascii_uppercase())
            .unwrap_or_default();
        if !matches!(action.as_str(), "link" | "owner")
            || !crate::telemetry::ingest::is_approval_code(&code)
        {
            // F30.4
            let mut spec = service_events::dashboard(
                "APPROVAL_CODE_REJECTED",
                "F30",
                if action == "owner" {
                    "CONFIRM_OWNER"
                } else {
                    "CONFIRM_LINK"
                },
                None,
                None,
            );
            spec.respond(
                "BAD_REQUEST",
                if matches!(action.as_str(), "link" | "owner") {
                    "CODE_MALFORMED"
                } else {
                    "UNKNOWN_ACTION"
                },
                "Approval refused: malformed code or action.",
            )
            .set("assessment", "assessment", "VIOLATION")
            .set("assessment", "assessment_checks", ["CODE_VALID"])
            .set("decision", "decision", "DENY")
            .set("decision", "decision_rule", "CODE_INVALID")
            .set("decision", "enforcement", "PREVENTED")
            .severity("MEDIUM", "FAILURE_STREAK", "NONE");
            self.log.record(spec, None, None);
            return Reply::json(
                400,
                json!({"error": "an approval needs action=link or action=owner and the 12-character code"}),
            );
        }
        let state = random_bytes(16)
            .map(|bytes| hex(&bytes))
            .unwrap_or_default();
        let intent = Intent {
            action,
            code,
            state: state.clone(),
            grant: None,
            dashboard_agent: None,
        };
        let private = query.get("private").is_some_and(|value| value == "1");
        Reply::redirect(&auth.authorize_url(request, &state, private))
            .with_header(
                "Set-Cookie",
                &Auth::cookie(request, "foxx_oauth", &state, 600),
            )
            .with_header(
                "Set-Cookie",
                &Auth::cookie(request, "foxx_intent", &auth.seal(&intent), 600),
            )
    }

    /** Start a credit grant: check what was asked, then GitHub sign-in carrying it in a signed
     * cookie; GitHub decides at the callback whether the person administers the repository
     * Input
        - request: &Request - request
        - query: &BTreeMap<String, String> - repository, session, amount (1 to 1000), reason,
          private=1 to also ask for repository access
     * Output
        - Reply
    */
    fn start_grant(&self, request: &Request, query: &BTreeMap<String, String>) -> Reply {
        let Some(auth) = &self.auth else {
            return Reply::redirect("/");
        };
        let repository = query.get("repository").cloned().unwrap_or_default();
        let session = query.get("session").cloned().unwrap_or_default();
        let reason = query
            .get("reason")
            .map(|reason| reason.trim().chars().take(200).collect::<String>())
            .unwrap_or_default();
        let amount = query
            .get("amount")
            .and_then(|amount| amount.parse::<u64>().ok())
            .unwrap_or(0);
        let session_ok = !session.is_empty()
            && session.len() <= 128
            && session
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-:.".contains(character));
        if !is_digest(&repository)
            || !session_ok
            || !(1..=1000).contains(&amount)
            || reason.is_empty()
        {
            return Reply::json(
                400,
                json!({"error": "a grant needs a repository, a session, an amount from 1 to 1000, and a reason"}),
            );
        }
        let state = random_bytes(16)
            .map(|bytes| hex(&bytes))
            .unwrap_or_default();
        let intent = Intent {
            action: "grant".into(),
            code: String::new(),
            state: state.clone(),
            grant: Some(
                json!({"repository": repository, "session": session, "amount": amount, "reason": reason}),
            ),
            dashboard_agent: None,
        };
        let private = query.get("private").is_some_and(|value| value == "1");
        Reply::redirect(&auth.authorize_url(request, &state, private))
            .with_header(
                "Set-Cookie",
                &Auth::cookie(request, "foxx_oauth", &state, 600),
            )
            .with_header(
                "Set-Cookie",
                &Auth::cookie(request, "foxx_intent", &auth.seal(&intent), 600),
            )
    }

    fn start_agent_approval(&self, request: &Request, query: &BTreeMap<String, String>) -> Reply {
        let Some(auth) = &self.auth else {
            return Reply::json(503, json!({"error": "GitHub sign-in is not configured"}));
        };
        let action = query.get("action").cloned().unwrap_or_default();
        let repository = query.get("repository").cloned().unwrap_or_default();
        let record_id = query.get("id").cloned().unwrap_or_default();
        if !matches!(
            action.as_str(),
            "approve-quarantine"
                | "reject-quarantine"
                | "release-quarantine"
                | "reject-release"
                | "approve-context"
                | "reject-context"
        ) || !is_digest(&repository)
            || !(8..=64).contains(&record_id.len())
            || !record_id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Reply::json(
                400,
                json!({"error": "invalid Dashboard Agent approval request"}),
            );
        }
        let repositories = match data::verified_repositories(&self.read) {
            Ok(repositories) => repositories,
            Err(_) => {
                return Reply::json(
                    503,
                    json!({"error": "repository access could not be verified"}),
                )
            }
        };
        if !repositories.iter().any(|(id, _)| id == &repository) {
            return Reply::json(404, json!({"error": "repository not found"}));
        }
        let state = match random_bytes(16) {
            Ok(bytes) => hex(&bytes),
            Err(error) => return Reply::json(500, json!({"error": error})),
        };
        let intent = Intent {
            action: "dashboard-agent".into(),
            code: String::new(),
            state: state.clone(),
            grant: None,
            dashboard_agent: Some(json!({
                "action": action,
                "repository": repository,
                "record_id": record_id,
            })),
        };
        Reply::redirect(&auth.authorize_url(request, &state, true))
            .with_header(
                "Set-Cookie",
                &Auth::cookie(request, "foxx_oauth", &state, 600),
            )
            .with_header(
                "Set-Cookie",
                &Auth::cookie(request, "foxx_intent", &auth.seal(&intent), 600),
            )
    }

    /** Carry out a credit grant: find the session's machine, ask GitHub whether the person
     * administers the repository, and record the grant
     * Input
        - auth: &Auth - sign-in settings
        - intent: &Intent - the grant asked for
        - viewer: &Viewer - who signed in
        - token: &str - their GitHub token (used for one question, then dropped)
     * Output
        - (bool, String) success and the message for the person
    */
    fn carry_out_grant(
        &self,
        auth: &Auth,
        intent: &Intent,
        viewer: &Viewer,
        token: &str,
    ) -> (bool, String) {
        let asked = intent.grant.clone().unwrap_or_default();
        let repository = asked["repository"].as_str().unwrap_or_default().to_string();
        let session = asked["session"].as_str().unwrap_or_default().to_string();
        let remote = match data::verified_repositories(&self.read) {
            Ok(list) => list
                .into_iter()
                .find(|(id, _)| *id == repository)
                .map(|(_, remote)| remote),
            Err(error) => return (false, error),
        };
        let Some(remote) = remote else {
            return (false, "that repository is not known to the service".into());
        };
        let record = match self.read.find(
            "sessions",
            mongodb::bson::doc! {"repository_id": &repository, "foxx_session_id": &session},
            mongodb::bson::doc! {"last_seen_at": -1},
            1,
        ) {
            Ok(rows) => rows.into_iter().next().map(data::json),
            Err(error) => return (false, error),
        };
        let Some(record) = record else {
            return (false, format!("no session '{session}' in this repository"));
        };
        let access = match auth.access(token, &remote) {
            Ok(access) => access,
            Err(error) => return (false, error),
        };
        let grant = approve::Grant {
            repository: repository.clone(),
            remote: remote.clone(),
            session: session.clone(),
            installation: record["installation"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            agent: record["agent_id"].as_str().unwrap_or_default().to_string(),
            amount: asked["amount"].as_u64().unwrap_or(0),
            reason: asked["reason"].as_str().unwrap_or_default().to_string(),
        };
        let outcome = self.approvals.grant(&grant, &viewer.login, access);
        // F27 (Part 4): a grant approved or refused on the dashboard
        let mut spec = service_events::dashboard(
            if outcome.is_ok() {
                "CREDITS_GRANT_APPROVED"
            } else {
                "CREDITS_GRANT_REFUSED"
            },
            "F27",
            "GRANT_CREDITS",
            Some(&viewer.login),
            Some((&repository, &remote)),
        );
        spec.respond(
            if outcome.is_ok() { "OK" } else { "REFUSED" },
            if outcome.is_ok() { "NA" } else { "NOT_ADMIN" },
            if outcome.is_ok() {
                "Admin granted credits to an agent session."
            } else {
                "Credit grant refused: not an admin."
            },
        )
        .set("session", "session_type", "AGENT_SESSION")
        .set("session", "session_id", &session)
        .set("agent", "agent_id", &grant.agent)
        .set("assessment", "assessment_checks", ["GITHUB_ADMIN_ROLE"])
        .set(
            "decision",
            "decision",
            if outcome.is_ok() { "ALLOW" } else { "DENY" },
        )
        .set("decision", "decision_rule", "GITHUB_ADMIN_REQUIRED")
        .set("resource", "resource", ["CREDITS"])
        .set("eval", "credits_granted", grant.amount)
        .set("other", "reason", &grant.reason)
        .severity(
            if outcome.is_ok() { "LOW" } else { "MEDIUM" },
            "OUTCOME",
            "NONE",
        );
        self.log
            .record(spec, Some(&repository), Some(&grant.installation));
        match outcome {
            Ok(message) => (true, message),
            Err(error) => (false, error),
        }
    }

    fn carry_out_dashboard_agent_action(
        &self,
        auth: &Auth,
        intent: &Intent,
        viewer: &Viewer,
        token: &str,
    ) -> (bool, String) {
        let Some(request) = intent.dashboard_agent.as_ref() else {
            return (false, "Dashboard Agent approval data is missing".into());
        };
        let action = request["action"].as_str().unwrap_or_default();
        let repository = request["repository"].as_str().unwrap_or_default();
        let record_id = request["record_id"].as_str().unwrap_or_default();
        let remote = match data::verified_repositories(&self.read) {
            Ok(repositories) => repositories
                .into_iter()
                .find(|(id, _)| id == repository)
                .map(|(_, remote)| remote),
            Err(error) => return (false, error),
        };
        let Some(remote) = remote else {
            return (false, "the repository is not known to the service".into());
        };
        let access = match auth.access(token, &remote) {
            Ok(access) => access,
            Err(error) => return (false, error),
        };
        let outcome = if access.admin {
            self.approvals
                .approve_dashboard_action(repository, action, record_id, &viewer.login)
                .map(|()| match action {
                    "approve-quarantine" => "Dashboard Agent quarantine activated.".to_string(),
                    "reject-quarantine" => {
                        "Dashboard Agent quarantine request rejected.".to_string()
                    }
                    "release-quarantine" => "Dashboard Agent quarantine released.".to_string(),
                    "reject-release" => {
                        "Quarantine release was rejected; enforcement remains active.".to_string()
                    }
                    "approve-context" => "Incident context attached to the flow.".to_string(),
                    _ => "Incident context proposal rejected.".to_string(),
                })
        } else {
            Err("GitHub did not confirm repository-admin access; no action was applied.".into())
        };
        let approved = outcome.is_ok();
        let mut spec = service_events::dashboard(
            if approved {
                "DASHBOARD_AGENT_ACTION_REVIEWED"
            } else {
                "DASHBOARD_AGENT_ACTION_REFUSED"
            },
            "F35",
            "DASHBOARD_AGENT_APPROVAL",
            Some(&viewer.login),
            Some((repository, &remote)),
        );
        spec.respond(
            if approved { "OK" } else { "REFUSED" },
            if approved { "NA" } else { "NOT_ADMIN" },
            if approved {
                "Repository admin reviewed a Dashboard Agent action."
            } else {
                "Dashboard Agent action was not applied."
            },
        )
        .set("assessment", "assessment_checks", ["GITHUB_ADMIN_ROLE"])
        .set(
            "decision",
            "decision",
            if approved { "ALLOW" } else { "DENY" },
        )
        .set("decision", "decision_rule", "GITHUB_ADMIN_REQUIRED")
        .set("other", "dashboard_agent_action", action)
        .set("other", "record_id", record_id)
        .severity(if approved { "LOW" } else { "MEDIUM" }, "OUTCOME", "NONE");
        self.log.record(spec, Some(repository), None);
        if approved {
            uploaded(repository);
        }
        match outcome {
            Ok(message) => (true, message),
            Err(error) => (false, error),
        }
    }

    /** Carry out an approval with the token of the sign-in that was started for it
     * Input
        - auth: &Auth - sign-in settings
        - intent: &Intent - what to approve
        - viewer: &Viewer - who signed in
        - token: &str - their GitHub token (used for one question, then dropped)
     * Output
        - (bool, String) success and the message for the person
    */
    fn carry_out(
        &self,
        auth: &Auth,
        intent: &Intent,
        viewer: &Viewer,
        token: &str,
    ) -> (bool, String) {
        let owner = intent.action == "owner";
        let action_type = if owner {
            "CONFIRM_OWNER"
        } else {
            "CONFIRM_LINK"
        };
        let (_, pending) = match self.approvals.pending(intent) {
            Ok(found) => found,
            Err(error) => {
                // F30.4
                let mut spec = service_events::dashboard(
                    "APPROVAL_CODE_REJECTED",
                    "F30",
                    action_type,
                    Some(&viewer.login),
                    None,
                );
                spec.respond(
                    "REFUSED",
                    "CODE_UNKNOWN",
                    "Approval refused: code unknown, used, or expired.",
                )
                .set("assessment", "assessment", "VIOLATION")
                .set("assessment", "assessment_checks", ["CODE_VALID"])
                .set("decision", "decision", "DENY")
                .set("decision", "decision_rule", "CODE_INVALID")
                .set("decision", "enforcement", "PREVENTED")
                .severity("MEDIUM", "FAILURE_STREAK", "NONE");
                self.log.record(spec, None, None);
                return (false, error);
            }
        };
        let remote = pending.get_str("remote").unwrap_or_default().to_string();
        let repository = pending
            .get_str("repository_id")
            .unwrap_or_default()
            .to_string();
        let installation = pending.get_str("installation").ok().map(String::from);
        let access = match auth.access(token, &remote) {
            Ok(access) => access,
            Err(error) => return (false, error),
        };
        let role = if access.admin {
            "ADMIN"
        } else if access.read {
            "READ"
        } else {
            "NONE"
        };
        let outcome = self.approvals.confirm(intent, &viewer.login, access);
        // F02.5 or F02.7
        let name = match (owner, outcome.is_ok()) {
            (true, true) => "OWNER_KEY_CONFIRMED",
            (true, false) => "OWNER_KEY_CONFIRMATION_REFUSED",
            (false, true) => "MACHINE_LINKED",
            (false, false) => "MACHINE_LINK_REFUSED",
        };
        let mut spec = service_events::dashboard(
            name,
            "F02",
            action_type,
            Some(&viewer.login),
            Some((&repository, &remote)),
        );
        spec.set("action", "action_source", "LINK")
            .respond(
                if outcome.is_ok() { "OK" } else { "REFUSED" },
                if outcome.is_ok() {
                    "NA"
                } else if owner {
                    "NOT_ADMIN"
                } else {
                    "NO_ACCESS"
                },
                if outcome.is_ok() {
                    "GitHub confirmed the person's role; approved."
                } else {
                    "GitHub did not confirm the needed role."
                },
            )
            .set(
                "assessment",
                "assessment",
                if outcome.is_ok() {
                    "COMPLIANT"
                } else {
                    "VIOLATION"
                },
            )
            .set(
                "assessment",
                "assessment_checks",
                [
                    if owner {
                        "GITHUB_ADMIN_ROLE"
                    } else {
                        "GITHUB_READ_ACCESS"
                    },
                    "CODE_VALID",
                ],
            )
            .set(
                "decision",
                "decision",
                if outcome.is_ok() { "ALLOW" } else { "DENY" },
            )
            .set(
                "decision",
                "decision_rule",
                if owner {
                    "GITHUB_ADMIN_REQUIRED"
                } else {
                    "GITHUB_ACCESS_REQUIRED"
                },
            )
            .severity(
                if outcome.is_ok() { "LOW" } else { "MEDIUM" },
                "FAILURE_STREAK",
                "NONE",
            )
            .set("other", "github_role", role);
        if owner {
            spec.set("other", "owner_key", pending.get_str("owner_key").ok());
        } else {
            spec.set("other", "machine_key", &installation).set(
                "other",
                "linked_to",
                outcome.is_ok().then_some(viewer.login.as_str()),
            );
        }
        self.log
            .record(spec, Some(&repository), installation.as_deref());
        if outcome.is_err() && !access.read {
            // F30.5
            let mut spec = service_events::dashboard(
                "APPROVAL_PRIVATE_RETRY_OFFERED",
                "F30",
                action_type,
                Some(&viewer.login),
                Some((&repository, &remote)),
            );
            spec.respond(
                "RETRY_OFFERED",
                "NO_ACCESS",
                "Retry with repository access offered.",
            )
            .set("assessment", "assessment", "UNRESOLVED")
            .set("assessment", "assessment_checks", ["GITHUB_READ_ACCESS"])
            .severity("LOW", "FIXED", "NONE")
            .set("other", "oauth_scope_next", "READ_ORG_AND_REPO");
            self.log
                .record(spec, Some(&repository), installation.as_deref());
        }
        match outcome {
            Ok(message) => (true, message),
            Err(error) if access.read => (false, error),
            Err(error) => (
                false,
                format!(
                    "{error}. If the repository is private, approve again with repository access: /auth/approve?action={}&code={}&private=1",
                    intent.action, intent.code
                ),
            ),
        }
    }

    /** Finish GitHub sign-in: check the state, exchange the code, and set the session cookie
     * Input
        - request: &Request - request
        - query: &BTreeMap<String, String> - code and state
     * Output
        - Reply
    */
    fn callback(&self, request: &Request, query: &BTreeMap<String, String>) -> Reply {
        let Some(auth) = &self.auth else {
            return Reply::redirect("/");
        };
        let expected = http::cookie(request, "foxx_oauth").filter(|state| !state.is_empty());
        let failed = |failure: &str, statement: &str| {
            // F00.4
            let mut spec =
                service_events::dashboard("DASH_SIGN_IN_FAILED", "F00", "SIGN_IN", None, None);
            spec.respond(
                if failure == "STATE_MISMATCH" {
                    "INVALID"
                } else {
                    "FAIL"
                },
                failure,
                statement,
            )
            .severity("MEDIUM", "FAILURE_STREAK", "NONE")
            .set("bindings", "binding_result", "FAILED")
            .set("telemetry", "agent_state", "ERROR");
            self.log.record(spec, None, None);
        };
        if expected.is_none() || query.get("state") != expected.as_ref() {
            failed("STATE_MISMATCH", "Sign-in did not match this browser.");
            return Reply::json(
                400,
                json!({"error": "the sign-in request did not match this browser; start again"}),
            );
        }
        let Some(code) = query.get("code") else {
            failed("NO_CODE", "GitHub returned no sign-in code.");
            return Reply::json(400, json!({"error": "GitHub returned no code"}));
        };
        match auth.sign_in(request, code) {
            Ok((viewer, token)) => {
                // F00.3
                let mut spec = service_events::dashboard(
                    "DASH_SIGN_IN_COMPLETED",
                    "F00",
                    "SIGN_IN",
                    Some(&viewer.login),
                    None,
                );
                spec.respond("OK", "NA", "Signed in with GitHub.")
                    .severity("LOW", "FAILURE_STREAK", "NONE")
                    .set(
                        "repository",
                        "repo",
                        if viewer.orgs.is_empty() {
                            "PERSONAL"
                        } else {
                            "ORGANIZATION"
                        },
                    )
                    .set("eval", "organizations_count", viewer.orgs.len())
                    .set("other", "session_expires_at", viewer.expires);
                self.log.record(spec, None, None);
                // An approval started with this sign-in: GitHub decides with this token, once
                let target = match auth
                    .intent(request)
                    .filter(|intent| Some(&intent.state) == expected.as_ref())
                {
                    Some(intent) => {
                        let grant = intent.action == "grant";
                        let agent_action = intent.action == "dashboard-agent";
                        let (ok, message) = if grant {
                            self.carry_out_grant(auth, &intent, &viewer, &token)
                        } else if agent_action {
                            self.carry_out_dashboard_agent_action(auth, &intent, &viewer, &token)
                        } else {
                            self.carry_out(auth, &intent, &viewer, &token)
                        };
                        format!(
                            "/?approval={}&ok={}#/{}",
                            auth::encode(&message),
                            u8::from(ok),
                            if grant {
                                "credits"
                            } else if agent_action {
                                "dashboard-agents"
                            } else {
                                "settings"
                            }
                        )
                    }
                    None => "/".into(),
                };
                drop(token);
                Reply::redirect(&target)
                    .with_header(
                        "Set-Cookie",
                        &Auth::cookie(
                            request,
                            "foxx_session",
                            &auth.session(&viewer),
                            SESSION_MILLIS / 1000,
                        ),
                    )
                    .with_header("Set-Cookie", &Auth::cookie(request, "foxx_oauth", "", 0))
                    .with_header("Set-Cookie", &Auth::cookie(request, "foxx_intent", "", 0))
            }
            Err(error) => {
                failed(
                    if error.contains("did not grant") {
                        "TOKEN_NOT_GRANTED"
                    } else {
                        "GITHUB_DENIED"
                    },
                    "GitHub sign-in failed.",
                );
                Reply::json(400, json!({"error": error}))
            }
        }
    }

    /** Answer an API request for a signed-in viewer
     * Input
        - request: &Request - request
        - route: &str - path after /api/
        - query: &BTreeMap<String, String> - parameters
     * Output
        - api::Answer
    */
    fn api(&self, request: &Request, route: &str, query: &BTreeMap<String, String>) -> api::Answer {
        let unavailable_event = |failure: &str, viewer: Option<&str>| {
            // F00.9
            let mut spec = service_events::dashboard(
                "DASH_SERVICE_UNAVAILABLE",
                "F00",
                "LOAD_VIEW",
                viewer,
                None,
            );
            spec.respond(
                "UNAVAILABLE",
                failure,
                "Dashboard data could not be served.",
            )
            .severity("HIGH", "FIXED", "NONE")
            .set("resource", "resource", ["DATABASE"])
            .set("bindings", "third_party", ["MONGODB", "RENDER"])
            .set("bindings", "binding_result", "FAILED")
            .set("telemetry", "agent_state", "ERROR");
            self.log.record(spec, None, None);
        };
        let Some(auth) = &self.auth else {
            if route == "me" {
                unavailable_event("SIGN_IN_NOT_CONFIGURED", None);
            }
            return api::error(
                503,
                "sign-in is not configured on this service; the operator sets CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET".into(),
            );
        };
        let Some(viewer) = auth.viewer(request) else {
            if route == "me" {
                // F00.1
                let failure = if http::cookie(request, "foxx_session")
                    .is_some_and(|value| !value.is_empty())
                {
                    "EXPIRED_COOKIE"
                } else {
                    "NO_COOKIE"
                };
                let mut spec = service_events::dashboard(
                    "DASH_OPENED_SIGNED_OUT",
                    "F00",
                    "OPEN_DASHBOARD",
                    None,
                    None,
                );
                spec.respond(
                    if failure == "NO_COOKIE" {
                        "SIGN_IN_REQUIRED"
                    } else {
                        "SESSION_EXPIRED"
                    },
                    failure,
                    "Sign-in required to see the dashboard.",
                )
                .severity("LOW", "FIXED", "NONE")
                .set("bindings", "third_party", ["NONE"]);
                self.log.record(spec, None, None);
            }
            return api::error(401, "sign in with GitHub".into());
        };
        let unavailable = |_| {
            unavailable_event("DATABASE_UNAVAILABLE", Some(&viewer.login));
            api::error(503, "the database is unavailable; try again shortly".into())
        };
        let repositories = match data::verified_repositories(&self.read) {
            Ok(repositories) => repositories
                .into_iter()
                .filter(|(_, remote)| viewer.allowed(remote))
                .collect::<Vec<_>>(),
            Err(error) => return unavailable(error),
        };
        if route == "me" {
            return (200, me(&viewer, &repositories), None);
        }
        // The viewer's own sign-in events (F00), for the Settings page
        if route == "me/sign-ins" {
            return match self.read.find(
                "service_events",
                mongodb::bson::doc! {"action.action_user": &viewer.login, "flow": "F00"},
                mongodb::bson::doc! {"occurred_at": -1},
                50,
            ) {
                Ok(rows) => (
                    200,
                    json!({"events": rows.into_iter().map(data::json).collect::<Vec<_>>()}),
                    None,
                ),
                Err(error) => unavailable(error),
            };
        }
        if let Some(account) = route.strip_prefix("accounts/") {
            // F00.5
            let mut spec = service_events::dashboard(
                "DASH_SCOPE_ACCOUNT_SELECTED",
                "F00",
                "SELECT_ACCOUNT",
                Some(&viewer.login),
                None,
            );
            let owns = viewer.owns(account);
            spec.respond(
                if owns { "OK" } else { "FORBIDDEN" },
                if owns { "NA" } else { "NOT_YOUR_ACCOUNT" },
                if owns {
                    "Account chosen."
                } else {
                    "Account is not the viewer's."
                },
            )
            .severity(
                if owns { "LOW" } else { "MEDIUM" },
                "FAILURE_STREAK",
                "NONE",
            )
            .set(
                "repository",
                "repo",
                if account.eq_ignore_ascii_case(&viewer.login) {
                    "PERSONAL"
                } else {
                    "ORGANIZATION"
                },
            )
            .set("other", "account_name", account)
            .set(
                "other",
                "account_kind",
                if account.eq_ignore_ascii_case(&viewer.login) {
                    "PERSONAL"
                } else {
                    "ORGANIZATION"
                },
            );
            self.log.record(spec, None, None);
            if !owns {
                return api::error(
                    403,
                    "this account is not yours or one of your organizations".into(),
                );
            }
            let owned = repositories
                .iter()
                .filter(|(_, remote)| {
                    auth::account(remote).as_deref() == Some(&account.to_ascii_lowercase())
                })
                .cloned()
                .collect::<Vec<_>>();
            return match data::summary(&self.read, &owned) {
                Ok(rows) => (200, json!({"account": account, "repositories": rows}), None),
                Err(error) => unavailable(error),
            };
        }
        let repository = query.get("repository").cloned().unwrap_or_default();
        let Some((_, remote)) = repositories.iter().find(|(id, _)| *id == repository) else {
            // F00.6 (refused)
            let mut spec = service_events::dashboard(
                "DASH_SCOPE_REPOSITORY_SELECTED",
                "F00",
                "SELECT_REPOSITORY",
                Some(&viewer.login),
                None,
            );
            spec.respond(
                if is_digest(&repository) {
                    "FORBIDDEN"
                } else {
                    "BAD_REQUEST"
                },
                if is_digest(&repository) {
                    "NOT_OWNED_BY_VIEWER"
                } else {
                    "MALFORMED_ID"
                },
                "Repository is not visible to the viewer.",
            )
            .severity("MEDIUM", "FAILURE_STREAK", "NONE");
            self.log.record(spec, None, None);
            return api::error(
                if is_digest(&repository) { 403 } else { 400 },
                "choose a repository owned by your account or one of your organizations".into(),
            );
        };
        let members = query
            .get("members")
            .map(|list| {
                list.split(',')
                    .map(str::trim)
                    .filter(|key| {
                        key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    .map(str::to_ascii_lowercase)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        match self.context(&repository, remote, &members) {
            Ok(context) => {
                let kind = if auth::account(remote).as_deref()
                    == Some(&viewer.login.to_ascii_lowercase())
                {
                    "PERSONAL"
                } else {
                    "ORGANIZATION"
                };
                let answer = api::route(&context, route, query, kind);
                self.record_view(
                    &viewer.login,
                    route,
                    (&repository, remote),
                    &members,
                    query,
                    answer.0,
                );
                answer
            }
            Err(error) => unavailable(error),
        }
    }

    /** Record what a signed-in person looked at (DASHBOARD.md F00.6, F00.7, F30)
     * Input
        - viewer: &str - GitHub login
        - route: &str - API route
        - repository: (&str, &str) - identity and remote
        - members: &BTreeSet<String> - member filter
        - query: &BTreeMap<String, String> - parameters
        - status: u16 - answer status
     * Output
        - None
    */
    fn record_view(
        &self,
        viewer: &str,
        route: &str,
        repository: (&str, &str),
        members: &BTreeSet<String>,
        query: &BTreeMap<String, String>,
        status: u16,
    ) {
        let ok = status < 400;
        let member = members.iter().next().cloned();
        let segments = route.split('/').collect::<Vec<_>>();
        let mut spec = match segments.as_slice() {
            ["members"] if member.is_some() => {
                let mut spec = service_events::dashboard(
                    "DASH_SCOPE_MEMBER_SELECTED",
                    "F00",
                    "SELECT_MEMBER",
                    Some(viewer),
                    Some(repository),
                );
                spec.set("other", "member_key", &member);
                spec
            }
            ["members"] => service_events::dashboard(
                "DASH_SCOPE_REPOSITORY_SELECTED",
                "F00",
                "SELECT_REPOSITORY",
                Some(viewer),
                Some(repository),
            ),
            ["events"] => {
                let filtered = query.keys().any(|key| {
                    !matches!(key.as_str(), "repository" | "members" | "cursor" | "limit")
                });
                let paged = query.get("cursor").is_some_and(|cursor| cursor != "0");
                let mut spec = service_events::dashboard(
                    if paged {
                        "DASH_EVENTS_PAGED"
                    } else {
                        "DASH_EVENTS_FILTERED"
                    },
                    "F30",
                    if paged {
                        "LOAD_MORE_EVENTS"
                    } else {
                        "FILTER_EVENTS"
                    },
                    Some(viewer),
                    Some(repository),
                );
                if !filtered && !paged {
                    spec = service_events::dashboard(
                        "DASH_VIEW_OPENED",
                        "F30",
                        "OPEN_VIEW",
                        Some(viewer),
                        Some(repository),
                    );
                    spec.set("other", "view", "OBSERVABILITY");
                } else {
                    spec.set(
                        "other",
                        "filters",
                        query
                            .iter()
                            .filter(|(key, _)| !matches!(key.as_str(), "repository" | "members"))
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect::<BTreeMap<_, _>>(),
                    );
                }
                spec
            }
            ["reports" | "evidence", "session", id] => {
                let mut spec = service_events::dashboard(
                    "DASH_DOWNLOAD",
                    "F30",
                    if segments[0] == "reports" {
                        "DOWNLOAD_REPORT"
                    } else {
                        "DOWNLOAD_EVIDENCE"
                    },
                    Some(viewer),
                    Some(repository),
                );
                spec.set("other", "agent_session_id", *id);
                spec
            }
            [view, ..] => {
                let view = match *view {
                    "overview" => "OVERVIEW",
                    "agents" => "AGENTS",
                    "governance" => "GOVERNANCE",
                    "compliance" => "COMPLIANCE",
                    "alerts" => "ALERTS",
                    "health" => "SETTINGS",
                    "sessions" if segments.len() > 1 => "SESSION",
                    _ => return,
                };
                let mut spec = service_events::dashboard(
                    "DASH_VIEW_OPENED",
                    "F30",
                    "OPEN_VIEW",
                    Some(viewer),
                    Some(repository),
                );
                spec.set("other", "view", view);
                if view == "SESSION" {
                    spec.set("other", "session_viewed", segments.get(1));
                }
                spec
            }
            [] => return,
        };
        spec.respond(
            if ok { "OK" } else { "FAILED" },
            if ok {
                "NA"
            } else if status == 404 {
                "NOT_FOUND"
            } else {
                "DATABASE_UNAVAILABLE"
            },
            if ok {
                "Dashboard view served."
            } else {
                "Dashboard view failed."
            },
        )
        .severity(
            if ok { "INFO" } else { "MEDIUM" },
            if ok { "FIXED" } else { "FAILURE_STREAK" },
            "NONE",
        )
        .set("other", "member_filter", &member);
        self.log.record(spec, Some(repository.0), None);
    }

    /** Load a repository view, reusing a recent one
     * Input
        - repository: &str - repository identity
        - remote: &str - verified remote
        - members: &BTreeSet<String> - selected members, empty for all
     * Output
        - Result<Arc<Context>, String>
    */
    fn context(
        &self,
        repository: &str,
        remote: &str,
        members: &BTreeSet<String>,
    ) -> Result<Arc<Context>, String> {
        let key = format!(
            "{repository}:{}",
            members.iter().cloned().collect::<Vec<_>>().join(",")
        );
        // A view loaded before the repository's last upload is stale
        let uploaded = UPLOADS
            .lock()
            .ok()
            .and_then(|uploads| uploads.get(repository).copied());
        if let Ok(cache) = self.cache.lock() {
            if let Some((at, context)) = cache.get(&key) {
                if at.elapsed() < CACHE_TTL && uploaded.is_none_or(|uploaded| uploaded < *at) {
                    return Ok(Arc::clone(context));
                }
            }
        }
        let context = Arc::new(data::load(&self.read, repository, remote, members)?);
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
            if cache.len() >= CACHE_SIZE {
                cache.clear();
            }
            cache.insert(key, (Instant::now(), Arc::clone(&context)));
        }
        Ok(context)
    }
}

/** The viewer, their accounts (personal first, then organizations), and the repositories they
 * may see
 * Input
    - viewer: &Viewer - viewer
    - repositories: &[(String, String)] - visible (identity, remote) pairs
 * Output
    - Value
*/
fn me(viewer: &Viewer, repositories: &[(String, String)]) -> Value {
    let accounts = std::iter::once(json!({"name": viewer.login, "kind": "personal"}))
        .chain(
            viewer
                .orgs
                .iter()
                .map(|org| json!({"name": org, "kind": "organization"})),
        )
        .collect::<Vec<_>>();
    json!({
        "login": viewer.login,
        "accounts": accounts,
        "repositories": repositories.iter().map(|(id, remote)| json!({"id": id, "remote": remote, "account": auth::account(remote)})).collect::<Vec<_>>(),
    })
}
