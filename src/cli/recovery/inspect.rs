// crane recovery inspect: the operations of an invocation (or the steps of a run) that failed, are
// partial, or have uncertain effects, whether each is idempotent, and the next safe choice. A failure
// is never read as "nothing happened".

use serde_json::json;

use crate::cli::family::{run_lines, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::engine::InvocationRecord;
use crate::invocation::lifecycle::{EffectStatus, InvocationState};

/** The next safe choice for an invocation
 * Input
    - record: &InvocationRecord - invocation
 * Output
    - &'static str
*/
pub(super) fn next(record: &InvocationRecord) -> &'static str {
    match record.state {
        InvocationState::ReconciliationRequired | InvocationState::Unknown => "reconcile against the authoritative system ('crane recovery reconcile'); never retry blindly",
        InvocationState::AwaitingApproval => "a person other than the requester approves ('crane approval approve'), then it runs",
        InvocationState::PlanReady => "run it ('crane recovery resume')",
        InvocationState::PartiallyVerified => "investigate the unresolved postconditions; record the outcome",
        InvocationState::Failed | InvocationState::Blocked => "fix the cause; a new invocation is a new request",
        _ => "nothing to recover",
    }
}

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run"], &[])?;
    args.expect_positional(0, "crane recovery inspect --run INVOCATION_OR_RUN_ID")?;
    let run = args.value("run").ok_or("'--run' is required")?;
    let local = Local::open()?;
    if run.starts_with("rbr_") {
        let store = runbooks(&local)?;
        let record = store.run(&run)?;
        let uncertain = record
            .steps
            .values()
            .filter(|step| step.state == crate::runbook::store::StepState::NeedsReconciliation)
            .count();
        let response = Response::new(
            "recovery.inspect",
            if uncertain > 0 {
                Status::Unknown
            } else {
                Status::Ok
            },
            format!(
                "run {run} is {:?}; {uncertain} steps need reconciliation",
                record.state
            ),
        )
        .reference("run_ref", run.clone())
        .data(crate::cli::family::run_view(record));
        return finish(&args, response, &run_lines(record));
    }
    let record = local.ledger.get(&run)?;
    let uncertain = record
        .operations
        .iter()
        .filter(|op| op.effect_status == EffectStatus::Unknown)
        .count();
    let mut detail = record
        .operations
        .iter()
        .map(|op| {
            format!(
                "  {} {}.{}: {:?}, effect {:?}, {} attempts",
                op.step_id,
                op.adapter_id,
                op.operation_id,
                op.status,
                op.effect_status,
                op.attempts
            )
        })
        .collect::<Vec<_>>();
    detail.push(format!("  next: {}", next(&record)));
    let response = Response::new("recovery.inspect", if uncertain > 0 { Status::Unknown } else { Status::Ok }, format!("invocation {run} is {:?}; {uncertain} effects are uncertain", record.state))
        .reference("run_ref", run.clone())
        .data(json!({"state": record.state, "operations": crate::cli::family::invocation_view(&record)["operations"], "unresolved_effects": record.unresolved_effects, "recovery_status": record.recovery_status, "next": next(&record)}));
    finish(&args, response, &detail)
}
