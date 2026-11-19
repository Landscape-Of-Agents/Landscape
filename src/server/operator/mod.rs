// The operator dashboard: settings only the Crane operator may change, separate from DataSea's
// pages. Today it holds one: whether AI agents may run read-only Crane commands inside their own
// sessions (trust/agent_commands.rs). It runs in one of two ways:
//   - inside the DataSea service ('crane server'), at /operator/ on the same host, reusing DataSea's
//     GitHub sign-in (same OAuth app, same session cookie) and its MongoDB;
//   - on its own ('crane operator serve'), with its own sign-in and database.
//
// Who may use it: a person signed in with GitHub whose login is listed in CRANE_OPERATOR_LOGINS
// (DEFAULT_OPERATORS when it is not set). Everyone else is refused: no operator session is set, and
// every /api route checks the list again on each request, so removing a login revokes access at
// once.
//
// What machines read: GET /v1/agent-commands, public, answering the current setting signed with the
// operator key and valid for POLICY_LIFETIME_MILLIS. The key is CRANE_OPERATOR_SIGNING_KEY when set,
// else one the service creates once and keeps in its database. Crane verifies the answer against
// the operator public key compiled into each build, so nobody without the key can forge it, and an
// unreachable service means "off" once the cached answer expires.
//
// Routes (under /operator/ inside DataSea): GET / (page), operator.js, auth/login, auth/logout,
// api/state (operator only), POST api/agent-commands (operator only; JSON with the X-Crane-Operator
// header, so no other site can submit it); GET /v1/agent-commands (machines, at the host's root);
// standalone also /health and /auth/callback.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Instant;

use mongodb::bson::{self, doc, Document};
use mongodb::sync::Database;
use serde_json::{json, Value};
use tiny_http::{Request, Server};

use super::dashboard::auth::{Auth, SESSION_MILLIS};
use super::http::{self, Reply};
use super::limits::Limiter;
use super::{database, log, store};
use crate::platform::{io_error, now_millis};
use crate::trust::agent_commands::{
    Policy, Signed, POLICY_DOMAIN, POLICY_LIFETIME_MILLIS, READ_ONLY,
};
use crate::trust::crypto::{hex, random_bytes, SigningIdentity};

/** The operator page */
const PAGE: &str = include_str!("page.html");

/** The operator page's script */
const SCRIPT: &str = include_str!("operator.js");

/** Requests served concurrently (standalone) */
const WORKERS: usize = 4;

/** Largest accepted request body */
const MAX_BODY: usize = 4 * 1024;

/** Name of the setting document */
const SETTING: &str = "agent_commands";

/** The operator logins when CRANE_OPERATOR_LOGINS is not set: the maintainer of the official
 * service (a self-hosted service sets its own) */
const DEFAULT_OPERATORS: &str = "VishuKalier2003";

/** Where the operator dashboard lives inside DataSea */
pub(crate) const PREFIX: &str = "/operator";

/** How the dashboard is served
 * Variants
    - Embedded - inside DataSea: its sign-in and address limits are DataSea's
    - Standalone - its own service, with its own sign-in and address limits
*/
enum Mode {
    Embedded,
    Standalone(Box<Standalone>),
}

/** What only the standalone service has
 * Fields
    - auth: Option<Auth> - its GitHub sign-in, None until configured
    - addresses: Limiter - requests per client address
*/
struct Standalone {
    auth: Option<Auth>,
    addresses: Limiter,
}

/** The operator dashboard's state
 * Fields
    - database: Database - MongoDB
    - key: SigningIdentity - the operator key that signs policies
    - logins: Vec<String> - GitHub logins allowed in (lowercase)
    - mode: Mode - embedded or standalone
*/
pub(crate) struct Operator {
    database: Database,
    key: SigningIdentity,
    logins: Vec<String>,
    mode: Mode,
}

