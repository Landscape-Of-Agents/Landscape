// The dashboard's data. The dashboard only ever holds a ReadOnly handle for reading: it can find,
// never insert, update, replace, or delete, so viewing can never change the source of truth. A
// repository's Flows view is read on each request from the members' latest repository status and
// the repository's Brick requests; nothing is cached in the service's memory.

use std::collections::BTreeMap;

use mongodb::bson::{doc, Bson, Document};
use mongodb::sync::Database;
use serde_json::{json, Value};

use crate::platform::io_error;

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
            .projection(doc! {"_id": 0})
            .run()
            .map_err(io_error)?
            .map(|document| document.map_err(io_error))
            .collect()
    }
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
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    found.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(found)
}

/** What the Flows section is built from
 * Fields
    - members: Vec<Value> - every member (one per installation): actor, verified, updated_at
    - statuses: Vec<Value> - the latest repository status of each shown member
    - brick_requests: Vec<Value> - Brick change requests made on the dashboard, newest first
*/
pub(crate) struct Context {
    pub(crate) members: Vec<Value>,
    pub(crate) statuses: Vec<Value>,
    pub(crate) brick_requests: Vec<Value>,
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

    /** Return the most recent repository status among the shown members
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

/** Load what a repository's Flows section shows: every member linked to a GitHub account
 * (machines nobody linked may be anyone's, so they are left out once a member is linked),
 * otherwise every member
 * Input
    - read: &ReadOnly - database
    - repository: &str - repository identity
 * Output
    - Result<Context, String>
*/
pub(crate) fn load(read: &ReadOnly, repository: &str) -> Result<Context, String> {
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
    let shown = members
        .iter()
        .filter_map(|member| member["installation"].as_str())
        .filter(|key| links.is_empty() || links.contains_key(*key))
        .map(|key| Bson::String(key.to_string()))
        .collect::<Vec<_>>();
    let statuses = read
        .find(
            "repository_status",
            doc! {"repository_id": repository, "installation": {"$in": shown}},
            doc! {"collected_at": -1},
            500,
        )?
        .into_iter()
        .map(json)
        .collect();
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
        .collect();
    Ok(Context {
        members,
        statuses,
        brick_requests,
    })
}

/** The Flows section of a repository: the flows, zones, drafts, watchers, and incidents the latest
 * reporting member sent, with its Bricks and agent assignments, and the flow and integration
 * requests made on the dashboard
 * Input
    - context: &Context - the repository's data
 * Output
    - Result<Value, (u16, String)> the view, or the status and message for the page
*/
pub(crate) fn flows(context: &Context) -> Result<Value, (u16, String)> {
    let Some(status) = context.latest_status() else {
        return Err((
            404,
            "no member has reported the repository's flows yet; they are sent with the next upload"
                .into(),
        ));
    };
    let mut view = status["flows"].clone();
    if view.is_null() {
        return Err((
            404,
            "the reporting machine runs a Crane without flow knowledge; update Crane on that machine"
                .into(),
        ));
    }
    if view.get("error").is_some() {
        return Err((
            500,
            format!(
                "the reporting machine could not read its flows: {}",
                view["error"]
            ),
        ));
    }
    let bricks = &status["bricks"];
    view["bricks"] = json!(bricks["bricks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| json!({"ref": row["ref"], "name": row["name"], "type": row["type"], "state": row["state"], "intent": row["intent"]}))
        .collect::<Vec<_>>());
    view["assignments"] = bricks["assignments"].clone();
    view["reported_by"] =
        context.member(status["installation"].as_str().map(String::from).as_ref());
    view["reported_at"] = status["collected_at"].clone();
    view["requests"] = json!(context
        .brick_requests
        .iter()
        .filter(|row| row["op"].as_str().is_some_and(flow_operation))
        .collect::<Vec<_>>());
    view["operations"] = crate::brick::requests::OPERATIONS
        .iter()
        .filter(|(op, _, _)| flow_operation(op))
        .map(|(op, role, label)| json!({"op": op, "role": role.name(), "label": label, "signed_file": super::auth::signed_path(op)}))
        .collect();
    Ok(view)
}

/** Report whether a request operation belongs to the Flows section (flow knowledge and the
 * connections its subagents use)
 * Input
    - op: &str - operation
 * Output
    - bool
*/
pub(crate) fn flow_operation(op: &str) -> bool {
    op.starts_with("flow-") || op.starts_with("integration-")
}
