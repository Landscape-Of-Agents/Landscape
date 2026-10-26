// crane brick stack create | show (resolve) | activate | revoke. A Stack is a named, version-pinned
// composition of activated Bricks (one level: a Stack never contains a Stack, and is never a Brick
// type). Each member keeps its own identity, version, validation, and audit history; the resolved
// view shows every member, the combined controls, and conflicts. Combining only tightens.

use serde_json::json;

use super::{open, require_verified};
use crate::brick::compose::{controls, members};
use crate::brick::model::{
    Attachment, AttachmentState, BrickRef, Stack, StackEntry, Subject, Target, TargetKind,
    ValidationStatus, VersionState,
};
use crate::brick::store::{self, StackDraft};
use crate::brick::validate;
use crate::cli::protocol::{finish, parse, versioned, Response, Status};

/** Run 'crane brick stack create'
 * Input
    - args: &[String] - arguments after "stack create"
 * Output
    - Result<(), String>
*/
pub(super) fn create(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["name", "members", "id", "purpose", "owner"], &[])?;
    args.expect_positional(
        0,
        "crane brick stack create --name NAME --members ID@N,ID@N [--id ID] [--purpose TEXT]",
    )?;
    let name = args.value("name").ok_or("--name is required")?;
    let id = args.value("id").unwrap_or_else(|| {
        name.to_ascii_lowercase()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    });
    crate::platform::validate_name("Stack", &id)?;
    let members = crate::cli::family::list(args.value("members"))
        .iter()
        .map(|text| match versioned(text)? {
            (brick_id, Some(version)) => Ok(BrickRef { brick_id, version }),
            (id, None) => Err(format!("member {id} needs an exact version: {id}@N")),
        })
        .collect::<Result<Vec<_>, String>>()?;
    let repo = open()?;
    let by = store::author(&repo.workspace);
    let stack = Stack {
        stack_id: id.clone(),
        version: repo.store.next_version(&id, true),
        name,
        purpose: args.value("purpose").unwrap_or_default(),
        owner: args.value("owner"),
        members,
        created_by: by.clone(),
        created_at: crate::platform::now_millis(),
    };
    let validation = validate::stack(&stack, &repo.store);
    store::save_stack_draft(
        &repo.workspace,
        &StackDraft {
            stack: stack.clone(),
            updated_by: by,
            updated_at: crate::platform::now_millis(),
        },
    )?;
    let reference = format!("stack:{}@{}", stack.stack_id, stack.version);
    crate::platform::notes::note(
        "BRICK_STACK_SAVED",
        json!({"stack": reference, "members": stack.members.len(), "validation": validation.status}),
    );
    let detail = validation
        .problems
        .iter()
        .map(|problem| format!("  {}: {}", problem.code, problem.message))
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.stack.create",
        Status::Ok,
        format!(
            "saved Stack draft {reference} with {} member(s); validation {}",
            stack.members.len(),
            crate::cli::protocol::name(&validation.status)
        ),
    )
    .reference("stack_ref", reference)
    .reference("digest", stack.digest())
    .data(json!({"state": "draft", "definition": stack, "validation": validation}));
    finish(&args, response, &detail)
}

/** Find a Stack: "ID@N" is that activated version; "ID" is the draft, else the newest version
 * Input
    - repo: &super::Repo - repository
    - text: &str - reference (with or without "stack:")
 * Output
    - Result<(Stack, String), String> definition and its state
*/
fn find(repo: &super::Repo, text: &str) -> Result<(Stack, String), String> {
    let (id, version) = versioned(text.strip_prefix("stack:").unwrap_or(text))?;
    if let Some(version) = version {
        return repo
            .store
            .stack(&id, version)
            .map(|entry| {
                (
                    entry.stack.clone(),
                    crate::cli::protocol::name(&entry.state),
                )
            })
            .ok_or_else(|| format!("stack:{id}@{version} is not an activated Stack version"));
    }
    if let Some(draft) = store::stack_draft(&repo.workspace, &id)? {
        return Ok((draft.stack, "draft".into()));
    }
    repo.store
        .stacks
        .iter()
        .rfind(|entry| entry.stack.stack_id == id)
        .map(|entry| {
            (
                entry.stack.clone(),
                crate::cli::protocol::name(&entry.state),
            )
        })
        .ok_or_else(|| format!("no Stack '{id}'"))
}

