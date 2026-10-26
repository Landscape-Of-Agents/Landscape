// crane brick activate | deprecate | revoke. Activation signs a draft into the store as an
// immutable version after validating it again (the draft's stored result is never trusted). The
// author cannot activate their own Brick while the repository has another admin who can review it;
// a repository with a single signer may, and the version records that it was self-reviewed.
// Deprecation keeps existing pins working and refuses new attachments; revocation withdraws the
// version: pins to it fail closed (changes in the target need approval) until an admin detaches or
// upgrades them, so a revoked restriction never silently disappears.

use serde_json::json;

use super::{find, open, require_verified, Found};
use crate::brick::model::{BrickEntry, BrickRef, Subject, ValidationStatus, VersionState};
use crate::brick::store;
use crate::brick::validate;
use crate::cli::protocol::{finish, parse, versioned, Response, Status};

/** Run 'crane brick activate'
 * Input
    - args: &[String] - arguments after "activate"
 * Output
    - Result<(), String>
*/
pub(super) fn activate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &["dry-run"])?;
    args.expect_positional(1, "crane brick activate ID --reason TEXT [--dry-run]")?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("--reason is required")?;
    let repo = open()?;
    let draft = match find(&repo, &args.positional[0])? {
        Found::Draft(draft) => draft,
        Found::Version(entry) => {
            return Err(format!(
                "{} is already activated; 'crane brick edit {}' starts the next version",
                entry.brick.reference(),
                entry.brick.brick_id
            ))
        }
    };
    let brick = draft.brick.clone();
    let reference = brick.reference().to_string();
    let validation = validate::brick(&brick, &repo.store, &repo.workspace.root);
    let activator = store::author(&repo.workspace);
    let signers = crate::governance::admins::signers(&repo.state);
    // Without a key on the draft (written on a machine without one) the name decides
    let own = match &brick.created_by.key {
        Some(key) => activator.key.as_ref() == Some(key),
        None => brick.created_by.name == activator.name,
    };
    let refusal = if validation.status != ValidationStatus::Passed {
        Some((
            if validation.status == ValidationStatus::Blocked {
                Status::Blocked
            } else {
                Status::Failed
            },
            format!(
                "{reference} does not validate: {}",
                validation
                    .problems
                    .iter()
                    .map(|problem| problem.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        ))
    } else if repo.store.next_version(&brick.brick_id, false) != brick.version {
        Some((
            Status::Blocked,
            format!(
                "{reference} is not the next version of {}; edit the draft again",
                brick.brick_id
            ),
        ))
    } else if own && signers.len() > 1 {
        Some((
            Status::Denied,
            "the author cannot activate their own Brick; another admin reviews and activates it"
                .into(),
        ))
    } else {
        None
    };
    if let Some((status, summary)) = refusal {
        let response = Response::new("brick.activate", status, summary)
            .reference("brick_ref", reference)
            .reasons(
                validation
                    .problems
                    .iter()
                    .map(|problem| problem.code.clone()),
            )
            .data(json!({"validation": validation}));
        return finish(&args, response, &[]);
    }
    let entry = BrickEntry {
        digest: brick.digest(),
        brick: brick.clone(),
        state: VersionState::Active,
        validation: validation.clone(),
        activated_by: activator,
        activated_at: crate::platform::now_millis(),
        reason: reason.clone(),
        self_reviewed: own,
        changed_by: None,
        changed_at: None,
        change_reason: None,
    };
    if args.flag("dry-run") {
        let response = Response::new(
            "brick.activate",
            Status::Ok,
            format!("would activate {reference} (dry run; nothing signed)"),
        )
        .reference("brick_ref", reference)
        .reference("digest", entry.digest.clone())
        .data(json!({"dry_run": true, "entry": entry}));
        return finish(&args, response, &[]);
    }
    require_verified(&repo)?;
    let digest = entry.digest.clone();
    store::change(
        &repo.workspace,
        "crane brick activate",
        "brick.activated",
        json!({"brick_id": brick.brick_id, "version": brick.version, "type": brick.brick_type, "digest": digest, "self_reviewed": own}),
        |store, _| {
            store.bricks.push(entry);
            Ok(())
        },
    )?;
    store::remove_draft(&repo.workspace, "bricks", &brick.brick_id);
    crate::platform::notes::note(
        "BRICK_ACTIVATED",
        json!({"brick": reference, "type": brick.brick_type, "digest": digest, "self_reviewed": own}),
    );
    let response = Response::new(
        "brick.activate",
        Status::Ok,
        format!(
            "activated {reference}{} and signed it into .crane/bricks.json; commit .crane so the team shares it",
            if own { " (self-reviewed: this repository has one signer)" } else { "" }
        ),
    )
    .reference("brick_ref", reference)
    .reference("digest", digest)
    .data(json!({"state": "active", "self_reviewed": own}));
    finish(&args, response, &[])
}

/** Run 'crane brick deprecate' or 'crane brick revoke'
 * Input
    - args: &[String] - arguments after the command
    - revoke: bool - revoke (else deprecate)
 * Output
    - Result<(), String>
*/
pub(super) fn withdraw(args: &[String], revoke: bool) -> Result<(), String> {
    let command = if revoke {
        "brick.revoke"
    } else {
        "brick.deprecate"
    };
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(
        1,
        if revoke {
            "crane brick revoke ID[@N] --reason TEXT"
        } else {
            "crane brick deprecate ID@N --reason TEXT"
        },
    )?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("--reason is required")?;
    let (id, version) = versioned(&args.positional[0])?;
    if !revoke && version.is_none() {
        return Err("deprecation names an exact version: ID@N".into());
    }
    let repo = open()?;
    let to = if revoke {
        VersionState::Revoked
    } else {
        VersionState::Deprecated
    };
    let affected = repo
        .store
        .versions(&id)
        .into_iter()
        .filter(|entry| version.is_none_or(|version| entry.brick.version == version))
        .filter(|entry| entry.state != VersionState::Revoked && entry.state != to)
        .map(|entry| entry.brick.version)
        .collect::<Vec<_>>();
    if affected.is_empty() {
        let response = Response::new(
            command,
            Status::Ok,
            format!(
                "nothing to change: no {} version of {id} is {}",
                if version.is_some() { "such" } else { "active" },
                if revoke { "revocable" } else { "active" }
            ),
        );
        return finish(&args, response, &[]);
    }
    let pinned = repo
        .store
        .attachments
        .iter()
        .filter(|attachment| match &attachment.subject {
            Subject::Brick { brick_id, version } => *brick_id == id && affected.contains(version),
            Subject::Stack { stack_id, version } => {
                repo.store.stack(stack_id, *version).is_some_and(|entry| {
                    entry
                        .stack
                        .members
                        .iter()
                        .any(|member| member.brick_id == id && affected.contains(&member.version))
                })
            }
        })
        .map(|attachment| {
            format!(
                "{} on {}",
                attachment.attachment_id,
                attachment.target.key()
            )
        })
        .collect::<Vec<_>>();
    require_verified(&repo)?;
    let by = store::author(&repo.workspace);
    let now = crate::platform::now_millis();
    store::change(
        &repo.workspace,
        if revoke {
            "crane brick revoke"
        } else {
            "crane brick deprecate"
        },
        if revoke {
            "brick.revoked"
        } else {
            "brick.deprecated"
        },
        json!({"brick_id": id, "versions": affected, "pinned_attachments": pinned.len()}),
        |store, _| {
            for entry in store.bricks.iter_mut().filter(|entry| {
                entry.brick.brick_id == id && affected.contains(&entry.brick.version)
            }) {
                entry.state = to;
                entry.changed_by = Some(by.clone());
                entry.changed_at = Some(now);
                entry.change_reason = Some(reason.clone());
            }
            Ok(())
        },
    )?;
    let references = affected
        .iter()
        .map(|version| {
            BrickRef {
                brick_id: id.clone(),
                version: *version,
            }
            .to_string()
        })
        .collect::<Vec<_>>();
    crate::platform::notes::note(
        if revoke {
            "BRICK_REVOKED"
        } else {
            "BRICK_DEPRECATED"
        },
        json!({"brick": references.join(","), "pinned_attachments": pinned.len()}),
    );
    let detail = pinned
        .iter()
        .map(|pin| {
            format!(
                "  {pin}: {}",
                if revoke {
                    "now fails closed (changes need approval) until detached or upgraded"
                } else {
                    "keeps working; upgrade it to a newer version"
                }
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        command,
        Status::Ok,
        format!(
            "{} {}; {} attachment(s) pinned to it",
            if revoke { "revoked" } else { "deprecated" },
            references.join(", "),
            pinned.len()
        ),
    )
    .reference("brick_ref", references.join(","))
    .data(json!({"versions": affected, "pinned_attachments": pinned, "state": to}));
    finish(&args, response, &detail)
}
