// crane runbook status: a run's steps, invocations, approvals, and outcome, read from its sealed
// record; it claims completion only when every required step succeeded with verified postconditions.

use crate::cli::family::{run_lines, run_status, run_view, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response};

/** Run the command
 * Input
    - args: &[String] - arguments after "status"
 * Output
    - Result<(), String>
*/
pub(super) fn status(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane runbook status RUN_ID")?;
    let local = Local::open()?;
    let store = runbooks(&local)?;
    let run = store.run(&args.positional[0])?;
    let response = Response::new(
        "runbook.status",
        run_status(run.state),
        format!(
            "run {} of {}@{} is {:?}",
            run.run_id, run.runbook_id, run.version, run.state
        ),
    )
    .reference("runbook_ref", format!("{}@{}", run.runbook_id, run.version))
    .reference("run_ref", run.run_id.clone())
    .data(run_view(run));
    let mut detail = run_lines(run);
    detail.extend(run.log.iter().map(|line| format!("  log: {line}")));
    finish(&args, response, &detail)
}
