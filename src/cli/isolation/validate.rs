// crane isolation validate: the required profile exists and is available, and every step of the
// skill has a registered operation under it; otherwise the skill is blocked, never run another way.

use serde_json::json;

use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::isolation::check;

/** Run the command
 * Input
    - args: &[String] - arguments after "validate"
 * Output
    - Result<(), String>
*/
pub(super) fn validate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill", "environment"], &[])?;
    args.expect_positional(
        0,
        "crane isolation validate --skill SKILL[@N] [--environment PROFILE]",
    )?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let wanted = args
        .value("environment")
        .unwrap_or_else(|| "typed-adapters".into());
    let problems = check(&record.definition, &local.adapters, &wanted);
    let detail = problems
        .iter()
        .map(|problem| format!("  {problem}"))
        .collect::<Vec<_>>();
    let response = Response::new(
        "isolation.validate",
        if problems.is_empty() {
            Status::Passed
        } else {
            Status::Blocked
        },
        format!(
            "{} under {wanted}: {}",
            reference(record),
            if problems.is_empty() {
                "supported"
            } else {
                "blocked"
            }
        ),
    )
    .reasons(problems.iter().map(|_| "ISOLATION_UNSUPPORTED".to_string()))
    .reference("skill_ref", reference(record))
    .data(json!({"profile": wanted, "problems": problems}));
    finish(&args, response, &detail)
}
