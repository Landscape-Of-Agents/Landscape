// crane selection remove: end a preserve or target selection (selection/remove.rs). Only a person
// runs it: Crane refuses it inside an agent session, and the hooks deny it in an agent's shell.

use serde_json::json;

use crate::cli::protocol::{finish, parse, Response, Status};
use crate::selection::remove::{apply, plan};

/** Run the command
 * Input
    - args: &[String] - arguments after "remove"
 * Output
    - Result<(), String>
*/
pub(super) fn remove(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &["dry-run"])?;
    args.expect_positional(
        1,
        "crane selection remove SELECTION_ID --reason TEXT [--dry-run]",
    )?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("--reason is required")?;
    let id = args.positional[0].clone();
    let plan = plan(&id)?;
    let facts = json!({
        "selection": id,
        "operation": plan.operation,
        "policy": plan.policy,
        "files": plan.files,
        "named_by": plan.named_by,
    });
    if !plan.named_by.is_empty() {
        let detail = plan
            .named_by
            .iter()
            .map(|zone| format!("  named by zone {zone}"))
            .collect::<Vec<_>>();
        let response = Response::new(
            "selection.remove",
            Status::Denied,
            format!("{id} is named in .crane/governance.yaml; remove it from the zone's selections first"),
        )
        .reference("selection_ref", id)
        .reasons(["SELECTION_NAMED_BY_ZONE"])
        .data(facts);
        return finish(&args, response, &detail);
    }
    let what = format!(
        "the {} selection {id} (markers in {} file(s), policy {})",
        plan.operation,
        plan.files.len(),
        plan.policy
    );
    if args.flag("dry-run") {
        let response = Response::new(
            "selection.remove",
            Status::Ok,
            format!(
                "would remove {what}; the code between its markers stays (dry run; nothing changed)"
            ),
        )
        .reference("selection_ref", id)
        .data(facts);
        return finish(&args, response, &[]);
    }
    apply(&plan, &reason)?;
    let response = Response::new(
        "selection.remove",
        Status::Ok,
        format!("removed {what}; commit the change so the team shares it"),
    )
    .reference("selection_ref", id)
    .data(facts);
    finish(&args, response, &[])
}
