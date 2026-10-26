// crane runbook revoke: a publisher revokes an exact version: no new run starts, and every running
// run stops at its next step. Past runs and their records stay; a revocation is never undone.

use serde_json::json;

use crate::capability::Role;
use crate::cli::family::{exact, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::runbook::store::RunbookState;

/** Run the command
 * Input
    - args: &[String] - arguments after "revoke"
 * Output
    - Result<(), String>
*/
pub(super) fn revoke(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane runbook revoke RUNBOOK@N --reason TEXT")?;
    let (id, version) = exact(&args.positional[0], "revocation")?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("'--reason' is required")?;
    let local = Local::open()?;
    let actor = local.context.actor.clone();
    if local
        .context
        .registry
        .grants
        .require(&actor, Role::Publisher)
        .is_err()
    {
        let response = Response::new("runbook.revoke", Status::Denied, "revocation needs the publisher role (this machine's key must be the owner's or an admin's)");
        return finish(&args, response, &[]);
    }
    let mut store = runbooks(&local)?;
    let record = store.transition(
        &id,
        version,
        RunbookState::Revoked,
        &actor,
        &reason,
        crate::platform::now_millis(),
    )?;
    store.save()?;
    let running = store
        .state
        .runs
        .values()
        .filter(|run| run.runbook_id == id && run.version == version && !run.state.ended())
        .count();
    let response = Response::new(
        "runbook.revoke",
        Status::Ok,
        format!(
            "revoked runbook {id}@{version}; {running} unfinished runs stop at their next step"
        ),
    )
    .reference("runbook_ref", format!("{id}@{version}"))
    .data(json!({"state": record.state, "unfinished_runs": running}));
    finish(&args, response, &[])
}
