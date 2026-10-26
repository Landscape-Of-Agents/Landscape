// crane constraint test: the limits hold at their boundaries, through the engine's own ceiling check
// and the registry's applicability matcher: defaults and exact ceilings are accepted, one step over
// any ceiling is refused, an excluded value is never applicable, and an unknown environment or task
// class is never a match.

use serde_json::json;

use crate::capability::query::{ApplicabilityMatcher, ApplicabilityQuery, ExactMatcher};
use crate::capture::Truth;
use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::engine::within_ceiling;
use crate::invocation::plan::PlanLimits;

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill"], &[])?;
    args.expect_positional(0, "crane constraint test --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let definition = &record.definition;
    let base = PlanLimits::default();
    let ceiling = PlanLimits {
        max_operations: 64,
        max_output_bytes: 1024 * 1024,
        deadline_ms: 600_000,
        max_retries: base.max_retries + 2,
    };
    let mut cases = vec![
        (
            "the default limits are within the ceiling".to_string(),
            within_ceiling(&base).is_ok(),
        ),
        (
            "limits exactly at the ceiling are accepted".to_string(),
            within_ceiling(&ceiling).is_ok(),
        ),
    ];
    for (name, over) in [
        (
            "operations",
            PlanLimits {
                max_operations: 65,
                ..ceiling
            },
        ),
        (
            "output",
            PlanLimits {
                max_output_bytes: 1024 * 1024 + 1,
                ..ceiling
            },
        ),
        (
            "deadline",
            PlanLimits {
                deadline_ms: 600_001,
                ..ceiling
            },
        ),
        (
            "retries",
            PlanLimits {
                max_retries: ceiling.max_retries + 1,
                ..ceiling
            },
        ),
    ] {
        cases.push((
            format!("one over the {name} ceiling is refused"),
            within_ceiling(&over).is_err(),
        ));
    }
    let class = definition.applicability.task_classes.first().cloned();
    let environment = definition.applicability.environments.first().cloned();
    let query = |task_class: Option<String>, environment: Option<String>| ApplicabilityQuery {
        task_class,
        environment,
        satisfied_preconditions: Vec::new(),
        entities: Vec::new(),
    };
    let unknown = ExactMatcher.evaluate(
        definition,
        &query(Some("never-declared-task".into()), environment.clone()),
    );
    cases.push((
        "an undeclared task class is never applicable".into(),
        unknown.applicable != Truth::Yes,
    ));
    let nowhere = ExactMatcher.evaluate(
        definition,
        &query(class.clone(), Some("never-declared-environment".into())),
    );
    cases.push((
        "an undeclared environment is never applicable".into(),
        nowhere.applicable != Truth::Yes,
    ));
    for exclusion in definition
        .applicability
        .exclusions
        .iter()
        .filter(|exclusion| exclusion.kind == "environment")
    {
        let excluded = ExactMatcher.evaluate(
            definition,
            &query(class.clone(), Some(exclusion.value.clone())),
        );
        cases.push((
            format!("excluded environment {} is not applicable", exclusion.value),
            excluded.applicable != Truth::Yes,
        ));
    }
    let failed = cases.iter().filter(|(_, passed)| !passed).count();
    let detail = cases
        .iter()
        .map(|(name, passed)| format!("  {:<5} {name}", if *passed { "PASS" } else { "FAIL" }))
        .collect::<Vec<_>>();
    let response = Response::new("constraint.test", if failed == 0 { Status::Passed } else { Status::Failed }, format!("{}: {} cases, {failed} failed", reference(record), cases.len()))
        .reference("skill_ref", reference(record))
        .data(json!({"cases": cases.iter().map(|(name, passed)| json!({"case": name, "status": if *passed { "PASS" } else { "FAIL" }})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
