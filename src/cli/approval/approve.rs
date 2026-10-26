// crane approval approve: record your approval of the exact action a reference names, after the
// engine rechecks it: you are a person other than the requester, you hold the role it needs, the
// digest is the current plan's, and it still waits. A runbook approval point then continues the run.

use super::{target, Target};
use crate::cli::family::{invocation_view, orchestrate, run_lines, run_status, run_view, runbooks};
use crate::cli::invoke::{with_engine, Local};
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "approve"
 * Output
    - Result<(), String>
*/
pub(super) fn approve(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["comment"], &[])?;
    args.expect_positional(1, "crane approval approve APPROVAL_REF [--comment TEXT]")?;
    let reference = args.positional[0].clone();
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    match target(&reference)? {
        Target::Invocation { id, digest } => {
            let record = with_engine(&mut local, |engine| {
                engine.approve(&actor, &id, &digest).map_err(String::from)
            })?;
            let response = Response::new("approval.approve", Status::Ok, format!("approved {reference}; the requester runs it with 'crane capability invoke run {id}' (or 'crane runbook resume' for a runbook step)"))
                .reference("approval_ref", reference.clone())
                .reference("run_ref", id)
                .data(invocation_view(&record));
            finish(&args, response, &[])
        }
        Target::RunStep { run, step, digest } => {
            let mut store = runbooks(&local)?;
            let record = orchestrate(&mut local, &mut store, |orchestrator| {
                orchestrator.approve(&run, &step, &digest)
            })?;
            let response = Response::new(
                "approval.approve",
                run_status(record.state),
                format!("approved {reference}; run {run} is {:?}", record.state),
            )
            .reference("approval_ref", reference.clone())
            .reference("run_ref", run)
            .data(run_view(&record));
            finish(&args, response, &run_lines(&record))
        }
    }
}
