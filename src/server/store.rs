// Storing and reading an installation's records. Streams (events, ledger entries, governance
// changes, alerts) are inserted unordered with duplicates skipped, so a retried upload never
// duplicates evidence; current state (repository, sessions, agents, tasks, selection records, the
// signed manifest) is upserted. Every document carries the repository identity and
// the installation key, and every filter includes the installation. Nothing here serves reads to
// machines: the dashboard reads through its own read-only handle (dashboard/data.rs).

use mongodb::bson::{self, doc, Bson, Document};
use mongodb::options::InsertManyOptions;
use mongodb::sync::Database;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

use super::database::{expiry, repository_collections, Settings};
use crate::governance::workspace::identity;
use crate::platform::{io_error, now_millis};
use crate::telemetry::events::Event;
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

/** Insert documents unordered, counting duplicate-key rejections as already stored
 * Input
    - database: &Database - database
    - name: &str - collection
    - documents: Vec<Document> - documents
 * Output
    - Result<usize, String> duplicates skipped
*/
fn insert(database: &Database, name: &str, documents: Vec<Document>) -> Result<usize, String> {
    if documents.is_empty() {
        return Ok(0);
    }
    let result = database
        .collection::<Document>(name)
        .insert_many(documents)
        .with_options(InsertManyOptions::builder().ordered(false).build())
        .run();
    match result {
        Ok(_) => Ok(0),
        Err(error) => match *error.kind {
            mongodb::error::ErrorKind::InsertMany(ref failure) => {
                let write_errors = failure.write_errors.clone().unwrap_or_default();
                if write_errors.iter().any(|error| error.code != 11000)
                    || failure.write_concern_error.is_some()
                {
                    return Err(error.to_string());
                }
                Ok(write_errors.len())
            }
            _ => Err(error.to_string()),
        },
    }
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

/** Check that uploaded events continue each machine's chain where the service last saw it, and
 * record a critical alert when they do not: an event that does not chain from the previous one
 * (history rewritten), a gap in the sequence (events removed), or a different event at a sequence
 * already stored (a fork). The head is kept in chain_heads, which never expires, so the check
 * survives the expiry of routine events. Events are still stored, so the dashboard shows the
 * broken chain too
 * Input
    - database: &Database - database
    - repository: &str - repository identity
    - installation: &str - installation key
    - events: &[Event] - uploaded events
    - retention_ms: Option<i64> - how long the alerts it raises are kept
 * Output
    - Result<Vec<Value>, String> the problems found: {sequence, kind (gap, rewrite, fork), detail}
*/
fn continuity(
    database: &Database,
    repository: &str,
    installation: &str,
    events: &[Event],
    retention_ms: Option<i64>,
) -> Result<Vec<Value>, String> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let heads = database.collection::<Document>("chain_heads");
    let scope = doc! {"repository_id": repository, "installation": installation};
    let stored = heads.find_one(scope.clone()).run().map_err(io_error)?;
    let mut head = stored.as_ref().and_then(|document| {
        Some((
            number(document.get("sequence")),
            document.get_str("digest").ok()?.to_string(),
        ))
    });
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|event| event.sequence);
    let mut problems: Vec<(u64, &str, String)> = Vec::new();
    for event in ordered {
        match &head {
            // The first event the service sees from this machine starts the chain
            None => head = Some((event.sequence, event.digest.clone())),
            Some((sequence, _)) if event.sequence <= *sequence => {
                let mut filter = scope.clone();
                filter.insert("sequence", event.sequence as i64);
                let earlier = database
                    .collection::<Document>("events")
                    .find_one(filter)
                    .run()
                    .map_err(io_error)?;
                if let Some(earlier) = earlier {
                    if earlier.get_str("digest").ok() != Some(event.digest.as_str()) {
                        problems.push((event.sequence, "fork", format!("event {} differs from the event {} the service already holds (the machine's history was replaced)", event.sequence, event.sequence)));
                    }
                }
            }
            Some((sequence, digest)) => {
                if event.sequence > sequence + 1 {
                    problems.push((event.sequence, "gap", format!("events {} to {} never arrived (removed from the machine before upload)", sequence + 1, event.sequence - 1)));
                } else if event.previous_digest != *digest {
                    problems.push((event.sequence, "rewrite", format!("event {} does not chain from event {} (the machine's history was rewritten)", event.sequence, sequence)));
                }
                head = Some((event.sequence, event.digest.clone()));
            }
        }
    }
    if let Some((sequence, digest)) = &head {
        let mut document = scope.clone();
        document.insert("sequence", *sequence as i64);
        document.insert("digest", digest.as_str());
        document.insert("updated_at", now_millis() as i64);
        heads
            .replace_one(scope.clone(), document)
            .upsert(true)
            .run()
            .map_err(io_error)?;
    }
    let now = now_millis() / 1000;
    let found = problems
        .iter()
        .map(|(sequence, kind, detail)| json!({"sequence": sequence, "kind": kind, "detail": detail}))
        .collect::<Vec<_>>();
    let alerts = problems
        .into_iter()
        .map(|(sequence, kind, detail)| {
            let mut alert = doc! {
                "alert_id": format!("srv_chain_{kind}_{sequence}"),
                "key": format!("chain:{kind}:{sequence}"),
                "kind": "evidence_chain_broken",
                "title": "Evidence chain broken: a machine's history was rewritten or removed",
                "severity": "CRITICAL",
                "at": now as i64,
                "state": "OPEN",
                "source": "service",
                "suppressed": false,
                "deliveries": Bson::Array(Vec::new()),
                "detail": detail,
                "repository_id": repository,
                "installation": installation,
            };
            if let Some(expiry) = expiry(retention_ms, now as i64 * 1000) {
                alert.insert("expires_at", expiry);
            }
            alert
        })
        .collect::<Vec<_>>();
    insert(database, "alerts", alerts)?;
    Ok(found)
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

