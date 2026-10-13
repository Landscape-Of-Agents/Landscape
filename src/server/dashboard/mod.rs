// The hosted Foxx dashboard: one dashboard for individuals and teams, served by the ingestion
// service at its root. People sign in with GitHub; a personal account sees the repositories it
// owns, and an organization member sees the organization's repositories, with every teammate's
// activity and a filter by member. Machines and agents cannot read anything back: the upload API
// only writes, and the dashboard answers only signed-in browsers. The dashboard holds a read-only
// database handle (data.rs), so viewing never changes the stored evidence.
//
// Approvals (approve.rs) are the one exception to read-only: /auth/approve?action=link|owner&code=
// runs a GitHub sign-in, and the callback asks GitHub for the person's role on that repository
// before marking a pending request confirmed. The GitHub token is used for that one question and
// never kept.
//
// Routes (GET only): /, /app.js, /logo.png, and /favicon.png (the page), /auth/login, /auth/approve, /auth/callback, /auth/logout,
// /api/me (the viewer, accounts, and repositories), /api/accounts/NAME (a summary of an account's
// repositories), and /api/ROUTE?repository=ID[&members=KEY,...] (api.rs). Responses carry a
// strict Content-Security-Policy; the page loads only its own script.

pub(crate) mod api;
pub(crate) mod approve;
pub(crate) mod auth;
pub(crate) mod data;
pub(crate) mod stream;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tiny_http::Request;

use super::http::{self, Reply};
use crate::server::service_events::{self, ServiceLog};
use crate::trust::crypto::{hex, is_digest, random_bytes};
use approve::Approvals;
use auth::{Auth, Intent, Viewer, SESSION_MILLIS};
use data::{Context, ReadOnly};

/** The page */
const PAGE: &str = include_str!("app.html");

/** The page's script */
const SCRIPT: &str = include_str!("app.js");

/** The Foxx logo (256 px, transparent background) */
const LOGO: &[u8] = include_bytes!("logo.png");

/** The browser tab icon (64 px) */
const FAVICON: &[u8] = include_bytes!("favicon.png");

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

    /** Answer a dashboard request
     * Input
        - request: &Request - request
     * Output
        - Reply
    */
    pub(crate) fn handle(&self, request: &Request) -> Reply {
        if !request.method().as_str().eq_ignore_ascii_case("GET") {
            return Reply::json(405, json!({"error": "the dashboard is read-only"}));
        }
        let (path, query) = http::split(request.url());
        match path.as_str() {
            "/" | "/index.html" => Reply::page(PAGE),
            "/app.js" => Reply::script(SCRIPT),
            "/logo.png" => Reply::image(LOGO, "image/png"),
            "/favicon.png" | "/favicon.ico" => Reply::image(FAVICON, "image/png"),
            "/auth/login" => self.login(request),
            "/auth/approve" => self.approve(request, &query),
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
                        let (ok, message) = self.carry_out(auth, &intent, &viewer, &token);
                        format!(
                            "/?approval={}&ok={}#/settings",
                            auth::encode(&message),
                            u8::from(ok)
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
        if let Ok(cache) = self.cache.lock() {
            if let Some((at, context)) = cache.get(&key) {
                if at.elapsed() < CACHE_TTL {
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
