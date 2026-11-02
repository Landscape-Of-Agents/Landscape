// Storing and reading an installation's records. A batch updates the repository (its remote and
// whether the upload proved it), the member's latest repository status (which DataSea's Flows
// section reads), and how far each of the machine's streams arrived. The streams themselves
// (events, ledger entries, governance changes, alerts) and the session, task, selection, and
// manifest records are no longer stored: only their positions are kept, so a machine resumes where
// it stopped and never uploads them twice. Every document carries the repository identity and the
// installation key, and every filter includes the installation.

use mongodb::bson::{self, doc, Bson, Document};
use mongodb::sync::Database;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

use super::database::repository_collections;
use crate::governance::workspace::identity;
use crate::platform::{io_error, now_millis};
use crate::telemetry::ingest::{
    ApprovalRequest, Batch, Positions, CODE_DOMAIN, CODE_LIFETIME_MILLIS,
};
use crate::trust::crypto::digest;

/** Convert a record to a BSON document tagged with the repository and installation
 * Input
    - value: &impl Serialize - record
    - repository: &str - repository identity
    - installation: &str - installation key
 * Output
    - Result<Document, String>
*/
fn document(
    value: &impl Serialize,
    repository: &str,
    installation: &str,
) -> Result<Document, String> {
    let mut value = serde_json::to_value(value).map_err(io_error)?;
    if !value.is_object() {
        return Err("a record must be a JSON object".into());
    }
    value["repository_id"] = Value::String(repository.into());
    value["installation"] = Value::String(installation.into());
    bson::to_document(&value).map_err(io_error)
}

/** Replace or insert one document
 * Input
    - database: &Database - database
    - name: &str - collection
    - filter: Document - identity of the document
    - value: Document - new content
 * Output
    - Result<(), String>
*/
fn upsert(
    database: &Database,
    name: &str,
    filter: Document,
    value: Document,
) -> Result<(), String> {
    database
        .collection::<Document>(name)
        .replace_one(filter, value)
        .upsert(true)
        .run()
        .map(|_| ())
        .map_err(io_error)
}

/** Hold an approval request until a person uses its code on the dashboard (the code itself is
 * never stored, only its digest; it expires after CODE_LIFETIME_MILLIS and works once)
 * Input
    - database: &Database - database
    - repository: &str - repository identity
    - installation: &str - the requesting machine
    - operation: &str - link (member_links) or owner (owner_keys)
    - approval: &ApprovalRequest - checked request
 * Output
    - Result<Value, String> {"expires_in_seconds"}
*/
pub(super) fn hold_approval(
    database: &Database,
    repository: &str,
    installation: &str,
    operation: &str,
    approval: &ApprovalRequest,
) -> Result<Value, String> {
    let now = now_millis();
    let mut document = doc! {
        "repository_id": repository,
        "installation": installation,
        "code_digest": digest(CODE_DOMAIN, &[approval.code.as_bytes()]),
        "remote": approval.remote.clone().unwrap_or_default(),
        "requested_at": now as i64,
        "expires_at": (now + CODE_LIFETIME_MILLIS) as i64,
    };
    let collection = if operation == "owner" {
        document.insert("owner_key", approval.owner_key.clone().unwrap_or_default());
        "owner_keys"
    } else {
        "member_links"
    };
    database
        .collection::<Document>(collection)
        .insert_one(document)
        .run()
        .map_err(io_error)?;
    Ok(json!({"expires_in_seconds": CODE_LIFETIME_MILLIS / 1000}))
}

/** Return the Brick change requests people made on the dashboard for this repository, confirmed
 * by GitHub, still waiting and not expired; any machine of the repository may apply them, and
 * only when a person there says so (cli/brick/inbox.rs)
 * Input
    - database: &Database - MongoDB
    - repository: &str - repository identity
 * Output
    - Result<Value, String> {requests: [{request_id, op, params, reason, role, requested_by,
      requested_at}]}
*/
pub(super) fn pending_brick_requests(
    database: &Database,
    repository: &str,
) -> Result<Value, String> {
    let found = database
        .collection::<Document>("brick_requests")
        .find(doc! {"repository_id": repository, "status": "PENDING", "expires_at": {"$gt": now_millis() as i64}})
        .sort(doc! {"requested_at": 1})
        .limit(100)
        .run()
        .map_err(io_error)?;
    let mut requests = Vec::new();
    for document in found {
        let document = document.map_err(io_error)?;
        let text = |key: &str| document.get_str(key).unwrap_or_default().to_string();
        requests.push(json!({
            "request_id": text("request_id"),
            "op": text("op"),
            "params": serde_json::from_str::<Value>(&text("params")).unwrap_or(Value::Null),
            "reason": text("reason"),
            "role": text("role"),
            "requested_by": text("requested_by"),
            "requested_at": number(document.get("requested_at")),
        }));
    }
    Ok(json!({"requests": requests}))
}

