// The signed upload API (/v1): every request carries a machine's upload key, a timestamp within
// five minutes of the service clock, and an Ed25519 signature over the method, path, timestamp,
// and body digest (see telemetry/ingest.rs). Uploads pass the limits (limits.rs) before they are
// stored (store.rs). Positions and batch handle evidence uploads; the sole policy read returns
// active repository quarantine rules, never telemetry or incident records.

use std::io::Read;

use serde_json::{json, Value};
use tiny_http::Request;

use super::http::header;
use super::{database, log, store, Service};
use crate::governance::workspace::identity;
use crate::platform::now_millis;
use crate::telemetry::ingest::{
    is_approval_code, owner_message, ApprovalRequest, CODE_DOMAIN, OWNER_DOMAIN,
};
use crate::telemetry::ingest::{request_message, Batch, BODY_ENCODING, REQUEST_DOMAIN};
use crate::trust::crypto::{digest, is_digest, verify_signature};

/** Largest accepted request body (as sent: base64 adds a third to the JSON) */
const MAX_BODY: usize = 12 * 1024 * 1024;

/** Largest accepted difference between a request timestamp and the service clock */
const WINDOW_MILLIS: u64 = 5 * 60 * 1000;

/** An HTTP status and a message for the client */
pub(super) type Failure = (u16, String);

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

/** Check an approval request: a well-formed code, a GitHub remote that the root commits prove to be
 * this repository (the dashboard checks the person's access to that remote), and for an owner
 * registration a valid signature by the owner key
 * Input
    - repository: &str - repository identity
    - installation: &str - the requesting machine
    - operation: &str - link or owner
    - approval: &ApprovalRequest - the request
 * Output
    - Result<(), Failure>
*/
fn check_approval(
    repository: &str,
    installation: &str,
    operation: &str,
    approval: &ApprovalRequest,
) -> Result<(), Failure> {
    if !is_approval_code(&approval.code) {
        return Err((400, "the approval code is malformed".into()));
    }
    let remote = approval.remote.as_deref().unwrap_or_default();
    if !remote.starts_with("github.com/") {
        return Err((
            400,
            "approvals need a GitHub remote, so the dashboard can check access with GitHub".into(),
        ));
    }
    if approval.root_commits.is_empty()
        || identity(&approval.root_commits, Some(remote)) != repository
    {
        return Err((
            403,
            "the root commits and remote do not prove this repository".into(),
        ));
    }
    if operation == "owner" {
        let key = approval.owner_key.as_deref().unwrap_or_default();
        let signature = approval.signature.as_deref().unwrap_or_default();
        let code_digest = digest(CODE_DOMAIN, &[approval.code.as_bytes()]);
        let message = owner_message(repository, key, installation, &code_digest);
        if key.len() != 64 || verify_signature(key, OWNER_DOMAIN, &message, signature).is_err() {
            return Err((403, "the owner key's signature does not verify".into()));
        }
    }
    Ok(())
}

