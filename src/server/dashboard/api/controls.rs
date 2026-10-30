// Governance and control views: the reported governance state, the autonomy ledger, compliance
// controls (with the members' combined inputs), integrations, and the connection.

use super::*;
use crate::server::dashboard::data::Context;
use crate::telemetry::compliance::{self, Inputs};
use crate::telemetry::evals::summarize;
use crate::telemetry::ledger::Ledger;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/** The governance view, as last reported by a member's machine
 * Input
    - context: &Context - repository data
 * Output
    - Answer
*/
pub(super) fn governance(context: &Context) -> Answer {
    let Some(status) = context.latest_status() else {
        return error(
            404,
            "no member has reported the repository's governance yet; it is sent with the next upload".into(),
        );
    };
    let mut view = status["governance"].clone();
    if view.get("error").is_some() {
        return error(
            500,
            format!(
                "the reporting machine could not evaluate governance: {}",
                view["error"]
            ),
        );
    }
    view["reported_by"] =
        context.member(status["installation"].as_str().map(String::from).as_ref());
    view["reported_at"] = status["collected_at"].clone();
    (200, view, None)
}

/** The Bricks view, as last reported by a member's machine: library, Stacks, attachments,
 * assignments, monitors, context cache, and signed history (identifiers and digests only)
 * Input
    - context: &Context - repository data
 * Output
    - Answer
*/
pub(super) fn bricks(context: &Context) -> Answer {
    let Some(status) = context.latest_status() else {
        return error(
            404,
            "no member has reported the repository's Bricks yet; they are sent with the next upload".into(),
        );
    };
    let mut view = status["bricks"].clone();
    if view.is_null() {
        return error(
            404,
            "the reporting machine runs a Crane without Bricks; update Crane on that machine"
                .into(),
        );
    }
    if view.get("error").is_some() {
        return error(
            500,
            format!(
                "the reporting machine could not read its Bricks: {}",
                view["error"]
            ),
        );
    }
    view["reported_by"] =
        context.member(status["installation"].as_str().map(String::from).as_ref());
    view["reported_at"] = status["collected_at"].clone();
    // Changes people requested here, and what may be requested with which GitHub role
    view["requests"] = json!(context
        .brick_requests
        .iter()
        .filter(|row| !row["op"]
            .as_str()
            .is_some_and(|op| op.starts_with("flow-") || op.starts_with("integration-")))
        .collect::<Vec<_>>());
    view["catalog"] = json!(context.brick_catalog);
    view["operations"] = crate::brick::requests::OPERATIONS
        .iter()
        .filter(|(op, _, _)| !op.starts_with("flow-") && !op.starts_with("integration-"))
        .map(|(op, role, label)| json!({"op": op, "role": role.name(), "label": label}))
        .collect();
    (200, view, None)
}

/** The Flows view, as last reported by a member's machine: every zone and flow with its knowledge,
 * trace drafts with their current graph, agents' proposals, watchers, incidents, models and
 * routing; the Bricks those targets carry (from the Brick report, for their names and kinds); the
 * agents assigned; the flow requests made here; and what may be requested with which role
 * Input
    - context: &Context - repository data
 * Output
    - Answer
*/
pub(super) fn flows(context: &Context) -> Answer {
    let Some(status) = context.latest_status() else {
        return error(
            404,
            "no member has reported the repository's flows yet; they are sent with the next upload"
                .into(),
        );
    };
    let mut view = status["flows"].clone();
    if view.is_null() {
        return error(
            404,
            "the reporting machine runs a Crane without flow knowledge; update Crane on that machine"
                .into(),
        );
    }
    if view.get("error").is_some() {
        return error(
            500,
            format!(
                "the reporting machine could not read its flows: {}",
                view["error"]
            ),
        );
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
        .filter(|row| row["op"]
            .as_str()
            .is_some_and(|op| op.starts_with("flow-") || op.starts_with("integration-")))
        .collect::<Vec<_>>());
    view["operations"] = crate::brick::requests::OPERATIONS
        .iter()
        .filter(|(op, _, _)| op.starts_with("flow-") || op.starts_with("integration-"))
        .map(|(op, role, label)| json!({"op": op, "role": role.name(), "label": label, "signed_file": super::super::auth::signed_path(op)}))
        .collect();
    (200, view, None)
}

