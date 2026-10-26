// crane runbook run: start a run of an exact active version through the orchestrator. Each step is
// its own invocation with the skill's full checks; the run pauses for approvals and uncertain
// effects, and its status comes from verified step outcomes.

use crate::cli::family::{
    exact, inputs, list, orchestrate, run_lines, run_status, run_view, runbooks,
};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response};

/** Run the command
 * Input
    - args: &[String] - arguments after "run"
 * Output
    - Result<(), String>
*/
pub(super) fn run(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["input", "confirm", "idempotency-key"], &[])?;
    args.expect_positional(
        1,
        "crane runbook run RUNBOOK@N [--input FILE] [--confirm ID,ID] --idempotency-key KEY",
    )?;
    let (id, version) = exact(&args.positional[0], "a run")?;
    let key = args
        .value("idempotency-key")
        .ok_or("'--idempotency-key' is required, so a repeated request returns the same run")?;
    let run_inputs = inputs(args.value("input"))?;
    let confirmed = list(args.value("confirm"));
    let mut local = Local::open()?;
    let mut store = runbooks(&local)?;
    let run = orchestrate(&mut local, &mut store, |orchestrator| {
        orchestrator.start(&id, version, run_inputs, confirmed, &key)
    })?;
    let response = Response::new(
        "runbook.run",
        run_status(run.state),
        format!("run {} of {id}@{version} is {:?}", run.run_id, run.state),
    )
    .reference("runbook_ref", format!("{id}@{version}"))
    .reference("run_ref", run.run_id.clone())
    .data(run_view(&run));
    finish(&args, response, &run_lines(&run))
}
