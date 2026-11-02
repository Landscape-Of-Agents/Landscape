// The hosted DataSea dashboard, served by the ingestion service at its root: a landing page and
// the organization's sections (Engineering, Product, Sales, Finance, Human Resources, ...), of which
// Engineering holds the Flows section. People sign in with GitHub; a personal account sees the
// repositories it owns, and an organization member sees the organization's repositories. Reads use
// a read-only database handle (data.rs); the only writes (approve.rs) are approvals of a machine's
// link or the owner key, and the change requests made in the Flows section, which GitHub authorizes
// and a person applies on a machine. The GitHub token of a sign-in is used once and never kept.
//
// Routes: GET /, /app.js, /flows.js, /logo.png, /favicon.png, /auth/login, /auth/approve,
// /auth/bricks, /auth/callback, /auth/logout, /api/me (the viewer, accounts, and repositories),
// /api/flows?repository=ID, and POST /api/flows?repository=ID (probe an address, stage or cancel a
// request). Responses carry a strict Content-Security-Policy; the page loads only its own scripts.

pub(crate) mod approve;
pub(crate) mod auth;
pub(crate) mod data;
mod probe;

use std::collections::BTreeMap;
use std::io::Read;

use serde_json::{json, Value};
use tiny_http::Request;

use super::http::{self, Reply};
use crate::trust::crypto::{hex, is_digest, random_bytes};
use approve::Approvals;
use auth::{Auth, Intent, Viewer, SESSION_MILLIS};
use data::ReadOnly;

/** The page: the landing page and the sections */
const PAGE: &str = include_str!("app.html");

/** The page's script */
const SCRIPT: &str = include_str!("app.js");

/** The Flows section (flows as a knowledge base), under Engineering */
const FLOWS: &str = include_str!("flows.js");

/** The fox logo (256 px, transparent background) */
const LOGO: &[u8] = include_bytes!("logo.png");

/** The browser tab icon (64 px) */
const FAVICON: &[u8] = include_bytes!("favicon.png");

/** The largest change request (a draft and a context text) */
const MAX_REQUEST_BODY: usize = 192 * 1024;

/** How long a settled Brick request stays on the page */
pub(crate) const BRICK_REQUEST_KEPT_MILLIS: u64 = 90 * 24 * 60 * 60 * 1000;

/** The dashboard
 * Fields
    - read: ReadOnly - read-only database handle
    - approvals: Approvals - confirms approvals and keeps change requests (the only writes)
    - auth: Option<Auth> - GitHub sign-in, None when not configured
*/
pub(crate) struct Dashboard {
    read: ReadOnly,
    approvals: Approvals,
    auth: Option<Auth>,
}