/** The stored setting
 * Fields
    - enabled: bool - agents may run read-only Crane commands
    - changed_by: String - who last changed it
    - changed_at: u64 - when (Unix milliseconds)
*/
struct Setting {
    enabled: bool,
    changed_by: String,
    changed_at: u64,
}

/** Read the operator logins
 * Input
    - value: &str - comma- or space-separated GitHub logins
 * Output
    - Vec<String> lowercase, without "@", without duplicates
*/
fn parse_logins(value: &str) -> Vec<String> {
    let mut logins = value
        .split([',', ' ', ';'])
        .map(|login| login.trim().trim_start_matches('@').to_ascii_lowercase())
        .filter(|login| !login.is_empty())
        .collect::<Vec<_>>();
    logins.sort();
    logins.dedup();
    logins
}

/** Read a non-empty environment variable
 * Input
    - name: &str - variable
 * Output
    - Option<String>
*/
fn variable(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/** The operator key: CRANE_OPERATOR_SIGNING_KEY when set, else the one this service created once
 * and keeps in its database (created on first start; another instance starting at the same time
 * reads the one that was stored)
 * Input
    - database: &Database - MongoDB
 * Output
    - Result<SigningIdentity, String>
*/
fn operator_key(database: &Database) -> Result<SigningIdentity, String> {
    if let Some(seed) = variable("CRANE_OPERATOR_SIGNING_KEY") {
        return SigningIdentity::from_seed_hex(seed.trim());
    }
    let settings = database.collection::<Document>("service_settings");
    let read = || -> Result<Option<String>, String> {
        Ok(settings
            .find_one(doc! {"name": "operator_key"})
            .run()
            .map_err(io_error)?
            .and_then(|document| document.get_str("seed").ok().map(String::from)))
    };
    if read()?.is_none() {
        let seed = SigningIdentity::generate()?.seed_hex();
        let _ = settings
            .insert_one(
                doc! {"name": "operator_key", "seed": &seed, "created_at": bson::DateTime::now()},
            )
            .run();
    }
    let seed = read()?.ok_or("the operator key could not be stored")?;
    SigningIdentity::from_seed_hex(&seed)
}

impl Operator {
    /** The operator dashboard inside DataSea, on DataSea's database
     * Input
        - database: &Database - DataSea's MongoDB
     * Output
        - Result<Operator, String>
    */
    pub(crate) fn embedded(database: &Database) -> Result<Self, String> {
        let operator = Self {
            key: operator_key(database)?,
            logins: parse_logins(
                &variable("CRANE_OPERATOR_LOGINS").unwrap_or_else(|| DEFAULT_OPERATORS.into()),
            ),
            database: database.clone(),
            mode: Mode::Embedded,
        };
        log::write(
            "info",
            "operator",
            None,
            "started",
            json!({"mode": "embedded", "public_key": operator.key.public_hex(), "operators": operator.logins.len()}),
        );
        Ok(operator)
    }

    /** Answer a request inside DataSea: the path is already stripped of PREFIX ("/" for the page),
     * or is /v1/agent-commands
     * Input
        - request: &mut Request - request
        - auth: Option<&Auth> - DataSea's sign-in
        - path: &str - path below PREFIX
     * Output
        - Reply
    */
    pub(crate) fn handle(&self, request: &mut Request, auth: Option<&Auth>, path: &str) -> Reply {
        let method = request.method().as_str().to_ascii_uppercase();
        let (_, query) = http::split(request.url());
        route(request, self, auth, &method, path, &query)
    }
}

/** Run the operator dashboard as its own service until it is stopped
 * Input
    - args: &[String] - optional --listen ADDRESS
 * Output
    - Result<(), String>
    - Error if a required setting is missing, the database is unreachable, or the address cannot
      be bound
*/
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let listen = match args {
        [] => std::env::var("PORT")
            .map(|port| format!("0.0.0.0:{port}"))
            .unwrap_or_else(|_| "0.0.0.0:8080".into()),
        [flag, address] if flag == "--listen" => address.clone(),
        _ => return Err("usage: crane operator serve [--listen ADDRESS]".into()),
    };
    let logins = parse_logins(
        &variable("CRANE_OPERATOR_LOGINS").unwrap_or_else(|| DEFAULT_OPERATORS.into()),
    );
    if logins.is_empty() {
        return Err("CRANE_OPERATOR_LOGINS names no GitHub login".into());
    }
    let settings = database::Settings {
        uri: variable("CRANE_MONGODB_URI").ok_or("set CRANE_MONGODB_URI")?,
        database: variable("CRANE_OPERATOR_DB").unwrap_or_else(|| "crane_operator".into()),
    };
    let connected = database::connect(&settings)?;
    let auth = Auth::from_environment(&store::session_seed(&connected)?)?;
    let operator = Arc::new(Operator {
        key: operator_key(&connected)?,
        database: connected,
        logins,
        mode: Mode::Standalone(Box::new(Standalone {
            auth,
            addresses: Limiter::new(600.0),
        })),
    });
    let server = Arc::new(
        Server::http(&listen).map_err(|error| format!("cannot listen on {listen}: {error}"))?,
    );
    let address = server
        .server_addr()
        .to_ip()
        .map_or(listen, |address| address.to_string());
    let signed_in =
        matches!(&operator.mode, Mode::Standalone(standalone) if standalone.auth.is_some());
    println!(
        "Crane operator dashboard listening on {address} (database {}; operator key {}; sign-in {}; operators: {})",
        settings.database,
        operator.key.public_hex(),
        if signed_in {
            "on"
        } else {
            "off: set CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET"
        },
        operator.logins.join(", ")
    );
    std::io::stdout().flush().ok();
    log::write(
        "info",
        "operator",
        None,
        "started",
        json!({"mode": "standalone", "database": settings.database, "public_key": operator.key.public_hex(), "operators": operator.logins.len()}),
    );
    let workers = (0..WORKERS)
        .map(|_| {
            let server = Arc::clone(&server);
            let operator = Arc::clone(&operator);
            std::thread::spawn(move || {
                for request in server.incoming_requests() {
                    respond(request, &operator);
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

/** Answer one request of the standalone service and log it
 * Input
    - request: Request - the request
    - operator: &Operator - shared state
 * Output
    - None
*/
fn respond(mut request: Request, operator: &Operator) {
    let started = Instant::now();
    let request_id = http::request_id(&request);
    let method = request.method().as_str().to_ascii_uppercase();
    let (path, query) = http::split(request.url());
    let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if path == "/health" {
            return match database::ping(&operator.database) {
                Ok(()) => Reply::json(200, json!({"status": "ok"})),
                Err(_) => Reply::json(503, json!({"error": "the database is unavailable"})),
            };
        }
        let Mode::Standalone(standalone) = &operator.mode else {
            return Reply::json(500, json!({"error": "not a standalone service"}));
        };
        if !standalone.addresses.allow(&http::client_address(&request)) {
            return Reply::json(
                429,
                json!({"error": "too many requests; try again in a minute"}),
            )
            .with_header("Retry-After", "60");
        }
        route(
            &mut request,
            operator,
            standalone.auth.as_ref(),
            &method,
            &path,
            &query,
        )
    }))
    .unwrap_or_else(|_| {
        Reply::json(
            500,
            json!({"error": format!("the operator dashboard failed while answering (request {request_id})")}),
        )
    });
    if path != "/health" {
        log::write(
            if reply.status >= 500 {
                "error"
            } else if reply.status >= 400 {
                "warn"
            } else {
                "info"
            },
            "operator",
            Some(&request_id),
            "request",
            json!({"method": method, "path": path, "status": reply.status, "ms": started.elapsed().as_millis() as u64}),
        );
    }
    http::send(request, reply, &request_id);
}

/** Route a request (paths relative to the dashboard)
 * Input
    - request: &mut Request - request
    - operator: &Operator - shared state
    - auth: Option<&Auth> - GitHub sign-in
    - method: &str - HTTP method (uppercase)
    - path: &str - path
    - query: &BTreeMap<String, String> - query
 * Output
    - Reply
*/
fn route(
    request: &mut Request,
    operator: &Operator,
    auth: Option<&Auth>,
    method: &str,
    path: &str,
    query: &BTreeMap<String, String>,
) -> Reply {
    let embedded = matches!(operator.mode, Mode::Embedded);
    if method == "POST" && path == "/api/agent-commands" {
        return change(request, operator, auth);
    }
    if method != "GET" {
        return Reply::json(405, json!({"error": "method not allowed"}));
    }
    match path {
        "/" | "/index.html" => Reply::page(PAGE),
        "/operator.js" => Reply::script(SCRIPT),
        "/v1/agent-commands" => match current(&operator.database) {
            Ok(setting) => Reply::json(200, json!(sign(operator, &setting, now_millis()))),
            Err(_) => Reply::json(503, json!({"error": "the database is unavailable"})),
        },
        // Inside DataSea, sign-in is DataSea's, returning here afterwards
        "/auth/login" if embedded => Reply::redirect(&format!("/auth/login?next={PREFIX}/")),
        "/auth/logout" if embedded => Reply::redirect(&format!("{PREFIX}/"))
            .with_header("Set-Cookie", &Auth::cookie(request, "foxx_session", "", 0)),
        "/auth/login" => {
            if let Some(location) = Auth::elsewhere(request) {
                return Reply::redirect(&location);
            }
            login(request, auth)
        }
        "/auth/callback" if !embedded => callback(request, operator, auth, query),
        "/auth/logout" => Reply::redirect("/")
            .with_header("Set-Cookie", &Auth::cookie(request, "foxx_session", "", 0)),
        "/api/state" => match signed_in(request, operator, auth) {
            Ok(login) => state(operator, &login),
            Err(reply) => reply,
        },
        _ => Reply::json(404, json!({"error": "not found"})),
    }
}

/** Sign the setting for machines, valid from now for POLICY_LIFETIME_MILLIS
 * Input
    - operator: &Operator - shared state (the key)
    - setting: &Setting - the stored setting
    - now: u64 - Unix milliseconds
 * Output
    - Signed
*/
fn sign(operator: &Operator, setting: &Setting, now: u64) -> Signed {
    let policy = Policy {
        enabled: setting.enabled,
        issued_at: now,
        expires_at: now + POLICY_LIFETIME_MILLIS,
        changed_by: setting.changed_by.clone(),
        changed_at: setting.changed_at,
    };
    Signed {
        signature: operator.key.sign(POLICY_DOMAIN, &policy.message()),
        public_key: operator.key.public_hex(),
        policy,
    }
}

/** Read the stored setting (off when never set)
 * Input
    - database: &Database - MongoDB
 * Output
    - Result<Setting, String>
*/
fn current(database: &Database) -> Result<Setting, String> {
    let found = database
        .collection::<Document>("operator_settings")
        .find_one(doc! {"name": SETTING})
        .run()
        .map_err(io_error)?;
    Ok(found.map_or(
        Setting {
            enabled: false,
            changed_by: String::new(),
            changed_at: 0,
        },
        |document| Setting {
            enabled: document.get_bool("enabled").unwrap_or(false),
            changed_by: document
                .get_str("changed_by")
                .unwrap_or_default()
                .to_string(),
            changed_at: store::number(document.get("changed_at")),
        },
    ))
}

/** Start GitHub sign-in (standalone) with a random state bound to a short-lived cookie
 * Input
    - request: &Request - request
    - auth: Option<&Auth> - sign-in
 * Output
    - Reply
*/
fn login(request: &Request, auth: Option<&Auth>) -> Reply {
    let Some(auth) = auth else {
        return Reply::redirect("/");
    };
    let state = random_bytes(16)
        .map(|bytes| hex(&bytes))
        .unwrap_or_default();
    Reply::redirect(&auth.authorize_url(request, &state, false)).with_header(
        "Set-Cookie",
        &Auth::cookie(request, "crane_operator_oauth", &state, 600),
    )
}

/** Finish GitHub sign-in (standalone): only a listed operator gets a session
 * Input
    - request: &Request - request
    - operator: &Operator - shared state
    - auth: Option<&Auth> - sign-in
    - query: &BTreeMap<String, String> - code and state
 * Output
    - Reply
*/
fn callback(
    request: &Request,
    operator: &Operator,
    auth: Option<&Auth>,
    query: &BTreeMap<String, String>,
) -> Reply {
    let Some(auth) = auth else {
        return Reply::redirect("/");
    };
    let expected = http::cookie(request, "crane_operator_oauth").filter(|state| !state.is_empty());
    if expected.is_none() || query.get("state") != expected.as_ref() {
        return Reply::json(
            400,
            json!({"error": "the sign-in request did not match this browser; start again"}),
        );
    }
    let Some(code) = query.get("code") else {
        return Reply::json(400, json!({"error": "GitHub returned no code"}));
    };
    let clear = Auth::cookie(request, "crane_operator_oauth", "", 0);
    match auth.sign_in(request, code) {
        Ok((viewer, token)) => {
            drop(token);
            if !operator.logins.contains(&viewer.login) {
                log::write(
                    "warn",
                    "operator",
                    None,
                    "sign_in_refused",
                    json!({"login": viewer.login}),
                );
                return Reply::redirect("/?refused=1").with_header("Set-Cookie", &clear);
            }
            Reply::redirect("/")
                .with_header(
                    "Set-Cookie",
                    &Auth::cookie(
                        request,
                        "foxx_session",
                        &auth.session(&viewer),
                        SESSION_MILLIS / 1000,
                    ),
                )
                .with_header("Set-Cookie", &clear)
        }
        Err(error) => Reply::json(400, json!({"error": error})),
    }
}

/** The signed-in operator's login, checked against the list on every request
 * Input
    - request: &Request - request
    - operator: &Operator - shared state
    - auth: Option<&Auth> - sign-in
 * Output
    - Result<String, Reply> - the login, or the refusal (503 not configured, 401 signed out, 403
      not an operator)
*/
fn signed_in(request: &Request, operator: &Operator, auth: Option<&Auth>) -> Result<String, Reply> {
    let Some(auth) = auth else {
        return Err(Reply::json(
            503,
            json!({"error": "sign-in is not configured: set CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET"}),
        ));
    };
    let Some(viewer) = auth.viewer(request) else {
        return Err(Reply::json(401, json!({"error": "sign in with GitHub"})));
    };
    if !operator.logins.contains(&viewer.login) {
        return Err(Reply::json(
            403,
            json!({"error": "this dashboard is for the operator only", "login": viewer.login}),
        ));
    }
    Ok(viewer.login)
}

/** What the operator page shows
 * Input
    - operator: &Operator - shared state
    - login: &str - the signed-in operator
 * Output
    - Reply
*/
fn state(operator: &Operator, login: &str) -> Reply {
    let Ok(setting) = current(&operator.database) else {
        return Reply::json(503, json!({"error": "the database is unavailable"}));
    };
    let audit = operator
        .database
        .collection::<Document>("operator_audit")
        .find(doc! {"setting": SETTING})
        .sort(doc! {"at": -1})
        .limit(50)
        .run()
        .map(|cursor| {
            cursor
                .filter_map(Result::ok)
                .map(|document| {
                    json!({
                        "at": store::number(document.get("at")),
                        "login": document.get_str("login").unwrap_or_default(),
                        "from": document.get_bool("from").unwrap_or(false),
                        "to": document.get_bool("to").unwrap_or(false),
                        "reason": document.get_str("reason").unwrap_or_default(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let commands = READ_ONLY
        .iter()
        .filter(|pattern| !pattern.is_empty())
        .map(|pattern| {
            format!(
                "crane {}",
                pattern
                    .iter()
                    .filter(|part| **part != "$")
                    .map(|part| if *part == "*" { "NAME" } else { part })
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect::<Vec<_>>();
    Reply::json(
        200,
        json!({
            "login": login,
            "agent_commands": {
                "enabled": setting.enabled,
                "changed_by": setting.changed_by,
                "changed_at": setting.changed_at,
            },
            "public_key": operator.key.public_hex(),
            "lifetime_minutes": POLICY_LIFETIME_MILLIS / 60_000,
            "commands": commands,
            "audit": audit,
        }),
    )
}

/** Change the switch: operator only, JSON with the X-Crane-Operator header, a reason required;
 * every change is kept in the audit
 * Input
    - request: &mut Request - request
    - operator: &Operator - shared state
    - auth: Option<&Auth> - sign-in
 * Output
    - Reply
*/
fn change(request: &mut Request, operator: &Operator, auth: Option<&Auth>) -> Reply {
    let login = match signed_in(request, operator, auth) {
        Ok(login) => login,
        Err(reply) => return reply,
    };
    // A custom header cannot be sent by another site's page without a CORS preflight, which this
    // service never answers, so a forged form or link cannot flip the switch
    if http::header(request, "X-Crane-Operator").as_deref() != Some("1")
        || !http::header(request, "Content-Type")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .starts_with("application/json")
    {
        return Reply::json(
            400,
            json!({"error": "changes need JSON and the X-Crane-Operator header"}),
        );
    }
    let mut body = Vec::new();
    if request
        .as_reader()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut body)
        .is_err()
        || body.len() > MAX_BODY
    {
        return Reply::json(413, json!({"error": "request body too large"}));
    }
    let Ok(asked) = serde_json::from_slice::<Value>(&body) else {
        return Reply::json(400, json!({"error": "the body is not JSON"}));
    };
    let Some(enabled) = asked["enabled"].as_bool() else {
        return Reply::json(400, json!({"error": "enabled must be true or false"}));
    };
    let reason = asked["reason"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if reason.is_empty() || reason.chars().count() > 300 {
        return Reply::json(
            400,
            json!({"error": "give a reason of at most 300 characters"}),
        );
    }
    let Ok(before) = current(&operator.database) else {
        return Reply::json(503, json!({"error": "the database is unavailable"}));
    };
    let now = now_millis();
    let stored = operator
        .database
        .collection::<Document>("operator_settings")
        .update_one(
            doc! {"name": SETTING},
            doc! {"$set": {"enabled": enabled, "changed_by": &login, "changed_at": now as i64}},
        )
        .upsert(true)
        .run();
    if stored.is_err() {
        return Reply::json(503, json!({"error": "the setting could not be stored"}));
    }
    let _ = operator
        .database
        .collection::<Document>("operator_audit")
        .insert_one(doc! {
            "setting": SETTING,
            "at": now as i64,
            "login": &login,
            "from": before.enabled,
            "to": enabled,
            "reason": &reason,
        })
        .run();
    log::write(
        "info",
        "operator",
        None,
        "agent_commands_changed",
        json!({"login": login, "from": before.enabled, "to": enabled}),
    );
    state(operator, &login)
}

#[cfg(test)]
mod tests {
    use super::{parse_logins, DEFAULT_OPERATORS};

    /** Operator logins are read case-insensitively, with or without "@", without duplicates
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn operator_logins_are_normalized() {
        assert_eq!(
            parse_logins(" @VishuKalier2003, vishukalier2003 ;Other "),
            vec!["other".to_string(), "vishukalier2003".to_string()]
        );
        assert!(parse_logins(" , ").is_empty());
        assert_eq!(
            parse_logins(DEFAULT_OPERATORS),
            vec!["vishukalier2003".to_string()]
        );
    }
}
