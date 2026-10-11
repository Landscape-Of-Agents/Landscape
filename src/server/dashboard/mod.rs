// The hosted Foxx dashboard: one dashboard for individuals and teams, served by the ingestion
// service at its root. People sign in with GitHub; a personal account sees the repositories it
// owns, and an organization member sees the organization's repositories, with every teammate's
// activity and a filter by member. Machines and agents cannot read anything back: the upload API
// only writes, and the dashboard answers only signed-in browsers. The dashboard holds a read-only
// database handle (data.rs), so viewing never changes the stored evidence.
//
// Routes (GET only): /, /app.js, /logo.png, and /favicon.png (the page), /auth/login, /auth/callback, /auth/logout,
// /api/me (the viewer, accounts, and repositories), /api/accounts/NAME (a summary of an account's
// repositories), and /api/ROUTE?repository=ID[&members=KEY,...] (api.rs). Responses carry a
// strict Content-Security-Policy; the page loads only its own script.

pub(crate) mod api;
pub(crate) mod auth;
pub(crate) mod data;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tiny_http::Request;

use super::http::{self, Reply};
use crate::trust::crypto::{hex, is_digest, random_bytes};
use auth::{Auth, Viewer, SESSION_MILLIS};
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
    - auth: Option<Auth> - GitHub sign-in, None when not configured
    - cache: Mutex<HashMap<String, (Instant, Arc<Context>)>> - recently loaded repository views
*/
pub(crate) struct Dashboard {
    read: ReadOnly,
    auth: Option<Auth>,
    cache: Mutex<HashMap<String, (Instant, Arc<Context>)>>,
}

impl Dashboard {
    /** Create the dashboard
     * Input
        - read: ReadOnly - read-only database handle
        - auth: Option<Auth> - sign-in settings
     * Output
        - Dashboard
    */
    pub(crate) fn new(read: ReadOnly, auth: Option<Auth>) -> Self {
        Self {
            read,
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
            "/auth/callback" => self.callback(request, &query),
            "/auth/logout" => Reply::redirect("/")
                .with_header("Set-Cookie", &Auth::cookie(request, "foxx_session", "", 0)),
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
        Reply::redirect(&auth.authorize_url(request, &state)).with_header(
            "Set-Cookie",
            &Auth::cookie(request, "foxx_oauth", &state, 600),
        )
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
            Ok(viewer) => Reply::redirect("/")
                .with_header(
                    "Set-Cookie",
                    &Auth::cookie(
                        request,
                        "foxx_session",
                        &auth.session(&viewer),
                        SESSION_MILLIS / 1000,
                    ),
                )
                .with_header("Set-Cookie", &Auth::cookie(request, "foxx_oauth", "", 0)),
            Err(error) => Reply::json(400, json!({"error": error})),
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
        let Some(auth) = &self.auth else {
            return api::error(
                503,
                "sign-in is not configured on this service; the operator sets CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET".into(),
            );
        };
        let Some(viewer) = auth.viewer(request) else {
            return api::error(401, "sign in with GitHub".into());
        };
        let unavailable =
            |_| api::error(503, "the database is unavailable; try again shortly".into());
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
        if let Some(account) = route.strip_prefix("accounts/") {
            if !viewer.owns(account) {
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
            Ok(context) => api::route(&context, route, query),
            Err(error) => unavailable(error),
        }
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
