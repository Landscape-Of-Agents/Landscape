// The Brick part of the repository status a machine reports to the DataSea dashboard
// (foxx::status): the library (drafts and activated versions), Stacks, attachments, assignments
// with their runtime status, monitors, and the context cache. Only identifiers, states, digests,
// sizes, and counts leave the machine; Common and Current Context text stays in the repository, so
// the dashboard shows that a context exists, how large it is, and which digest agents received.

use serde_json::{json, Value};

use super::compose::{controls, members};
use super::model::entry_summary;
use super::store::{self, Trust};
use super::types::BrickType;
use crate::governance::workspace::Workspace;
use crate::trust::crypto::digest;

/** Collect the Brick report of a repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Value (an {error} object when the store cannot be read)
*/
pub(crate) fn collect(workspace: &Workspace) -> Value {
    let (state, store, trust) = match store::load(workspace) {
        Ok(loaded) => loaded,
        Err(error) => return json!({"error": error}),
    };
    let runtime = workspace.trust().map(|trust| trust.runtime()).ok();
    let (drafts, unreadable) = store::brick_drafts(workspace);
    let library = store
        .bricks
        .iter()
        .map(|entry| {
            let mut row = entry_summary(entry);
            row["common_context_digest"] = json!(digest(super::model::CONTEXT_DOMAIN, &[entry.brick.common_context.text.as_bytes()]));
            row["common_context_bytes"] = json!(entry.brick.common_context.text.len());
            row["sensitivity"] = json!(entry.brick.common_context.sensitivity);
            row["problems"] = json!(entry.validation.problems.len());
            row
        })
        .chain(drafts.iter().map(|draft| {
            json!({
                "ref": draft.brick.brick_id, "brick_id": draft.brick.brick_id, "version": draft.brick.version,
                "type": draft.brick.brick_type, "name": draft.brick.name, "intent": draft.brick.intent,
                "owner": draft.brick.owner, "departments": draft.brick.departments, "state": "draft",
                "digest": draft.brick.digest(), "validation": draft.validation.status,
                "validated_digest_current": draft.validation.digest == draft.brick.digest(),
                "problems": draft.validation.problems.len(),
                "updated_by": draft.updated_by.name, "updated_at": draft.updated_at,
                "common_context_bytes": draft.brick.common_context.text.len(),
            })
        }))
        .collect::<Vec<_>>();
    let stacks = store
        .stacks
        .iter()
        .map(|entry| json!({
            "ref": format!("stack:{}@{}", entry.stack.stack_id, entry.stack.version),
            "name": entry.stack.name, "purpose": entry.stack.purpose, "state": entry.state,
            "digest": entry.digest, "members": entry.stack.members.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>();
    let attachments = store
        .attachments
        .iter()
        .map(|attachment| {
            let combined = controls(&store, &attachment.target.key(), &workspace.root);
            json!({
                "attachment_id": attachment.attachment_id,
                "target": attachment.target.key(),
                "subject": attachment.subject.text(),
                "state": attachment.state,
                "current_context_digest": digest(super::model::CONTEXT_DOMAIN, &[attachment.current_context.text.as_bytes()]),
                "current_context_bytes": attachment.current_context.text.len(),
                "current_context_updated_by": attachment.current_context.updated_by.name,
                "current_context_updated_at": attachment.current_context.updated_at,
                "attached_by": attachment.attached_by.name,
                "attached_at": attachment.attached_at,
                "issues": members(&store, &attachment.subject).into_iter().filter_map(|member| member.issue.map(|issue| format!("{} {issue}", member.reference))).collect::<Vec<_>>(),
                "target_restricted": combined.restricts(),
                "gaps": combined.gaps.len(),
            })
        })
        .collect::<Vec<_>>();
    let (assignments, monitors, cache) = match &runtime {
        Some(runtime) => (
            super::runtime::assignments(workspace, runtime).unwrap_or_default(),
            super::runtime::monitor_status(workspace, runtime),
            super::context::status(runtime),
        ),
        None => (Vec::new(), Vec::new(), Value::Null),
    };
    // The cache without per-session detail: counts and the remembered snapshots
    let sessions = cache["sessions"].as_array().cloned().unwrap_or_default();
    let delivered = sessions
        .iter()
        .map(|session| session["deliveries"].as_array().map_or(0, Vec::len))
        .sum::<usize>();
    let reused = sessions
        .iter()
        .flat_map(|session| {
            session["deliveries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|delivery| delivery["reused"].as_u64().unwrap_or(0))
        .sum::<u64>();
    json!({
        "trust": match &trust { Trust::Absent => "absent".to_string(), Trust::Verified => "verified".into(), Trust::Unverified(reason) => format!("unverified: {reason}") },
        "store_digest": state.bricks_digest,
        "generation": state.generation(),
        "types": BrickType::ALL.map(BrickType::catalog),
        "bricks": library,
        "stacks": stacks,
        "attachments": attachments,
        "assignments": assignments,
        "monitors": monitors,
        "context": {
            "snapshots": cache["snapshots"],
            "sessions": sessions.len(),
            "deliveries": delivered,
            "reused": reused,
            "provider_caching": "not measured",
        },
        "unreadable_drafts": unreadable,
        "history": state.audit.iter().filter(|entry| entry.action.starts_with("brick.")).rev().take(100).map(|entry| json!({"seq": entry.seq, "at": entry.at, "actor": entry.actor, "action": entry.action, "detail": entry.detail})).collect::<Vec<_>>(),
    })
}
