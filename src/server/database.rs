// The ingestion service's MongoDB database. Migrations are versioned and idempotent: collections
// get $jsonSchema validators, unique indexes enforce idempotency per installation, filter indexes
// serve the dashboard, and TTL indexes expire telemetry (events, service events, alerts, sessions,
// tasks, and agents) after the retention period, 30 days by default, so a small cluster never
// fills up. Records older than the retention period less 7 days are shown as stale data on the
// dashboard. Durable state and history are never expired: repositories, chain heads, the autonomy
// ledger, selection registry, policy versions, governance audit trail, status reports, approvals,
// credit grants, Dashboard Agent definitions/incidents/quarantines, and approved flow context.
// Detailed Dashboard Agent activity logs expire after 30 days. CRANE_MONGODB_RETENTION_DAYS=persist
// keeps telemetry forever (for a larger cluster), and removes the expiry of records already stored.
// MongoDB is append-only through this service, not immutable by itself, which is why every stored
// record keeps its SHA-512 chain digest for independent verification.

use std::time::Duration;

use mongodb::bson::{self, doc, Bson, Document};
use mongodb::options::{ClientOptions, IndexOptions};
use mongodb::sync::{Client, Database};
use mongodb::IndexModel;

use crate::platform::io_error;

/** Current schema version applied by the migrations (3: records keyed by installation; 4: the
 * members' repository status and the service's settings; 10: Dashboard Agent enforcement and
 * approval-gated flow context; 11: resident-agent decisions made on the dashboard) */
pub(crate) const SCHEMA_VERSION: i64 = 11;

/** How the service reaches its database
 * Fields
    - uri: String - connection string (secret)
    - database: String - database name
    - retention_ms: Option<i64> - lifetime of telemetry records; None keeps them for ever
*/
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub(crate) uri: String,
    pub(crate) database: String,
    pub(crate) retention_ms: Option<i64>,
}

/** Days of telemetry kept by default */
pub(crate) const DEFAULT_RETENTION_DAYS: i64 = 30;

/** One day in milliseconds */
pub(crate) const DAY_MS: i64 = 86_400_000;

/** Days before deletion during which a record is shown as stale data */
pub(crate) const STALE_WINDOW_DAYS: i64 = 7;

/** The collections whose records expire, the time each record's age counts from, and that time's
 * unit in milliseconds */
pub(crate) const EXPIRING: [(&str, &str, i64); 6] = [
    ("events", "occurred_at", 1),
    ("service_events", "occurred_at", 1),
    ("alerts", "at", 1000),
    ("sessions", "last_seen_at", 1),
    ("tasks", "started_at", 1),
    ("agents", "last_seen_at", 1),
];

/** Read the retention period from the environment: CRANE_MONGODB_RETENTION_DAYS (or the older
 * CRANE_MONGODB_EVENT_RETENTION_DAYS), in days (fractions allowed), 30 by default; persist,
 * forever, off, or 0 keeps every record
 * Input
    - None
 * Output
    - Option<i64> milliseconds, None to keep everything
*/
pub(crate) fn retention_from_environment() -> Option<i64> {
    let text = std::env::var("CRANE_MONGODB_RETENTION_DAYS")
        .or_else(|_| std::env::var("CRANE_MONGODB_EVENT_RETENTION_DAYS"))
        .unwrap_or_default();
    match text.trim().to_ascii_lowercase().as_str() {
        "" => Some(DEFAULT_RETENTION_DAYS * DAY_MS),
        "persist" | "forever" | "off" | "none" | "0" => None,
        other => Some(
            other
                .parse::<f64>()
                .ok()
                .filter(|days| *days > 0.0 && days.is_finite())
                .map_or(DEFAULT_RETENTION_DAYS * DAY_MS, |days| {
                    ((days * DAY_MS as f64) as i64).max(1)
                }),
        ),
    }
}

