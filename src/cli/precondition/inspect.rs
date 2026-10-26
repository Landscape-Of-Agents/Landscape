// crane precondition inspect: a skill's preconditions (or one step's), where each one's truth comes
// from (a claim verified from evidence, an unregistered check, or a person), and its declared status.

use serde_json::json;

use crate::capability::preconditions::{conditions, source};
use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, name, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill", "step"], &[])?;
    args.expect_positional(
        0,
        "crane precondition inspect --skill SKILL[@N] [--step STEP]",
    )?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let definition = &record.definition;
    let found = conditions(definition, args.value("step").as_deref())?;
    let rows = found
        .iter()
        .map(|condition| json!({"id": condition.id, "description": condition.description, "check": condition.check, "source": source(definition, condition), "declared_status": condition.status}))
        .collect::<Vec<_>>();
    let detail = rows
        .iter()
        .map(|row| {
            format!(
                "  {} [{}]: {} (declared {})",
                row["id"].as_str().unwrap_or_default(),
                name(&row["source"]),
                row["description"].as_str().unwrap_or_default(),
                row["declared_status"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "precondition.inspect",
        Status::Ok,
        format!("{}: {} preconditions", reference(record), rows.len()),
    )
    .reference("skill_ref", reference(record))
    .data(json!({"preconditions": rows}));
    finish(&args, response, &detail)
}
