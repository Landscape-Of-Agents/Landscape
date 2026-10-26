// crane approval request: show the approval an invocation or a runbook run is waiting for, with the
// exact reference a person approves (the request itself was made when the invocation or run paused;
// asking executes nothing).

use serde_json::json;

use crate::cli::family::runbooks;
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::lifecycle::InvocationState;
use crate::runbook::store::StepState;

/** Run the command
 * Input
    - args: &[String] - arguments after "request"
 * Output
    - Result<(), String>
*/
pub(super) fn request(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "step"], &[])?;
    args.expect_positional(
        0,
        "crane approval request --run INVOCATION_OR_RUN_ID [--step STEP]",
    )?;
    let run = args.value("run").ok_or("'--run' is required")?;
    let local = Local::open()?;
    let mut pending = Vec::new();
    if run.starts_with("inv_") {
        let record = local.ledger.get(&run)?;
        if record.state == InvocationState::AwaitingApproval {
            let digest = record
                .plan
                .as_ref()
                .map(|plan| plan.plan_digest.clone())
                .unwrap_or_default();
            pending.push(json!({"approval_ref": format!("{run}@{digest}"), "what": format!("{}@{} (plan {digest})", record.capability_id, record.version), "level": record.approval.level, "reasons": record.approval.reasons, "requester": record.requester.id}));
        }
    } else {
        let store = runbooks(&local)?;
        let record = store.run(&run)?;
        let definition = &store
            .runbook(&record.runbook_id, Some(record.version))?
            .definition;
        for (step_id, step) in record
            .steps
            .iter()
            .filter(|(id, _)| args.value("step").is_none_or(|wanted| &wanted == *id))
        {
            match step.state {
                StepState::AwaitingApproval => {
                    let digest = definition
                        .step(step_id)
                        .map(|step| record.approval_digest(step))
                        .unwrap_or_default();
                    pending.push(json!({"approval_ref": format!("{run}:{step_id}@{digest}"), "what": format!("runbook approval point {step_id}"), "requester": record.started_by.id}));
                }
                StepState::AwaitingSkillApproval => {
                    if let Some(invocation) = &step.invocation_id {
                        let digest = local
                            .ledger
                            .get(invocation)
                            .ok()
                            .and_then(|record| record.plan.map(|plan| plan.plan_digest))
                            .unwrap_or_default();
                        pending.push(json!({"approval_ref": format!("{invocation}@{digest}"), "what": format!("the skill of step {step_id}"), "requester": record.started_by.id}));
                    }
                }
                _ => {}
            }
        }
    }
    let detail = pending
        .iter()
        .map(|item| {
            format!(
                "  {} - {} (requested by {})",
                item["approval_ref"].as_str().unwrap_or_default(),
                item["what"].as_str().unwrap_or_default(),
                item["requester"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = if pending.is_empty() {
        Response::new(
            "approval.request",
            Status::Ok,
            format!("nothing in {run} waits for approval"),
        )
    } else {
        let mut response = Response::new("approval.request", Status::ApprovalRequired, format!("{} approvals wait in {run}; another person approves with 'crane approval approve REF'", pending.len()));
        if let Some(first) = pending
            .first()
            .and_then(|item| item["approval_ref"].as_str())
        {
            response = response.reference("approval_ref", first);
        }
        response
    };
    finish(
        &args,
        response
            .reference("run_ref", run.clone())
            .data(json!({"pending": pending})),
        &detail,
    )
}
