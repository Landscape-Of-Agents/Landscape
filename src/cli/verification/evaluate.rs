// crane verification evaluate: judge a skill's claims from their evidence now (through your access),
// or a run's postconditions as the engine verified them. Passed only when every required check
// passed; anything unresolved is unknown.

use serde_json::json;

use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response};

/** Run the command
 * Input
    - args: &[String] - arguments after "evaluate"
 * Output
    - Result<(), String>
*/
pub(super) fn evaluate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill", "run"], &[])?;
    args.expect_positional(
        0,
        "crane verification evaluate --skill SKILL[@N] | --run INVOCATION_ID",
    )?;
    let local = Local::open()?;
    let (rows, subject, refs) = match args.value("run") {
        Some(run) => {
            let record = local.ledger.get(&run)?;
            (
                super::results(&record),
                format!("run {run}"),
                ("run_ref", run),
            )
        }
        None => {
            let record = skill(
                &local,
                &args
                    .value("skill")
                    .ok_or("'--skill' or '--run' is required")?,
            )?;
            (
                super::claims(&record.definition, &local.context.sources()),
                reference(record),
                ("skill_ref", reference(record)),
            )
        }
    };
    let status = super::overall(&rows);
    let passed = rows.iter().filter(|row| row["result"] == "PASS").count();
    let response = Response::new(
        "verification.evaluate",
        status,
        format!("{subject}: {passed} of {} checks passed", rows.len()),
    )
    .reasons(
        rows.iter()
            .filter(|row| row["result"] != "PASS")
            .filter_map(|row| {
                row["claim"]
                    .as_str()
                    .map(|claim| format!("{}:{claim}", row["result"].as_str().unwrap_or_default()))
            }),
    )
    .reference(refs.0, refs.1)
    .data(json!({"checks": rows}));
    finish(&args, response, &super::lines(&rows))
}
