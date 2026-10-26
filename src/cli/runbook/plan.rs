// crane runbook plan: compile each step into its invocation plan (registered operations, effect
// classes, approval requirement) and cost, against the current registry and adapters, without any
// effect. Inputs that come from earlier steps are shown as placeholders; the real values exist only
// when those steps have run.

use serde_json::json;

use crate::cli::family::{inputs, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, versioned, Response, Status};
use crate::invocation::engine::InvocationRequest;
use crate::invocation::plan::{self, PlanLimits};
use crate::runbook::model::InputSource;

/** Run the command
 * Input
    - args: &[String] - arguments after "plan"
 * Output
    - Result<(), String>
*/
pub(super) fn plan(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["input"], &[])?;
    args.expect_positional(1, "crane runbook plan RUNBOOK[@N] [--input FILE]")?;
    let (id, version) = versioned(&args.positional[0])?;
    let local = Local::open()?;
    let store = runbooks(&local)?;
    let record = store.runbook(&id, version)?;
    let definition = &record.definition;
    let run_inputs = inputs(args.value("input"))?;
    let sources = local.context.sources();
    let mut steps = Vec::new();
    let mut detail = Vec::new();
    let mut blocked = false;
    let mut approvals = 0;
    for step_id in definition.order()? {
        let step = definition.step(&step_id).ok_or("unknown step")?;
        let resolved = step
            .inputs
            .iter()
            .map(|(name, source)| {
                let value = match source {
                    InputSource::Literal { value } => value.clone(),
                    InputSource::RunInput { name: input } => run_inputs
                        .get(input)
                        .cloned()
                        .unwrap_or_else(|| format!("missing-input:{input}")),
                    InputSource::StepOutput { step: from, .. } => format!("from-step:{from}"),
                };
                (name.clone(), value)
            })
            .collect();
        let request = InvocationRequest {
            capability_id: step.skill_id.clone(),
            version: step.skill_version,
            digest: step.skill_digest.clone(),
            task_id: None,
            episode_id: None,
            environment: definition.environment.clone(),
            repository: definition.repository.clone(),
            confirmed_preconditions: step.preconditions.clone(),
            inputs: resolved,
            bindings: step.bindings.clone(),
            discovery_report: None,
            compilation_report: None,
            idempotency_key: "runbook-plan".into(),
            execute: false,
            limits: PlanLimits::default(),
        };
        let built = local
            .context
            .registry
            .state
            .record(&step.skill_id, step.skill_version)
            .map_err(|error| vec![error.to_string()])
            .and_then(|stored| {
                plan::build(
                    &stored.definition,
                    None,
                    &request,
                    &local.adapters,
                    &sources,
                )
            });
        match built {
            Ok(planned) => {
                if planned.approval.required || step.approval {
                    approvals += 1;
                }
                detail.push(format!(
                    "  {step_id}: {} operations{}{}",
                    planned.operations.len(),
                    if step.approval {
                        ", runbook approval point"
                    } else {
                        ""
                    },
                    if planned.approval.required {
                        format!(", skill approval ({})", planned.approval.reasons.join("; "))
                    } else {
                        String::new()
                    }
                ));
                steps.push(json!({"step": step_id, "plan_digest": planned.plan_digest, "operations": planned.operations.iter().map(|op| json!({"operation": format!("{}.{}", op.adapter_id, op.operation_id), "effect": op.effect})).collect::<Vec<_>>(), "skill_approval": planned.approval, "approval_point": step.approval, "cost": step.cost()}));
            }
            Err(problems) => {
                blocked = true;
                detail.push(format!("  {step_id}: BLOCKED - {}", problems.join("; ")));
                steps.push(json!({"step": step_id, "blocked": problems}));
            }
        }
    }
    let total: u64 = definition
        .steps
        .iter()
        .map(|step| step.cost() * (1 + u64::from(step.max_retries)))
        .sum();
    let status = if blocked { Status::Blocked } else { Status::Ok };
    let response = Response::new("runbook.plan", status, format!("runbook {id}@{}: {} steps, up to {total} of {} budgeted operations, {approvals} approvals; nothing was executed", definition.version, definition.steps.len(), definition.budget.operations))
        .reference("runbook_ref", format!("{id}@{}", definition.version))
        .reference("digest", record.digest.clone())
        .data(json!({"steps": steps, "planned_operations": total, "budget": definition.budget}));
    finish(&args, response, &detail)
}