/** Run 'crane brick stack show' (also 'resolve'): members, combined controls, and conflicts
 * Input
    - args: &[String] - arguments after "stack show"
 * Output
    - Result<(), String>
*/
pub(super) fn show(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane brick stack show ID[@N]")?;
    let repo = open()?;
    let (stack, state) = find(&repo, &args.positional[0])?;
    let validation = validate::stack(&stack, &repo.store);
    // Resolve as if attached, so the combined controls are the ones enforcement would apply
    let mut simulated = repo.store.clone();
    if !simulated
        .stacks
        .iter()
        .any(|entry| entry.stack.stack_id == stack.stack_id && entry.stack.version == stack.version)
    {
        simulated.stacks.push(StackEntry {
            stack: stack.clone(),
            digest: stack.digest(),
            state: VersionState::Active,
            activated_by: Default::default(),
            activated_at: 0,
            reason: "preview".into(),
            changed_by: None,
            changed_at: None,
            change_reason: None,
        });
    }
    let subject = Subject::Stack {
        stack_id: stack.stack_id.clone(),
        version: stack.version,
    };
    simulated.attachments = vec![Attachment {
        attachment_id: "preview".into(),
        target: Target {
            kind: TargetKind::Flow,
            id: "preview".into(),
        },
        subject: subject.clone(),
        current_context: Default::default(),
        state: AttachmentState::Enabled,
        attached_by: Default::default(),
        attached_at: 0,
    }];
    let resolved = members(&simulated, &subject)
        .into_iter()
        .map(|member| json!({
            "ref": member.reference,
            "type": member.entry.map(|entry| entry.brick.brick_type),
            "name": member.entry.map(|entry| entry.brick.name.clone()),
            "state": member.entry.map(|entry| entry.state),
            "issue": member.issue,
            "deprecated": member.deprecated,
            "common_context_bytes": member.entry.map(|entry| entry.brick.common_context.text.len()),
        }))
        .collect::<Vec<_>>();
    let combined = controls(&simulated, "flow:preview", &repo.workspace.root);
    crate::platform::notes::note(
        "BRICK_STACK_RESOLVED",
        json!({"stack": subject.text(), "members": resolved.len(), "validation": validation.status}),
    );
    let detail = resolved
        .iter()
        .map(|member| {
            format!(
                "  {} {} {}",
                member["ref"].as_str().unwrap_or_default(),
                member["type"].as_str().unwrap_or("?"),
                member["issue"].as_str().unwrap_or("")
            )
        })
        .chain(
            validation
                .problems
                .iter()
                .map(|problem| format!("  {}: {}", problem.code, problem.message)),
        )
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.stack.show",
        if validation.status == ValidationStatus::Passed { Status::Ok } else { Status::Failed },
        format!("{} ({state}): {} member(s)", subject.text(), resolved.len()),
    )
    .reference("stack_ref", subject.text())
    .reference("digest", stack.digest())
    .data(json!({"definition": stack, "state": state, "members": resolved, "controls": combined, "validation": validation}));
    finish(&args, response, &detail)
}

