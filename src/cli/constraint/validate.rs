// crane constraint validate: limits that could be read permissively are refused: wildcards in task
// classes, environments, or exclusions; a required dependency that must be pinned but is not; and a
// high-risk side effect whose reversibility is unknown or false without a step where a person
// approves. An empty environment list is reported: it means no known environment, so applicability
// stays unresolved and invocation blocks.

use serde_json::json;

use crate::capability::model::{RiskLevel, StepKind, VersionConstraint};
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
    args.expect_positional(0, "crane constraint validate --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let definition = &record.definition;
    let applicability = &definition.applicability;
    let mut problems = Vec::new();
    let mut notes = Vec::new();
    let wild = |value: &str| {
        value.contains('*')
            || value.eq_ignore_ascii_case("any")
            || value.eq_ignore_ascii_case("all")
    };
    for value in applicability
        .task_classes
        .iter()
        .chain(applicability.environments.iter())
    {
        if wild(value) {
            problems.push(format!(
                "'{value}' is a wildcard; every task class and environment must be named"
            ));
        }
    }
    for exclusion in &applicability.exclusions {
        if wild(&exclusion.value) {
            problems.push(format!("exclusion '{}' is a wildcard", exclusion.value));
        }
    }
    if applicability.environments.is_empty() {
        notes.push("no environment is named: applicability stays unresolved and invocation blocks until one is".to_string());
    }
    for dependency in definition
        .dependencies
        .iter()
        .filter(|dependency| dependency.required && dependency.kind.needs_pin())
    {
        if dependency.constraint == VersionConstraint::Unpinned {
            problems.push(format!(
                "required {:?} {} is not pinned to a version",
                dependency.kind, dependency.target_id
            ));
        }
    }
    let approves = definition
        .execution
        .steps
        .iter()
        .any(|step| step.kind == StepKind::Approval);
    for effect in &definition.execution.side_effects {
        if effect.risk >= RiskLevel::High && effect.reversible != Some(true) && !approves {
            problems.push(format!("side effect on {} is {:?} risk and not known to be reversible, but no step has a person approve it", effect.system, effect.risk));
        }
    }
    let mut detail = problems
        .iter()
        .map(|problem| format!("  problem: {problem}"))
        .collect::<Vec<_>>();
    detail.extend(notes.iter().map(|note| format!("  note: {note}")));
    let response = Response::new(
        "constraint.validate",
        if problems.is_empty() {
            Status::Passed
        } else {
            Status::Failed
        },
        format!("{}: {} problems", reference(record), problems.len()),
    )
    .reference("skill_ref", reference(record))
    .data(json!({"problems": problems, "notes": notes}));
    finish(&args, response, &detail)
}
