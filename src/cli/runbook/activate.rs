// crane runbook activate: a person other than the author, holding the publisher role, activates an
// exact draft version after it validates against the registry (every pinned skill published with
// its pinned digest). Only an active version runs.

use serde_json::json;

use crate::capability::Role;
use crate::cli::family::{exact, registry_findings, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::runbook::model;
use crate::runbook::store::RunbookState;

/** Run the command
 * Input
    - args: &[String] - arguments after "activate"
 * Output
    - Result<(), String>
*/
pub(super) fn activate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["review-ref"], &[])?;
    args.expect_positional(1, "crane runbook activate RUNBOOK@N --review-ref REF")?;
    let (id, version) = exact(&args.positional[0], "activation")?;
    let review = args
        .value("review-ref")
        .filter(|value| !value.trim().is_empty())
        .ok_or("'--review-ref' is required")?;
    let local = Local::open()?;
    let actor = local.context.actor.clone();
    let mut store = runbooks(&local)?;
    let record = store.runbook(&id, Some(version))?.clone();
    let mut problems = model::validate(&record.definition);
    problems.extend(registry_findings(&local, &record.definition));
    let refusal = if record.created_by.id == actor.id {
        Some("the author cannot activate their own runbook; another person reviews it".to_string())
    } else if local
        .context
        .registry
        .grants
        .require(&actor, Role::Publisher)
        .is_err()
    {
        Some("activation needs the publisher role (this machine's key must be the owner's or an admin's)".into())
    } else if !problems.is_empty() {
        Some(format!(
            "the runbook does not validate: {}",
            problems
                .iter()
                .filter_map(|finding| finding["message"].as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ))
    } else {
        None
    };
    if let Some(reason) = refusal {
        let response = Response::new("runbook.activate", Status::Denied, reason)
            .reference("runbook_ref", format!("{id}@{version}"))
            .data(json!({"findings": problems}));
        return finish(&args, response, &[]);
    }
    let changed = store.transition(
        &id,
        version,
        RunbookState::Active,
        &actor,
        &format!("activated after review {review}"),
        crate::platform::now_millis(),
    )?;
    store.save()?;
    let response = Response::new(
        "runbook.activate",
        Status::Ok,
        format!("activated runbook {id}@{version}"),
    )
    .reference("runbook_ref", format!("{id}@{version}"))
    .reference("digest", changed.digest.clone())
    .reference("review_ref", review)
    .data(json!({"state": changed.state}));
    finish(&args, response, &[])
}