/** Record what a machine did with Brick requests: applied, failed (with why), or dismissed by a
 * person there; only waiting requests of this repository change
 * Input
    - database: &Database - MongoDB
    - repository: &str - repository identity
    - installation: &str - the machine
    - body: &Value - {results: [{request_id, status: APPLIED|FAILED|DISMISSED, detail, command}]}
 * Output
    - Result<Value, String> {settled, results: [{request_id, status}]} (only those settled)
*/
pub(super) fn settle_brick_requests(
    database: &Database,
    repository: &str,
    installation: &str,
    body: &Value,
) -> Result<Value, String> {
    let mut settled = Vec::new();
    for result in body["results"].as_array().into_iter().flatten().take(100) {
        let status = match result["status"].as_str() {
            Some("APPLIED") => "APPLIED",
            Some("DISMISSED") => "DISMISSED",
            _ => "FAILED",
        };
        let bounded = |key: &str| {
            result[key]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect::<String>()
        };
        let id = result["request_id"].as_str().unwrap_or_default();
        let now = now_millis();
        let changed = database
            .collection::<Document>("brick_requests")
            .update_one(
                doc! {"request_id": id, "repository_id": repository, "status": "PENDING"},
                doc! {"$set": {"status": status, "settled_at": now as i64, "settled_by": installation, "detail": bounded("detail"), "command": bounded("command"),
                    "purge_at": bson::DateTime::from_millis((now + super::dashboard::BRICK_REQUEST_KEPT_MILLIS) as i64)}},
            )
            .run()
            .map_err(io_error)?;
        if changed.modified_count > 0 {
            settled.push(json!({"request_id": id, "status": status}));
        }
    }
    Ok(json!({"settled": settled.len(), "results": settled}))
}

/** The account of the org catalog a machine's repository shares in: the GitHub owner of the
 * remote this machine verified for it
 * Input
    - database: &Database - MongoDB
    - repository: &str - repository identity
    - installation: &str - the machine
 * Output
    - Result<Option<(String, String)>, String> (account, remote); None without a verified GitHub
      remote
*/
pub(super) fn catalog_account(
    database: &Database,
    repository: &str,
    installation: &str,
) -> Result<Option<(String, String)>, String> {
    let known = database
        .collection::<Document>("repositories")
        .find_one(
            doc! {"repository_id": repository, "installation": installation, "verified": true},
        )
        .run()
        .map_err(io_error)?;
    Ok(known.and_then(|document| {
        let remote = document.get_str("remote").ok()?.to_string();
        let account = super::dashboard::auth::account(&remote)?;
        Some((account, remote))
    }))
}

/** The digest an account's catalog holds for a Brick version, if any
 * Input
    - database: &Database - MongoDB
    - account: &str - account
    - brick_id: &str - Brick
    - version: u32 - version
 * Output
    - Result<Option<String>, String>
*/
pub(super) fn catalog_digest(
    database: &Database,
    account: &str,
    brick_id: &str,
    version: u32,
) -> Result<Option<String>, String> {
    Ok(database
        .collection::<Document>("brick_catalog")
        .find_one(doc! {"account": account, "brick_id": brick_id, "version": version as i64})
        .run()
        .map_err(io_error)?
        .and_then(|document| document.get_str("digest").ok().map(str::to_string)))
}

/** Keep a checked catalog entry (brick::share::verify) in an account's catalog
 * Input
    - database: &Database - MongoDB
    - account: &str - account
    - remote: &str - the publishing repository's remote
    - repository: &str - its identity
    - brick: &crate::brick::model::Brick - the definition
    - entry: &Value - the entry
 * Output
    - Result<Value, String> {account, ref, stored: true}
*/
pub(super) fn keep_catalog_entry(
    database: &Database,
    account: &str,
    remote: &str,
    repository: &str,
    brick: &crate::brick::model::Brick,
    entry: &Value,
) -> Result<Value, String> {
    let text = |key: &str| entry[key].as_str().unwrap_or_default().to_string();
    database
        .collection::<Document>("brick_catalog")
        .insert_one(doc! {
            "account": account, "brick_id": &brick.brick_id, "version": brick.version as i64,
            "digest": brick.digest(), "type": brick.brick_type.name(), "name": &brick.name, "intent": &brick.intent,
            "entry": serde_json::to_string(entry).map_err(io_error)?,
            "published_by": text("published_by"), "publisher_key": text("publisher_key"), "reason": text("reason"),
            "source_repository": repository, "source_remote": remote, "published_at": now_millis() as i64,
        })
        .run()
        .map_err(io_error)?;
    Ok(json!({"account": account, "ref": brick.reference().to_string(), "stored": true}))
}

