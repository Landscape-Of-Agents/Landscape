// The Brick part of the repository status a machine reports to the DataSea dashboard
// (foxx::status): the library (drafts and activated versions), Stacks, attachments, assignments
// with their runtime status, monitors, the context cache, and the Zones and Flows Bricks can attach
// to. Besides identifiers, states, digests, sizes, and counts, each Brick's configuration and its
// Common Context text, and each attachment's Current Context text, are sent so DataSea can show and
// edit them (changes come back as requests a person applies here: cli/brick/inbox.rs). A text or
// configuration that looks like it holds a secret is withheld (only its digest and size are sent),
// and a text longer than SHARED_TEXT_LIMIT is cut and marked, so the dashboard does not edit it.

use serde_json::{json, Value};

use super::compose::{controls, members};
use super::model::entry_summary;
use super::store::{self, Trust};
use super::types::BrickType;
use crate::governance::workspace::Workspace;
use crate::trust::crypto::digest;

/** The longest context text sent to the dashboard (bytes) */
pub(crate) const SHARED_TEXT_LIMIT: usize = 16 * 1024;

/** A context text as the dashboard receives it
 * Input
    - text: &str - Common or Current Context
 * Output
    - Value {text, withheld, truncated}: text is null when it looks like it holds a secret
*/
fn shared_text(text: &str) -> Value {
    if crate::telemetry::redact::redact(text).1 > 0 {
        return json!({"text": null, "withheld": true, "truncated": false});
    }
    let mut end = text.len().min(SHARED_TEXT_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({"text": &text[..end], "withheld": false, "truncated": end < text.len()})
}

/** A Brick's editable fields as the dashboard receives them: description, configuration (withheld
 * when it looks like it holds a secret), dependencies, and Common Context
 * Input
    - brick: &super::model::Brick - definition
 * Output
    - Value
*/
fn shared_definition(brick: &super::model::Brick) -> Value {
    let config_text = serde_json::to_string(&brick.config).unwrap_or_default();
    let config_withheld = crate::telemetry::redact::redact(&config_text).1 > 0;
    json!({
        "description": brick.description,
        "config": if config_withheld { Value::Null } else { brick.config.clone() },
        "config_withheld": config_withheld,
        "dependencies": brick.dependencies.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "common_context": shared_text(&brick.common_context.text),
    })
}

/** The Zones and Flows of the repository, with their lifecycle (what Bricks can attach to)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Vec<Value> [{target, name, status}]
*/
fn targets(workspace: &Workspace) -> Vec<Value> {
    let Ok(governance) = crate::governance::zones::load(workspace) else {
        return Vec::new();
    };
    let lifecycle = |status: crate::governance::zones::Lifecycle| {
        serde_json::to_value(status).unwrap_or(Value::Null)
    };
    governance
        .zones
        .iter()
        .map(
            |zone| json!({"target": format!("zone:{}", zone.id), "status": lifecycle(zone.status)}),
        )
        .chain(governance.flows.iter().map(
            |flow| json!({"target": format!("flow:{}", flow.id), "status": lifecycle(flow.status)}),
        ))
        .collect()
}

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
            row["definition"] = shared_definition(&entry.brick);
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
                "sensitivity": draft.brick.common_context.sensitivity,
                "definition": shared_definition(&draft.brick),
            })
        }))
        .collect::<Vec<_>>();
    let stacks = store
        .stacks
        .iter()
        .map(|entry| json!({
            "ref": format!("stack:{}@{}", entry.stack.stack_id, entry.stack.version),
            "name": entry.stack.name, "purpose": entry.stack.purpose, "state": entry.state,
            "stack_id": entry.stack.stack_id, "version": entry.stack.version,
            "digest": entry.digest, "members": entry.stack.members.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }))
        .chain(store::stack_drafts(workspace).0.iter().map(|draft| json!({
            "ref": draft.stack.stack_id, "stack_id": draft.stack.stack_id, "version": draft.stack.version,
            "name": draft.stack.name, "purpose": draft.stack.purpose, "state": "draft",
            "members": draft.stack.members.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "updated_by": draft.updated_by.name, "updated_at": draft.updated_at,
        })))
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
                "current_context": shared_text(&attachment.current_context.text),
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
        "targets": targets(workspace),
        "history": state.audit.iter().filter(|entry| entry.action.starts_with("brick.")).rev().take(100).map(|entry| json!({"seq": entry.seq, "at": entry.at, "actor": entry.actor, "action": entry.action, "detail": entry.detail})).collect::<Vec<_>>(),
    })
}
