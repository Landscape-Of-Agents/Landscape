// crane approval status: whether an approval is pending, approved (and whether it was used),
// denied, or revoked, read from the invocation or run record.

use serde_json::json;

use super::{target, Target};
use crate::cli::family::runbooks;
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::lifecycle::InvocationState;
use crate::runbook::store::StepState;

/** Run the command
 * Input
    - args: &[String] - arguments after "status"
 * Output
    - Result<(), String>
*/
pub(super) fn status(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane approval status APPROVAL_REF")?;
    let reference = args.positional[0].clone();
    let local = Local::open()?;
    let (state, detail) = match target(&reference)? {
        Target::Invocation { id, digest } => {
            let record = local.ledger.get(&id)?;
            let live = record
                .approvals
                .iter()
                .filter(|approval| approval.plan_digest == digest && approval.revoked.is_none())
                .collect::<Vec<_>>();
            let revoked = record
                .approvals
                .iter()
                .any(|approval| approval.plan_digest == digest && approval.revoked.is_some());
            let denied = record
                .failures
                .iter()
                .any(|failure| failure.starts_with("approval denied"));
            let state = match record.state {
                InvocationState::AwaitingApproval if !live.is_empty() => "approved, not used yet",
                InvocationState::AwaitingApproval if revoked => "revoked; waits for a new approval",
                InvocationState::AwaitingApproval => "pending",
                InvocationState::Blocked if denied => "denied",
                _ if !live.is_empty() => "approved and used",
                _ => "not required or no longer applicable",
            };
            (
                state,
                json!({"invocation": id, "invocation_state": record.state, "approvals": record.approvals, "required": record.approval}),
            )
        }
        Target::RunStep { run, step, digest } => {
            let store = runbooks(&local)?;
            let record = store.run(&run)?;
            let live = record.approvals.iter().any(|approval| {
                approval.step == step && approval.digest == digest && approval.revoked.is_none()
            });
            let step_state = record
                .steps
                .get(&step)
                .map(|entry| entry.state)
                .ok_or_else(|| format!("run {run} has no step {step}"))?;
            let state = match step_state {
                StepState::AwaitingApproval
                    if record
                        .approvals
                        .iter()
                        .any(|approval| approval.step == step && approval.revoked.is_some()) =>
                {
                    "revoked; waits for a new approval"
                }
                StepState::AwaitingApproval => "pending",
                StepState::Blocked if record.steps[&step].detail.starts_with("approval denied") => {
                    "denied"
                }
                _ if live => "approved and used",
                _ => "not required or no longer applicable",
            };
            (
                state,
                json!({"run": run, "step": step, "step_state": step_state, "approvals": record.approvals.iter().filter(|approval| approval.step == step).collect::<Vec<_>>()}),
            )
        }
    };
    let status = if state == "pending" || state.starts_with("revoked") {
        Status::ApprovalRequired
    } else {
        Status::Ok
    };
    let response = Response::new("approval.status", status, format!("{reference}: {state}"))
        .reference("approval_ref", reference.clone())
        .data(detail);
    finish(&args, response, &[])
}
