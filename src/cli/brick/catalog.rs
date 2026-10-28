// crane brick types | list | show | history: the fixed type catalog, the library of drafts and
// activated versions, one Brick's detail (definition, versions, attachments, Stacks, recent use),
// and the signed audit trail of Brick changes. Read-only.

use serde_json::{json, Value};

use super::{find, open, target_option, trust_word, Found};
use crate::brick::model::{entry_summary, AttachmentState, Subject};
use crate::brick::store;
use crate::brick::types::BrickType;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run 'crane brick types'
 * Input
    - args: &[String] - arguments after "types"
 * Output
    - Result<(), String>
*/
pub(super) fn types(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane brick types")?;
    let catalog = BrickType::ALL.map(BrickType::catalog);
    let detail = catalog
        .iter()
        .map(|info| {
            format!(
                "  {:<13} {:<9} {}",
                info.brick_type.name(),
                crate::cli::protocol::name(&info.support),
                info.runtime
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.types",
        Status::Ok,
        format!(
            "{} fixed Brick types; new types are a platform change, not a setting",
            catalog.len()
        ),
    )
    .data(json!({"types": catalog, "custom_types": false}));
    finish(&args, response, &detail)
}

/** The attachments that use a Brick (directly or through a Stack)
 * Input
    - store: &store::BrickStore - signed store
    - brick_id: &str - Brick
 * Output
    - Vec<Value>
*/
fn uses(store: &store::BrickStore, brick_id: &str) -> Vec<Value> {
    store
        .attachments
        .iter()
        .filter_map(|attachment| {
            let version = match &attachment.subject {
                Subject::Brick {
                    brick_id: id,
                    version,
                } if id == brick_id => Some((*version, None)),
                Subject::Stack { stack_id, version } => store
                    .stack(stack_id, *version)
                    .and_then(|entry| {
                        entry
                            .stack
                            .members
                            .iter()
                            .find(|member| member.brick_id == brick_id)
                    })
                    .map(|member| (member.version, Some(attachment.subject.text()))),
                _ => None,
            }?;
            Some(json!({
                "attachment_id": attachment.attachment_id,
                "target": attachment.target.key(),
                "version": version.0,
                "via": version.1,
                "state": attachment.state,
                "current_context_bytes": attachment.current_context.text.len(),
            }))
        })
        .collect()
}

/** Run 'crane brick list'
 * Input
    - args: &[String] - arguments after "list"
 * Output
    - Result<(), String>
*/
pub(super) fn list(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["type", "state", "target", "flow", "zone"], &[])?;
    args.expect_positional(
        0,
        "crane brick list [--type TYPE] [--state STATE] [--target KEY]",
    )?;
    let wanted_type = args
        .value("type")
        .map(|text| BrickType::parse_any(&text))
        .transpose()?;
    let wanted_state = args.value("state");
    let wanted_target = target_option(&args)?.map(|target| target.key());
    let repo = open()?;
    let (drafts, unreadable) = store::brick_drafts(&repo.workspace);
    let mut rows = Vec::new();
    for draft in &drafts {
        rows.push(json!({
            "ref": draft.brick.brick_id,
            "brick_id": draft.brick.brick_id,
            "version": draft.brick.version,
            "type": draft.brick.brick_type,
            "name": draft.brick.name,
            "intent": draft.brick.intent,
            "owner": draft.brick.owner,
            "departments": draft.brick.departments,
            "state": "draft",
            "digest": draft.brick.digest(),
            "validation": draft.validation.status,
            "validated_digest_current": draft.validation.digest == draft.brick.digest(),
            "updated_by": draft.updated_by.name,
            "updated_at": draft.updated_at,
            "attachments": [],
        }));
    }
    for entry in &repo.store.bricks {
        let mut row = entry_summary(entry);
        row["attachments"] = json!(uses(&repo.store, &entry.brick.brick_id)
            .into_iter()
            .filter(|used| used["version"] == entry.brick.version)
            .collect::<Vec<_>>());
        rows.push(row);
    }
    rows.retain(|row| {
        wanted_type.is_none_or(|brick_type| row["type"] == json!(brick_type))
            && wanted_state
                .as_ref()
                .is_none_or(|state| row["state"] == state.as_str())
            && wanted_target.as_ref().is_none_or(|target| {
                row["attachments"]
                    .as_array()
                    .is_some_and(|used| used.iter().any(|used| used["target"] == target.as_str()))
            })
    });
    let stacks = repo
        .store
        .stacks
        .iter()
        .map(|entry| json!({
            "ref": format!("stack:{}@{}", entry.stack.stack_id, entry.stack.version),
            "stack_id": entry.stack.stack_id, "version": entry.stack.version, "name": entry.stack.name,
            "purpose": entry.stack.purpose, "state": entry.state, "digest": entry.digest,
            "members": entry.stack.members.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }))
        .chain(store::stack_drafts(&repo.workspace).0.iter().map(|draft| json!({
            "ref": format!("stack:{}", draft.stack.stack_id),
            "stack_id": draft.stack.stack_id, "version": draft.stack.version, "name": draft.stack.name,
            "purpose": draft.stack.purpose, "state": "draft", "digest": draft.stack.digest(),
            "members": draft.stack.members.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })))
        .collect::<Vec<_>>();
    let attachments = repo
        .store
        .attachments
        .iter()
        .map(|attachment| json!({
            "attachment_id": attachment.attachment_id,
            "target": attachment.target.key(),
            "subject": attachment.subject.text(),
            "state": attachment.state,
            "current_context": attachment.current_context.text,
            "current_context_updated_by": attachment.current_context.updated_by.name,
            "current_context_updated_at": attachment.current_context.updated_at,
            "attached_by": attachment.attached_by.name,
            "attached_at": attachment.attached_at,
            "issues": crate::brick::compose::members(&repo.store, &attachment.subject)
                .into_iter()
                .filter_map(|member| member.issue.map(|issue| format!("{} {issue}", member.reference)))
                .collect::<Vec<_>>(),
        }))
        .collect::<Vec<_>>();
    let detail = rows
        .iter()
        .map(|row| {
            format!(
                "  {:<28} {:<13} {:<10} {}",
                row["ref"].as_str().unwrap_or_default(),
                row["type"].as_str().unwrap_or_default(),
                row["state"].as_str().unwrap_or_default(),
                row["name"].as_str().unwrap_or_default()
            )
        })
        .chain(attachments.iter().map(|attachment| {
            format!(
                "  attachment {} {} -> {} ({})",
                attachment["attachment_id"].as_str().unwrap_or_default(),
                attachment["subject"].as_str().unwrap_or_default(),
                attachment["target"].as_str().unwrap_or_default(),
                attachment["state"].as_str().unwrap_or_default()
            )
        }))
        .chain(
            unreadable
                .iter()
                .map(|problem| format!("  unreadable draft: {problem}")),
        )
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.list",
        Status::Ok,
        format!(
            "{} Bricks, {} Stacks, {} attachments (store {})",
            rows.len(),
            stacks.len(),
            attachments.len(),
            trust_word(&repo.trust)
        ),
    )
    .reference("store_digest", repo.state.bricks_digest.clone())
    .data(json!({
        "bricks": rows,
        "stacks": stacks,
        "attachments": attachments,
        "assignments": repo.store.assignments,
        "store": {"trust": trust_word(&repo.trust), "digest": repo.state.bricks_digest, "generation": repo.state.generation()},
        "unreadable_drafts": unreadable,
    }));
    finish(&args, response, &detail)
}

