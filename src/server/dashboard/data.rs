// The dashboard's data. The dashboard only ever holds a ReadOnly handle: it can find and
// aggregate, never insert, update, replace, or delete, and pipelines that would write ($out,
// $merge) are refused, so viewing can never change the source of truth. A repository's view is
// assembled from every selected member's records: each member's event chain and ledger are
// verified separately (their sequences are independent), then the records are merged in time
// order, and every event and session remembers which member it came from.

use std::collections::{BTreeMap, BTreeSet};

use mongodb::bson::{self, doc, Bson, Document};
use mongodb::sync::Database;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::foxx::views::Snapshot;
use crate::platform::io_error;
use crate::telemetry::events::{verify_retained, Event};
use crate::telemetry::identity::{SessionRecord, TaskRecord};
use crate::telemetry::ledger::{self, Entry};

/** Most events loaded per member, the most recent ones: a whole repository view is held in memory
 * per request, so this bounds the service's memory on a small hosted instance */
const MAX_EVENTS: i64 = 20_000;

/** Most autonomy ledger entries loaded per member (the ledger is verified from its first entry, so
 * it is loaded oldest first) */
const MAX_LEDGER: i64 = 100_000;

/** Minutes without any hook activity after which an open session without a signed attestation is
 * reported as stale (it stopped reporting: the agent was killed, the hooks were removed, or its
 * evidence is not reaching the service); CRANE_STALE_SESSION_MINUTES overrides it */
const STALE_SESSION_MINUTES: u64 = 120;

