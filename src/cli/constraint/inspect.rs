// crane constraint inspect: the effective limits of a skill: engine defaults and hard ceilings,
// the skill's exclusions, environments, task classes, risk and review level, side effects, and
// dependencies with their pinned versions and what they are now.

use serde_json::json;

use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::plan::PlanLimits;

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill"], &[])?;
    args.expect_positional(0, "crane constraint inspect --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let definition = &record.definition;
    let applicability = &definition.applicability;
    let current = &local.context.current;
    let dependencies = definition
        .dependencies
        .iter()
        .map(|dependency| {
            let now = match dependency.kind {
                crate::capability::model::DependencyKind::Zone => current.zones.get(&dependency.target_id).cloned(),
                crate::capability::model::DependencyKind::Flow => current.flows.get(&dependency.target_id).cloned(),
                crate::capability::model::DependencyKind::Policy => current.policies.get(&dependency.target_id).cloned(),
                _ => None,
            };
            json!({"kind": dependency.kind, "target": dependency.target_id, "required": dependency.required, "constraint": dependency.constraint, "now": now})
        })
        .collect::<Vec<_>>();
    let defaults = PlanLimits::default();
    let mut detail = vec![
        format!("  limits: {} operations, {} bytes per result, {} ms, {} retries (ceilings 64, 1048576, 600000, {})", defaults.max_operations, defaults.max_output_bytes, defaults.deadline_ms, defaults.max_retries, defaults.max_retries + 2),
        format!("  applies to task classes {:?} in environments {:?}", applicability.task_classes, applicability.environments),
        format!("  risk {:?}, review {:?}", applicability.risk, applicability.review_level).to_lowercase(),
    ];
    for exclusion in &applicability.exclusions {
        detail.push(format!(
            "  never for {} {}: {}",
            exclusion.kind, exclusion.value, exclusion.reason
        ));
    }
    for effect in &definition.execution.side_effects {
        detail.push(format!(
            "  side effect on {} ({:?} risk, reversible: {})",
            effect.system,
            effect.risk,
            effect
                .reversible
                .map_or("unknown".into(), |value| value.to_string())
        ));
    }
    for dependency in &dependencies {
        detail.push(format!(
            "  depends on {} {}: {} (now {})",
            dependency["kind"].as_str().unwrap_or_default(),
            dependency["target"].as_str().unwrap_or_default(),
            dependency["constraint"],
            dependency["now"]
        ));
    }
    let response = Response::new("constraint.inspect", Status::Ok, format!("{}: limits and exclusions", reference(record)))
        .reference("skill_ref", reference(record))
        .data(json!({"limits": defaults, "ceilings": {"max_operations": 64, "max_output_bytes": 1048576, "deadline_ms": 600000, "max_retries": defaults.max_retries + 2}, "task_classes": applicability.task_classes, "environments": applicability.environments, "exclusions": applicability.exclusions, "risk": applicability.risk, "review_level": applicability.review_level, "side_effects": definition.execution.side_effects, "dependencies": dependencies}));
    finish(&args, response, &detail)
}