/** How many entries an account's catalog holds
 * Input
    - database: &Database - MongoDB
    - account: &str - account
 * Output
    - Result<u64, String>
*/
pub(super) fn catalog_size(database: &Database, account: &str) -> Result<u64, String> {
    database
        .collection::<Document>("brick_catalog")
        .count_documents(doc! {"account": account})
        .run()
        .map_err(io_error)
}

/** An account's catalog, newest first, without definitions
 * Input
    - database: &Database - MongoDB
    - account: &str - account
 * Output
    - Result<Value, String> {account, entries: [{ref, brick_id, version, type, name, intent, digest,
      published_by, publisher_key, reason, source_remote, published_at}]}
*/
pub(super) fn brick_catalog(database: &Database, account: &str) -> Result<Value, String> {
    let found = database
        .collection::<Document>("brick_catalog")
        .find(doc! {"account": account})
        .projection(doc! {"entry": 0})
        .sort(doc! {"published_at": -1})
        .limit(crate::brick::share::MAX_ENTRIES as i64)
        .run()
        .map_err(io_error)?;
    let mut entries = Vec::new();
    for document in found {
        entries.push(catalog_row(&document.map_err(io_error)?));
    }
    Ok(json!({"account": account, "entries": entries}))
}

/** One catalog entry as listed
 * Input
    - document: &Document - stored entry
 * Output
    - Value
*/
pub(super) fn catalog_row(document: &Document) -> Value {
    let text = |key: &str| document.get_str(key).unwrap_or_default().to_string();
    let version = number(document.get("version"));
    json!({
        "ref": format!("{}@{version}", text("brick_id")), "brick_id": text("brick_id"), "version": version,
        "type": text("type"), "name": text("name"), "intent": text("intent"), "digest": text("digest"),
        "published_by": text("published_by"), "publisher_key": text("publisher_key"), "reason": text("reason"),
        "source_remote": text("source_remote"), "published_at": number(document.get("published_at")),
    })
}

/** One entry of an account's catalog, with its signed definition
 * Input
    - database: &Database - MongoDB
    - account: &str - account
    - brick_id: &str - Brick
    - version: u32 - version
 * Output
    - Result<Option<Value>, String> the entry as published
*/
pub(super) fn catalog_entry(
    database: &Database,
    account: &str,
    brick_id: &str,
    version: u32,
) -> Result<Option<Value>, String> {
    let found = database
        .collection::<Document>("brick_catalog")
        .find_one(doc! {"account": account, "brick_id": brick_id, "version": version as i64})
        .run()
        .map_err(io_error)?;
    found
        .map(|document| {
            serde_json::from_str::<Value>(document.get_str("entry").unwrap_or_default())
                .map_err(io_error)
        })
        .transpose()
}

/** Return the repository's owner key most recently confirmed by a GitHub admin of its remote
 * (public information: a public key and who confirmed it)
 * Input
    - database: &Database - database
    - repository: &str - repository identity
 * Output
    - Result<Value, String> {owner_key, confirmed_by, confirmed_at, remote}, owner_key null when
      none is confirmed
*/
pub(super) fn confirmed_owner(database: &Database, repository: &str) -> Result<Value, String> {
    let found = database
        .collection::<Document>("owner_keys")
        .find_one(doc! {"repository_id": repository, "confirmed_by": {"$type": "string"}})
        .sort(doc! {"confirmed_at": -1})
        .run()
        .map_err(io_error)?;
    Ok(match found {
        Some(document) => json!({
            "owner_key": document.get_str("owner_key").unwrap_or_default(),
            "confirmed_by": document.get_str("confirmed_by").unwrap_or_default(),
            "confirmed_at": number(document.get("confirmed_at")),
            "remote": document.get_str("remote").unwrap_or_default(),
        }),
        None => json!({"owner_key": null}),
    })
}

/** Check that a batch proves its remote: the root commits and the remote reproduce the repository
 * identity
 * Input
    - repository: &str - repository identity from the request path
    - batch: &Batch - the batch
 * Output
    - bool
*/
fn verified(repository: &str, batch: &Batch) -> bool {
    !batch.root_commits.is_empty()
        && batch.remote.is_some()
        && identity(&batch.root_commits, batch.remote.as_deref()) == repository
}

