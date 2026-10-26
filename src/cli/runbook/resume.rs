// crane runbook resume: continue a paused run once its approvals were given and its uncertain
// effects reconciled; with --from-step, retry a step that was blocked or failed without any effect,
// within its retry limit, under a new invocation identity.

use crate::cli::family::{orchestrate, run_lines, run_status, run_view, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response};

/** Run the command
 * Input
    - args: &[String] - arguments after "resume"
 * Output
    - Result<(), String>
*/
pub(super) fn resume(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["from-step"], &[])?;
    args.expect_positional(1, "crane runbook resume RUN_ID [--from-step STEP]")?;
    let run_id = args.positional[0].clone();
    let from = args.value("from-step");
    let mut local = Local::open()?;
    let mut store = runbooks(&local)?;
    let run = orchestrate(&mut local, &mut store, |orchestrator| {
        orchestrator.resume(&run_id, from.as_deref())
    })?;
    let response = Response::new(
        "runbook.resume",
        run_status(run.state),
        format!("run {run_id} is {:?}", run.state),
    )
    .reference("run_ref", run_id.clone())
    .data(run_view(&run));
    finish(&args, response, &run_lines(&run))
}