/** Return the stale-session timeout in milliseconds
 * Input
    - None
 * Output
    - u64
*/
fn stale_after_millis() -> u64 {
    std::env::var("CRANE_STALE_SESSION_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(STALE_SESSION_MINUTES)
        .max(1)
        * 60_000
}

/** A database handle that can only read
 * Fields
    - database: Database - MongoDB (private, so no write method is reachable)
*/
pub(crate) struct ReadOnly {
    database: Database,
}

impl ReadOnly {
    /** Wrap a database
     * Input
        - database: Database - MongoDB
     * Output
        - ReadOnly
    */
    pub(crate) fn new(database: Database) -> Self {
        Self { database }
    }

    /** Find documents
     * Input
        - collection: &str - collection
        - filter: Document - filter
        - sort: Document - order
        - limit: i64 - most documents
     * Output
        - Result<Vec<Document>, String>
    */
    pub(crate) fn find(
        &self,
        collection: &str,
        filter: Document,
        sort: Document,
        limit: i64,
    ) -> Result<Vec<Document>, String> {
        self.database
            .collection::<Document>(collection)
            .find(filter)
            .sort(sort)
            .limit(limit)
            .projection(doc! {"_id": 0, "ingested_at": 0, "expires_at": 0})
            .run()
            .map_err(io_error)?
            .map(|document| document.map_err(io_error))
            .collect()
    }

    /** Run an aggregation pipeline that only reads
     * Input
        - collection: &str - collection
        - pipeline: Vec<Document> - stages
     * Output
        - Result<Vec<Document>, String>
        - Error if a stage would write ($out or $merge)
    */
    pub(crate) fn aggregate(
        &self,
        collection: &str,
        pipeline: Vec<Document>,
    ) -> Result<Vec<Document>, String> {
        if pipeline
            .iter()
            .any(|stage| stage.contains_key("$out") || stage.contains_key("$merge"))
        {
            return Err("the dashboard cannot run pipelines that write".into());
        }
        self.database
            .collection::<Document>(collection)
            .aggregate(pipeline)
            .run()
            .map_err(io_error)?
            .map(|document| document.map_err(io_error))
            .collect()
    }
}

/** Decode documents into a type
 * Input
    - documents: Vec<Document> - documents
    - what: &str - collection, for errors
 * Output
    - Result<Vec<T>, String>
*/
fn decode<T: DeserializeOwned>(documents: Vec<Document>, what: &str) -> Result<Vec<T>, String> {
    documents
        .into_iter()
        .map(|document| bson::from_document(document).map_err(|error| format!("{what}: {error}")))
        .collect()
}

/** Convert a document to JSON
 * Input
    - document: Document - document
 * Output
    - Value
*/
pub(crate) fn json(document: Document) -> Value {
    serde_json::to_value(document).unwrap_or(Value::Null)
}

/** List the repositories whose remote an upload verified
 * Input
    - read: &ReadOnly - database
 * Output
    - Result<Vec<(String, String)>, String> (repository identity, remote), sorted by remote
*/
pub(crate) fn verified_repositories(read: &ReadOnly) -> Result<Vec<(String, String)>, String> {
    let mut found = read
        .find("repositories", doc! {"verified": true}, doc! {}, 10_000)?
        .into_iter()
        .filter_map(|document| {
            Some((
                document.get_str("repository_id").ok()?.to_string(),
                document.get_str("remote").ok()?.to_string(),
            ))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    found.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(found)
}

/** Everything one repository view is built from
 * Fields
    - repository: String - repository identity
    - remote: String - verified remote
    - snapshot: Snapshot - the selected members' records merged in time order
    - members: Vec<Value> - every member (one per installation): actor, verified, updated_at
    - selected: Vec<String> - the members shown
    - chains: Vec<Value> - per selected member: event chain and ledger verification
    - statuses: Vec<Value> - the latest repository status of each selected member
    - event_member: BTreeMap<String, String> - event identifier to member
    - session_member: BTreeMap<String, String> - session to member
    - attested: BTreeSet<String> - sessions with a session.attested event
    - links: BTreeMap<String, String> - member to the GitHub login that linked it
    - owner: Value - the owner key most recently confirmed by a GitHub admin (null when none)
    - specs: Vec<Value> - the repository's event stream (DASHBOARD.md), oldest first
    - grants: Vec<Value> - credit grants approved on the dashboard, newest first
    - brick_requests: Vec<Value> - Brick change requests made on the dashboard, newest first
    - brick_catalog: Vec<Value> - the org catalog of the remote's account, newest first, without
      definitions
    - dashboard_agents: Vec<Value> - configured monitor-only dashboard agents
    - dashboard_incidents: Vec<Value> - detected and manually recorded 3A incidents
    - dashboard_agent_activity: Vec<Value> - recent configuration, evaluation, and incident actions
    - dashboard_quarantines: Vec<Value> - proposed, active, and released enforcement actions
    - dashboard_flow_context: Vec<Value> - proposed and approved incident context attachments
    - truncated: bool - a member had more events than are loaded
*/
pub(crate) struct Context {
    pub(crate) repository: String,
    pub(crate) remote: String,
    pub(crate) snapshot: Snapshot,
    pub(crate) members: Vec<Value>,
    pub(crate) selected: Vec<String>,
    pub(crate) chains: Vec<Value>,
    pub(crate) statuses: Vec<Value>,
    pub(crate) event_member: BTreeMap<String, String>,
    pub(crate) session_member: BTreeMap<String, String>,
    pub(crate) attested: BTreeSet<String>,
    pub(crate) links: BTreeMap<String, String>,
    pub(crate) owner: Value,
    pub(crate) specs: Vec<Value>,
    pub(crate) grants: Vec<Value>,
    pub(crate) brick_requests: Vec<Value>,
    pub(crate) brick_catalog: Vec<Value>,
    pub(crate) dashboard_agents: Vec<Value>,
    pub(crate) dashboard_incidents: Vec<Value>,
    pub(crate) dashboard_agent_activity: Vec<Value>,
    pub(crate) dashboard_quarantines: Vec<Value>,
    pub(crate) dashboard_flow_context: Vec<Value>,
    pub(crate) truncated: bool,
}

impl Context {
    /** Describe a member: Git user name and the short code of its upload key
     * Input
        - installation: Option<&String> - member key
     * Output
        - Value {key, short, actor}
    */
    pub(crate) fn member(&self, installation: Option<&String>) -> Value {
        let Some(key) = installation else {
            return Value::Null;
        };
        let actor = self
            .members
            .iter()
            .find(|member| member["installation"] == key.as_str())
            .and_then(|member| member["actor"].as_str())
            .unwrap_or("unknown");
        json!({"key": key, "short": &key[..key.len().min(8)], "actor": actor})
    }

    /** Report whether a session stopped reporting: it never ended, has no signed attestation,
     * and no hook has seen it for the stale-session timeout
     * Input
        - session: &SessionRecord - session
     * Output
        - bool
    */
    pub(crate) fn stale(&self, session: &SessionRecord) -> bool {
        session.ended_at.is_none()
            && !self.attested.contains(&session.foxx_session_id)
            && crate::platform::now_millis().saturating_sub(session.last_seen_at)
                > stale_after_millis()
    }

    /** Return the most recent repository status among the selected members
     * Input
        - None (uses self)
     * Output
        - Option<&Value>
    */
    pub(crate) fn latest_status(&self) -> Option<&Value> {
        self.statuses
            .iter()
            .max_by_key(|status| status["collected_at"].as_u64().unwrap_or(0))
    }
}

/** Load a repository's view for the selected members: the named ones, otherwise every member
 * linked to a GitHub account (machines nobody linked may be anyone's, so they are hidden by
 * default once a member is linked), otherwise every member
 * Input
    - read: &ReadOnly - database
    - repository: &str - repository identity
    - remote: &str - verified remote
    - wanted: &BTreeSet<String> - members to show, empty for all
 * Output
    - Result<Context, String>
*/
pub(crate) fn load(
    read: &ReadOnly,
    repository: &str,
    remote: &str,
    wanted: &BTreeSet<String>,
) -> Result<Context, String> {
    let members = read
        .find(
            "repositories",
            doc! {"repository_id": repository},
            doc! {"updated_at": -1},
            500,
        )?
        .into_iter()
        .map(json)
        .collect::<Vec<_>>();
    let links = read
        .find(
            "member_links",
            doc! {"repository_id": repository, "confirmed_by": {"$type": "string"}},
            doc! {"confirmed_at": 1},
            1_000,
        )?
        .into_iter()
        .filter_map(|link| {
            Some((
                link.get_str("installation").ok()?.to_string(),
                link.get_str("confirmed_by").ok()?.to_string(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let owner = read
        .find(
            "owner_keys",
            doc! {"repository_id": repository, "confirmed_by": {"$type": "string"}},
            doc! {"confirmed_at": -1},
            1,
        )?
        .into_iter()
        .next()
        .map_or(Value::Null, |document| {
            json!({
                "owner_key": document.get_str("owner_key").unwrap_or_default(),
                "confirmed_by": document.get_str("confirmed_by").unwrap_or_default(),
                "confirmed_at": crate::server::store::number(document.get("confirmed_at")),
            })
        });
    let dashboard_agents = read
        .find(
            "dashboard_agents",
            doc! {"repository_id": repository},
            doc! {"updated_at": -1},
            500,
        )?
        .into_iter()
        .map(json)
        .collect();
    let dashboard_incidents = read
        .find(
            "dashboard_incidents",
            doc! {"repository_id": repository},
            doc! {"created_at": -1},
            1_000,
        )?
        .into_iter()
        .map(json)
        .collect();
    let dashboard_agent_activity = read
        .find(
            "dashboard_agent_activity",
            doc! {"repository_id": repository},
            doc! {"at": -1},
            500,
        )?
        .into_iter()
        .map(json)
        .collect();
    let dashboard_quarantines = read
        .find(
            "dashboard_quarantines",
            doc! {"repository_id": repository},
            doc! {"requested_at": -1},
            500,
        )?
        .into_iter()
        .map(json)
        .collect();
    let dashboard_flow_context = read
        .find(
            "dashboard_flow_context",
            doc! {"repository_id": repository},
            doc! {"proposed_at": -1},
            500,
        )?
        .into_iter()
        .map(json)
        .collect();
    let selected = members
        .iter()
        .filter_map(|member| member["installation"].as_str())
        .filter(|key| {
            if wanted.is_empty() {
                links.is_empty() || links.contains_key(*key)
            } else {
                wanted.contains(*key)
            }
        })
        .map(String::from)
        .collect::<Vec<_>>();
    let mut snapshot = Snapshot::default();
    let mut chains = Vec::new();
    let mut event_member = BTreeMap::new();
    let mut truncated = false;
    for installation in &selected {
        let scope = doc! {"repository_id": repository, "installation": installation};
        // The most recent events, put back in chain order (verify_retained accepts a chain that
        // starts after event 1)
        let mut found = read.find("events", scope.clone(), doc! {"sequence": -1}, MAX_EVENTS)?;
        truncated |= found.len() as i64 >= MAX_EVENTS;
        found.reverse();
        let events: Vec<Event> = decode(found, "events")?;
        let entries: Vec<Entry> = decode(
            read.find("autonomy_ledger", scope, doc! {"seq": 1}, MAX_LEDGER)?,
            "autonomy_ledger",
        )?;
        chains.push(json!({
            "member": installation,
            // Events older than the retention period expire; the rest must still chain
            "event_chain": verify_retained(&events),
            "ledger_reconciles": ledger::verify(&entries).is_ok(),
        }));
        for event in &events {
            event_member.insert(event.event_id.clone(), installation.clone());
        }
        snapshot.events.extend(events);
        snapshot.ledger.extend(entries);
    }
    snapshot.events.sort_by(|left, right| {
        (left.occurred_at, left.sequence).cmp(&(right.occurred_at, right.sequence))
    });
    let among = doc! {
        "repository_id": repository,
        "installation": {"$in": selected.iter().map(|key| Bson::String(key.clone())).collect::<Vec<_>>()},
    };
    let sessions = read.find("sessions", among.clone(), doc! {"last_seen_at": -1}, 5_000)?;
    let mut session_member = BTreeMap::new();
    for session in &sessions {
        if let (Ok(id), Ok(key)) = (
            session.get_str("foxx_session_id"),
            session.get_str("installation"),
        ) {
            session_member.insert(id.to_string(), key.to_string());
        }
    }
    snapshot.sessions = decode::<SessionRecord>(sessions, "sessions")?;
    let attested = snapshot
        .events
        .iter()
        .filter(|event| event.event_type == "session.attested")
        .filter_map(|event| event.scope.foxx_session_id.clone())
        .collect::<BTreeSet<_>>();
    snapshot.tasks = decode::<TaskRecord>(
        read.find("tasks", among.clone(), doc! {"started_at": -1}, 5_000)?,
        "tasks",
    )?;
    snapshot.alerts = read
        .find("alerts", among.clone(), doc! {"at": -1}, 5_000)?
        .into_iter()
        .map(json)
        .collect();
    let audit = read
        .find("governance_changes", among.clone(), doc! {"seq": 1}, 10_000)?
        .into_iter()
        .map(json)
        .collect::<Vec<_>>();
    // The service's events about the selected machines, and (unless one member is chosen) those
    // about the repository itself, such as dashboard views
    let machines = selected
        .iter()
        .map(|key| Bson::String(key.clone()))
        .collect::<Vec<_>>();
    let mut owners = vec![doc! {"installation": {"$in": machines}}];
    if wanted.is_empty() {
        owners.push(doc! {"installation": Bson::Null});
    }
    let service = read
        .find(
            "service_events",
            doc! {"repository_id": repository, "$or": owners},
            doc! {"occurred_at": 1},
            20_000,
        )?
        .into_iter()
        .map(json)
        .collect::<Vec<_>>();
    let grants = read
        .find(
            "credit_grants",
            doc! {"repository_id": repository},
            doc! {"approved_at": -1},
            500,
        )?
        .into_iter()
        .map(json)
        .collect::<Vec<_>>();
    // Unconfirmed requests are left out: GitHub has not yet said who may make them
    let brick_requests = read
        .find(
            "brick_requests",
            doc! {"repository_id": repository, "status": {"$ne": "STAGED"}},
            doc! {"requested_at": -1},
            200,
        )?
        .into_iter()
        .map(json)
        .map(|mut row| {
            if let Some(text) = row["params"].as_str() {
                row["params"] = serde_json::from_str(text).unwrap_or(Value::Null);
            }
            row
        })
        .collect::<Vec<_>>();
    // The organization's shared Bricks (brick/share.rs), listed without their definitions
    let brick_catalog = match super::auth::account(remote) {
        Some(account) => read
            .aggregate(
                "brick_catalog",
                vec![
                    doc! {"$match": {"account": &account}},
                    doc! {"$sort": {"published_at": -1}},
                    doc! {"$limit": crate::brick::share::MAX_ENTRIES as i64},
                    doc! {"$project": {"_id": 0, "entry": 0}},
                ],
            )?
            .into_iter()
            .map(json)
            .map(|mut row| {
                row["ref"] = json!(format!(
                    "{}@{}",
                    row["brick_id"].as_str().unwrap_or_default(),
                    row["version"]
                ));
                row
            })
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };
    let statuses = read
        .find("repository_status", among, doc! {"collected_at": -1}, 500)?
        .into_iter()
        .map(json)
        .collect();
    let specs = super::stream::build(
        &super::stream::Sources {
            repository,
            remote,
            snapshot: &snapshot,
            event_member: &event_member,
            session_member: &session_member,
            attested: &attested,
            audit: &audit,
            service: &service,
            chains: &chains,
        },
        crate::platform::now_millis(),
    );
    Ok(Context {
        repository: repository.into(),
        remote: remote.into(),
        snapshot,
        members,
        selected,
        chains,
        statuses,
        event_member,
        session_member,
        attested,
        links,
        owner,
        specs,
        grants,
        brick_requests,
        brick_catalog,
        dashboard_agents,
        dashboard_incidents,
        dashboard_agent_activity,
        dashboard_quarantines,
        dashboard_flow_context,
        truncated,
    })
}

/** Summarize every repository of an account: members, events, denials, sessions, alerts, and
 * the last activity
 * Input
    - read: &ReadOnly - database
    - repositories: &[(String, String)] - (identity, remote) of the account's repositories
 * Output
    - Result<Vec<Value>, String>
*/
pub(crate) fn summary(
    read: &ReadOnly,
    repositories: &[(String, String)],
) -> Result<Vec<Value>, String> {
    let ids = repositories
        .iter()
        .map(|(id, _)| Bson::String(id.clone()))
        .collect::<Vec<_>>();
    let count_by = |collection: &str| -> Result<BTreeMap<String, u64>, String> {
        Ok(read
            .aggregate(
                collection,
                vec![
                    doc! {"$match": {"repository_id": {"$in": ids.clone()}}},
                    doc! {"$group": {"_id": "$repository_id", "count": {"$sum": 1}}},
                ],
            )?
            .into_iter()
            .filter_map(|row| {
                Some((
                    row.get_str("_id").ok()?.to_string(),
                    crate::server::store::number(row.get("count")),
                ))
            })
            .collect())
    };
    let activity = read
        .aggregate(
            "events",
            vec![
                doc! {"$match": {"repository_id": {"$in": ids.clone()}}},
                doc! {"$group": {
                    "_id": "$repository_id",
                    "events": {"$sum": 1},
                    "denied": {"$sum": {"$cond": [{"$eq": ["$decision", "DENY"]}, 1, 0]}},
                    "bypass": {"$sum": {"$cond": [{"$eq": ["$enforcement", "BYPASS_CONFIRMED"]}, 1, 0]}},
                    "last": {"$max": "$occurred_at"},
                }},
            ],
        )?
        .into_iter()
        .map(json)
        .collect::<Vec<_>>();
    let members = count_by("repositories")?;
    let sessions = count_by("sessions")?;
    let alerts = count_by("alerts")?;
    Ok(repositories
        .iter()
        .map(|(id, remote)| {
            let row = activity
                .iter()
                .find(|row| row["_id"] == id.as_str())
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "repository": id,
                "remote": remote,
                "members": members.get(id).copied().unwrap_or(0),
                "sessions": sessions.get(id).copied().unwrap_or(0),
                "alerts": alerts.get(id).copied().unwrap_or(0),
                "events": row["events"].as_u64().unwrap_or(0),
                "denied": row["denied"].as_u64().unwrap_or(0),
                "bypasses_confirmed": row["bypass"].as_u64().unwrap_or(0),
                "last_activity": row["last"],
            })
        })
        .collect())
}
