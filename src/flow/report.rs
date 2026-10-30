// The flow part of the repository status a machine reports to the DataSea dashboard (foxx::status):
// every zone and flow with what the dashboard draws (stages, criticality, ceiling, status, Bricks
// attached, files covered), each flow's knowledge (priority and routed model, people, context,
// tests with their last results, approved trace and its kept call graph), the trace drafts under review with the current
// revision's functions and calls and its difference from the revision before, the proposals agents
// made, subagents (watchers) with their subtype, configuration, pending change and runs, the
// subagent catalog, which integrations this machine has connected, incidents, the organization's
// models, routing, and limits. Context and test text
// that looks like it holds a secret is withheld, and long texts are cut.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::model::{route, FlowBook, Priority};
use super::{trace, watch};
use crate::governance::workspace::Workspace;

/** Longest text of one item the dashboard receives (bytes) */
const TEXT_LIMIT: usize = 4 * 1024;

/** Most functions of a draft's current revision the dashboard receives */
const MAX_NODES: usize = 400;

/** A text as the dashboard receives it
 * Input
    - text: &str - text
 * Output
    - Value - the text, or null with withheld true when it looks like a secret
*/
fn shared(text: &str) -> Value {
    if crate::telemetry::redact::redact(text).1 > 0 {
        return json!({"text": null, "withheld": true});
    }
    let mut end = text.len().min(TEXT_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({"text": &text[..end], "withheld": false, "truncated": end < text.len()})
}

/** A call graph as the dashboard receives it: at most MAX_NODES functions and the calls into them
 * Input
    - graph: &trace::TraceGraph - graph
 * Output
    - Value
*/
fn graph_view(graph: &trace::TraceGraph) -> Value {
    let kept = graph.nodes.iter().take(MAX_NODES).collect::<Vec<_>>();
    json!({
        "entry_points": graph.entry_points,
        "nodes": kept,
        "edges": graph.edges.iter().filter(|edge| kept.iter().any(|node| node.id == edge.to)).collect::<Vec<_>>(),
        "unresolved": graph.unresolved.iter().take(100).collect::<Vec<_>>(),
        "files": graph.files, "status": graph.status,
        "truncated": graph.nodes.len() > MAX_NODES,
    })
}

/** A name of an enum value as serde writes it
 * Input
    - value: impl serde::Serialize - value
 * Output
    - String
*/
fn name(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/** Collect the flow report of a repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Value (an {error} object when the governance cannot be read)
*/
pub(crate) fn collect(workspace: &Workspace) -> Value {
    let governance = match crate::governance::zones::load(workspace) {
        Ok(governance) => governance,
        Err(error) => return json!({"error": error}),
    };
    let state = match crate::governance::state::State::load(workspace) {
        Ok(state) => state,
        Err(error) => return json!({"error": error}),
    };
    let book = match FlowBook::of(&state) {
        Ok(book) => book,
        Err(error) => return json!({"error": error}),
    };
    let runtime = workspace.trust().map(|trust| trust.runtime()).ok();
    let results = runtime
        .as_deref()
        .map(watch::test_results)
        .unwrap_or_default();
    let watchers = runtime.as_deref().map(watch::watchers).unwrap_or_default();
    let incidents = runtime.as_deref().map(watch::incidents).unwrap_or_default();
    // Files and Bricks per target, from the resolution the hooks use
    let resolution = crate::governance::policy::PolicySet::load(workspace)
        .ok()
        .and_then(|policies| {
            crate::governance::zones::resolve(workspace, &governance, &state, &policies).ok()
        });
    let files = resolution
        .as_ref()
        .map(|resolution| {
            resolution
                .entities
                .iter()
                .map(|entity| (format!("{}:{}", entity.kind, entity.id), entity.files.len()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let bricks = crate::brick::store::BrickStore::of(&state)
        .map(|store| {
            let mut by_target: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for attachment in &store.attachments {
                by_target
                    .entry(attachment.target.key())
                    .or_default()
                    .push(attachment.subject.text());
            }
            by_target
        })
        .unwrap_or_default();
    let zones = governance
        .zones
        .iter()
        .map(|zone| {
            let key = format!("zone:{}", zone.id);
            json!({
                "id": zone.id, "key": key, "description": zone.description,
                "criticality": name(zone.criticality), "ceiling": zone.autonomy_ceiling.name(), "status": name(zone.status),
                "files": files.get(&key), "bricks": bricks.get(&key).cloned().unwrap_or_default(),
                "owner": zone.owner,
            })
        })
        .collect::<Vec<_>>();
    let flows = governance
        .flows
        .iter()
        .map(|flow| {
            let key = format!("flow:{}", flow.id);
            let knowledge = book.flows.get(&flow.id).cloned().unwrap_or_default();
            let routed = knowledge.priority.map(|priority| {
                let (tiers, chosen) = route(&book, priority, None);
                json!({"tiers": tiers.iter().map(|tier| tier.name()).collect::<Vec<_>>(), "model": chosen.map(|entry| &entry.model), "provider": chosen.map(|entry| &entry.provider)})
            });
            let tests = knowledge
                .tests
                .iter()
                .map(|test| {
                    let result = results.get(&flow.id).and_then(|tests| tests.get(&test.id));
                    json!({
                        "id": test.id, "path": test.path, "name": test.name,
                        "description": shared(&test.description), "assumptions": test.assumptions.iter().map(|item| shared(item)).collect::<Vec<_>>(),
                        "mock_data": shared(&test.mock_data), "command": test.command.as_deref().map(shared), "required": test.required,
                        "proposed_by": test.proposed_by, "approved_by": test.approved_by, "approved_at": test.approved_at,
                        "exists": workspace.root.join(&test.path).is_file(),
                        "last_result": result.map(|result| result["status"].clone()), "last_run": result.map(|result| result["at"].clone()),
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "id": flow.id, "key": key, "description": flow.description, "entry_points": flow.entry_points,
                "stages": flow.stages.iter().map(|stage| stage.id.clone()).collect::<Vec<_>>(),
                "criticality": name(flow.criticality), "ceiling": flow.autonomy_ceiling.name(), "status": name(flow.status),
                "max_depth": flow.max_depth.unwrap_or(trace::DEFAULT_DEPTH), "owner": flow.owner,
                "files": files.get(&key), "bricks": bricks.get(&key).cloned().unwrap_or_default(),
                "priority": knowledge.priority.map(Priority::name), "enforce_model": knowledge.enforce_model, "routed": routed,
                "people": knowledge.people,
                "context": knowledge.context.iter().map(|item| json!({
                    "id": item.id, "kind": item.kind.name(), "title": item.title, "value": shared(&item.value),
                    "unit": item.unit, "threshold": item.threshold, "file": item.file, "lines": item.lines,
                    "added_by": item.added_by, "added_at": item.added_at,
                })).collect::<Vec<_>>(),
                "tests": tests,
                "trace": knowledge.trace,
                // The approved trace's functions and calls (kept since graphs are kept at approval)
                "graph": knowledge.trace.as_ref().and_then(|approved| trace::load_approved(workspace, &flow.id, &approved.digest)).map(|graph| graph_view(&graph)),
                "watchers": {
                    "active": watchers.iter().filter(|watcher| watcher.active && watcher.flow == flow.id).count(),
                    "limit": book.watcher_limit(&flow.id),
                },
                "incidents": incidents.iter().filter(|incident| incident.flow == flow.id).count(),
            })
        })
        .collect::<Vec<_>>();
    let (drafts, unreadable) = trace::drafts(workspace);
    let drafts = drafts
        .iter()
        .map(|draft| {
            let current = draft.current();
            let previous = current.and_then(|current| {
                draft
                    .revisions
                    .iter()
                    .filter(|revision| revision.revision < current.revision)
                    .max_by_key(|revision| revision.revision)
            });
            json!({
                "flow_id": draft.flow_id, "description": shared(&draft.description), "current": draft.current,
                "requested_by": draft.requested_by, "via": draft.via, "created_at": draft.created_at, "updated_at": draft.updated_at,
                "exists": governance.flows.iter().any(|flow| flow.id == draft.flow_id),
                "revisions": draft.revisions.iter().map(|revision| json!({
                    "revision": revision.revision, "from": revision.from, "max_depth": revision.max_depth, "digest": revision.digest,
                    "nodes": revision.graph.nodes.len(), "edges": revision.graph.edges.len(), "files": revision.graph.files.len(),
                    "status": revision.graph.status, "by": revision.by, "reason": revision.reason, "traced_at": revision.traced_at,
                })).collect::<Vec<_>>(),
                "graph": current.map(|revision| graph_view(&revision.graph)),
                "diff": match (previous, current) {
                    (Some(previous), Some(current)) => Some(trace::diff(&previous.graph, &current.graph)),
                    _ => None,
                },
            })
        })
        .collect::<Vec<_>>();
    let proposals = watch::proposals(workspace)
        .into_iter()
        .rev()
        .take(200)
        .map(|proposal| {
            let mut payload = proposal.payload.clone();
            if let Some(object) = payload.as_object_mut() {
                for (_, value) in object.iter_mut() {
                    if let Some(text) = value.as_str() {
                        *value = shared(text);
                    }
                }
            }
            json!({
                "proposal_id": proposal.proposal_id, "kind": proposal.kind, "flow": proposal.flow, "payload": payload,
                "proposed_by": proposal.proposed_by, "proposed_at": proposal.proposed_at, "status": proposal.status,
                "decided_by": proposal.decided_by, "decided_at": proposal.decided_at, "reason": proposal.reason,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "zones": zones,
        "flows": flows,
        "drafts": drafts,
        "unreadable_drafts": unreadable,
        "proposals": proposals,
        "watchers": watchers,
        "incidents": incidents.iter().take(100).collect::<Vec<_>>(),
        "models": book.models,
        "routing": Priority::ALL.iter().map(|priority| {
            let (tiers, chosen) = route(&book, *priority, None);
            json!({"priority": priority.name(), "tiers": tiers.iter().map(|tier| tier.name()).collect::<Vec<_>>(), "model": chosen.map(|entry| &entry.model)})
        }).collect::<Vec<_>>(),
        "limits": book.limits,
        // The subtypes and actions subagents may have, and which integrations this machine has
        "subagent_catalog": super::subagent::catalog(),
        "connections": super::subagent::connections(workspace),
        "store_digest": state.flows_digest,
        "channel": super::agent::HELP,
    })
}
