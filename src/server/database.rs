// The ingestion service's MongoDB database. Migrations are versioned and idempotent: collections
// get $jsonSchema validators and unique indexes enforce idempotency per installation. The service
// keeps only what DataSea's Flows section and Crane machines need: repositories with their verified
// remotes, each member's latest repository status, upload positions, approval requests (machine
// links and owner keys), Brick change requests, and the org catalog of Bricks. Telemetry (events,
// sessions, tasks, agents, alerts, the autonomy ledger, the governance audit trail, the selection
// registry, policy versions, and service events) is accepted from machines for compatibility but no
// longer stored; schema 14 removed it, together with the dashboard's metric, Dashboard Agent,
// credit grant, and resident decision records.

use std::time::Duration;

use mongodb::bson::{self, doc, Bson, Document};
use mongodb::options::{ClientOptions, IndexOptions};
use mongodb::sync::{Client, Database};
use mongodb::IndexModel;

use crate::platform::io_error;

/** Current schema version applied by the migrations (3: records keyed by installation; 4: the
 * members' repository status and the service's settings; 10: Dashboard Agent enforcement and
 * approval-gated flow context; 11: resident-agent decisions made on the dashboard; 12: Brick change
 * requests made on the dashboard; 14: telemetry and the dashboard's metric records removed, upload
 * positions kept as checkpoints) */
pub(crate) const SCHEMA_VERSION: i64 = 14;

/** How the service reaches its database
 * Fields
    - uri: String - connection string (secret)
    - database: String - database name
*/
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub(crate) uri: String,
    pub(crate) database: String,
}

impl Settings {
    /** Read the settings from the service's environment
     * Input
        - None
     * Output
        - Result<Settings, String>
        - Error if CRANE_MONGODB_URI is not set
    */
    pub(crate) fn from_environment() -> Result<Self, String> {
        let uri = std::env::var("CRANE_MONGODB_URI")
            .ok()
            .filter(|uri| !uri.is_empty())
            .ok_or("set CRANE_MONGODB_URI in the service's environment")?;
        Ok(Self {
            uri,
            database: std::env::var("CRANE_MONGODB_DB").unwrap_or_else(|_| "foxx".into()),
        })
    }
}

/** Open and ping the database
 * Input
    - settings: &Settings - connection settings
 * Output
    - Result<Database, String>
    - Error if the URI is invalid or the server cannot be reached within the timeout
*/
pub(crate) fn connect(settings: &Settings) -> Result<Database, String> {
    let mut options = ClientOptions::parse(&settings.uri).run().map_err(|error| {
        format!(
            "invalid MongoDB connection string: {}",
            redact_uri(&error.to_string())
        )
    })?;
    options.app_name = Some("crane-server".into());
    options.server_selection_timeout = Some(Duration::from_secs(8));
    options.connect_timeout = Some(Duration::from_secs(8));
    options.max_pool_size = Some(16);
    options.retry_writes = Some(true);
    let client = Client::with_options(options).map_err(|error| redact_uri(&error.to_string()))?;
    let database = client.database(&settings.database);
    ping(&database).map_err(|error| format!("MongoDB is unreachable: {}", redact_uri(&error)))?;
    Ok(database)
}

/** Check that the database answers
 * Input
    - database: &Database - database
 * Output
    - Result<(), String>
*/
pub(crate) fn ping(database: &Database) -> Result<(), String> {
    database
        .run_command(doc! {"ping": 1})
        .run()
        .map(|_| ())
        .map_err(io_error)
}

/** Remove credentials from a connection-related message
 * Input
    - text: &str - message
 * Output
    - String
*/
pub(crate) fn redact_uri(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::telemetry::redact::redact(&crate::telemetry::redact::strip_url_credentials(&text)).0
}

