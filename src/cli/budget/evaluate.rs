// crane budget evaluate: would a step of a run fit the run's remaining budget? Exhaustion blocks the
// step; it never grants more.

use serde_json::json;

use crate::cli::family::{budget, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::runbook::run::account;
use crate::telemetry::ledger::Ledger;

/** Run the command
 * Input
    - args: &[String] - arguments after "evaluate"
 * Output
    - Result<(), String>
*/
pub(super) fn evaluate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "operation", "skill"], &[])?;
    args.expect_positional(0, "crane budget evaluate --run RUN_ID --operation STEP")?;
    let run_id = args.value("run").ok_or("'--run' is required")?;
    let step_id = args
        .value("operation")
        .ok_or("'--operation' (the step) is required")?;
    let local = Local::open()?;
    let store = runbooks(&local)?;
    let run = store.run(&run_id)?;
    let step = store
        .runbook(&run.runbook_id, Some(run.version))?
        .definition
        .step(&step_id)
        .cloned()
        .ok_or_else(|| format!("run {run_id} has no step {step_id}"))?;
    let available = Ledger::balance_of(&budget(&local).entries()?, &account(&run_id)).available;
    let fits = available >= step.cost();
    let response = Response::new(
        "budget.evaluate",
        if fits { Status::Ok } else { Status::Denied },
        format!(
            "step {step_id} needs {} operations; {available} are available",
            step.cost()
        ),
    )
    .reasons(if fits {
        Vec::new()
    } else {
        vec!["BUDGET_EXHAUSTED".to_string()]
    })
    .reference("run_ref", run_id.clone())
    .data(json!({"cost": step.cost(), "available": available, "fits": fits}));
    finish(&args, response, &[])
}