impl Dashboard {
    /** Create the dashboard
     * Input
        - read: ReadOnly - read-only database handle
        - approvals: Approvals - approval writer
        - auth: Option<Auth> - sign-in settings
     * Output
        - Dashboard
    */
    pub(crate) fn new(read: ReadOnly, approvals: Approvals, auth: Option<Auth>) -> Self {
        Self {
            read,
            approvals,
            auth,
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

    /** Check with GitHub that a token's person is a code owner of a repository (clearing its data)
     * Input
        - token: &str - the person's GitHub token
        - remote: &str - the repository's remote
     * Output
        - Result<String, String> their login
    */
    pub(crate) fn codeowner(&self, token: &str, remote: &str) -> Result<String, String> {
        self.auth
            .as_ref()
            .ok_or_else(|| "GitHub sign-in must be configured on the service".to_string())?
            .codeowner(token, remote)
    }

    /** Answer a dashboard request
     * Input
        - request: &mut Request - request
     * Output
        - Reply
    */
    pub(crate) fn handle(&self, request: &mut Request) -> Reply {
        let (path, query) = http::split(request.url());
        if request.method().as_str().eq_ignore_ascii_case("POST") && path == "/api/flows" {
            let content_type = http::header(request, "Content-Type").unwrap_or_default();
            if !content_type
                .to_ascii_lowercase()
                .starts_with("application/json")
            {
                return Reply::json(415, json!({"error": "changes require application/json"}));
            }
            if request
                .body_length()
                .is_some_and(|length| length > MAX_REQUEST_BODY)
            {
                return Reply::json(413, json!({"error": "request body too large"}));
            }
            let mut body = Vec::new();
            if request
                .as_reader()
                .take(MAX_REQUEST_BODY as u64 + 1)
                .read_to_end(&mut body)
                .is_err()
                || body.len() > MAX_REQUEST_BODY
            {
                return Reply::json(413, json!({"error": "request body could not be read"}));
            }
            // Flow requests join the Brick request queue: a person applies them on a machine
            return match self.flow_requests(request, &query, &body) {
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
        if matches!(
            path.as_str(),
            "/auth/login" | "/auth/approve" | "/auth/bricks"
        ) {
            if let Some(location) = Auth::elsewhere(request) {
                return Reply::redirect(&location);
            }
        }
        match path.as_str() {
            "/" | "/index.html" => Reply::page(PAGE),
            "/app.js" => Reply::script(SCRIPT),
            "/flows.js" => Reply::script(FLOWS),
            "/logo.png" => Reply::image(LOGO, "image/png"),
            "/favicon.png" | "/favicon.ico" => Reply::image(FAVICON, "image/png"),
            "/auth/login" => self.login(request),
            "/auth/approve" => self.approve(request, &query),
            "/auth/bricks" => self.start_bricks(request, &query),
            "/auth/callback" => self.callback(request, &query),
            "/auth/logout" => Reply::redirect("/")
                .with_header("Set-Cookie", &Auth::cookie(request, "foxx_session", "", 0)),
            "/api/me" => self.me(request),
            "/api/flows" => self.flows(request, &query),
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
            brick: None,
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

    /** Answer a change request from the Flows section: "probe" checks one address, "stage" keeps a
     * request until GitHub confirms the person's role at sign-in (the page then goes to
     * /auth/bricks), "cancel" withdraws one of theirs
     * Input
        - request: &Request - request (the session cookie names the person)
        - query: &BTreeMap<String, String> - repository
        - body: &[u8] - {action: probe, url}, {action: stage, op, params, reason}, or
          {action: cancel, request_id}
     * Output
        - Result<Value, (u16, String)>
    */
    fn flow_requests(
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
        if !repositories
            .iter()
            .any(|(id, remote)| *id == repository && viewer.allowed(remote))
        {
            return Err((
                403,
                "choose a repository visible to this GitHub account".into(),
            ));
        }
        let input: Value = serde_json::from_slice(body)
            .map_err(|error| (400, format!("invalid request: {error}")))?;
        let text = |key: &str| input[key].as_str().unwrap_or_default().to_string();
        match text("action").as_str() {
            // Check one address as the person types it (before anything is staged)
            "probe" => Ok(match probe::check(&text("url")) {
                probe::Found::Answered(message) => {
                    json!({"ok": true, "private": false, "message": message})
                }
                probe::Found::Private(message) => {
                    json!({"ok": true, "private": true, "message": message})
                }
                probe::Found::Failed(message) => {
                    json!({"ok": false, "private": false, "message": message})
                }
            }),
            "stage" => {
                let op = text("op");
                if !data::flow_operation(&op) {
                    return Err((400, format!("'{op}' cannot be requested from DataSea")));
                }
                // Every address a request carries must point somewhere before it is staged (a
                // private one is checked by the machine that applies it)
                let carried = match op.as_str() {
                    "integration-set" => {
                        crate::platform::probe::urls_in(&input["params"]["settings"])
                    }
                    "flow-watch-add" | "flow-watch-edit" => {
                        let config = match &input["params"]["config"] {
                            Value::String(json_text) => {
                                serde_json::from_str(json_text).unwrap_or(Value::Null)
                            }
                            other => other.clone(),
                        };
                        crate::platform::probe::urls_in(&config)
                    }
                    _ => Vec::new(),
                };
                let mut checked = Vec::new();
                let mut later = Vec::new();
                for url in carried.iter().take(20) {
                    match probe::check(url) {
                        probe::Found::Answered(message) => checked.push(message),
                        probe::Found::Private(message) => later.push(message),
                        probe::Found::Failed(message) => {
                            return Err((400, format!("nothing was requested: an address does not point to a service: {message}")))
                        }
                    }
                }
                let (request_id, role) = self
                    .approvals
                    .stage_brick_request(
                        &repository,
                        &viewer.login,
                        &op,
                        &input["params"],
                        &text("reason"),
                    )
                    .map_err(|message| (400, message))?;
                Ok(json!({
                    "request_id": request_id,
                    "role": role,
                    "checked": checked,
                    "checked_on_machine": later,
                    "continue": format!("/auth/bricks?repository={}&request={request_id}", auth::encode(&repository)),
                }))
            }
            "cancel" => {
                let request_id = text("request_id");
                self.approvals
                    .cancel_brick_request(&repository, &request_id, &viewer.login)
                    .map_err(|message| (404, message))?;
                Ok(json!({"ok": true, "request_id": request_id, "status": "CANCELLED"}))
            }
            _ => Err((400, "action is probe, stage, or cancel".into())),
        }
    }

    /** Confirm a staged change request: GitHub sign-in carrying its id in a signed cookie; GitHub
     * decides at the callback whether the person may make it
     * Input
        - request: &Request - request
        - query: &BTreeMap<String, String> - repository, request
     * Output
        - Reply
    */
    fn start_bricks(&self, request: &Request, query: &BTreeMap<String, String>) -> Reply {
        let Some(auth) = &self.auth else {
            return Reply::redirect("/");
        };
        let value = |key: &str| query.get(key).cloned().unwrap_or_default();
        let request_id = value("request");
        if !is_digest(&value("repository"))
            || !request_id.starts_with("brq_")
            || request_id.len() > 64
            || !request_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Reply::json(
                400,
                json!({"error": "a change request needs its repository and request id"}),
            );
        }
        let state = random_bytes(16)
            .map(|bytes| hex(&bytes))
            .unwrap_or_default();
        let intent = Intent {
            action: "bricks".into(),
            code: String::new(),
            state: state.clone(),
            brick: Some(json!({"repository": value("repository"), "request_id": request_id})),
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

    /** Carry out a change request confirmation: ask GitHub for the person's role in the
     * repository and the CODEOWNERS of the signed file, and keep the request for a machine or
     * refuse it
     * Input
        - auth: &Auth - sign-in settings
        - intent: &Intent - the request to confirm
        - viewer: &Viewer - who signed in
        - token: &str - their GitHub token (used for these questions, then dropped)
     * Output
        - (bool, String) success and the message for the person
    */
    fn carry_out_bricks(
        &self,
        auth: &Auth,
        intent: &Intent,
        viewer: &Viewer,
        token: &str,
    ) -> (bool, String) {
        let asked = intent.brick.clone().unwrap_or_default();
        let repository = asked["repository"].as_str().unwrap_or_default().to_string();
        let request_id = asked["request_id"].as_str().unwrap_or_default().to_string();
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
        let access = match auth.access(token, &remote) {
            Ok(access) => access,
            Err(error) => return (false, error),
        };
        // The CODEOWNERS of the signed file the change writes decide it when they cover it
        let op = self
            .approvals
            .staged_brick_op(&request_id, &repository, &viewer.login)
            .unwrap_or_default();
        let path = auth::signed_path(&op);
        let owners = match auth.owners_of(token, &remote, &viewer.login, path) {
            Ok(owners) => owners,
            Err(error) => {
                return (
                    false,
                    format!("CODEOWNERS could not be read, so the request was not kept: {error}"),
                )
            }
        };
        match self.approvals.confirm_brick_request(
            &request_id,
            &repository,
            &remote,
            &viewer.login,
            access,
            owners.as_ref(),
        ) {
            Ok(message) => (true, message),
            Err(message) => (false, message),
        }
    }

    /** Carry out an approval of a machine's link or the owner key: ask GitHub for the person's
     * role in the request's repository, then confirm or refuse
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
        let pending = match self.approvals.pending(intent) {
            Ok((_, pending)) => pending,
            Err(error) => return (false, error),
        };
        let remote = pending.get_str("remote").unwrap_or_default().to_string();
        let access = match auth.access(token, &remote) {
            Ok(access) => access,
            Err(error) => return (false, error),
        };
        match self.approvals.confirm(intent, &viewer.login, access) {
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

    /** Finish GitHub sign-in: check the state, exchange the code, carry out what the sign-in was
     * started for, and set the session cookie
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
        if expected.is_none() || query.get("state") != expected.as_ref() {
            return Reply::json(
                400,
                json!({"error": "the sign-in request did not match this browser; start again"}),
            );
        }
        let Some(code) = query.get("code") else {
            return Reply::json(400, json!({"error": "GitHub returned no code"}));
        };
        match auth.sign_in(request, code) {
            Ok((viewer, token)) => {
                // An approval started with this sign-in: GitHub decides with this token, once
                let target = match auth
                    .intent(request)
                    .filter(|intent| Some(&intent.state) == expected.as_ref())
                {
                    Some(intent) => {
                        let bricks = intent.action == "bricks";
                        let (ok, message) = if bricks {
                            self.carry_out_bricks(auth, &intent, &viewer, &token)
                        } else {
                            self.carry_out(auth, &intent, &viewer, &token)
                        };
                        format!(
                            "/?approval={}&ok={}#/{}",
                            auth::encode(&message),
                            u8::from(ok),
                            if bricks { "engineering/flows" } else { "" }
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
            Err(error) => Reply::json(400, json!({"error": error})),
        }
    }

    /** Find the signed-in viewer and the repositories they may see
     * Input
        - request: &Request - request
     * Output
        - Result<(Viewer, Vec<(String, String)>), Reply> - the error reply when sign-in is not
          configured, the viewer is signed out, or the database is unavailable
    */
    fn viewer(&self, request: &Request) -> Result<(Viewer, Vec<(String, String)>), Reply> {
        let Some(auth) = &self.auth else {
            return Err(Reply::json(
                503,
                json!({"error": "sign-in is not configured on this service; the operator sets CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET"}),
            ));
        };
        let Some(viewer) = auth.viewer(request) else {
            return Err(Reply::json(401, json!({"error": "sign in with GitHub"})));
        };
        let repositories = data::verified_repositories(&self.read)
            .map_err(|_| {
                Reply::json(
                    503,
                    json!({"error": "the database is unavailable; try again shortly"}),
                )
            })?
            .into_iter()
            .filter(|(_, remote)| viewer.allowed(remote))
            .collect::<Vec<_>>();
        Ok((viewer, repositories))
    }

    /** The viewer, their accounts (personal first, then organizations), and the repositories they
     * may see
     * Input
        - request: &Request - request
     * Output
        - Reply
    */
    fn me(&self, request: &Request) -> Reply {
        let (viewer, repositories) = match self.viewer(request) {
            Ok(found) => found,
            Err(reply) => return reply,
        };
        let accounts = std::iter::once(json!({"name": viewer.login, "kind": "personal"}))
            .chain(
                viewer
                    .orgs
                    .iter()
                    .map(|org| json!({"name": org, "kind": "organization"})),
            )
            .collect::<Vec<_>>();
        Reply::json(
            200,
            json!({
                "login": viewer.login,
                "accounts": accounts,
                "repositories": repositories.iter().map(|(id, remote)| json!({"id": id, "remote": remote, "account": auth::account(remote)})).collect::<Vec<_>>(),
            }),
        )
    }

    /** The Flows section of a repository the viewer may see
     * Input
        - request: &Request - request
        - query: &BTreeMap<String, String> - repository
     * Output
        - Reply
    */
    fn flows(&self, request: &Request, query: &BTreeMap<String, String>) -> Reply {
        let (_, repositories) = match self.viewer(request) {
            Ok(found) => found,
            Err(reply) => return reply,
        };
        let repository = query.get("repository").cloned().unwrap_or_default();
        if !repositories.iter().any(|(id, _)| *id == repository) {
            return Reply::json(
                if is_digest(&repository) { 403 } else { 400 },
                json!({"error": "choose a repository owned by your account or one of your organizations"}),
            );
        }
        match data::load(&self.read, &repository)
            .map_err(|_| {
                (
                    503,
                    "the database is unavailable; try again shortly".to_string(),
                )
            })
            .and_then(|context| data::flows(&context))
        {
            Ok(view) => Reply::json(200, view),
            Err((status, message)) => Reply::json(status, json!({"error": message})),
        }
    }
}