/** A collection, its validator, and its indexes
 * Fields
    - name: &'static str - collection name
    - required: &'static [&'static str] - fields every document must have
    - indexes: &'static [(&'static [(&'static str, i32)], bool)] - index keys and uniqueness
*/
struct Collection {
    name: &'static str,
    required: &'static [&'static str],
    indexes: &'static [(&'static [(&'static str, i32)], bool)],
}

/** The collections the migrations create */
const COLLECTIONS: &[Collection] = &[
    Collection {
        name: "repositories",
        required: &["repository_id", "installation"],
        indexes: &[(&[("repository_id", 1), ("installation", 1)], true)],
    },
    // How far each of a machine's streams arrived (events, ledger, audit, alerts) and its last
    // upload: machines resume from these positions, so nothing is uploaded twice
    Collection {
        name: "ingestion_checkpoints",
        required: &["repository_id", "installation", "stream"],
        indexes: &[(
            &[("repository_id", 1), ("installation", 1), ("stream", 1)],
            true,
        )],
    },
    Collection {
        name: "repository_status",
        required: &["repository_id", "installation", "collected_at"],
        indexes: &[(&[("repository_id", 1), ("installation", 1)], true)],
    },
    Collection {
        name: "member_links",
        required: &["repository_id", "installation", "code_digest", "expires_at"],
        indexes: &[
            (&[("code_digest", 1)], true),
            (&[("repository_id", 1), ("installation", 1)], false),
        ],
    },
    Collection {
        name: "owner_keys",
        required: &["repository_id", "owner_key", "code_digest", "expires_at"],
        indexes: &[
            (&[("code_digest", 1)], true),
            (&[("repository_id", 1), ("confirmed_at", -1)], false),
        ],
    },
    Collection {
        name: "brick_requests",
        required: &[
            "request_id",
            "repository_id",
            "op",
            "requested_by",
            "status",
        ],
        indexes: &[
            (&[("request_id", 1)], true),
            (&[("repository_id", 1), ("status", 1)], false),
            (&[("repository_id", 1), ("requested_at", -1)], false),
        ],
    },
    // The org catalog of Bricks: one immutable entry per account, Brick id, and version
    Collection {
        name: "brick_catalog",
        required: &["account", "brick_id", "version", "digest", "entry"],
        indexes: &[
            (&[("account", 1), ("brick_id", 1), ("version", 1)], true),
            (&[("account", 1), ("published_at", -1)], false),
        ],
    },
    Collection {
        name: "service_settings",
        required: &["name"],
        indexes: &[(&[("name", 1)], true)],
    },
    Collection {
        name: "schema_migrations",
        required: &["version", "applied_at"],
        indexes: &[(&[("version", 1)], true)],
    },
];

/** Collections earlier versions created that the service no longer keeps (schema 14 drops them
 * once their upload positions are carried into ingestion_checkpoints) */
pub(crate) const RETIRED: &[&str] = &[
    "events",
    "sessions",
    "tasks",
    "agents",
    "alerts",
    "autonomy_ledger",
    "governance_changes",
    "selection_registry",
    "policy_versions",
    "chain_heads",
    "service_events",
    "metric_rollups",
    "credit_grants",
    "resident_decisions",
    "dashboard_agents",
    "dashboard_incidents",
    "dashboard_quarantines",
    "dashboard_flow_context",
    "dashboard_agent_activity",
    "dashboard_agent_leases",
];

/** The repository-scoped collections; global service settings and migration state are excluded. */
pub(super) fn repository_collections() -> impl Iterator<Item = &'static str> {
    COLLECTIONS
        .iter()
        .filter(|collection| !matches!(collection.name, "service_settings" | "schema_migrations"))
        .map(|collection| collection.name)
}

/** Raise one upload position of every machine found by a pipeline that yields
 * {_id: {repository_id, installation}, value}
 * Input
    - database: &Database - database
    - source: &str - collection the pipeline reads
    - pipeline: Vec<Document> - grouping stages
    - stream: &str - checkpoint stream (events, ledger, audit, alerts)
    - field: &str - checkpoint field (seq or count)
 * Output
    - Result<(), String>
*/
fn carry_position(
    database: &Database,
    source: &str,
    pipeline: Vec<Document>,
    stream: &str,
    field: &str,
) -> Result<(), String> {
    let checkpoints = database.collection::<Document>("ingestion_checkpoints");
    let found = database
        .collection::<Document>(source)
        .aggregate(pipeline)
        .run()
        .map_err(io_error)?;
    for group in found {
        let group = group.map_err(io_error)?;
        let Ok(key) = group.get_document("_id") else {
            continue;
        };
        let (Ok(repository), Ok(installation)) =
            (key.get_str("repository_id"), key.get_str("installation"))
        else {
            continue;
        };
        let value = match group.get("value") {
            Some(Bson::Int32(number)) => *number as i64,
            Some(Bson::Int64(number)) => *number,
            Some(Bson::Double(number)) => *number as i64,
            _ => continue,
        };
        checkpoints
            .update_one(
                doc! {"repository_id": repository, "installation": installation, "stream": stream},
                doc! {"$max": {field: value}},
            )
            .upsert(true)
            .run()
            .map_err(io_error)?;
    }
    Ok(())
}