/** Delete every stored record for a verified repository, across all installations. */
pub(super) enum ClearRepositoryError {
    NotVerified,
    Database(String),
}

pub(super) fn clear_repository(
    database: &Database,
    repository: &str,
    installation: &str,
    remote: &str,
) -> Result<Value, ClearRepositoryError> {
    let known = database
        .collection::<Document>("repositories")
        .find_one(doc! {
            "repository_id": repository,
            "installation": installation,
            "verified": true,
            "remote": remote,
        })
        .run()
        .map_err(|error| ClearRepositoryError::Database(io_error(error)))?;
    if known.is_none() {
        return Err(ClearRepositoryError::NotVerified);
    }
    let mut deleted = BTreeMap::new();
    let mut total = 0_u64;
    for name in repository_collections() {
        let result = database
            .collection::<Document>(name)
            .delete_many(doc! {"repository_id": repository})
            .run()
            .map_err(|error| {
                ClearRepositoryError::Database(format!(
                    "could not clear {name}: {}",
                    io_error(error)
                ))
            })?;
        total = total.saturating_add(result.deleted_count);
        deleted.insert(name, result.deleted_count);
    }
    Ok(json!({"repository": remote, "deleted_total": total, "deleted_by_collection": deleted}))
}

/** What an installation already uploads
 * Fields
    - repositories: u64 - repositories
    - known: bool - this repository is one of them
*/
pub(super) struct Quota {
    pub(super) repositories: u64,
    pub(super) known: bool,
}

/** Measure an installation's repositories
 * Input
    - database: &Database - database
    - repository: &str - repository identity
    - installation: &str - installation key
 * Output
    - Result<Quota, String>
*/
pub(super) fn quota(
    database: &Database,
    repository: &str,
    installation: &str,
) -> Result<Quota, String> {
    let count = |filter: Document| {
        database
            .collection::<Document>("repositories")
            .count_documents(filter)
            .run()
            .map_err(io_error)
    };
    Ok(Quota {
        repositories: count(doc! {"installation": installation})?,
        known: count(doc! {"installation": installation, "repository_id": repository})? > 0,
    })
}

/** Raise one of a machine's upload positions (never lowers it)
 * Input
    - database: &Database - database
    - scope: &Document - repository and installation filter
    - stream: &str - events, ledger, audit, or alerts
    - field: &str - seq or count
    - value: u64 - the position reached
 * Output
    - Result<(), String>
*/
fn advance(
    database: &Database,
    scope: &Document,
    stream: &str,
    field: &str,
    value: u64,
) -> Result<(), String> {
    let mut filter = scope.clone();
    filter.insert("stream", stream);
    database
        .collection::<Document>("ingestion_checkpoints")
        .update_one(filter, doc! {"$max": {field: value as i64}})
        .upsert(true)
        .run()
        .map(|_| ())
        .map_err(io_error)
}

/** Read one of a machine's upload positions
 * Input
    - database: &Database - database
    - scope: &Document - repository and installation filter
    - stream: &str - events, ledger, audit, or alerts
    - field: &str - seq or count
 * Output
    - Result<u64, String> 0 when nothing arrived yet
*/
fn position(
    database: &Database,
    scope: &Document,
    stream: &str,
    field: &str,
) -> Result<u64, String> {
    let mut filter = scope.clone();
    filter.insert("stream", stream);
    Ok(database
        .collection::<Document>("ingestion_checkpoints")
        .find_one(filter)
        .run()
        .map_err(io_error)?
        .map_or(0, |document| number(document.get(field))))
}

