// crane recovery abort: stop an invocation (abandon it, recording its unresolved effects) or cancel a
// runbook run. Nothing is rolled back implicitly: compensation is a separate, separately authorized
// operation, and irreversible effects stay recorded as they are.

use crate::cli::family::{invocation_view, orchestrate, run_lines, run_view, runbooks};
use crate::cli::invoke::{with_engine, Local};
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "abort"
 * Output
    - Result<(), String>
*/
pub(super) fn abort(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "reason"], &[])?;
    args.expect_positional(
        0,
        "crane recovery abort --run INVOCATION_OR_RUN_ID --reason TEXT",
    )?;
    let run = args.value("run").ok_or("'--run' is required")?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("'--reason' is required")?;
    let mut local = Local::open()?;
    if run.starts_with("rbr_") {
        let mut store = runbooks(&local)?;
        let record = orchestrate(&mut local, &mut store, |orchestrator| {
            orchestrator.cancel(&run, &reason)
        })?;
        let response = Response::new(
            "recovery.abort",
            Status::Ok,
            format!("run {run} is cancelled; completed effects stay recorded"),
        )
        .reference("run_ref", run.clone())
        .data(run_view(&record));
        return finish(&args, response, &run_lines(&record));
    }
    let actor = local.context.actor.clone();
    let record = with_engine(&mut local, |engine| {
        engine.abandon(&actor, &run, &reason).map_err(String::from)
    })?;
    let response = Response::new(
        "recovery.abort",
        Status::Ok,
        format!(
            "invocation {run} is abandoned; {} effects remain unresolved and are recorded",
            record.unresolved_effects.len()
        ),
    )
    .reference("run_ref", run.clone())
    .data(invocation_view(&record));
    finish(
        &args,
        response,
        &record
            .unresolved_effects
            .iter()
            .map(|effect| format!("  unresolved: {effect}"))
            .collect::<Vec<_>>(),
    )
}
