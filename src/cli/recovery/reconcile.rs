// crane recovery reconcile: ask the authoritative system (through the adapter's reconciliation, a
// lookup, never a repeat) whether an uncertain effect happened; the engine then resumes, verifies, or
// leaves it unknown. For a runbook run, the step's invocation is reconciled and the run takes its
// outcome on 'crane runbook resume'.

use crate::cli::family::{invocation_status, invocation_view, runbooks};
use crate::cli::invoke::{with_engine, Local};
use crate::cli::protocol::{finish, parse, Response};

/** Run the command
 * Input
    - args: &[String] - arguments after "reconcile"
 * Output
    - Result<(), String>
*/
pub(super) fn reconcile(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "step"], &[])?;
    args.expect_positional(
        0,
        "crane recovery reconcile --run INVOCATION_OR_RUN_ID [--step STEP]",
    )?;
    let run = args.value("run").ok_or("'--run' is required")?;
    let mut local = Local::open()?;
    let invocation = if run.starts_with("rbr_") {
        let store = runbooks(&local)?;
        let record = store.run(&run)?;
        let step = match args.value("step") {
            Some(step) => step,
            None => record
                .steps
                .iter()
                .find(|(_, step)| {
                    step.state == crate::runbook::store::StepState::NeedsReconciliation
                })
                .map(|(id, _)| id.clone())
                .ok_or("no step of the run needs reconciliation")?,
        };
        record
            .steps
            .get(&step)
            .and_then(|entry| entry.invocation_id.clone())
            .ok_or_else(|| format!("step {step} has no invocation"))?
    } else {
        run.clone()
    };
    let actor = local.context.actor.clone();
    let record = with_engine(&mut local, |engine| {
        engine.reconcile(&actor, &invocation).map_err(String::from)
    })?;
    let response = Response::new(
        "recovery.reconcile",
        invocation_status(record.state),
        format!(
            "invocation {invocation} is {:?} after reconciliation",
            record.state
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
            .map(|effect| format!("  still uncertain: {effect}"))
            .collect::<Vec<_>>(),
    )
}
