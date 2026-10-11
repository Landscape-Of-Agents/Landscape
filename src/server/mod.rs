// The Foxx ingestion service ('crane server', cargo feature "server"): the only component that
// holds MongoDB credentials. Crane installations upload their evidence here and never see the
// database. TLS is terminated by the hosting platform in front of the service.
//
// Every /v1 request must carry an installation's Ed25519 public key, a timestamp within five
// minutes of the service clock, and a signature over the method, path, timestamp, and body digest
// (see telemetry/ingest.rs). The public key is the installation's identity: every stored record is
// tagged with it and every read is filtered by it, so an installation can only add to and read its
// own records, never read or overwrite another's, and no account or enrollment is needed.
//
// Configuration comes from the service's environment only: CRANE_MONGODB_URI (required),
// CRANE_MONGODB_DB (default foxx), CRANE_MONGODB_EVENT_RETENTION_DAYS (default 30), and the listen
// address from --listen ADDRESS, else 0.0.0.0:$PORT, else 0.0.0.0:8080.

mod database;
mod store;

use std::io::{Read, Write};
use std::sync::Arc;

use serde_json::{json, Value};
use tiny_http::{Header, Request, Response, Server};

use crate::platform::now_millis;
use crate::telemetry::ingest::{request_message, Batch, REQUEST_DOMAIN};
use crate::trust::crypto::{is_digest, verify_signature};

/** Largest accepted request body */
const MAX_BODY: usize = 16 * 1024 * 1024;

/** Largest accepted difference between a request timestamp and the service clock */
const WINDOW_MILLIS: u64 = 5 * 60 * 1000;

/** Requests served concurrently */
const WORKERS: usize = 8;

/** An HTTP status and a message for the client */
type Failure = (u16, String);

/** Run the service until it is stopped
 * Input
    - args: &[String] - optional --listen ADDRESS
 * Output
    - Result<(), String>
    - Error if the database is unreachable or the address cannot be bound
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
    let server = Arc::new(
        Server::http(&listen).map_err(|error| format!("cannot listen on {listen}: {error}"))?,
    );
    let address = server
        .server_addr()
        .to_ip()
        .map_or(listen, |address| address.to_string());
    println!(
        "Foxx ingestion service listening on {address} (database {})",
        settings.database
    );
    std::io::stdout().flush().ok();
    let workers = (0..WORKERS)
        .map(|_| {
            let server = Arc::clone(&server);
            let connected = connected.clone();
            let settings = settings.clone();
            std::thread::spawn(move || {
                for request in server.incoming_requests() {
                    respond(request, &connected, &settings);
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

/** Answer one request with JSON
 * Input
    - request: Request - the request
    - database: &mongodb::sync::Database - database
    - settings: &database::Settings - service settings
 * Output
    - None
*/
fn respond(
    mut request: Request,
    database: &mongodb::sync::Database,
    settings: &database::Settings,
) {
    let (status, body) = match handle(&mut request, database, settings) {
        Ok(value) => (200, value),
        Err((status, message)) => (status, json!({"error": message})),
    };
    let mut response = Response::from_string(body.to_string()).with_status_code(status);
    for (field, value) in [
        ("Content-Type", "application/json"),
        ("Cache-Control", "no-store"),
    ] {
        if let Ok(header) = Header::from_bytes(field.as_bytes(), value.as_bytes()) {
            response.add_header(header);
        }
    }
    let _ = request.respond(response);
}

/** Read a request header
 * Input
    - request: &Request - request
    - name: &str - header name
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

/** Route a request
 * Input
    - request: &mut Request - request
    - database: &mongodb::sync::Database - database
    - settings: &database::Settings - service settings
 * Output
    - Result<Value, Failure>
*/
fn handle(
    request: &mut Request,
    database: &mongodb::sync::Database,
    settings: &database::Settings,
) -> Result<Value, Failure> {
    let method = request.method().as_str().to_ascii_uppercase();
    let url = request.url().to_string();
    if url == "/health" || url.ends_with("/health") {
        return database::ping(database)
            .map(|()| json!({"status": "ok", "schema_version": database::SCHEMA_VERSION}))
            .map_err(|_| (503, "the database is unavailable".to_string()));
    }
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
    let stored = match (method.as_str(), operation.as_str()) {
        ("GET", "positions") => {
            store::positions(database, &repository, &installation).map(|positions| json!(positions))
        }
        ("GET", "snapshot") => store::snapshot(database, &repository, &installation),
        ("POST", "batch") => {
            let batch: Batch = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid batch: {error}")))?;
            store::ingest(database, settings, &repository, &installation, &batch)
        }
        (_, "positions" | "snapshot" | "batch") => return Err((405, "method not allowed".into())),
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
