// The dashboard's JSON API for one repository (and the selected members): every view the old local
// dashboard had (overview, agents and sessions, the action graph, the event timeline, governance,
// the autonomy ledger, metrics, compliance, alerts, health, the repository connection, integrations,
// session reports, and evidence bundles), built from the uploaded records and the members' latest
// repository status. No route changes anything. Lists are bounded and paginated.

mod activity;
mod controls;
mod reports;
mod timeline;

use activity::*;
use controls::*;
use reports::*;
use timeline::*;

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::foxx::views::{agents, session_view, task_view};
use crate::server::dashboard::data::Context;
use crate::telemetry::compliance::{self};
use crate::telemetry::metrics::DEFINITIONS;

/** Largest page of events */
const MAX_LIMIT: usize = 500;

/** Default page of events */
const DEFAULT_LIMIT: usize = 100;

/** A route's answer: status, body, and a download file name for attachments */
pub(crate) type Answer = (u16, Value, Option<String>);

/** Build an error answer
 * Input
    - status: u16 - HTTP status
    - message: String - explanation
 * Output
    - Answer
*/
pub(crate) fn error(status: u16, message: String) -> Answer {
    (status, json!({"error": message}), None)
}

/** Route a repository request
 * Input
    - context: &Context - the repository's data
    - path: &str - request path after /api/
    - query: &BTreeMap<String, String> - query parameters
 * Output
    - Answer
*/
pub(crate) fn route(context: &Context, path: &str, query: &BTreeMap<String, String>) -> Answer {
    let snapshot = &context.snapshot;
    let segments = path.trim_end_matches('/').split('/').collect::<Vec<_>>();
    let session_detail = |id: &str| {
        snapshot.session(id).map(|session| {
            let mut view = session_view(snapshot, session);
            view["member"] = context.member(context.session_member.get(&session.foxx_session_id));
            view["tasks"] = json!(snapshot
                .tasks
                .iter()
                .filter(|task| task.foxx_session_id == session.foxx_session_id)
                .map(|task| task_view(snapshot, task))
                .collect::<Vec<_>>());
            view
        })
    };
    match segments.as_slice() {
        ["health"] => (200, health(context), None),
        ["overview"] => (200, overview(context, query), None),
        ["members"] => (200, members(context), None),
        ["agents"] => (200, json!({"agents": agents(snapshot)}), None),
        ["sessions"] => (200, sessions(context, query), None),
        ["sessions", id] => match session_detail(id) {
            Some(view) => (200, view, None),
            None => error(404, format!("no session '{id}'")),
        },
        ["tasks"] => (
            200,
            json!({"tasks": snapshot.tasks.iter().filter(|task| query.get("session").is_none_or(|session| &task.foxx_session_id == session)).take(MAX_LIMIT).map(|task| task_view(snapshot, task)).collect::<Vec<_>>()}),
            None,
        ),
        ["tasks", id] => match snapshot.task(id) {
            Some(task) => (200, task_view(snapshot, task), None),
            None => error(404, format!("no task '{id}'")),
        },
        ["events"] => (200, events(context, query), None),
        ["graph", id] => match snapshot.session(id) {
            Some(session) => (
                200,
                graph(context, &snapshot.session_events(&session.foxx_session_id)),
                None,
            ),
            None => error(404, format!("no session '{id}'")),
        },
        ["governance"] => governance(context),
        ["ledger"] => (200, ledger_view(context, query), None),
        ["metrics"] => metrics_view(context, query),
        ["definitions"] => (
            200,
            json!({"metrics": DEFINITIONS, "controls": compliance::CONTROLS, "domains": crate::telemetry::model::Domain::ALL}),
            None,
        ),
        ["compliance"] => (200, compliance_view(context), None),
        ["alerts"] => (200, alerts(context), None),
        ["integrations"] => (200, integrations(context), None),
        ["repo"] => (200, repo_view(context), None),
        ["reports", "session", id] => match snapshot.session(id) {
            Some(session) => (200, session_report(context, &session.foxx_session_id), None),
            None => error(404, format!("no session '{id}'")),
        },
        ["evidence", "session", id] => match snapshot.session(id) {
            Some(session) => {
                let bundle = evidence_bundle(context, &session.foxx_session_id);
                let text = serde_json::to_string_pretty(&bundle).unwrap_or_default();
                (
                    200,
                    Value::String(text),
                    Some(format!("crane-evidence-{}.json", session.foxx_session_id)),
                )
            }
            None => error(404, format!("no session '{id}'")),
        },
        _ => error(404, format!("no API route /api/{path}")),
    }
}

/** Parse a time filter given as Unix milliseconds
 * Input
    - query: &BTreeMap<String, String> - query
    - key: &str - since or until
 * Output
    - Option<u64>
*/
fn time_filter(query: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    query.get(key).and_then(|value| value.parse::<u64>().ok())
}

/** Combine the members' event chain and ledger results
 * Input
    - context: &Context - repository data
 * Output
    - (Value, bool) event chain summary {status, length, members} and whether every ledger reconciles
*/
fn chains(context: &Context) -> (Value, bool) {
    let statuses = context
        .chains
        .iter()
        .map(|chain| {
            chain["event_chain"]["status"]
                .as_str()
                .unwrap_or("unknown")
                .to_string()
        })
        .collect::<Vec<_>>();
    let status = if statuses.iter().any(|status| status == "broken") {
        "broken"
    } else if statuses.iter().all(|status| status == "verified") && !statuses.is_empty() {
        "verified"
    } else if statuses.is_empty() {
        "empty"
    } else {
        statuses.first().map_or("unknown", String::as_str)
    };
    let members = context
        .chains
        .iter()
        .map(|chain| {
            json!({
                "member": context.member(chain["member"].as_str().map(String::from).as_ref()),
                "event_chain": chain["event_chain"],
                "ledger_reconciles": chain["ledger_reconciles"],
            })
        })
        .collect::<Vec<_>>();
    (
        json!({"status": status, "length": context.snapshot.events.len(), "members": members}),
        context
            .chains
            .iter()
            .all(|chain| chain["ledger_reconciles"] == true),
    )
}

/** Combine the members' latest integrity reports
 * Input
    - context: &Context - repository data
 * Output
    - Value {ok, findings, reported}
*/
fn integrity(context: &Context) -> Value {
    let mut findings = Vec::new();
    let mut ok = true;
    for status in &context.statuses {
        let member = context.member(status["installation"].as_str().map(String::from).as_ref());
        ok &= status["integrity"]["ok"] != false;
        for finding in status["integrity"]["findings"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let mut finding = finding.clone();
            finding["member"] = member.clone();
            findings.push(finding);
        }
    }
    json!({"ok": ok, "findings": findings, "reported": !context.statuses.is_empty()})
}
