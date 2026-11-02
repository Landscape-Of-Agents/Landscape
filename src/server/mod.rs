// The DataSea ingestion service ('crane server', cargo feature "server"): the only component that
// holds MongoDB credentials. Crane installations upload their evidence here and never see the
// database. TLS is terminated by the hosting platform in front of the service.
//
// Every /v1 request must carry an installation's Ed25519 public key (each machine's upload key), a
// timestamp within five minutes of the service clock, and a signature over the method, path,
// timestamp, and body digest (see telemetry/ingest.rs). The public key is the installation's
// identity: every stored record is tagged with it and every /v1 read is filtered by it, so an
// installation can upload without an account. People open DataSea, the hosted dashboard at the
// service's root (dashboard/): a landing page and the organization's sections, of which
// Engineering holds Flows, after signing in with GitHub.
//
// Configuration comes from the service's environment only: CRANE_MONGODB_URI (required),
// CRANE_MONGODB_DB (default foxx), the limits, CRANE_GITHUB_CLIENT_ID and
// CRANE_GITHUB_CLIENT_SECRET for dashboard sign-in, and the listen address from --listen ADDRESS,
// else 0.0.0.0:$PORT, else 0.0.0.0:8080.

mod dashboard;
mod database;
mod http;
mod ingest;
mod limits;
mod log;
mod store;

use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use mongodb::sync::Database;
use serde_json::json;
use tiny_http::{Request, Server};

pub(crate) use http::Reply;
use limits::{Limiter, Limits, Storage};

/** Requests served concurrently */
const WORKERS: usize = 8;

/** Everything the request handlers share
 * Fields
    - database: Database - MongoDB
    - settings: database::Settings - database settings
    - limits: Limits - upload limits
    - installations: Limiter - signed requests per installation
    - addresses: Limiter - requests per client address
    - storage: Storage - database size gauge
    - agent_policies: Limiter - quarantine reads per installation (asked before tool calls)
    - dashboard: dashboard::Dashboard - the hosted dashboard
*/
struct Service {
    database: Database,
    settings: database::Settings,
    limits: Limits,
    installations: Limiter,
    agent_policies: Limiter,
    addresses: Limiter,
    storage: Storage,
    dashboard: dashboard::Dashboard,
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
            dashboard::approve::Approvals::new(connected.clone()),
            auth,
        ),
        database: connected,
        installations: Limiter::new(limits.requests_per_minute),
        agent_policies: Limiter::new(1_200.0),
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
    log::write(
        "info",
        "service",
        None,
        "started",
        json!({
            "database": service.settings.database,
            "schema_version": database::SCHEMA_VERSION,
            "sign_in": service.dashboard.configured(),
        }),
    );
    println!(
        "DataSea ingestion service listening on {address} (database {}; dashboard sign-in {})",
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

/** Turn a panic raised while answering a request into a logged JSON error
 * Input
    - panic: Box<dyn std::any::Any + Send> - the panic payload
    - method: &str - HTTP method
    - path: &str - path
    - request_id: &str - correlation identifier
 * Output
    - Reply - 500 with the request identifier
*/
fn panicked(
    panic: Box<dyn std::any::Any + Send>,
    method: &str,
    path: &str,
    request_id: &str,
) -> Reply {
    let message = panic
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into());
    log::write(
        "error",
        "http",
        Some(request_id),
        "request_panicked",
        json!({"method": method, "path": path, "error": message}),
    );
    Reply::json(
        500,
        json!({"error": format!("the service failed while answering (request {request_id})"), "request_id": request_id}),
    )
}

/** Answer one request, logging it with its correlation identifier
 * Input
    - request: Request - the request
    - service: &Service - shared state
 * Output
    - None
*/
fn respond(mut request: Request, service: &Service) {
    let started = Instant::now();
    let request_id = http::request_id(&request);
    let method = request.method().as_str().to_string();
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or_default().to_string();
    // The service's own health check (also behind a path prefix), not the dashboard's /api/health
    let health = path == "/health" || (path.ends_with("/health") && !path.starts_with("/api/"));
    // A panic while answering becomes a JSON error for this request instead of a dropped
    // connection (which the platform's proxy turns into a non-JSON error page) and a lost worker
    let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if health {
            let ping = database::ping(&service.database);
            match ping {
                Ok(()) => Reply::json(
                    200,
                    json!({"status": "ok", "schema_version": database::SCHEMA_VERSION}),
                ),
                Err(_) => Reply::json(503, json!({"error": "the database is unavailable"})),
            }
        } else if !service.addresses.allow(&http::client_address(&request)) {
            Reply::json(
                429,
                json!({"error": "too many requests from this address; try again in a minute"}),
            )
            .with_header("Retry-After", "60")
        } else if !url.starts_with("/v1/") && !url.contains("/v1/repositories/") {
            service.dashboard.handle(&mut request)
        } else {
            match ingest::handle(&mut request, service, &request_id) {
                Ok(value) => Reply::json(200, value),
                Err((status, message)) => {
                    let reply =
                        Reply::json(status, json!({"error": message, "request_id": request_id}));
                    if status == 429 {
                        reply.with_header("Retry-After", "60")
                    } else {
                        reply
                    }
                }
            }
        }
    }))
    .unwrap_or_else(|panic| panicked(panic, &method, &path, &request_id));
    // Health checks arrive every few seconds from the platform; they are not logged
    if !health {
        log::write(
            if reply.status >= 500 {
                "error"
            } else if reply.status >= 400 {
                "warn"
            } else {
                "info"
            },
            "http",
            Some(&request_id),
            "request",
            json!({"method": method, "path": path, "status": reply.status, "ms": started.elapsed().as_millis() as u64}),
        );
    }
    http::send(request, reply, &request_id);
}