/** The Resident Agents view, as last reported by a member's machine: definitions, assignments
 * with their derived status and health, recent activations with timelines, the review inbox, idle
 * cost and health, test reports, and the catalogs (identifiers, states, digests, and counts only)
 * Input
    - context: &Context - repository data
 * Output
    - Answer
*/
pub(super) fn residents(context: &Context) -> Answer {
    let reports = context
        .statuses
        .iter()
        .map(|status| {
            (
                status["installation"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                context.member(status["installation"].as_str().map(String::from).as_ref()),
                status,
            )
        })
        .collect::<Vec<_>>();
    if reports.is_empty() {
        return error(
            404,
            "no member has reported the repository's resident agents yet; they are sent with the next upload".into(),
        );
    }
    match merge_residents(&reports) {
        Ok(view) => (200, view, None),
        Err((status, message)) => error(status, message),
    }
}

/** Merge the resident reports of every member's machine: each machine keeps its own resident
 * store, so the page lists every machine's assignments, activations, inbox rows, and tests
 * (each tagged with its machine, which is where a dashboard decision is applied), sums the
 * health counters, and takes the catalogs and the Crane view from the newest report
 * Input
    - reports: &[(String, Value, &Value)] - installation, member, and latest status of each machine
 * Output
    - Result<Value, (u16, String)> the merged view, or the status and message of why there is none
*/
pub(super) fn merge_residents(reports: &[(String, Value, &Value)]) -> Result<Value, (u16, String)> {
    let mut usable = reports
        .iter()
        .filter(|(_, _, status)| {
            status["residents"].is_object() && status["residents"].get("error").is_none()
        })
        .collect::<Vec<_>>();
    usable.sort_by_key(|(_, _, status)| {
        std::cmp::Reverse(status["collected_at"].as_u64().unwrap_or(0))
    });
    let Some((_, newest_member, newest)) = usable.first() else {
        let failed = reports
            .iter()
            .find_map(|(_, _, status)| status["residents"].get("error").cloned());
        return Err(match failed {
            Some(error) => (
                500,
                format!("the reporting machine could not read its resident store: {error}"),
            ),
            None => (
                404,
                "the reporting machines run a Crane without resident agents; update Crane on them"
                    .into(),
            ),
        });
    };
    let mut view = json!({
        "schema_version": newest["residents"]["schema_version"],
        "catalog": newest["residents"]["catalog"],
        "crane": newest["residents"]["crane"],
        "reported_by": newest_member,
        "reported_at": newest["collected_at"],
        "definitions": [], "assignments": [], "activations": [], "rcas": [], "tests": [],
        "inbox": {"approvals": [], "questions": [], "reconciliation": [], "rca_proposals": [], "assignments": []},
        "health": {"counters": {}, "ignored": {}, "outcomes": {}, "cache": {}, "queue_depth": 0, "dead_letter": 0, "idle_model_calls": 0, "token_usage": newest["residents"]["health"]["token_usage"]},
        "members": [],
    });
    let mut definitions = std::collections::BTreeSet::new();
    let mut rcas = std::collections::BTreeSet::new();
    for (installation, member, status) in &usable {
        let report = &status["residents"];
        let tag = |row: &Value| {
            let mut row = row.clone();
            row["installation"] = json!(installation);
            row["member"] = member.clone();
            row
        };
        for (key, list) in [
            ("assignments", &report["assignments"]),
            ("activations", &report["activations"]),
            ("tests", &report["tests"]),
        ] {
            for row in list.as_array().into_iter().flatten() {
                if let Some(rows) = view[key].as_array_mut() {
                    rows.push(tag(row));
                }
            }
        }
        // Definitions and graphs are the same records on every machine that holds them: once each
        for (key, seen, id) in [
            ("definitions", &mut definitions, "ref"),
            ("rcas", &mut rcas, "rca"),
        ] {
            for row in report[key].as_array().into_iter().flatten() {
                let identity = format!(
                    "{}#{}",
                    row[id].as_str().unwrap_or_default(),
                    row["digest"].as_str().unwrap_or_default()
                );
                if seen.insert(identity) {
                    if let Some(rows) = view[key].as_array_mut() {
                        rows.push(tag(row));
                    }
                }
            }
        }
        for key in [
            "approvals",
            "questions",
            "reconciliation",
            "rca_proposals",
            "assignments",
        ] {
            for row in report["inbox"][key].as_array().into_iter().flatten() {
                if let Some(rows) = view["inbox"][key].as_array_mut() {
                    rows.push(tag(row));
                }
            }
        }
        let health = &report["health"];
        for (group, source) in [
            ("counters", &health["counters"]),
            ("ignored", &health["ignored"]),
            ("outcomes", &health["outcomes"]),
            ("cache", &health["cache"]),
        ] {
            for (name, value) in source.as_object().into_iter().flatten() {
                if let Some(number) = value.as_u64() {
                    let total = view["health"][group][name].as_u64().unwrap_or(0) + number;
                    view["health"][group][name] = json!(total);
                }
            }
        }
        for name in ["queue_depth", "dead_letter", "idle_model_calls"] {
            let total =
                view["health"][name].as_u64().unwrap_or(0) + health[name].as_u64().unwrap_or(0);
            view["health"][name] = json!(total);
        }
        if let Some(members) = view["members"].as_array_mut() {
            members.push(json!({"installation": installation, "member": member, "reported_at": status["collected_at"], "assignments": report["assignments"].as_array().map_or(0, Vec::len)}));
        }
    }
    if let Some(rows) = view["activations"].as_array_mut() {
        rows.sort_by_key(|row| std::cmp::Reverse(row["created_at"].as_u64().unwrap_or(0)));
    }
    Ok(view)
}

/** The ledger view of one session or all sessions: entries, balances, and whether every member's
 * ledger reconciles
 * Input
    - context: &Context - repository data
    - query: &BTreeMap<String, String> - optional session
 * Output
    - Value
*/
pub(super) fn ledger_view(context: &Context, query: &BTreeMap<String, String>) -> Value {
    let snapshot = &context.snapshot;
    let entries = snapshot
        .ledger
        .iter()
        .filter(|entry| {
            query
                .get("session")
                .is_none_or(|session| &entry.session == session)
        })
        .collect::<Vec<_>>();
    let sessions = entries
        .iter()
        .map(|entry| entry.session.clone())
        .collect::<BTreeSet<_>>();
    json!({
        "reconciles": chains(context).1,
        "balances": sessions.iter().map(|session| json!({"session": session, "balance": Ledger::balance_of(&snapshot.ledger, session)})).collect::<Vec<_>>(),
        "entries": entries.iter().rev().take(MAX_LIMIT).collect::<Vec<_>>(),
    })
}

/** Combine the members' reported compliance inputs: policy results from the latest report, and for
 * each machine-level control the best evidence any member reported (alert delivery, valid hooks, a
 * rollback anchor) or the worst finding (integrity, audit chain)
 * Input
    - context: &Context - repository data
 * Output
    - Inputs
*/
pub(super) fn compliance_inputs(context: &Context) -> Inputs {
    let reported = context
        .statuses
        .iter()
        .filter_map(|status| {
            serde_json::from_value::<Inputs>(status["compliance_inputs"].clone()).ok()
        })
        .collect::<Vec<_>>();
    let mut inputs: Inputs = context
        .latest_status()
        .and_then(|status| serde_json::from_value(status["compliance_inputs"].clone()).ok())
        .unwrap_or_default();
    if !reported.is_empty() {
        inputs.alerts_configured = reported.iter().any(|item| item.alerts_configured);
        inputs.hooks_valid = reported.iter().any(|item| item.hooks_valid);
        inputs.anchor = reported.iter().any(|item| item.anchor);
        inputs.audit_verified = reported.iter().all(|item| item.audit_verified);
        inputs.integrity_findings = reported
            .iter()
            .map(|item| item.integrity_findings)
            .max()
            .unwrap_or(0);
    }
    inputs
}

/** The compliance view: every control with outcome, evidence, gaps, and mappings, from the
 * uploaded records and the latest reported local verification
 * Input
    - context: &Context - repository data
 * Output
    - Value
*/
pub(super) fn compliance_view(context: &Context) -> Value {
    let snapshot = &context.snapshot;
    let sessions = snapshot
        .sessions
        .iter()
        .map(|session| serde_json::to_value(session).unwrap_or_default())
        .collect::<Vec<_>>();
    let evals = summarize(&context.specs.iter().collect::<Vec<_>>());
    let (chain, ledger_reconciles) = chains(context);
    let mut inputs = compliance_inputs(context);
    inputs.events_verified = chain["status"] != "broken";
    inputs.ledger_verified = ledger_reconciles;
    let controls = compliance::evaluate(
        &inputs,
        &evals,
        snapshot.events.len(),
        snapshot.ledger.len(),
        &sessions,
    );
    let mut outcomes: BTreeMap<&str, usize> = BTreeMap::new();
    for control in &controls {
        *outcomes.entry(control.outcome).or_default() += 1;
    }
    // What hooks can only infer is observability, not a control: network destinations (F36)
    let mut destinations: BTreeMap<String, u64> = BTreeMap::new();
    for spec in context
        .specs
        .iter()
        .filter(|spec| spec["event_name"] == "NETWORK_DESTINATION_INFERRED")
    {
        for host in crate::telemetry::query::values(spec, "resource.resource_ids") {
            *destinations.entry(host).or_default() += 1;
        }
    }
    json!({
        "disclaimer": "Framework mappings show which requirements this evidence supports; they are not a certification.",
        "controls": controls,
        "outcomes": outcomes,
        "evaluated_at": crate::platform::now_millis(),
        "inputs_reported": !context.statuses.is_empty(),
        "unsupported_telemetry": compliance::UNSUPPORTED,
        "inferred": compliance::INFERRED.iter().map(|(name, meaning)| json!({
            "name": name,
            "meaning": meaning,
            "values": destinations.iter().map(|(host, count)| json!({"value": host, "count": count})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/** Every member's configured integrations (non-secret settings), with the member
 * Input
    - context: &Context - repository data
 * Output
    - Value {integrations}
*/
pub(super) fn integrations(context: &Context) -> Value {
    let list = context
        .statuses
        .iter()
        .flat_map(|status| {
            let member = context.member(status["installation"].as_str().map(String::from).as_ref());
            status["integrations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |mut integration| {
                    integration["member"] = member.clone();
                    integration
                })
        })
        .collect::<Vec<_>>();
    json!({"integrations": list})
}

/** The repository connection, as last reported
 * Input
    - context: &Context - repository data
 * Output
    - Value
*/
pub(super) fn repo_view(context: &Context) -> Value {
    let mut view = context.latest_status().map_or_else(
        || json!({"connection": null}),
        |status| status["repo"].clone(),
    );
    view["remote"] = json!(context.remote);
    view["owner"] = context.owner.clone();
    view
}

#[cfg(test)]
mod tests {
    use super::merge_residents;
    use serde_json::json;

    /** Two machines' resident reports merge: every assignment is kept and tagged with its machine,
     * shared definitions appear once, counters add up, and an older Crane without residents
     * neither hides the others nor fails the page
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn resident_reports_of_every_machine_are_merged() {
        let report = |id: &str, idle: u64, at: u64| {
            json!({"collected_at": at, "residents": {
                "schema_version": 1, "catalog": {"steps": []}, "crane": null,
                "definitions": [{"ref": "ci-watch@1", "digest": "d1"}], "rcas": [],
                "assignments": [{"assignment_id": id, "status": "ACTIVE_IDLE"}],
                "activations": [{"activation_id": format!("act_{id}"), "created_at": at}], "tests": [],
                "inbox": {"approvals": [{"activation": format!("act_{id}"), "step": "notify"}], "questions": [], "reconciliation": [], "rca_proposals": [], "assignments": []},
                "health": {"counters": {"ticks": 10, "idle_model_calls": idle}, "ignored": {"NO_MATCH": 2}, "outcomes": {}, "cache": {}, "queue_depth": 1, "dead_letter": 0, "idle_model_calls": idle, "token_usage": "unmeasured"}
            }})
        };
        let (first, second) = (report("alpha", 0, 100), report("beta", 0, 200));
        let older = json!({"collected_at": 300, "residents": null});
        let reports = vec![
            ("m1".to_string(), json!({"actor": "ana"}), &first),
            ("m2".to_string(), json!({"actor": "bo"}), &second),
            ("m3".to_string(), json!({"actor": "cy"}), &older),
        ];
        let view = merge_residents(&reports).unwrap();
        let assignments = view["assignments"].as_array().unwrap();
        assert_eq!(
            assignments.len(),
            2,
            "the newest status (m3, no residents) does not hide the others"
        );
        assert_eq!(
            assignments
                .iter()
                .find(|row| row["assignment_id"] == "alpha")
                .unwrap()["installation"],
            "m1"
        );
        assert_eq!(
            view["definitions"].as_array().unwrap().len(),
            1,
            "a shared definition appears once"
        );
        assert_eq!(
            view["inbox"]["approvals"][0]["installation"], "m2",
            "inbox rows say where a decision is applied"
        );
        assert_eq!(view["health"]["counters"]["ticks"], 20);
        assert_eq!(view["health"]["ignored"]["NO_MATCH"], 4);
        assert_eq!(view["health"]["queue_depth"], 2);
        assert_eq!(
            view["activations"][0]["activation_id"], "act_beta",
            "newest first"
        );
        assert_eq!(view["reported_by"]["actor"], "bo");
        let none = vec![("m3".to_string(), json!(null), &older)];
        assert_eq!(merge_residents(&none).unwrap_err().0, 404);
    }
}