/** Store one batch: its streams move the machine's positions forward, and the last batch of a
 * sync updates the repository and its status
 * Input
    - database: &Database - database
    - repository: &str - repository identity
    - installation: &str - installation key
    - batch: &Batch - records
 * Output
    - Result<Value, String> {"duplicates": 0}
*/
pub(super) fn ingest(
    database: &Database,
    repository: &str,
    installation: &str,
    batch: &Batch,
) -> Result<Value, String> {
    let scope = doc! {"repository_id": repository, "installation": installation};
    if let Some(last) = batch.events.iter().map(|event| event.sequence).max() {
        advance(database, &scope, "events", "seq", last)?;
    }
    if let Some(last) = batch.ledger.iter().map(|entry| entry.seq).max() {
        advance(database, &scope, "ledger", "seq", last)?;
    }
    if let Some(last) = batch.audit.iter().map(|entry| entry.seq).max() {
        advance(database, &scope, "audit", "seq", last)?;
    }
    // A machine sends the alerts after the count it was told, so the new count follows from it
    if !batch.alerts.is_empty() {
        let uploaded = position(database, &scope, "alerts", "count")?;
        advance(
            database,
            &scope,
            "alerts",
            "count",
            uploaded + batch.alerts.len() as u64,
        )?;
    }
    if batch.complete {
        upsert(
            database,
            "repositories",
            scope.clone(),
            document(
                &json!({
                    "remote": batch.remote,
                    "verified": verified(repository, batch),
                    "actor": batch.actor.as_deref().map(|actor| actor.chars().take(100).collect::<String>()),
                    "registry_generation": batch.registry_generation,
                    "updated_at": now_millis(),
                }),
                repository,
                installation,
            )?,
        )?;
    }
    if let Some(status) = batch.status.as_ref().filter(|status| status.is_object()) {
        let mut status = status.clone();
        status["collected_at"] = json!(status["collected_at"].as_u64().unwrap_or_else(now_millis));
        upsert(
            database,
            "repository_status",
            scope.clone(),
            document(&status, repository, installation)?,
        )?;
    }
    let mut uploads = scope.clone();
    uploads.insert("stream", "uploads");
    upsert(
        database,
        "ingestion_checkpoints",
        uploads,
        document(
            &json!({"stream": "uploads", "at": now_millis()}),
            repository,
            installation,
        )?,
    )?;
    Ok(json!({"duplicates": 0}))
}

/** Report how far each of an installation's streams arrived
 * Input
    - database: &Database - database
    - repository: &str - repository identity
    - installation: &str - installation key
 * Output
    - Result<Positions, String>
*/
pub(super) fn positions(
    database: &Database,
    repository: &str,
    installation: &str,
) -> Result<Positions, String> {
    let scope = doc! {"repository_id": repository, "installation": installation};
    Ok(Positions {
        events: position(database, &scope, "events", "seq")?,
        ledger: position(database, &scope, "ledger", "seq")?,
        audit: position(database, &scope, "audit", "seq")?,
        alerts: position(database, &scope, "alerts", "count")?,
    })
}

/** Read a BSON number as u64
 * Input
    - value: Option<&Bson> - number
 * Output
    - u64, 0 when absent or not a number
*/
pub(super) fn number(value: Option<&Bson>) -> u64 {
    match value {
        Some(Bson::Int32(number)) => *number as u64,
        Some(Bson::Int64(number)) => *number as u64,
        Some(Bson::Double(number)) => *number as u64,
        _ => 0,
    }
}

/** Return the service's session key seed, creating it on first start (the only document the
 * service writes for itself; the dashboard reads it at startup to sign session cookies)
 * Input
    - database: &Database - database
 * Output
    - Result<String, String> 64 hex characters
*/
pub(super) fn session_seed(database: &Database) -> Result<String, String> {
    let settings = database.collection::<Document>("service_settings");
    let read = || -> Result<Option<String>, String> {
        Ok(settings
            .find_one(doc! {"name": "session_key"})
            .run()
            .map_err(io_error)?
            .and_then(|document| document.get_str("seed").ok().map(String::from)))
    };
    if let Some(seed) = read()? {
        return Ok(seed);
    }
    let seed = crate::trust::crypto::SigningIdentity::generate()?.seed_hex();
    // Another instance may have created it at the same time; the unique name keeps one
    let _ = settings
        .insert_one(
            doc! {"name": "session_key", "seed": &seed, "created_at": bson::DateTime::now()},
        )
        .run();
    read()?.ok_or_else(|| "the session key could not be stored".to_string())
}

#[cfg(test)]
mod tests {
    use super::verified;
    use crate::governance::workspace::identity;
    use crate::telemetry::ingest::Batch;

    /** Check that a remote is verified only when the root commits and remote reproduce the identity
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn remotes_are_verified_by_identity() {
        let roots = vec!["abc".to_string()];
        let repository = identity(&roots, Some("github.com/acme/widgets"));
        let batch = |remote: &str, roots: Vec<String>| Batch {
            remote: Some(remote.into()),
            root_commits: roots,
            ..Batch::default()
        };
        assert!(verified(
            &repository,
            &batch("github.com/acme/widgets", roots.clone())
        ));
        assert!(!verified(
            &repository,
            &batch("github.com/evil/widgets", roots.clone())
        ));
        assert!(!verified(
            &repository,
            &batch("github.com/acme/widgets", vec!["other".into()])
        ));
        assert!(!verified(
            &repository,
            &batch("github.com/acme/widgets", Vec::new())
        ));
    }
}
