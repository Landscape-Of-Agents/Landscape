// The Foxx ingestion service ('crane server', cargo feature "server"): the only component that
// holds MongoDB credentials. Crane installations upload their evidence here and never see the
// database. TLS is terminated by the hosting platform in front of the service.
//
// Every /v1 request must carry an installation's Ed25519 public key (each machine's upload key), a
// timestamp within five minutes of the service clock, and a signature over the method, path,
// timestamp, and body digest (see telemetry/ingest.rs). The public key is the installation's
// identity: every stored record is tagged with it and every /v1 read is filtered by it, so an
// installation can only add to and read its own records, and no account or enrollment is needed.
// Machines can only upload: no /v1 route returns stored records. Uploads are rate-limited and
// capped (limits.rs). People read the evidence on the hosted dashboard at the service's root
// (dashboard/), after signing in with GitHub, through a read-only database handle.
//
// Configuration comes from the service's environment only: CRANE_MONGODB_URI (required),
// CRANE_MONGODB_DB (default foxx), CRANE_MONGODB_EVENT_RETENTION_DAYS (default 30), the limits,
// CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET for dashboard sign-in, and the listen
// address from --listen ADDRESS, else 0.0.0.0:$PORT, else 0.0.0.0:8080.

mod dashboard;
mod database;
mod limits;
mod store;

use std::io::{Read, Write};
use std::sync::Arc;

use mongodb::sync::Database;
use serde_json::{json, Value};
use tiny_http::{Header, Request, Response, Server};

use crate::platform::now_millis;
use crate::telemetry::ingest::{request_message, Batch, REQUEST_DOMAIN};
use crate::trust::crypto::{is_digest, verify_signature};
use limits::{Limiter, Limits, Storage};

/** Largest accepted request body */
const MAX_BODY: usize = 8 * 1024 * 1024;

/** Largest accepted difference between a request timestamp and the service clock */
const WINDOW_MILLIS: u64 = 5 * 60 * 1000;

/** Requests served concurrently */
const WORKERS: usize = 8;

/** An HTTP status and a message for the client */
type Failure = (u16, String);

/** Everything the request handlers share
 * Fields
    - database: Database - MongoDB
    - settings: database::Settings - database settings
    - limits: Limits - upload limits
    - installations: Limiter - signed requests per installation
    - addresses: Limiter - requests per client address
    - storage: Storage - database size gauge
    - dashboard: dashboard::Dashboard - the hosted dashboard
*/
struct Service {
    database: Database,
    settings: database::Settings,
    limits: Limits,
    installations: Limiter,
    addresses: Limiter,
    storage: Storage,
    dashboard: dashboard::Dashboard,
}

/** A response
 * Fields
    - status: u16 - HTTP status
    - content_type: &'static str - media type
    - body: String - body
    - headers: Vec<(String, String)> - extra headers
*/
pub(super) struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
    headers: Vec<(String, String)>,
}

