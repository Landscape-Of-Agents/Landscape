// crane selection validate: the shared verifier (the one behind 'crane validate', 'crane test .',
// and the hooks) judges every selection, or one: the signed registry, anchors, checkpoint binding,
// and the policy command against the real content. Tampered, duplicated, or unresolved selections
// never pass.

use serde_json::json;

use crate::cli::protocol::{finish, name, parse, Response, Status};
use crate::governance::verify::{evaluate, Options, Outcome};
use crate::governance::workspace::Workspace;

/** Run the command
 * Input
    - args: &[String] - arguments after "validate"
 * Output
    - Result<(), String>
*/
pub(super) fn validate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    if args.positional.len() > 1 {
        return Err("usage: crane selection validate [SELECTION_ID]".into());
    }
    let wanted = args.positional.first().cloned();
    let workspace = Workspace::locate()?;
    let (report, _) = evaluate(&workspace, &Options::default())?;
    let commands = report
        .policies
        .iter()
        .flat_map(|policy| policy.commands.iter())
        .filter(|command| wanted.as_ref().is_none_or(|id| &command.id == id))
        .collect::<Vec<_>>();
    if let Some(id) = &wanted {
        if commands.is_empty() {
            return Err(format!("no policy command names selection {id}"));
        }
    }
    let findings = report
        .findings
        .iter()
        .filter(|finding| finding.blocking())
        .filter(|finding| {
            wanted.is_none() || finding.selection == wanted || finding.selection.is_none()
        })
        .collect::<Vec<_>>();
    let failing = commands
        .iter()
        .filter(|command| command.outcome != Outcome::Pass)
        .count();
    let mut detail = commands
        .iter()
        .map(|command| {
            format!(
                "  {:<10} {} {} ({}): {}",
                name(&command.outcome),
                command.operation.name(),
                command.id,
                command.policy,
                command.message
            )
        })
        .collect::<Vec<_>>();
    detail.extend(findings.iter().map(|finding| {
        format!(
            "  [{}] {}: {}",
            name(&finding.severity),
            finding.code,
            finding.message
        )
    }));
    let status = if failing == 0 && findings.is_empty() {
        Status::Passed
    } else {
        Status::Failed
    };
    let response = Response::new(
        "selection.validate",
        status,
        format!(
            "{} selections checked, {failing} failing, {} blocking findings",
            commands.len(),
            findings.len()
        ),
    )
    .reasons(findings.iter().map(|finding| finding.code.clone()))
    .data(json!({"commands": commands, "findings": findings}));
    finish(&args, response, &detail)
}
