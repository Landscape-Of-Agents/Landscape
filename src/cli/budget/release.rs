// crane budget release: return the unused operations of a reservation: a hold, or a step reservation
// whose step never started. A reservation whose step executed is settled from its record instead;
// consumed usage is never erased.

use serde_json::json;

use crate::cli::family::{budget, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::telemetry::ledger::{Kind, Operation};

/** Run the command
 * Input
    - args: &[String] - arguments after "release"
 * Output
    - Result<(), String>
*/
pub(super) fn release(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reservation", "reason"], &[])?;
    args.expect_positional(
        0,
        "crane budget release --reservation ENTRY_ID --reason TEXT",
    )?;
    let id = args
        .value("reservation")
        .ok_or("'--reservation' is required")?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("'--reason' is required")?;
    let local = Local::open()?;
    let ledger = budget(&local);
    let entry = super::reservation(&ledger, &id)?;
    let (run_id, step_id, attempt) = super::parts(&entry.idempotency_key)
        .ok_or("the reservation's key is not a step reservation")?;
    if !attempt.starts_with("hold:") {
        let store = runbooks(&local)?;
        let run = store.run(&run_id)?;
        if run.steps.get(&step_id).is_some_and(|step| {
            step.invocation_id.is_some() && step.attempts.to_string() == attempt
        }) {
            let response = Response::new("budget.release", Status::Denied, format!("step {step_id} started; settle its reservation from the invocation record instead")).reasons(["STEP_STARTED"]);
            return finish(&args, response, &[]);
        }
    }
    let (applied, _) = ledger.apply(Operation {
        session: &entry.session,
        task: None,
        tool_call_id: None,
        kind: Kind::Release,
        amount: entry.amount,
        reason: &reason,
        registry_generation: 0,
        idempotency_key: &format!(
            "{}:release",
            entry.idempotency_key.trim_end_matches(":reserve")
        ),
    })?;
    let response = Response::new(
        "budget.release",
        Status::Ok,
        format!("released {} operations of {id}", applied.amount),
    )
    .reference("budget_reservation_ref", id.clone())
    .reference("run_ref", run_id)
    .data(json!(applied));
    finish(&args, response, &[])
}
