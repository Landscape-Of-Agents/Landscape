// crane budget reserve: hold a step's operations in a run's budget by hand (idempotent by key), for
// example before a manual review. A hold only restricts: it is never settled as usage, and is
// released with 'crane budget release'. A hold larger than what is left is refused, not clamped.

use serde_json::json;

use crate::cli::family::{budget, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::runbook::run::account;
use crate::telemetry::ledger::{Kind, Ledger, Operation};

/** Run the command
 * Input
    - args: &[String] - arguments after "reserve"
 * Output
    - Result<(), String>
*/
pub(super) fn reserve(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "operation", "idempotency-key"], &[])?;
    args.expect_positional(
        0,
        "crane budget reserve --run RUN_ID --operation STEP --idempotency-key KEY",
    )?;
    let run_id = args.value("run").ok_or("'--run' is required")?;
    let step_id = args
        .value("operation")
        .ok_or("'--operation' (the step) is required")?;
    let key = args
        .value("idempotency-key")
        .filter(|key| !key.contains(':') && !key.is_empty())
        .ok_or("'--idempotency-key' is required (no ':')")?;
    let local = Local::open()?;
    let store = runbooks(&local)?;
    let run = store.run(&run_id)?;
    if run.state.ended() {
        return Err(format!(
            "run {run_id} ended ({:?}); its budget is closed",
            run.state
        ));
    }
    let cost = store
        .runbook(&run.runbook_id, Some(run.version))?
        .definition
        .step(&step_id)
        .map(|step| step.cost())
        .ok_or_else(|| format!("run {run_id} has no step {step_id}"))?;
    let ledger = budget(&local);
    let account = account(&run_id);
    let idempotency = format!("{run_id}:{step_id}:hold:{key}");
    let existing = ledger
        .entries()?
        .into_iter()
        .find(|entry| entry.idempotency_key == idempotency);
    let available = Ledger::balance_of(&ledger.entries()?, &account).available;
    if existing.is_none() && available < cost {
        let response = Response::new(
            "budget.reserve",
            Status::Denied,
            format!("step {step_id} needs {cost}; only {available} are available"),
        )
        .reasons(["BUDGET_EXHAUSTED"]);
        return finish(&args, response, &[]);
    }
    let (entry, new) = ledger.apply(Operation {
        session: &account,
        task: None,
        tool_call_id: None,
        kind: Kind::Reserve,
        amount: cost,
        reason: &format!("hold for step {step_id}"),
        registry_generation: 0,
        idempotency_key: &idempotency,
    })?;
    let response = Response::new(
        "budget.reserve",
        Status::Ok,
        format!(
            "held {} operations for step {step_id} ({}reservation {})",
            entry.amount,
            if new { "" } else { "existing " },
            entry.entry_id
        ),
    )
    .reference("run_ref", run_id.clone())
    .reference("budget_reservation_ref", entry.entry_id.clone())
    .data(json!(entry));
    finish(&args, response, &[])
}
