// crane runbook cancel: cancel a run: steps that have not started never will, a step waiting for a
// skill approval is abandoned, unused budget is released, and completed effects stay recorded. A
// run with uncertain effects must be reconciled first.

use crate::cli::family::{orchestrate, run_lines, run_view, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "cancel"
 * Output
    - Result<(), String>
*/
pub(super) fn cancel(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane runbook cancel RUN_ID --reason TEXT")?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("'--reason' is required")?;
    let run_id = args.positional[0].clone();
    let mut local = Local::open()?;
    let mut store = runbooks(&local)?;
    let run = orchestrate(&mut local, &mut store, |orchestrator| {
        orchestrator.cancel(&run_id, &reason)
    })?;
    let response = Response::new(
        "runbook.cancel",
        Status::Ok,
        format!("run {run_id} is cancelled"),
    )
    .reference("run_ref", run_id.clone())
    .data(run_view(&run));
    finish(&args, response, &run_lines(&run))
}