/** Run 'crane brick show'
 * Input
    - args: &[String] - arguments after "show"
 * Output
    - Result<(), String>
*/
pub(super) fn show(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane brick show ID[@N]")?;
    let repo = open()?;
    let found = find(&repo, &args.positional[0])?;
    let (brick, state, validation, digest) = match &found {
        Found::Draft(draft) => (
            &draft.brick,
            json!("draft"),
            json!(draft.validation),
            draft.brick.digest(),
        ),
        Found::Version(entry) => (
            &entry.brick,
            json!(entry.state),
            json!(entry.validation),
            entry.digest.clone(),
        ),
    };
    let parsed = crate::brick::config::parse(brick.brick_type, &brick.config);
    let versions = repo
        .store
        .versions(&brick.brick_id)
        .into_iter()
        .map(entry_summary)
        .collect::<Vec<_>>();
    let stacks = repo
        .store
        .stacks
        .iter()
        .filter(|entry| {
            entry
                .stack
                .members
                .iter()
                .any(|member| member.brick_id == brick.brick_id)
        })
        .map(|entry| {
            format!(
                "stack:{}@{} ({:?})",
                entry.stack.stack_id, entry.stack.version, entry.state
            )
            .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    let used = uses(&repo.store, &brick.brick_id);
    // Recent use: decisions the hooks bound to this Brick (the guard binds "brick:ID@N")
    let prefix = format!("brick:{}@", brick.brick_id);
    let since = crate::platform::now_millis().saturating_sub(7 * 86_400_000);
    let events = repo
        .workspace
        .trust()
        .map(|trust| {
            crate::telemetry::Stores::open(&trust.runtime())
                .events
                .read_all()
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let mut decisions = std::collections::BTreeMap::<String, u64>::new();
    for event in events.iter().filter(|event| {
        event.occurred_at >= since
            && event
                .bindings
                .zones
                .iter()
                .any(|entity| entity.starts_with(&prefix))
    }) {
        let decision = event.decision.map_or("NONE".to_string(), |decision| {
            crate::cli::protocol::name(&decision)
        });
        *decisions.entry(decision).or_default() += 1;
    }
    let info = brick.brick_type.catalog();
    let detail = vec![
        format!("  {} ({}) — {}", brick.name, info.label, brick.intent),
        format!(
            "  support: {} — {}",
            crate::cli::protocol::name(&info.support),
            info.runtime
        ),
        format!("  digest {}", &digest[..16.min(digest.len())]),
        format!(
            "  attached {} time(s); in {} Stack(s)",
            used.len(),
            stacks.len()
        ),
    ];
    let response = Response::new(
        "brick.show",
        Status::Ok,
        format!(
            "{}@{} ({})",
            brick.brick_id,
            brick.version,
            state.as_str().unwrap_or_default()
        ),
    )
    .reference("brick_ref", format!("{}@{}", brick.brick_id, brick.version))
    .reference("digest", digest.clone())
    .data(json!({
        "definition": brick,
        "state": state,
        "digest": digest,
        "validation": validation,
        "config": parsed.as_ref().ok(),
        "config_problem": parsed.as_ref().err(),
        "type_info": info,
        "versions": versions,
        "attachments": used,
        "stacks": stacks,
        "usage": {"window_days": 7, "decisions": decisions},
    }));
    finish(&args, response, &detail)
}

/** Run 'crane brick history': the signed audit entries of Brick changes
 * Input
    - args: &[String] - arguments after "history"
 * Output
    - Result<(), String>
*/
pub(super) fn history(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    if args.positional.len() > 1 {
        return Err("usage: crane brick history [ID]".into());
    }
    let repo = open()?;
    let wanted = args.positional.first().cloned();
    let entries = repo
        .state
        .audit
        .iter()
        .filter(|entry| entry.action.starts_with("brick."))
        .filter(|entry| {
            wanted.as_ref().is_none_or(|id| {
                let detail = entry.detail.to_string();
                detail.contains(&format!("\"{id}\"")) || detail.contains(&format!("{id}@"))
            })
        })
        .map(|entry| json!({"seq": entry.seq, "at": entry.at, "actor": entry.actor, "action": entry.action, "detail": entry.detail, "digest": entry.digest}))
        .collect::<Vec<_>>();
    let detail = entries
        .iter()
        .map(|entry| {
            format!(
                "  #{} {} {} {}",
                entry["seq"],
                entry["action"].as_str().unwrap_or_default(),
                entry["actor"].as_str().unwrap_or_default(),
                entry["detail"]
            )
        })
        .collect::<Vec<_>>();
    let enabled = repo
        .store
        .attachments
        .iter()
        .filter(|attachment| attachment.state == AttachmentState::Enabled)
        .count();
    let response = Response::new(
        "brick.history",
        Status::Ok,
        format!(
            "{} signed Brick changes; {enabled} enabled attachments",
            entries.len()
        ),
    )
    .data(json!({"entries": entries}));
    finish(&args, response, &detail)
}
