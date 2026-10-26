// crane skill validate: the capability validation gates (structure, provenance, applicability,
// verification, governance) and the precondition rules on one version. A passing run makes the
// version validated; it never activates it. --dry-run records nothing.

use serde_json::json;

use crate::capability::validate::{RunOutcome, Severity};
use crate::cli::capability::Context;
use crate::cli::protocol::{finish, parse, versioned, Response, Status};
use crate::platform::now_millis;

/** Run the command
 * Input
    - args: &[String] - arguments after "validate"
 * Output
    - Result<(), String>
*/
pub(super) fn validate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &["dry-run"])?;
    args.expect_positional(1, "crane skill validate SKILL[@N] [--dry-run]")?;
    let (id, version) = versioned(&args.positional[0])?;
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let version = match version {
        Some(version) => version,
        None => context
            .registry
            .state
            .versions
            .get(&id)
            .and_then(|versions| versions.keys().next_back().copied())
            .ok_or_else(|| format!("skill {id} does not exist"))?,
    };
    let definition = context
        .registry
        .state
        .record(&id, version)?
        .definition
        .clone();
    let (problems, warnings) = crate::capability::preconditions::validate(&definition);
    let sources = crate::capability::refs::Sources {
        capture: &context.capture,
        current: &context.current,
        reader: &context.reader,
    };
    let run = context.registry.validate(
        &actor,
        &id,
        version,
        &sources,
        args.flag("dry-run") || !problems.is_empty(),
        now_millis(),
    )?;
    let findings = run
        .gates
        .iter()
        .flat_map(|gate| gate.findings.iter().filter(|finding| finding.severity != Severity::Info).map(move |finding| (gate.gate, finding)))
        .map(|(gate, finding)| json!({"gate": gate, "severity": finding.severity, "code": finding.code, "message": finding.message}))
        .collect::<Vec<_>>();
    let status = if !problems.is_empty() || run.outcome != RunOutcome::Passed {
        Status::Failed
    } else {
        Status::Passed
    };
    let mut detail = findings
        .iter()
        .map(|finding| {
            format!(
                "  [{}] {}: {}",
                finding["severity"].as_str().unwrap_or_default(),
                finding["code"].as_str().unwrap_or_default(),
                finding["message"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    detail.extend(
        problems
            .iter()
            .map(|problem| format!("  [precondition] {problem}")),
    );
    detail.extend(warnings.iter().map(|warning| format!("  [note] {warning}")));
    let response = Response::new(
        "skill.validate",
        status,
        format!(
            "{id}@{version}: {:?}{}{}",
            run.outcome,
            if problems.is_empty() {
                ""
            } else {
                ", precondition rules failed"
            },
            if run.dry_run {
                " (nothing recorded)"
            } else {
                ""
            }
        ),
    )
    .reasons(
        findings
            .iter()
            .filter_map(|finding| finding["code"].as_str().map(str::to_string)),
    )
    .reference("skill_ref", format!("{id}@{version}"))
    .data(
        json!({"run": run, "precondition_problems": problems, "precondition_warnings": warnings}),
    );
    finish(&args, response, &detail)
}
