// crane permission inspect: the effective permissions of a skill: for each step, the registered
// operations that can implement it and their effect classes, which steps change state, the approval
// its plans need, and the ceilings no skill can raise (engine limits, agents only read). A skill
// requests these; it never grants them.

use serde_json::json;

use crate::capability::Role;
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
    args.expect_positional(0, "crane permission inspect --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let definition = &record.definition;
    let operations = local.adapters.operations();
    let mut detail = Vec::new();
    let mut steps = Vec::new();
    let mut unsupported = 0;
    for step in &definition.execution.steps {
        let candidates = operations
            .iter()
            .filter(|operation| operation.step_kinds.contains(&step.kind))
            .collect::<Vec<_>>();
        if candidates.is_empty() && step.kind != crate::capability::model::StepKind::Approval {
            unsupported += 1;
        }
        detail.push(format!(
            "  {} ({:?}{}): {}",
            step.step_id,
            step.kind,
            if step.kind.mutating() {
                ", changes state"
            } else {
                ""
            },
            if candidates.is_empty() {
                "no registered operation".into()
            } else {
                candidates
                    .iter()
                    .map(|op| format!("{}/{} ({:?})", op.adapter_id, op.operation_id, op.effect))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        steps.push(json!({"step": step.step_id, "kind": step.kind, "changes_state": step.kind.mutating(), "operations": candidates.iter().map(|op| json!({"operation": format!("{}/{}", op.adapter_id, op.operation_id), "effect": op.effect, "idempotent": op.idempotent, "reconcilable": op.reconcilable})).collect::<Vec<_>>()}));
    }
    let changes = definition
        .execution
        .steps
        .iter()
        .any(|step| step.kind.mutating());
    let actor = &local.context.actor;
    let roles = [Role::Contributor, Role::Validator, Role::Publisher]
        .into_iter()
        .filter(|role| local.context.registry.grants.require(actor, *role).is_ok())
        .collect::<Vec<_>>();
    detail.push(format!(
        "  approval: {}",
        if changes || definition.applicability.risk >= crate::capability::model::RiskLevel::Medium {
            format!(
                "required, by at least {:?}",
                definition.applicability.review_level
            )
        } else {
            "only if a plan uses a state-changing operation".into()
        }
    ));
    detail.push(format!(
        "  your roles: {roles:?}; ceilings: 64 operations, 10 minutes, agents read only"
    ));
    let status = if unsupported > 0 {
        Status::Blocked
    } else {
        Status::Ok
    };
    let response = Response::new("permission.inspect", status, format!("{}: {} steps, {unsupported} without a registered operation", reference(record), definition.execution.steps.len()))
        .reference("skill_ref", reference(record))
        .data(json!({"steps": steps, "risk": definition.applicability.risk, "review_level": definition.applicability.review_level, "roles": roles, "default_limits": PlanLimits::default()}));
    finish(&args, response, &detail)
}
