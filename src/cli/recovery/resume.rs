// crane recovery resume: continue only what is safe: an invocation runs after the engine rechecks
// its plan, approval, preflight, and revocation; a runbook run resumes through the orchestrator
// (reconciled steps take their outcome, --from-step retries a step without effects).

use crate::cli::family::{
    invocation_status, invocation_view, orchestrate, run_lines, run_status, run_view, runbooks,
};
use crate::cli::invoke::{with_engine, Local};
use crate::cli::protocol::{finish, parse, Response};

/** Run the command
 * Input
    - args: &[String] - arguments after "resume"
 * Output
    - Result<(), String>
*/
pub(super) fn resume(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "from-step"], &[])?;
    args.expect_positional(
        0,
        "crane recovery resume --run INVOCATION_OR_RUN_ID [--from-step STEP]",
    )?;
    let run = args.value("run").ok_or("'--run' is required")?;
    let mut local = Local::open()?;
    if run.starts_with("rbr_") {
        let from = args.value("from-step");
        let mut store = runbooks(&local)?;
        let record = orchestrate(&mut local, &mut store, |orchestrator| {
            orchestrator.resume(&run, from.as_deref())
        })?;
        let response = Response::new(
            "recovery.resume",
            run_status(record.state),
            format!("run {run} is {:?}", record.state),
        )
        .reference("run_ref", run.clone())
        .data(run_view(&record));
        return finish(&args, response, &run_lines(&record));
    }
    let actor = local.context.actor.clone();
    let record = with_engine(&mut local, |engine| {
        engine.run(&actor, &run).map_err(String::from)
    })?;
    let response = Response::new(
        "recovery.resume",
        invocation_status(record.state),
        format!("invocation {run} is {:?}", record.state),
    )
    .reference("run_ref", run.clone())
    .data(invocation_view(&record));
    finish(&args, response, &[])
}