/** Run 'crane brick stack activate'
 * Input
    - args: &[String] - arguments after "stack activate"
 * Output
    - Result<(), String>
*/
pub(super) fn activate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &["dry-run"])?;
    args.expect_positional(1, "crane brick stack activate ID --reason TEXT [--dry-run]")?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("--reason is required")?;
    let repo = open()?;
    let id = args.positional[0].trim_start_matches("stack:").to_string();
    let draft = store::stack_draft(&repo.workspace, &id)?
        .ok_or_else(|| format!("no Stack draft '{id}'"))?;
    let stack = draft.stack;
    let validation = validate::stack(&stack, &repo.store);
    let reference = format!("stack:{}@{}", stack.stack_id, stack.version);
    if validation.status != ValidationStatus::Passed {
        let response = Response::new(
            "brick.stack.activate",
            Status::Failed,
            format!("{reference} does not validate"),
        )
        .reasons(
            validation
                .problems
                .iter()
                .map(|problem| problem.code.clone()),
        )
        .data(json!({"validation": validation}));
        return finish(&args, response, &[]);
    }
    let entry = StackEntry {
        digest: stack.digest(),
        stack: stack.clone(),
        state: VersionState::Active,
        activated_by: store::author(&repo.workspace),
        activated_at: crate::platform::now_millis(),
        reason,
        changed_by: None,
        changed_at: None,
        change_reason: None,
    };
    if args.flag("dry-run") {
        let response = Response::new(
            "brick.stack.activate",
            Status::Ok,
            format!("would activate {reference} (dry run; nothing signed)"),
        )
        .data(json!({"dry_run": true, "entry": entry}));
        return finish(&args, response, &[]);
    }
    require_verified(&repo)?;
    let digest = entry.digest.clone();
    store::change(
        &repo.workspace,
        "crane brick stack activate",
        "brick.stack_activated",
        json!({"stack_id": stack.stack_id, "version": stack.version, "digest": digest, "members": stack.members.iter().map(ToString::to_string).collect::<Vec<_>>()}),
        |store, _| {
            store.stacks.push(entry);
            Ok(())
        },
    )?;
    store::remove_draft(&repo.workspace, "stacks", &stack.stack_id);
    crate::platform::notes::note(
        "BRICK_STACK_ACTIVATED",
        json!({"stack": reference, "digest": digest}),
    );
    let response = Response::new(
        "brick.stack.activate",
        Status::Ok,
        format!("activated {reference} and signed it into .crane/bricks.json"),
    )
    .reference("stack_ref", reference)
    .reference("digest", digest);
    finish(&args, response, &[])
}

/** Run 'crane brick stack revoke'
 * Input
    - args: &[String] - arguments after "stack revoke"
 * Output
    - Result<(), String>
*/
pub(super) fn revoke(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane brick stack revoke ID@N --reason TEXT")?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("--reason is required")?;
    let (id, version) = match versioned(args.positional[0].trim_start_matches("stack:"))? {
        (id, Some(version)) => (id, version),
        (id, None) => return Err(format!("name an exact version: stack:{id}@N")),
    };
    let repo = open()?;
    if repo
        .store
        .stack(&id, version)
        .is_none_or(|entry| entry.state == VersionState::Revoked)
    {
        return Err(format!(
            "stack:{id}@{version} is not an active Stack version"
        ));
    }
    require_verified(&repo)?;
    let by = store::author(&repo.workspace);
    store::change(
        &repo.workspace,
        "crane brick stack revoke",
        "brick.stack_revoked",
        json!({"stack_id": id, "version": version}),
        |store, _| {
            if let Some(entry) = store
                .stacks
                .iter_mut()
                .find(|entry| entry.stack.stack_id == id && entry.stack.version == version)
            {
                entry.state = VersionState::Revoked;
                entry.changed_by = Some(by);
                entry.changed_at = Some(crate::platform::now_millis());
                entry.change_reason = Some(reason);
            }
            Ok(())
        },
    )?;
    crate::platform::notes::note(
        "BRICK_STACK_REVOKED",
        json!({"stack": format!("stack:{id}@{version}")}),
    );
    let response = Response::new("brick.stack.revoke", Status::Ok, format!("revoked stack:{id}@{version}; attachments of it now fail closed until detached or upgraded"))
        .reference("stack_ref", format!("stack:{id}@{version}"));
    finish(&args, response, &[])
}