/** The credit grants approved on the dashboard for one machine and not yet applied (only that
 * machine's own; nothing else is ever read back)
 * Input
    - database: &Database - MongoDB
    - repository: &str - repository identity
    - installation: &str - the machine
 * Output
    - Result<Value, String> {grants: [{grant_id, session, amount, reason, approved_by, approved_at}]}
*/
pub(super) fn pending_grants(
    database: &Database,
    repository: &str,
    installation: &str,
) -> Result<Value, String> {
    let found = database
        .collection::<Document>("credit_grants")
        .find(doc! {"repository_id": repository, "installation": installation, "status": "PENDING", "expires_at": {"$gt": now_millis() as i64}})
        .sort(doc! {"approved_at": 1})
        .limit(50)
        .run()
        .map_err(io_error)?;
    let mut grants = Vec::new();
    for document in found {
        let document = document.map_err(io_error)?;
        grants.push(json!({
            "grant_id": document.get_str("grant_id").unwrap_or_default(),
            "session": document.get_str("session").unwrap_or_default(),
            "amount": number(document.get("amount")),
            "reason": document.get_str("reason").unwrap_or_default(),
            "approved_by": document.get_str("approved_by").unwrap_or_default(),
            "approved_at": number(document.get("approved_at")),
        }));
    }
    Ok(json!({"grants": grants}))
}

