// crane approval revoke: withdraw an approval before it is used (by its approver, or a publisher);
// the action waits for a new approval. An approval that was used cannot be revoked: its effects are
// reconciled, never assumed undone.

use super::{target, Target};
use crate::cli::family::{invocation_view, orchestrate, run_view, runbooks};
use crate::cli::invoke::{with_engine, Local};
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "revoke"
 * Output
    - Result<(), String>
*/
pub(super) fn revoke(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane approval revoke APPROVAL_REF --reason TEXT")?;
    let reference = args.positional[0].clone();
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("'--reason' is required")?;
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let data = match target(&reference)? {
        Target::Invocation { id, .. } => invocation_view(&with_engine(&mut local, |engine| {
            engine
                .revoke_approval(&actor, &id, &reason)
                .map_err(String::from)
        })?),
        Target::RunStep { run, step, .. } => {
            let mut store = runbooks(&local)?;
            run_view(&orchestrate(&mut local, &mut store, |orchestrator| {
                orchestrator.revoke_approval(&run, &step, &reason)
            })?)
        }
    };
    let response = Response::new(
        "approval.revoke",
        Status::Ok,
        format!("revoked {reference}; it waits for a new approval"),
    )
    .reference("approval_ref", reference.clone())
    .data(data);
    finish(&args, response, &[])
}
