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
    (200, view, None)
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