/** When a record whose age counts from a time expires
 * Input
    - retention_ms: Option<i64> - retention period
    - from_millis: i64 - the record's time (Unix milliseconds)
 * Output
    - Option<bson::DateTime>, None when records are kept
*/
pub(crate) fn expiry(retention_ms: Option<i64>, from_millis: i64) -> Option<bson::DateTime> {
    retention_ms.map(|retention| bson::DateTime::from_millis(from_millis + retention))
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
            retention_ms: retention_from_environment(),
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
        name: "chain_heads",
        required: &["repository_id", "installation", "sequence", "digest"],
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
        name: "service_events",
        required: &["event_id", "event_name", "flow", "occurred_at"],
        indexes: &[
            (&[("event_id", 1)], true),
            (&[("repository_id", 1), ("occurred_at", -1)], false),
            (&[("action.action_user", 1), ("occurred_at", -1)], false),
        ],
    },
    Collection {
        name: "credit_grants",
        required: &[
            "grant_id",
            "repository_id",
            "installation",
            "session",
            "amount",
            "status",
        ],
        indexes: &[
            (&[("grant_id", 1)], true),
            (
                &[("repository_id", 1), ("installation", 1), ("status", 1)],
                false,
            ),
            (&[("repository_id", 1), ("approved_at", -1)], false),
        ],
    },
    Collection {
        name: "resident_decisions",
        required: &[
            "decision_id",
            "repository_id",
            "installation",
            "action",
            "decided_by",
            "status",
        ],
        indexes: &[
            (&[("decision_id", 1)], true),
            (
                &[("repository_id", 1), ("installation", 1), ("status", 1)],
                false,
            ),
            (&[("repository_id", 1), ("decided_at", -1)], false),
        ],
    },
    Collection {
        name: "dashboard_agents",
        required: &[
            "agent_id",
            "repository_id",
            "name",
            "reason",
            "created_by",
            "created_at",
            "updated_at",
            "state",
            "metric",
            "threshold",
            "window_minutes",
            "assignments",
        ],
        indexes: &[
            (&[("repository_id", 1), ("agent_id", 1)], true),
            (
                &[("repository_id", 1), ("state", 1), ("updated_at", -1)],
                false,
            ),
        ],
    },
    Collection {
        name: "dashboard_incidents",
        required: &[
            "incident_id",
            "incident_key",
            "repository_id",
            "agent_id",
            "created_at",
            "status",
            "reason",
            "three_a",
        ],
        indexes: &[
            (&[("repository_id", 1), ("incident_id", 1)], true),
            (&[("repository_id", 1), ("incident_key", 1)], true),
            (&[("repository_id", 1), ("created_at", -1)], false),
            (
                &[("repository_id", 1), ("status", 1), ("created_at", -1)],
                false,
            ),
        ],
    },
    Collection {
        name: "dashboard_quarantines",
        required: &[
            "quarantine_id",
            "repository_id",
            "kind",
            "target_id",
            "reason",
            "requested_by",
            "requested_at",
            "state",
        ],
        indexes: &[
            (&[("repository_id", 1), ("quarantine_id", 1)], true),
            (
                &[("repository_id", 1), ("state", 1), ("requested_at", -1)],
                false,
            ),
        ],
    },
    Collection {
        name: "dashboard_flow_context",
        required: &[
            "context_id",
            "repository_id",
            "incident_id",
            "flow_id",
            "context",
            "proposed_by",
            "proposed_at",
            "status",
        ],
        indexes: &[
            (&[("repository_id", 1), ("context_id", 1)], true),
            (
                &[("repository_id", 1), ("flow_id", 1), ("status", 1)],
                false,
            ),
        ],
    },
    Collection {
        name: "dashboard_agent_activity",
        required: &[
            "activity_id",
            "repository_id",
            "action",
            "actor",
            "at",
            "details",
            "expires_at",
        ],
        indexes: &[
            (&[("repository_id", 1), ("at", -1)], false),
            (&[("activity_id", 1)], true),
        ],
    },
    Collection {
        name: "dashboard_agent_leases",
        required: &["repository_id", "owner", "lease_until"],
        indexes: &[(&[("repository_id", 1)], true)],
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

/** The repository-scoped collections; global service settings and migration state are excluded. */
pub(super) fn repository_collections() -> impl Iterator<Item = &'static str> {
    COLLECTIONS
        .iter()
        .filter(|collection| !matches!(collection.name, "service_settings" | "schema_migrations"))
        .map(|collection| collection.name)
}

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
    // Metric rollups were replaced by evals computed from the event stream (DASHBOARD.md); they
    // were derived data, so nothing is lost
    if existing.iter().any(|name| name == "metric_rollups") {
        database
            .collection::<Document>("metric_rollups")
            .drop()
            .run()
            .map_err(io_error)?;
    }
    // Telemetry expires after the retention period (apply_retention sets each record's time)
    for (collection, _, _) in EXPIRING {
        database
            .collection::<Document>(collection)
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
    }
    database
        .collection::<Document>("dashboard_agent_activity")
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

/** Bring stored records in line with the retention period, at every start: when records are
 * kept, remove every expiry; otherwise give records without one their expiry, and recompute all
 * of them when the period changed since the last start. The period applied is remembered in
 * service_settings
 * Input
    - database: &Database - database
    - retention_ms: Option<i64> - retention period
 * Output
    - Result<u64, String> records changed
*/
pub(crate) fn apply_retention(
    database: &Database,
    retention_ms: Option<i64>,
) -> Result<u64, String> {
    let settings = database.collection::<Document>("service_settings");
    let wanted = match retention_ms {
        Some(retention) => Bson::Int64(retention),
        None => Bson::String("persist".into()),
    };
    let applied = settings
        .find_one(doc! {"name": "retention"})
        .run()
        .map_err(io_error)?
        .and_then(|document| document.get("retention_ms").cloned());
    let changed_period = applied.as_ref() != Some(&wanted);
    let mut changed = 0;
    for (collection, field, unit) in EXPIRING {
        let records = database.collection::<Document>(collection);
        let result = match retention_ms {
            None => records
                .update_many(
                    doc! {"expires_at": {"$exists": true}},
                    doc! {"$unset": {"expires_at": ""}},
                )
                .run(),
            Some(retention) => {
                let filter = if changed_period {
                    doc! {field: {"$exists": true}}
                } else {
                    doc! {"expires_at": {"$exists": false}, field: {"$exists": true}}
                };
                records
                    .update_many(
                        filter,
                        vec![doc! {"$set": {"expires_at": {"$toDate": {"$add": [
                            {"$multiply": [{"$toLong": format!("${field}")}, unit]},
                            retention,
                        ]}}}}],
                    )
                    .run()
            }
        }
        .map_err(io_error)?;
        changed += result.modified_count;
    }
    settings
        .replace_one(
            doc! {"name": "retention"},
            doc! {"name": "retention", "retention_ms": wanted, "applied_at": bson::DateTime::now()},
        )
        .upsert(true)
        .run()
        .map_err(io_error)?;
    Ok(changed)
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
