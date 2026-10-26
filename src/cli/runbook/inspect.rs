// crane runbook inspect: a readable step map (order, dependencies, branches, approval points,
// retries, failure policy, the exact skill of each step), the run inputs, the completion
// requirement, the budget, and the lifecycle.

use crate::cli::family::runbooks;
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, versioned, Response, Status};
use crate::runbook::store::summary;

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane runbook inspect RUNBOOK[@N]")?;
    let (id, version) = versioned(&args.positional[0])?;
    let local = Local::open()?;
    let store = runbooks(&local)?;
    let record = store.runbook(&id, version)?;
    let definition = &record.definition;
    let reference = format!("{id}@{}", definition.version);
    let mut detail = vec![
        format!(
            "  {} ({:?}); department {}",
            definition.description,
            record.state,
            definition
                .department
                .clone()
                .unwrap_or_else(|| "not set".into())
        ),
        format!(
            "  inputs: {}",
            if definition.inputs.is_empty() {
                "none".into()
            } else {
                definition.inputs.join(", ")
            }
        ),
    ];
    for step_id in definition.order().unwrap_or_default() {
        let Some(step) = definition.step(&step_id) else {
            continue;
        };
        let mut notes = Vec::new();
        if !step.depends_on.is_empty() {
            notes.push(format!("after {}", step.depends_on.join(", ")));
        }
        if let Some(when) = &step.when {
            notes.push(format!("only if {} {:?}", when.step, when.outcome).to_lowercase());
        }
        if step.approval {
            notes.push("a person approves first".into());
        }
        if step.max_retries > 0 {
            notes.push(format!(
                "up to {} retries without effects",
                step.max_retries
            ));
        }
        notes.push(format!("on failure: {:?}", step.on_failure).to_lowercase());
        detail.push(format!(
            "  {step_id}: {}@{} ({} operations) - {}",
            step.skill_id,
            step.skill_version,
            step.cost(),
            notes.join("; ")
        ));
    }
    detail.push(format!(
        "  succeeds when: {} succeed; budget {} operations",
        definition.required().join(", "),
        definition.budget.operations
    ));
    let response = Response::new(
        "runbook.inspect",
        Status::Ok,
        format!("runbook {reference}: {}", definition.name),
    )
    .reference("runbook_ref", reference)
    .reference("digest", record.digest.clone())
    .data(summary(record));
    finish(&args, response, &detail)
}
