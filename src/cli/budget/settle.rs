// crane budget settle: settle a step's reservation from the run's own invocation record (the
// outcome given must match the recorded one; usage is never taken from the caller), under the
// orchestrator's idempotency keys, so a settled reservation is never settled twice. An uncertain
// outcome consumes everything until it is reconciled. A hold has no execution and is not settled.

use serde_json::json;

use crate::cli::family::{budget, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::lifecycle::{InvocationState, OperationStatus};
use crate::telemetry::ledger::{Kind, Operation};

/** Run the command
 * Input
    - args: &[String] - arguments after "settle"
 * Output
    - Result<(), String>
*/
pub(super) fn settle(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reservation", "outcome", "usage"], &[])?;
    args.expect_positional(
        0,
        "crane budget settle --reservation ENTRY_ID --outcome success|failure|partial|unknown",
    )?;
    if args.value("usage").is_some() {
        return Err(
            "usage is never taken from the caller; it is read from the run's invocation record"
                .into(),
        );
    }
    let id = args
        .value("reservation")
        .ok_or("'--reservation' is required")?;
    let outcome = args.value("outcome").ok_or("'--outcome' is required")?;
    let local = Local::open()?;
    let ledger = budget(&local);
    let entry = super::reservation(&ledger, &id)?;
    let (run_id, step_id, attempt) = super::parts(&entry.idempotency_key)
        .ok_or("the reservation's key is not a step reservation")?;
    if attempt.starts_with("hold:") {
        return Err(
            "a hold has no execution to settle; release it with 'crane budget release'".into(),
        );
    }
    let store = runbooks(&local)?;
    let run = store.run(&run_id)?;
    let invocation = run
        .steps
        .get(&step_id)
        .and_then(|step| step.invocation_id.clone())
        .ok_or("the step has not started; release the reservation instead")?;
    let record = local.ledger.get(&invocation)?;
    let recorded = match record.state {
        InvocationState::Succeeded => "success",
        InvocationState::Failed | InvocationState::Blocked | InvocationState::Abandoned => {
            "failure"
        }
        InvocationState::PartiallyVerified => "partial",
        InvocationState::AwaitingApproval | InvocationState::PlanReady => {
            return Err("the step has not executed yet".into())
        }
        _ => "unknown",
    };
    if recorded != outcome {
        let response = Response::new(
            "budget.settle",
            Status::Denied,
            format!("the invocation record says {recorded}, not {outcome}"),
        )
        .reasons(["OUTCOME_MISMATCH"]);
        return finish(&args, response, &[]);
    }
    let uncertain = matches!(recorded, "unknown" | "partial");
    let started = record
        .operations
        .iter()
        .filter(|op| {
            !matches!(
                op.status,
                OperationStatus::Pending | OperationStatus::NotStarted
            )
        })
        .count() as u64;
    let consume = if uncertain {
        entry.amount
    } else {
        started.min(entry.amount)
    };
    let prefix = format!("{run_id}:{step_id}:{attempt}");
    let account = entry.session.clone();
    if consume > 0 {
        ledger.apply(Operation {
            session: &account,
            task: None,
            tool_call_id: None,
            kind: Kind::Consume,
            amount: consume,
            reason: &format!("step {step_id}: operations that started"),
            registry_generation: 0,
            idempotency_key: &format!("{prefix}:consume"),
        })?;
    }
    if entry.amount > consume {
        ledger.apply(Operation {
            session: &account,
            task: None,
            tool_call_id: None,
            kind: Kind::Release,
            amount: entry.amount - consume,
            reason: &format!("step {step_id}: operations that never started"),
            registry_generation: 0,
            idempotency_key: &format!("{prefix}:release"),
        })?;
    }
    let response = Response::new(
        "budget.settle",
        Status::Ok,
        format!(
            "settled {id}: {consume} consumed, {} released (from invocation {invocation})",
            entry.amount - consume
        ),
    )
    .reference("budget_reservation_ref", id.clone())
    .reference("run_ref", run_id)
    .data(json!({"consumed": consume, "released": entry.amount - consume, "outcome": recorded}));
    finish(&args, response, &[])
}