/** Route a /v1 request
 * Input
    - request: &mut Request - request
    - service: &Service - shared state
    - request_id: &str - correlation identifier, for the log
 * Output
    - Result<Value, Failure>
*/
pub(super) fn handle(
    request: &mut Request,
    service: &Service,
    request_id: &str,
) -> Result<Value, Failure> {
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
    // Current clients send base64 bodies (signed as sent) so the hosting edge firewall cannot
    // mistake shell commands in telemetry for an attack; older clients send plain JSON
    if header(request, BODY_ENCODING).is_some_and(|value| value.eq_ignore_ascii_case("base64")) {
        body = crate::platform::http::unbase64(&body)
            .map_err(|error| (400, format!("the request body is not {error}")))?;
    }
    let allowed = if operation == "agent-policy" {
        service.agent_policies.allow(&installation)
    } else {
        service.installations.allow(&installation)
    };
    if !allowed {
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
            let before = super::observe::before(database, &repository, &installation);
            let stored = store::ingest(
                database,
                &service.settings,
                &repository,
                &installation,
                &batch,
            );
            if let Ok(stored) = &stored {
                super::dashboard::uploaded(&repository);
                if let Err(error) = service.agent_wake.try_send(repository.clone()) {
                    let (level, message) = match error {
                        std::sync::mpsc::TrySendError::Full(_) => (
                            "warn",
                            "agent evaluation queue is full; the periodic sweep will recover it",
                        ),
                        std::sync::mpsc::TrySendError::Disconnected(_) => (
                            "error",
                            "agent evaluation worker is unavailable; check service health",
                        ),
                    };
                    log::write(
                        level,
                        "dashboard_agents",
                        None,
                        "upload_evaluation_enqueue_failed",
                        json!({"repository_id": repository, "reason": message}),
                    );
                }
                let verified = !batch.root_commits.is_empty()
                    && batch.remote.is_some()
                    && identity(&batch.root_commits, batch.remote.as_deref()) == repository;
                super::observe::batch(
                    &service.log,
                    &repository,
                    &installation,
                    &batch,
                    before,
                    stored,
                    verified,
                );
            }
            stored
        }
        ("POST", operation @ ("link" | "owner")) => {
            let approval: ApprovalRequest = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid request: {error}")))?;
            check_approval(&repository, &installation, operation, &approval)?;
            store::hold_approval(database, &repository, &installation, operation, &approval)
        }
        ("GET", "owner") => store::confirmed_owner(database, &repository),
        ("GET", "agent-policy") => dashboard_agent_policy(database, &repository),
        // Credit grants an admin approved on the dashboard, for this machine only
        ("GET", "grants") => store::pending_grants(database, &repository, &installation),
        ("POST", "grants") => {
            let results: Value = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid grant results: {error}")))?;
            store::settle_grants(database, &repository, &installation, &results)
        }
        // Resident-agent decisions an admin made on the dashboard, for this machine only
        ("GET", "resident-decisions") => {
            store::pending_resident_decisions(database, &repository, &installation)
        }
        ("POST", "resident-decisions") => {
            let results: Value = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid decision results: {error}")))?;
            store::settle_resident_decisions(database, &repository, &installation, &results)
        }
        // The org catalog of Bricks (brick/share.rs): an admin's machine publishes an activated
        // version, signed; the account is the owner of the remote this machine verified
        ("POST", "brick-catalog") => {
            let entry: Value = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid catalog entry: {error}")))?;
            let brick = crate::brick::share::verify(&entry).map_err(|error| (400, error))?;
            let (account, remote) = store::catalog_account(database, &repository, &installation)
                .map_err(|error| (500, error))?
                .ok_or((403, "only a repository whose GitHub remote this machine verified can publish to an organization's catalog".to_string()))?;
            let reference = brick.reference().to_string();
            match store::catalog_digest(database, &account, &brick.brick_id, brick.version)
                .map_err(|error| (500, error))?
            {
                Some(digest) if digest == brick.digest() => {
                    Ok(json!({"account": account, "ref": reference, "stored": false}))
                }
                Some(_) => return Err((409, format!("org:{reference} is already published with other content; a published version never changes, so publish the next version"))),
                None => {
                    if store::catalog_size(database, &account).map_err(|error| (500, error))?
                        >= crate::brick::share::MAX_ENTRIES
                    {
                        return Err((409, format!("the catalog of {account} holds {} entries, the most it keeps", crate::brick::share::MAX_ENTRIES)));
                    }
                    let kept = store::keep_catalog_entry(database, &account, &remote, &repository, &brick, &entry);
                    if kept.is_ok() {
                        super::observe::brick_published(&service.log, &repository, &installation, &account, &reference);
                    }
                    kept
                }
            }
        }
        ("GET", "brick-catalog") => {
            match store::catalog_account(database, &repository, &installation)
                .map_err(|error| (500, error))?
            {
                Some((account, _)) => store::brick_catalog(database, &account),
                None => Ok(
                    json!({"account": null, "entries": [], "detail": "this machine has not verified a GitHub remote for the repository, so it has no organization catalog"}),
                ),
            }
        }
        ("POST", "brick-catalog-entry") => {
            let asked: Value = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid request: {error}")))?;
            let (id, version) =
                crate::brick::share::reference(asked["ref"].as_str().unwrap_or_default())
                    .map_err(|error| (400, error))?;
            let (account, _) = store::catalog_account(database, &repository, &installation)
                .map_err(|error| (500, error))?
                .ok_or((403, "this machine has not verified a GitHub remote for the repository, so it has no organization catalog".to_string()))?;
            match store::catalog_entry(database, &account, &id, version).map_err(|error| (500, error))? {
                Some(entry) => Ok(json!({"account": account, "entry": entry})),
                None => return Err((404, format!("org:{id}@{version} is not in the catalog of {account} ('crane brick catalog' lists it)"))),
            }
        }
        // Brick changes people requested on the dashboard, for any machine of the repository
        ("GET", "brick-requests") => store::pending_brick_requests(database, &repository),
        ("POST", "brick-requests") => {
            let results: Value = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid Brick request results: {error}")))?;
            let settled =
                store::settle_brick_requests(database, &repository, &installation, &results);
            if let Ok(settled) = &settled {
                super::observe::brick_requests_settled(
                    &service.log,
                    &repository,
                    &installation,
                    settled,
                );
            }
            settled
        }
        ("POST", "clear") => {
            let request: Value = serde_json::from_slice(&body)
                .map_err(|error| (400, format!("invalid clear request: {error}")))?;
            let remote = request["remote"]
                .as_str()
                .ok_or_else(|| (400, "repository remote is required".to_string()))?;
            let token = request["github_token"]
                .as_str()
                .ok_or_else(|| (400, "GitHub authorization is required".to_string()))?;
            let login = service
                .dashboard
                .codeowner(token, remote)
                .map_err(|message| (403, message))?;
            let mut result = match store::clear_repository(
                database,
                &repository,
                &installation,
                remote,
            ) {
                Ok(result) => result,
                Err(store::ClearRepositoryError::NotVerified) => {
                    return Err((
                        403,
                        "this machine has no verified record for that repository remote".into(),
                    ))
                }
                Err(store::ClearRepositoryError::Database(error)) => {
                    log::write(
                        "error",
                        "ingest",
                        Some(request_id),
                        "repository data clear failed",
                        json!({"operation": operation, "installation": &installation[..16], "error": database::redact_uri(&error)}),
                    );
                    return Err((
                        500,
                        "repository data could not be cleared; try again later".into(),
                    ));
                }
            };
            result["authorized_as"] = json!(login);
            super::dashboard::uploaded(&repository);
            Ok(result)
        }
        (
            _,
            "positions"
            | "batch"
            | "link"
            | "owner"
            | "agent-policy"
            | "grants"
            | "resident-decisions"
            | "brick-requests"
            | "brick-catalog"
            | "brick-catalog-entry"
            | "clear",
        ) => return Err((405, "method not allowed".into())),
        _ => return Err(not_found()),
    };
    stored.map_err(|error| {
        log::write(
            "error",
            "ingest",
            Some(request_id),
            "storing failed",
            json!({"operation": operation, "installation": &installation[..16], "error": database::redact_uri(&error)}),
        );
        (
            500,
            "the record could not be stored; try again later".to_string(),
        )
    })
}

/** Return only active quarantine rules; telemetry remains unavailable to installations. */
fn dashboard_agent_policy(
    database: &mongodb::sync::Database,
    repository: &str,
) -> Result<Value, String> {
    let documents = database
        .collection::<mongodb::bson::Document>("dashboard_quarantines")
        .find(mongodb::bson::doc! {
            "repository_id": repository,
            "state": "ACTIVE",
        })
        .projection(mongodb::bson::doc! {
            "_id": 0,
            "quarantine_id": 1,
            "kind": 1,
            "target_id": 1,
            "scope_session_id": 1,
            "reason": 1,
        })
        .run()
        .map_err(crate::platform::io_error)?
        .map(|document| document.map_err(crate::platform::io_error))
        .collect::<Result<Vec<_>, _>>()?;
    let quarantines = documents
        .into_iter()
        .map(|document| serde_json::to_value(document).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"quarantines": quarantines}))
}