impl Reply {
    /** A JSON response
     * Input
        - status: u16 - HTTP status
        - value: Value - body
     * Output
        - Reply
    */
    pub(crate) fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string(),
            headers: Vec::new(),
        }
    }

    /** The dashboard page, with a strict content security policy: only its own script, same-origin
     * requests, no frames, no forms
     * Input
        - body: &str - page
     * Output
        - Reply
    */
    pub(crate) fn page(body: &str) -> Self {
        Self {
            status: 200,
            content_type: "text/html; charset=utf-8",
            body: body.into(),
            headers: vec![
                ("Content-Security-Policy".into(), "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'unsafe-inline'; img-src 'self' data:; form-action 'none'; frame-ancestors 'none'; base-uri 'none'".into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
                ("Referrer-Policy".into(), "no-referrer".into()),
            ],
        }
    }

    /** The dashboard script
     * Input
        - body: &str - script
     * Output
        - Reply
    */
    pub(crate) fn script(body: &str) -> Self {
        Self {
            status: 200,
            content_type: "text/javascript; charset=utf-8",
            body: body.into(),
            headers: vec![("X-Content-Type-Options".into(), "nosniff".into())],
        }
    }

    /** A redirect
     * Input
        - location: &str - target
     * Output
        - Reply
    */
    pub(crate) fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            content_type: "text/plain",
            body: String::new(),
            headers: vec![("Location".into(), location.into())],
        }
    }

    /** Add a header
     * Input
        - name: &str - header name
        - value: &str - header value
     * Output
        - Reply
    */
    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/** Run the service until it is stopped
 * Input
    - args: &[String] - optional --listen ADDRESS
 * Output
    - Result<(), String>
    - Error if the database is unreachable, the team settings are incomplete, or the address
      cannot be bound
*/
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let listen = match args {
        [] => std::env::var("PORT")
            .map(|port| format!("0.0.0.0:{port}"))
            .unwrap_or_else(|_| "0.0.0.0:8080".into()),
        [flag, address] if flag == "--listen" => address.clone(),
        _ => return Err("usage: crane server [--listen ADDRESS]".into()),
    };
    let settings = database::Settings::from_environment()?;
    let connected = database::connect(&settings)?;
    database::migrate(&connected)?;
    let limits = Limits::from_environment();
    let auth = dashboard::auth::Auth::from_environment(&store::session_seed(&connected)?)?;
    let service = Arc::new(Service {
        dashboard: dashboard::Dashboard::new(
            dashboard::data::ReadOnly::new(connected.clone()),
            auth,
        ),
        database: connected,
        installations: Limiter::new(limits.requests_per_minute),
        addresses: Limiter::new(limits.addresses_per_minute),
        storage: Storage::new(),
        settings,
        limits,
    });
    let server = Arc::new(
        Server::http(&listen).map_err(|error| format!("cannot listen on {listen}: {error}"))?,
    );
    let address = server
        .server_addr()
        .to_ip()
        .map_or(listen, |address| address.to_string());
    println!(
        "Foxx ingestion service listening on {address} (database {}; dashboard sign-in {})",
        service.settings.database,
        if service.dashboard.configured() {
            "on"
        } else {
            "off: set CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET"
        }
    );
    std::io::stdout().flush().ok();
    let workers = (0..WORKERS)
        .map(|_| {
            let server = Arc::clone(&server);
            let service = Arc::clone(&service);
            std::thread::spawn(move || {
                for request in server.incoming_requests() {
                    respond(request, &service);
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

/** Answer one request
 * Input
    - request: Request - the request
    - service: &Service - shared state
 * Output
    - None
*/
fn respond(mut request: Request, service: &Service) {
    let address = header(&request, "X-Forwarded-For")
        .and_then(|list| list.split(',').next().map(|first| first.trim().to_string()))
        .or_else(|| {
            request
                .remote_addr()
                .map(|address| address.ip().to_string())
        })
        .unwrap_or_default();
    let url = request.url().to_string();
    let reply = if url == "/health" || url.ends_with("/health") {
        match database::ping(&service.database) {
            Ok(()) => Reply::json(
                200,
                json!({"status": "ok", "schema_version": database::SCHEMA_VERSION}),
            ),
            Err(_) => Reply::json(503, json!({"error": "the database is unavailable"})),
        }
    } else if !service.addresses.allow(&address) {
        Reply::json(
            429,
            json!({"error": "too many requests from this address; try again in a minute"}),
        )
        .with_header("Retry-After", "60")
    } else if !url.starts_with("/v1/") && !url.contains("/v1/repositories/") {
        service.dashboard.handle(&request)
    } else {
        match handle(&mut request, service) {
            Ok(value) => Reply::json(200, value),
            Err((status, message)) => {
                let reply = Reply::json(status, json!({"error": message}));
                if status == 429 {
                    reply.with_header("Retry-After", "60")
                } else {
                    reply
                }
            }
        }
    };
    let mut response = Response::from_string(reply.body).with_status_code(reply.status);
    let headers = [
        ("Content-Type".to_string(), reply.content_type.to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
    ];
    for (field, value) in headers.into_iter().chain(reply.headers) {
        if let Ok(header) = Header::from_bytes(field.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
    let _ = request.respond(response);
}

/** Read a request header
 * Input
    - request: &Request - request
    - name: &'static str - header name
 * Output
    - Option<String>
*/
fn header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str().trim().to_string())
}

/** Check a request's installation key, timestamp, and signature
 * Input
    - request: &Request - request
    - method: &str - HTTP method
    - path: &str - path starting at /v1/
    - body: &[u8] - body
 * Output
    - Result<String, Failure> the installation's public key in hex
*/
fn authenticate(
    request: &Request,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<String, Failure> {
    let unauthorized = |message: &str| (401, message.to_string());
    let installation = header(request, "X-Crane-Installation")
        .filter(|key| {
            key.len() == 64
                && key
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
        .ok_or_else(|| unauthorized("missing or malformed installation key"))?;
    let timestamp = header(request, "X-Crane-Timestamp")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| unauthorized("missing request timestamp"))?;
    if now_millis().abs_diff(timestamp) > WINDOW_MILLIS {
        return Err(unauthorized(
            "the request timestamp is more than five minutes off; check the clock",
        ));
    }
    let signature = header(request, "X-Crane-Signature")
        .ok_or_else(|| unauthorized("missing request signature"))?;
    verify_signature(
        &installation,
        REQUEST_DOMAIN,
        &request_message(method, path, timestamp, body),
        &signature,
    )
    .map_err(|_| unauthorized("invalid request signature"))?;
    Ok(installation)
}

/** Check a batch against the limits before storing it
 * Input
    - service: &Service - shared state
    - repository: &str - repository identity
    - installation: &str - installation key
    - batch: &Batch - batch
 * Output
    - Result<(), Failure>
*/
fn admit(
    service: &Service,
    repository: &str,
    installation: &str,
    batch: &Batch,
) -> Result<(), Failure> {
    let limits = &service.limits;
    let largest = [
        batch.events.len(),
        batch.ledger.len(),
        batch.audit.len(),
        batch.alerts.len(),
        batch.sessions.len(),
        batch.tasks.len(),
        batch.selections.len(),
        batch.rollups.len(),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    if largest > limits.max_batch_records {
        return Err((
            413,
            format!(
                "a batch may carry at most {} records of each kind",
                limits.max_batch_records
            ),
        ));
    }
    let unavailable = |_| {
        (
            503,
            "the database is unavailable; try again later".to_string(),
        )
    };
    if service
        .storage
        .used(&service.database)
        .map_err(unavailable)?
        > limits.max_storage_bytes
    {
        return Err((
            507,
            "the service's storage is full; uploads resume when old telemetry expires".into(),
        ));
    }
    let quota = store::quota(&service.database, repository, installation).map_err(unavailable)?;
    if !quota.known && quota.repositories >= limits.max_repositories_per_installation {
        return Err((
            429,
            format!(
                "this installation already uploads {} repositories, the most allowed",
                quota.repositories
            ),
        ));
    }
    if quota.events + batch.events.len() as u64 > limits.max_events_per_installation {
        return Err((
            507,
            format!(
                "this installation reached its quota of {} stored events",
                limits.max_events_per_installation
            ),
        ));
    }
    Ok(())
}

/** Route a /v1 request
 * Input
    - request: &mut Request - request
    - service: &Service - shared state
 * Output
    - Result<Value, Failure>
*/
fn handle(request: &mut Request, service: &Service) -> Result<Value, Failure> {
    let method = request.method().as_str().to_ascii_uppercase();
    let url = request.url().to_string();
    let not_found = || (404, "not found".to_string());
    let path = url
        .find("/v1/")
        .map(|start| url[start..].to_string())
        .ok_or_else(not_found)?;
    let (repository, operation) = match path.split('/').collect::<Vec<_>>().as_slice() {
        ["", "v1", "repositories", repository, operation] if is_digest(repository) => {
            (repository.to_string(), operation.to_string())
        }
        _ => return Err(not_found()),
    };
    if request
        .body_length()
        .is_some_and(|length| length > MAX_BODY)
    {
        return Err((413, "the request body is too large".into()));
    }
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| (400, "the request body could not be read".to_string()))?;
    if body.len() > MAX_BODY {
        return Err((413, "the request body is too large".into()));
    }
    let installation = authenticate(request, &method, &path, &body)?;
    if !service.installations.allow(&installation) {
        return Err((
            429,
            "too many requests from this installation; try again in a minute".into(),
        ));
    }
    let database = &service.database;
    let stored = match (method.as_str(), operation.as_str()) {
        ("GET", "positions") => {
            store::positions(database, &repository, &installation).map(|positions| json!(positions))
        }
        ("POST", "batch") => {
            let batch: Batch = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid batch: {error}")))?;
            admit(service, &repository, &installation, &batch)?;
            store::ingest(
                database,
                &service.settings,
                &repository,
                &installation,
                &batch,
            )
        }
        (_, "positions" | "batch") => return Err((405, "method not allowed".into())),
        _ => return Err(not_found()),
    };
    stored.map_err(|error| {
        eprintln!(
            "crane server: {method} {operation} for {}: {}",
            &installation[..16],
            database::redact_uri(&error)
        );
        (
            500,
            "the record could not be stored; try again later".to_string(),
        )
    })
}
