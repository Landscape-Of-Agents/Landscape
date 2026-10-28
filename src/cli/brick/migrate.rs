// crane brick migrate: rewrite the Bricks of former kinds (context, scope, permission, constraint,
// isolation, budget, revocation, monitoring, intelligence) as the current kinds, through the same
// conversion that reads them (brick::legacy). It writes unsigned drafts only: the next version of
// each Brick in its new kind (and, for a constraint with protected paths, a second Access Brick
// "ID-access"), validated, and lists what an admin then signs: activating each draft and moving
// the attachments to it. Until then the former versions keep working exactly as before.

use serde_json::{json, Value};

use super::open;
use crate::brick::model::{Brick, BrickRef, VersionState};
use crate::brick::store::{self, BrickDraft};
use crate::brick::{legacy, validate};
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run 'crane brick migrate'
 * Input
    - args: &[String] - arguments after "migrate"
 * Output
    - Result<(), String>
*/
pub(super) fn migrate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &["dry-run"])?;
    let repo = open()?;
    let dry_run = args.flag("dry-run");
    let mut ids = repo
        .store
        .bricks
        .iter()
        .filter(|entry| entry.brick.brick_type.is_legacy() && entry.state == VersionState::Active)
        .map(|entry| entry.brick.brick_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    let by = store::author(&repo.workspace);
    let now = crate::platform::now_millis();
    let mut rows = Vec::new();
    let mut steps = Vec::new();
    for id in ids {
        let Some(latest) = repo
            .store
            .versions(&id)
            .into_iter()
            .filter(|entry| entry.state == VersionState::Active)
            .max_by_key(|entry| entry.brick.version)
        else {
            continue;
        };
        let old = latest.brick.clone();
        let parts = match legacy::convert(old.brick_type, &old.config) {
            Ok(parts) => parts,
            Err(problem) => {
                rows.push(json!({"brick": old.reference().to_string(), "status": "failed", "problem": problem}));
                continue;
            }
        };
        let attachments = repo
            .store
            .attachments
            .iter()
            .filter(|attachment| {
                attachment
                    .subject
                    .text()
                    .strip_prefix("brick:")
                    .is_some_and(|subject| subject.starts_with(&format!("{id}@")))
            })
            .map(|attachment| (attachment.attachment_id.clone(), attachment.target.key()))
            .collect::<Vec<_>>();
        let mut drafts = Vec::new();
        for (index, (kind, config)) in parts.into_iter().enumerate() {
            let (brick_id, version) = if index == 0 {
                (id.clone(), repo.store.next_version(&id, false))
            } else {
                let split = format!("{id}-access");
                (split.clone(), repo.store.next_version(&split, false))
            };
            let brick = Brick {
                brick_id: brick_id.clone(),
                version,
                brick_type: kind,
                name: if index == 0 {
                    old.name.clone()
                } else {
                    format!("{} (protected paths)", old.name)
                },
                config: config.clone(),
                created_by: by.clone(),
                created_at: now,
                ..old.clone()
            };
            let validation = validate::brick(&brick, &repo.store, &repo.workspace.root);
            let reference = BrickRef {
                brick_id: brick_id.clone(),
                version,
            };
            if !dry_run {
                store::save_brick_draft(
                    &repo.workspace,
                    &BrickDraft {
                        brick,
                        validation: validation.clone(),
                        updated_by: by.clone(),
                        updated_at: now,
                    },
                )?;
            }
            steps.push(format!(
                "crane brick activate {brick_id} --reason \"migrate {} to {}\"",
                old.reference(),
                kind.name()
            ));
            for (attachment, target) in &attachments {
                steps.push(if index == 0 {
                    format!("crane brick upgrade {attachment} --to {reference} --reason \"migrate to {}\"", kind.name())
                } else {
                    format!("crane brick attach {reference} --target {target}")
                });
            }
            drafts.push(json!({"draft": reference.to_string(), "type": kind.name(), "config": config, "validation": validation.status, "problems": validation.problems}));
        }
        rows.push(json!({"brick": old.reference().to_string(), "from": old.brick_type.name(), "status": "drafted", "drafts": drafts, "attachments": attachments.iter().map(|(id, target)| json!({"attachment": id, "target": target})).collect::<Vec<Value>>()}));
    }
    let drafted = rows.iter().filter(|row| row["status"] == "drafted").count();
    crate::platform::notes::note(
        "BRICK_MIGRATION_DRAFTED",
        json!({"bricks": drafted, "dry_run": dry_run}),
    );
    let summary = if rows.is_empty() {
        "no Brick of a former kind is active; nothing to migrate".to_string()
    } else if dry_run {
        format!(
            "{drafted} Brick(s) would be rewritten as current kinds; nothing was saved (--dry-run)"
        )
    } else {
        format!("wrote {drafted} draft(s) of current kinds; an admin activates them and moves the attachments (the former versions keep working until then)")
    };
    let status = if rows.iter().any(|row| row["status"] == "failed") {
        Status::Failed
    } else {
        Status::Ok
    };
    let detail = steps
        .iter()
        .map(|step| format!("  next: {step}"))
        .collect::<Vec<_>>();
    let response = Response::new("brick.migrate", status, summary)
        .data(json!({"bricks": rows, "next": steps}));
    finish(&args, response, &detail)
}
