// crane precondition evaluate: judge each precondition now, from evidence read through your access;
// manual ones hold only as your declaration (--confirm). Anything unresolved makes the result
// unresolved, never passed.

use serde_json::json;

use crate::capability::preconditions::{evaluate as judge, Truth};
use crate::cli::family::{list, reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, name, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "evaluate"
 * Output
    - Result<(), String>
*/
pub(super) fn evaluate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill", "step", "confirm"], &[])?;
    args.expect_positional(
        0,
        "crane precondition evaluate --skill SKILL[@N] [--step STEP] [--confirm ID,ID]",
    )?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let evaluations = judge(
        &record.definition,
        args.value("step").as_deref(),
        &local.context.sources(),
        &list(args.value("confirm")),
        &local.context.actor,
    )?;
    let unresolved = evaluations
        .iter()
        .filter(|evaluation| evaluation.truth == Truth::Unresolved)
        .count();
    let detail = evaluations
        .iter()
        .map(|evaluation| {
            format!(
                "  {} {}: {}",
                name(&evaluation.truth),
                evaluation.id,
                evaluation.detail
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "precondition.evaluate",
        if unresolved == 0 {
            Status::Passed
        } else {
            Status::Unresolved
        },
        format!(
            "{}: {} preconditions, {unresolved} unresolved",
            reference(record),
            evaluations.len()
        ),
    )
    .reasons(
        evaluations
            .iter()
            .filter(|evaluation| evaluation.truth == Truth::Unresolved)
            .map(|evaluation| format!("PRECONDITION_UNRESOLVED:{}", evaluation.id)),
    )
    .reference("skill_ref", reference(record))
    .data(json!({"preconditions": evaluations}));
    finish(&args, response, &detail)
}
