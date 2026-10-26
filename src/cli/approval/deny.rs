// crane approval deny: decide against the action a reference names; it is blocked and can never be
// approved or run (a runbook step is blocked and the run follows its failure policy).

use super::{target, Target};
use crate::cli::family::{invocation_view, orchestrate, run_lines, run_view, runbooks};
use crate::cli::invoke::{with_engine, Local};
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "deny"
 * Output
    - Result<(), String>
*/
pub(super) fn deny(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason", "comment"], &[])?;
    args.expect_positional(1, "crane approval deny APPROVAL_REF --reason TEXT")?;
    let reference = args.positional[0].clone();
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("'--reason' is required")?;
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let response = match target(&reference)? {
        Target::Invocation { id, digest } => {
            let current = local
                .ledger
                .get(&id)?
                .plan
                .map(|plan| plan.plan_digest)
                .unwrap_or_default();
            if current != digest {
                return Err(format!(
                    "{reference} names plan {digest}, but the invocation's plan is {current}"
                ));
            }
            let record = with_engine(&mut local, |engine| {
                engine.deny(&actor, &id, &reason).map_err(String::from)
            })?;
            Response::new(
                "approval.deny",
                Status::Ok,
                format!("denied {reference}; invocation {id} is blocked"),
            )
            .data(invocation_view(&record))
        }
        Target::RunStep { run, step, digest } => {
            let mut store = runbooks(&local)?;
            let record = orchestrate(&mut local, &mut store, |orchestrator| {
                let current = orchestrator.store.run(&run)?.clone();
                let definition = orchestrator
                    .store
                    .runbook(&current.runbook_id, Some(current.version))?
                    .definition
                    .clone();
                let wanted = definition
                    .step(&step)
                    .map(|step| current.approval_digest(step))
                    .unwrap_or_default();
                if wanted != digest {
                    return Err(format!(
                        "{reference} names digest {digest}, but the step's is {wanted}"
                    ));
                }
                orchestrator.deny(&run, &step, &reason)
            })?;
            let lines = run_lines(&record);
            return finish(
                &args,
                Response::new(
                    "approval.deny",
                    Status::Ok,
                    format!("denied {reference}; run {run} is {:?}", record.state),
                )
                .reference("approval_ref", reference.clone())
                .data(run_view(&record)),
                &lines,
            );
        }
    };
    finish(
        &args,
        response.reference("approval_ref", reference.clone()),
        &[],
    )
}