/** Carry every machine's upload positions out of the telemetry collections schema 14 drops, so no
 * machine uploads its history again: the last event (chain head, events that arrived already
 * expired, and stored events), the last ledger entry and governance change, and the alerts count
 * Input
    - database: &Database - database
    - existing: &[String] - collections present before the migration
 * Output
    - Result<(), String>
*/
fn carry_positions(database: &Database, existing: &[String]) -> Result<(), String> {
    let present = |name: &str| existing.iter().any(|found| found == name);
    let highest = |field: &str| {
        vec![doc! {"$group": {
            "_id": {"repository_id": "$repository_id", "installation": "$installation"},
            "value": {"$max": format!("${field}")},
        }}]
    };
    if present("chain_heads") {
        carry_position(
            database,
            "chain_heads",
            highest("sequence"),
            "events",
            "seq",
        )?;
        carry_position(
            database,
            "chain_heads",
            highest("expired_through"),
            "events",
            "seq",
        )?;
    }
    if present("events") {
        carry_position(database, "events", highest("sequence"), "events", "seq")?;
    }
    if present("autonomy_ledger") {
        carry_position(database, "autonomy_ledger", highest("seq"), "ledger", "seq")?;
    }
    if present("governance_changes") {
        carry_position(
            database,
            "governance_changes",
            highest("seq"),
            "audit",
            "seq",
        )?;
    }
    if present("alerts") {
        // Uploaded alerts only (the service raised its own chain alerts with source "service")
        carry_position(
            database,
            "alerts",
            vec![
                doc! {"$match": {"source": {"$ne": "service"}}},
                doc! {"$group": {
                    "_id": {"repository_id": "$repository_id", "installation": "$installation"},
                    "value": {"$sum": 1},
                }},
            ],
            "alerts",
            "count",
        )?;
    }
    Ok(())
}

/** Apply the migrations that have not been applied: create missing collections with validators,
 * update the validators of existing ones, replace their indexes with the current ones, carry the
 * upload positions forward, drop the retired collections, and record the version
 * Input
    - database: &Database - database
 * Output
    - Result<i64, String> the schema version now applied
*/
pub(crate) fn migrate(database: &Database) -> Result<i64, String> {
    let applied = database
        .collection::<Document>("schema_migrations")
        .find_one(doc! {"version": SCHEMA_VERSION})
        .run()
        .map_err(io_error)?;
    if applied.is_some() {
        return Ok(SCHEMA_VERSION);
    }
    let existing = database.list_collection_names().run().map_err(io_error)?;
    for collection in COLLECTIONS {
        let required = collection
            .required
            .iter()
            .map(|field| Bson::String(field.to_string()))
            .collect::<Vec<_>>();
        let validator = doc! {"$jsonSchema": {"bsonType": "object", "required": required}};
        if existing.iter().any(|name| name == collection.name) {
            database
                .run_command(doc! {"collMod": collection.name, "validator": validator})
                .run()
                .map_err(io_error)?;
            // Indexes of earlier versions identify records without the installation
            database
                .collection::<Document>(collection.name)
                .drop_indexes()
                .run()
                .map_err(io_error)?;
        } else {
            database
                .create_collection(collection.name)
                .validator(validator)
                .run()
                .map_err(io_error)?;
        }
        let models = collection
            .indexes
            .iter()
            .map(|(keys, unique)| {
                let mut document = Document::new();
                for (key, order) in keys.iter() {
                    document.insert(*key, *order);
                }
                IndexModel::builder()
                    .keys(document)
                    .options(IndexOptions::builder().unique(*unique).build())
                    .build()
            })
            .collect::<Vec<_>>();
        database
            .collection::<Document>(collection.name)
            .create_indexes(models)
            .run()
            .map_err(io_error)?;
    }
    // The positions are carried before anything is dropped; a failure stops the migration here and
    // it runs again at the next start (raising a position is idempotent)
    carry_positions(database, &existing)?;
    for name in RETIRED {
        if existing.iter().any(|found| found == name) {
            database
                .collection::<Document>(name)
                .drop()
                .run()
                .map_err(io_error)?;
        }
    }
    // Telemetry no longer expires because none is kept
    database
        .collection::<Document>("service_settings")
        .delete_one(doc! {"name": "retention"})
        .run()
        .map_err(io_error)?;
    // Brick requests leave once purge_at passes (an unconfirmed one within the hour)
    database
        .collection::<Document>("brick_requests")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"purge_at": 1})
                .options(
                    IndexOptions::builder()
                        .expire_after(Duration::from_secs(0))
                        .build(),
                )
                .build(),
        )
        .run()
        .map_err(io_error)?;
    database
        .collection::<Document>("schema_migrations")
        .insert_one(
            doc! {"version": SCHEMA_VERSION, "applied_at": bson::DateTime::now(), "by": "crane server"},
        )
        .run()
        .map_err(io_error)?;
    Ok(SCHEMA_VERSION)
}

#[cfg(test)]
mod tests {
    use super::redact_uri;

    /** Check that connection errors never reveal credentials
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn redacts_credentials_in_errors() {
        let text = redact_uri("failed mongodb+srv://crane:hunter22@cluster0.example.net/foxx");
        assert!(!text.contains("hunter22"), "{text}");
        assert!(text.contains("cluster0.example.net"));
    }
}