/** Record what a machine did with its grants: applied to its ledger (with the entry) or rejected
 * (with why); only the machine's own pending grants change
 * Input
    - database: &Database - MongoDB
    - repository: &str - repository identity
    - installation: &str - the machine
    - body: &Value - {results: [{grant_id, status: APPLIED|REJECTED, entry_id, detail}]}
 * Output
    - Result<Value, String> {settled}
*/
pub(super) fn settle_grants(
    database: &Database,
    repository: &str,
    installation: &str,
    body: &Value,
) -> Result<Value, String> {
    let mut settled = 0;
    for result in body["results"].as_array().into_iter().flatten().take(50) {
        let status = match result["status"].as_str() {
            Some("APPLIED") => "APPLIED",
            _ => "REJECTED",
        };
        let detail = result["detail"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect::<String>();
        let entry = result["entry_id"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(64)
            .collect::<String>();
        let changed = database
            .collection::<Document>("credit_grants")
            .update_one(
                doc! {"grant_id": result["grant_id"].as_str().unwrap_or_default(), "repository_id": repository, "installation": installation, "status": "PENDING"},
                doc! {"$set": {"status": status, "settled_at": now_millis() as i64, "ledger_entry": entry, "detail": detail}},
            )
            .run()
            .map_err(io_error)?;
        settled += changed.modified_count;
    }
    Ok(json!({"settled": settled}))
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

/** What an installation already stores
 * Fields
    - events: u64 - events across every repository
    - repositories: u64 - repositories
    - known: bool - this repository is one of them
*/
pub(super) struct Quota {
    pub(super) events: u64,
    pub(super) repositories: u64,
    pub(super) known: bool,
}

/** Measure an installation's stored data
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
    let count = |collection: &str, filter: Document| {
        database
            .collection::<Document>(collection)
            .count_documents(filter)
            .run()
            .map_err(io_error)
    };
    Ok(Quota {
        events: count("events", doc! {"installation": installation})?,
        repositories: count("repositories", doc! {"installation": installation})?,
        known: count(
            "repositories",
            doc! {"installation": installation, "repository_id": repository},
        )? > 0,
    })
}

/** Store one batch
 * Input
    - database: &Database - database
    - settings: &Settings - service settings
    - repository: &str - repository identity
    - installation: &str - installation key
    - batch: &Batch - records
 * Output
    - Result<Value, String> {"duplicates": N}
*/
pub(super) fn ingest(
    database: &Database,
    settings: &Settings,
    repository: &str,
    installation: &str,
    batch: &Batch,
) -> Result<Value, String> {
    macro_rules! tag {
        ($value:expr) => {
            document($value, repository, installation)
        };
    }
    let mut duplicates = 0;
    let chain_problems = continuity(
        database,
        repository,
        installation,
        &batch.events,
        settings.retention_ms,
    )?;
    // Events older than the retention period (re-sent after they expired, or uploaded late) are
    // not stored again; the last of them is remembered so the machine moves past them
    let now = now_millis() as i64;
    let (current, expired): (Vec<&Event>, Vec<&Event>) = batch.events.iter().partition(|event| {
        expiry(settings.retention_ms, event.occurred_at as i64)
            .is_none_or(|expiry| expiry.timestamp_millis() > now)
    });
    if let Some(last) = expired.iter().map(|event| event.sequence).max() {
        database
            .collection::<Document>("chain_heads")
            .update_one(
                doc! {"repository_id": repository, "installation": installation},
                doc! {"$max": {"expired_through": last as i64}},
            )
            .upsert(true)
            .run()
            .map_err(io_error)?;
    }
    let events = current
        .into_iter()
        .map(|event| {
            let mut document = tag!(event)?;
            document.insert("ingested_at", bson::DateTime::now());
            if let Some(expiry) = expiry(settings.retention_ms, event.occurred_at as i64) {
                document.insert("expires_at", expiry);
            }
            Ok(document)
        })
        .collect::<Result<Vec<_>, String>>()?;
    duplicates += insert(database, "events", events)?;
    for (name, documents) in [
        (
            "autonomy_ledger",
            batch
                .ledger
                .iter()
                .map(|entry| tag!(entry))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        (
            "governance_changes",
            batch
                .audit
                .iter()
                .map(|entry| tag!(entry))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        (
            "alerts",
            batch
                .alerts
                .iter()
                .map(|alert| {
                    let mut document = tag!(alert)?;
                    let at = alert["at"]
                        .as_i64()
                        .unwrap_or_else(|| now_millis() as i64 / 1000);
                    if let Some(expiry) = expiry(settings.retention_ms, at * 1000) {
                        document.insert("expires_at", expiry);
                    }
                    Ok(document)
                })
                .collect::<Result<Vec<_>, String>>()?,
        ),
    ] {
        duplicates += insert(database, name, documents)?;
    }
    let scope = doc! {"repository_id": repository, "installation": installation};
    // How many alerts this machine has uploaded, kept apart from the alerts themselves, which
    // expire (the machine resumes its uploads from it)
    if !batch.alerts.is_empty() {
        let stored = database
            .collection::<Document>("alerts")
            .count_documents(scope.clone())
            .run()
            .map_err(io_error)?;
        database
            .collection::<Document>("ingestion_checkpoints")
            .update_one(
                doc! {"repository_id": repository, "installation": installation, "stream": "alerts"},
                doc! {"$max": {"count": stored as i64}},
            )
            .upsert(true)
            .run()
            .map_err(io_error)?;
    }
    let living = |mut document: Document, from: u64| {
        if let Some(expiry) = expiry(settings.retention_ms, from as i64) {
            document.insert("expires_at", expiry);
        }
        document
    };
    let within = |extra: Document| {
        let mut filter = scope.clone();
        filter.extend(extra);
        filter
    };
    if batch.complete {
        upsert(
            database,
            "repositories",
            scope.clone(),
            tag!(&json!({
                "remote": batch.remote,
                "verified": verified(repository, batch),
                "actor": batch.actor.as_deref().map(|actor| actor.chars().take(100).collect::<String>()),
                "registry_generation": batch.registry_generation,
                "updated_at": now_millis(),
            }))?,
        )?;
        for session in &batch.sessions {
            upsert(
                database,
                "sessions",
                doc! {"installation": installation, "foxx_session_id": &session.foxx_session_id},
                living(tag!(session)?, session.last_seen_at),
            )?;
            upsert(
                database,
                "agents",
                doc! {"installation": installation, "agent_id": &session.agent_id},
                living(
                    tag!(
                        &json!({"agent_id": session.agent_id, "provider": session.provider, "last_seen_at": session.last_seen_at})
                    )?,
                    session.last_seen_at,
                ),
            )?;
        }
        for task in &batch.tasks {
            upsert(
                database,
                "tasks",
                doc! {"installation": installation, "foxx_task_id": &task.foxx_task_id},
                living(
                    tag!(task)?,
                    task.ended_at
                        .unwrap_or(task.started_at)
                        .max(task.started_at),
                ),
            )?;
        }
        for record in &batch.selections {
            upsert(
                database,
                "selection_registry",
                within(doc! {"id": &record.id}),
                tag!(record)?,
            )?;
        }
        if let Some(manifest) = &batch.manifest {
            upsert(
                database,
                "policy_versions",
                within(doc! {"generation": manifest.generation as i64}),
                tag!(manifest)?,
            )?;
        }
    }
    if let Some(status) = batch.status.as_ref().filter(|status| status.is_object()) {
        let mut status = status.clone();
        status["collected_at"] = json!(status["collected_at"].as_u64().unwrap_or_else(now_millis));
        upsert(database, "repository_status", scope.clone(), tag!(&status)?)?;
    }
    upsert(
        database,
        "ingestion_checkpoints",
        within(doc! {"stream": "uploads"}),
        tag!(&json!({"stream": "uploads", "at": now_millis()}))?,
    )?;
    Ok(json!({"duplicates": duplicates, "chain_problems": chain_problems}))
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

/** Return the highest value of a field among an installation's records
 * Input
    - database: &Database - database
    - collection: &str - collection
    - scope: &Document - repository and installation filter
    - field: &str - numeric field
 * Output
    - Result<u64, String> highest value, 0 when empty
*/
fn highest(
    database: &Database,
    collection: &str,
    scope: &Document,
    field: &str,
) -> Result<u64, String> {
    let found = database
        .collection::<Document>(collection)
        .find_one(scope.clone())
        .sort(doc! {field: -1})
        .run()
        .map_err(io_error)?;
    Ok(found.map_or(0, |document| number(document.get(field))))
}

/** Report how far each of an installation's streams is stored
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
    // Events and alerts expire, so their positions also come from records that never do: the
    // last event that arrived already expired (ingest drops it), and the count of alerts uploaded.
    // Otherwise a machine would upload expired records again; recent events lost are still re-sent
    let expired = database
        .collection::<Document>("chain_heads")
        .find_one(scope.clone())
        .run()
        .map_err(io_error)?
        .map_or(0, |document| number(document.get("expired_through")));
    let mut checkpoint = scope.clone();
    checkpoint.insert("stream", "alerts");
    let uploaded = database
        .collection::<Document>("ingestion_checkpoints")
        .find_one(checkpoint)
        .run()
        .map_err(io_error)?
        .map_or(0, |document| number(document.get("count")));
    Ok(Positions {
        events: highest(database, "events", &scope, "sequence")?.max(expired),
        ledger: highest(database, "autonomy_ledger", &scope, "seq")?,
        audit: highest(database, "governance_changes", &scope, "seq")?,
        alerts: database
            .collection::<Document>("alerts")
            .count_documents(scope)
            .run()
            .map_err(io_error)?
            .max(uploaded),
    })
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
