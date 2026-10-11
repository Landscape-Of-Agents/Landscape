// Storing and reading an installation's records. Streams (events, ledger entries, governance
// changes, alerts) are inserted unordered with duplicates skipped, so a retried upload never
// duplicates evidence; current state (repository, sessions, agents, tasks, selection records, the
// signed manifest, metric rollups) is upserted. Every document carries the repository identity and
// the installation key, and every filter includes the installation.

use mongodb::bson::{self, doc, Bson, Document};
use mongodb::options::InsertManyOptions;
use mongodb::sync::Database;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use super::database::Settings;
use crate::platform::{io_error, now_millis};
use crate::telemetry::events::Event;
use crate::telemetry::identity::{SessionRecord, TaskRecord};
use crate::telemetry::ingest::{Batch, Positions, Remote};
use crate::telemetry::ledger::Entry;
use crate::telemetry::model::Severity;

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
    let events = batch
        .events
        .iter()
        .map(|event| {
            let mut document = tag!(event)?;
            document.insert("ingested_at", bson::DateTime::now());
            if event.severity <= Severity::Medium {
                let expiry = event.occurred_at as i64 + settings.retention_days * 86_400_000;
                document.insert("expires_at", bson::DateTime::from_millis(expiry));
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
                .map(|alert| tag!(alert))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    ] {
        duplicates += insert(database, name, documents)?;
    }
    let scope = doc! {"repository_id": repository, "installation": installation};
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
            tag!(
                &json!({"remote": batch.remote, "registry_generation": batch.registry_generation, "updated_at": now_millis()})
            )?,
        )?;
        for session in &batch.sessions {
            upsert(
                database,
                "sessions",
                doc! {"installation": installation, "foxx_session_id": &session.foxx_session_id},
                tag!(session)?,
            )?;
            upsert(
                database,
                "agents",
                doc! {"installation": installation, "agent_id": &session.agent_id},
                tag!(
                    &json!({"agent_id": session.agent_id, "provider": session.provider, "last_seen_at": session.last_seen_at})
                )?,
            )?;
        }
        for task in &batch.tasks {
            upsert(
                database,
                "tasks",
                doc! {"installation": installation, "foxx_task_id": &task.foxx_task_id},
                tag!(task)?,
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
        for rollup in &batch.rollups {
            upsert(
                database,
                "metric_rollups",
                within(doc! {"scope": &rollup.scope, "id": &rollup.id}),
                tag!(rollup)?,
            )?;
        }
    }
    upsert(
        database,
        "ingestion_checkpoints",
        within(doc! {"stream": "uploads"}),
        tag!(&json!({"stream": "uploads", "at": now_millis()}))?,
    )?;
    Ok(json!({"duplicates": duplicates}))
}

/** Read a BSON number as u64
 * Input
    - value: Option<&Bson> - number
 * Output
    - u64, 0 when absent or not a number
*/
fn number(value: Option<&Bson>) -> u64 {
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
    Ok(Positions {
        events: highest(database, "events", &scope, "sequence")?,
        ledger: highest(database, "autonomy_ledger", &scope, "seq")?,
        audit: highest(database, "governance_changes", &scope, "seq")?,
        alerts: database
            .collection::<Document>("alerts")
            .count_documents(scope)
            .run()
            .map_err(io_error)?,
    })
}

/** Read and decode one collection of an installation
 * Input
    - database: &Database - database
    - collection: &str - collection
    - scope: &Document - repository and installation filter
    - sort: Document - sort order
 * Output
    - Result<Vec<T>, String>
*/
fn read<T: DeserializeOwned>(
    database: &Database,
    collection: &str,
    scope: &Document,
    sort: Document,
) -> Result<Vec<T>, String> {
    database
        .collection::<Document>(collection)
        .find(scope.clone())
        .sort(sort)
        .projection(doc! {"_id": 0, "ingested_at": 0, "expires_at": 0, "installation": 0})
        .run()
        .map_err(io_error)?
        .map(|document| {
            bson::from_document(document.map_err(io_error)?)
                .map_err(|error| format!("{collection}: {error}"))
        })
        .collect()
}

/** Read an installation's data for its dashboard
 * Input
    - database: &Database - database
    - repository: &str - repository identity
    - installation: &str - installation key
 * Output
    - Result<Value, String> a telemetry::ingest::Remote as JSON
*/
pub(super) fn snapshot(
    database: &Database,
    repository: &str,
    installation: &str,
) -> Result<Value, String> {
    let scope = doc! {"repository_id": repository, "installation": installation};
    let alerts: Vec<Document> = read(database, "alerts", &scope, doc! {"at": -1})?;
    let events: Vec<Event> = read(database, "events", &scope, doc! {"sequence": 1})?;
    let ledger: Vec<Entry> = read(database, "autonomy_ledger", &scope, doc! {"seq": 1})?;
    let sessions: Vec<SessionRecord> =
        read(database, "sessions", &scope, doc! {"last_seen_at": -1})?;
    let tasks: Vec<TaskRecord> = read(database, "tasks", &scope, doc! {"started_at": -1})?;
    let remote = Remote {
        events,
        ledger,
        sessions,
        tasks,
        alerts: alerts
            .into_iter()
            .map(|mut alert| {
                alert.remove("repository_id");
                serde_json::to_value(alert).map_err(io_error)
            })
            .collect::<Result<_, _>>()?,
    };
    serde_json::to_value(remote).map_err(io_error)
}
