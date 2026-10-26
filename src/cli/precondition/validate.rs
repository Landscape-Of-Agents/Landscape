// crane precondition validate: every precondition has a unique id, every check names a claim of the
// skill, and every step and the procedure require only declared preconditions. Manual preconditions
// are allowed and reported as declared-only.

use serde_json::json;

use crate::capability::preconditions::validate as rules;
use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "validate"
 * Output
    - Result<(), String>
*/
pub(super) fn validate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill"], &[])?;
    args.expect_positional(0, "crane precondition validate --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let (problems, warnings) = rules(&record.definition);
    let mut detail = problems
        .iter()
        .map(|problem| format!("  problem: {problem}"))
        .collect::<Vec<_>>();
    detail.extend(warnings.iter().map(|warning| format!("  note: {warning}")));
    let response = Response::new(
        "precondition.validate",
        if problems.is_empty() {
            Status::Passed
        } else {
            Status::Failed
        },
        format!(
            "{}: {} problems, {} declared-only preconditions",
            reference(record),
            problems.len(),
            warnings.len()
        ),
    )
    .reference("skill_ref", reference(record))
    .data(json!({"problems": problems, "warnings": warnings}));
    finish(&args, response, &detail)
}
