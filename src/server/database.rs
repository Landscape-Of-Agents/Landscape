// The ingestion service's MongoDB database. Migrations are versioned and idempotent: collections
// get $jsonSchema validators, unique indexes enforce idempotency per installation, filter indexes
// serve the dashboard, and a TTL index expires only routine telemetry (events at or below MEDIUM
// severity); security evidence, the ledger, alerts, and governance history are never expired.
// MongoDB is append-only through this service, not immutable by itself, which is why every stored
// record keeps its SHA-512 chain digest for independent verification.

use std::time::Duration;

use mongodb::bson::{self, doc, Bson, Document};
use mongodb::options::{ClientOptions, IndexOptions};
use mongodb::sync::{Client, Database};
use mongodb::IndexModel;

use crate::platform::io_error;

/** Current schema version applied by the migrations (3: records keyed by installation; 4: the
 * members' repository status and the service's settings) */
pub(crate) const SCHEMA_VERSION: i64 = 4;

/** How the service reaches its database
 * Fields
    - uri: String - connection string (secret)
    - database: String - database name
    - retention_days: i64 - lifetime of routine telemetry events
*/
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub(crate) uri: String,
    pub(crate) database: String,
    pub(crate) retention_days: i64,
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
            retention_days: std::env::var("CRANE_MONGODB_EVENT_RETENTION_DAYS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(30),
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
    crate::telemetry::redact::redact(
        &text
            .split_whitespace()
            .map(|word| match (word.find("://"), word.rfind('@')) {
                (Some(scheme), Some(at)) if at > scheme => {
                    format!("{}://[REDACTED]@{}", &word[..scheme], &word[at + 1..])
                }
                _ => word.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
    .0
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
    Collection {
        name: "agents",
        required: &["agent_id", "repository_id", "installation"],
        indexes: &[(&[("installation", 1), ("agent_id", 1)], true)],
    },
    Collection {
        name: "sessions",
        required: &["foxx_session_id", "repository_id", "installation"],
        indexes: &[
            (&[("installation", 1), ("foxx_session_id", 1)], true),
            (&[("repository_id", 1), ("last_seen_at", -1)], false),
            (&[("agent_id", 1), ("started_at", -1)], false),
        ],
    },
    Collection {
        name: "tasks",
        required: &[
            "foxx_task_id",
            "foxx_session_id",
            "repository_id",
            "installation",
        ],
        indexes: &[
            (&[("installation", 1), ("foxx_task_id", 1)], true),
            (&[("foxx_session_id", 1), ("started_at", -1)], false),
            (&[("external_task_id", 1)], false),
        ],
    },
    Collection {
        name: "events",
        required: &[
            "event_id",
            "event_type",
            "repository_id",
            "installation",
            "sequence",
            "digest",
            "occurred_at",
        ],
        indexes: &[
            (&[("installation", 1), ("event_id", 1)], true),
            (
                &[("repository_id", 1), ("installation", 1), ("sequence", 1)],
                true,
            ),
            (&[("repository_id", 1), ("occurred_at", -1)], false),
            (&[("scope.foxx_session_id", 1), ("sequence", 1)], false),
            (&[("scope.foxx_task_id", 1), ("sequence", 1)], false),
            (&[("scope.tool_call_id", 1)], false),
            (&[("event_type", 1), ("occurred_at", -1)], false),
            (&[("decision", 1), ("occurred_at", -1)], false),
            (&[("severity", 1), ("occurred_at", -1)], false),
            (&[("domain", 1), ("occurred_at", -1)], false),
            (&[("enforcement", 1), ("occurred_at", -1)], false),
        ],
    },
    Collection {
        name: "autonomy_ledger",
        required: &[
            "entry_id",
            "session",
            "kind",
            "idempotency_key",
            "digest",
            "repository_id",
            "installation",
        ],
        indexes: &[
            (&[("installation", 1), ("idempotency_key", 1)], true),
            (
                &[("repository_id", 1), ("installation", 1), ("seq", 1)],
                true,
            ),
            (&[("session", 1), ("seq", 1)], false),
        ],
    },
    Collection {
        name: "selection_registry",
        required: &[
            "id",
            "repository_id",
            "installation",
            "record_digest",
            "signature",
        ],
        indexes: &[(
            &[("repository_id", 1), ("installation", 1), ("id", 1)],
            true,
        )],
    },
    Collection {
        name: "policy_versions",
        required: &[
            "repository_id",
            "installation",
            "generation",
            "head",
            "signature",
        ],
        indexes: &[(
            &[("repository_id", 1), ("installation", 1), ("generation", 1)],
            true,
        )],
    },
    Collection {
        name: "governance_changes",
        required: &["repository_id", "installation", "seq", "action", "digest"],
        indexes: &[(
            &[("repository_id", 1), ("installation", 1), ("seq", 1)],
            true,
        )],
    },
    Collection {
        name: "alerts",
        required: &["alert_id", "kind", "repository_id", "installation"],
        indexes: &[
            (
                &[("repository_id", 1), ("installation", 1), ("alert_id", 1)],
                true,
            ),
            (&[("repository_id", 1), ("at", -1)], false),
            (&[("session", 1), ("at", -1)], false),
        ],
    },
    Collection {
        name: "metric_rollups",
        required: &[
            "repository_id",
            "installation",
            "scope",
            "id",
            "metrics",
            "computed_at",
        ],
        indexes: &[
            (
                &[
                    ("repository_id", 1),
                    ("installation", 1),
                    ("scope", 1),
                    ("id", 1),
                ],
                true,
            ),
            (&[("scope", 1), ("computed_at", -1)], false),
        ],
    },
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

/** Apply the migrations that have not been applied: create missing collections with validators,
 * update the validators of existing ones, replace their indexes with the current ones, and record
 * the version
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
    database
        .collection::<Document>("events")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"expires_at": 1})
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
