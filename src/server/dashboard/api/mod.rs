// The dashboard's JSON API for one repository (and the selected members): the overview, agents and
// sessions, the action graph, the event timeline, governance, the autonomy ledger, compliance,
// alerts, health, the repository connection, integrations, session reports, and evidence bundles.
// Every count comes from the repository's event stream (DASHBOARD.md) and its evals. No route
// changes anything. Lists are bounded and paginated.

mod activity;
mod aggregate;
mod controls;
mod filter;
mod insights;
mod operate;
mod reports;
mod timeline;

use activity::*;
use controls::*;
use reports::*;
use timeline::*;

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::foxx::views::{agents, session_view_with, task_view_with};
use crate::server::dashboard::data::Context;
use crate::telemetry::compliance::{self};
use crate::telemetry::query::Query;
use crate::telemetry::spec::{GROUPS, SPEC_VERSION};

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
    - kind: &str - PERSONAL or ORGANIZATION (whose account owns the repository)
 * Output
    - Answer
*/
pub(crate) fn route(
    context: &Context,
    path: &str,
    query: &BTreeMap<String, String>,
    kind: &str,
) -> Answer {
    let snapshot = &context.snapshot;
    let segments = path.trim_end_matches('/').split('/').collect::<Vec<_>>();
    // The global filter (telemetry/query.rs), shared by every event view
    let filter = match Query::from_params(query) {
        Ok(filter) => filter,
        Err(message) => return error(400, message),
    };
    let session_detail = |id: &str| {
        snapshot.session(id).map(|session| {
            let mut view = session_view_with(snapshot, session, &context.specs);
            view["member"] = context.member(context.session_member.get(&session.foxx_session_id));
            view["stale"] = json!(context.stale(session));
            view["tasks"] = json!(snapshot
                .tasks
                .iter()
                .filter(|task| task.foxx_session_id == session.foxx_session_id)
                .map(|task| task_view_with(snapshot, task, &context.specs))
                .collect::<Vec<_>>());
            view
        })
    };
    match segments.as_slice() {
        ["health"] => (200, health(context), None),
        ["overview"] => (200, overview(context, &filter, kind), None),
        ["members"] => (200, members(context), None),
        ["agents"] => (
            200,
            json!({"agents": agents(snapshot, &context.specs)}),
            None,
        ),
        ["sessions"] => (200, sessions(context, query), None),
        ["sessions", id] => match session_detail(id) {
            Some(view) => (200, view, None),
            None => error(404, format!("no session '{id}'")),
        },
        ["tasks"] => (
            200,
            json!({"tasks": snapshot.tasks.iter().filter(|task| query.get("session").is_none_or(|session| &task.foxx_session_id == session)).take(MAX_LIMIT).map(|task| task_view_with(snapshot, task, &context.specs)).collect::<Vec<_>>()}),
            None,
        ),
        ["tasks", id] => match snapshot.task(id) {
            Some(task) => (200, task_view_with(snapshot, task, &context.specs), None),
            None => error(404, format!("no task '{id}'")),
        },
        ["events"] => (200, events(context, &filter, query, kind), None),
        ["series"] => (200, filter::series(context, &filter, query), None),
        ["matrix"] => (200, aggregate::matrix(context, &filter, query), None),
        ["calendar"] => (200, aggregate::calendar(context, &filter, query), None),
        ["retention"] => (200, activity::retention(context), None),
        ["facets"] => (200, filter::facets(context, &filter), None),
        ["attributes"] => (200, filter::attributes(context, &filter), None),
        ["gantt"] => (200, insights::gantt(context, &filter, query), None),
        ["timeline"] => (200, insights::timeline(context, &filter, query), None),
        ["traces"] => (200, insights::trace_table(context, &filter, query), None),
        ["usage"] => (200, insights::usage(context, &filter, query), None),
        ["traces", id] => match insights::trace_detail(context, id, kind) {
            Some(view) => (200, view, None),
            None => error(404, format!("no trace '{id}'")),
        },
        ["topology"] => (200, insights::topology(context, &filter, query), None),
        ["dependencies"] => (200, insights::dependencies(context, &filter), None),
        ["histogram"] => (200, insights::histogram(context, &filter, query), None),
        ["semantics"] => (200, insights::semantics(context, &filter), None),
        ["alerting"] => (200, operate::alerting(context, kind), None),
        ["grants"] => (200, operate::grants(context), None),
        ["graph", id] => match snapshot.session(id) {
            Some(session) => (200, graph(context, &session.foxx_session_id, kind), None),
            None => error(404, format!("no session '{id}'")),
        },
        ["governance"] => governance(context),
        ["ledger"] => (200, ledger_view(context, query), None),
        ["definitions"] => (
            200,
            json!({"spec_version": SPEC_VERSION, "groups": GROUPS, "facets": crate::telemetry::query::FACETS.iter().map(|(name, path)| json!({"name": name, "path": path})).collect::<Vec<_>>(), "controls": compliance::CONTROLS, "domains": crate::telemetry::model::Domain::ALL}),
            None,
        ),
        ["compliance"] => (200, compliance_view(context), None),
        ["alerts"] => (200, alerts(context), None),
        ["integrations"] => (200, integrations(context), None),
        ["repo"] => (200, repo_view(context), None),
        ["reports", "session", id] => match snapshot.session(id) {
            Some(session) => (
                200,
                session_report(context, &session.foxx_session_id, kind),
                None,
            ),
            None => error(404, format!("no session '{id}'")),
        },
        ["evidence", "session", id] => match snapshot.session(id) {
            Some(session) => {
                let bundle = evidence_bundle(context, &session.foxx_session_id, kind);
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
