// crane isolation inspect: the profile a skill runs under (typed adapters), what that profile
// confines and does not, and any step no registered operation can carry.

use serde_json::json;

use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::isolation::{check, profile};

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill"], &[])?;
    args.expect_positional(0, "crane isolation inspect --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let typed = profile("typed-adapters").ok_or("profile missing")?;
    let problems = check(&record.definition, &local.adapters, "typed-adapters");
    let mut detail = super::lines(&typed);
    detail.extend(problems.iter().map(|problem| format!("  gap: {problem}")));
    let summary = if problems.is_empty() {
        format!("{} runs under typed-adapters", reference(record))
    } else {
        format!(
            "{} runs under typed-adapters; {} steps cannot be executed by any registered operation",
            reference(record),
            problems.len()
        )
    };
    let response = Response::new(
        "isolation.inspect",
        if problems.is_empty() {
            Status::Ok
        } else {
            Status::Blocked
        },
        summary,
    )
    .reference("skill_ref", reference(record))
    .data(json!({"profile": typed, "gaps": problems}));
    finish(&args, response, &detail)
}
