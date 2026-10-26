// crane skill plan / crane skill invoke: one invocation request through the invocation engine.
// plan stops after the plan and preflight (no effects); invoke executes when nothing needs approval.
// Both bind the exact version and digest, use only the reviewed binding set's registered
// operations, and report the engine's state: a plan needing approval stops for a person, a blocked
// preflight is blocked, and only verified postconditions are a success.

use crate::cli::capability::read_json;
use crate::cli::family::{exact, inputs, invocation_status, invocation_view, list};
use crate::cli::invoke::{with_engine, Local};
use crate::cli::protocol::{finish, parse, Response};
use crate::invocation::engine::InvocationRequest;
use crate::invocation::plan::{BindingSet, PlanLimits};

/** Run plan (execute false) or invoke (execute true)
 * Input
    - args: &[String] - arguments after "plan" or "invoke"
    - execute: bool - invoke
 * Output
    - Result<(), String>
*/
pub(super) fn submit(args: &[String], execute: bool) -> Result<(), String> {
    let command = if execute { "invoke" } else { "plan" };
    let args = parse(
        args,
        &[
            "bindings",
            "input",
            "confirm",
            "environment",
            "repository",
            "idempotency-key",
            "task",
        ],
        &[],
    )?;
    args.expect_positional(1, &format!("crane skill {command} SKILL@N --bindings FILE [--input FILE] [--confirm ID,ID] [--environment ENV] [--repository REPO] --idempotency-key KEY"))?;
    let (id, version) = exact(&args.positional[0], "a plan")?;
    let bindings: BindingSet = serde_json::from_value(read_json(
        &args
            .value("bindings")
            .ok_or("'--bindings' (the reviewed binding set) is required")?,
    )?)
    .map_err(|error| format!("bindings: {error}"))?;
    let key = match args.value("idempotency-key") {
        Some(key) => key,
        None if !execute => format!("skill-plan-{}", crate::platform::now_millis()),
        None => return Err("'--idempotency-key' is required to invoke, so a repeated request never repeats an effect".into()),
    };
    let mut local = Local::open()?;
    let digest = local
        .context
        .registry
        .state
        .record(&id, version)?
        .definition_digest
        .clone();
    let request = InvocationRequest {
        capability_id: id.clone(),
        version,
        digest,
        task_id: args.value("task"),
        episode_id: None,
        environment: args.value("environment"),
        repository: args.value("repository"),
        confirmed_preconditions: list(args.value("confirm")),
        inputs: inputs(args.value("input"))?,
        bindings,
        discovery_report: None,
        compilation_report: None,
        idempotency_key: key,
        execute,
        limits: PlanLimits::default(),
    };
    let actor = local.context.actor.clone();
    let record = with_engine(&mut local, |engine| {
        engine.submit(&actor, request).map_err(String::from)
    })?;
    let status = invocation_status(record.state);
    let mut response = Response::new(
        &format!("skill.{command}"),
        status,
        format!(
            "{id}@{version}: invocation {} is {:?}",
            record.invocation_id, record.state
        ),
    )
    .reasons(record.failures.clone())
    .reference("skill_ref", format!("{id}@{version}"))
    .reference("run_ref", record.invocation_id.clone())
    .data(invocation_view(&record));
    if let Some(plan) = &record.plan {
        response = response.reference("plan_digest", plan.plan_digest.clone());
        if record.approval.required {
            response = response.reference(
                "approval_ref",
                format!("{}@{}", record.invocation_id, plan.plan_digest),
            );
        }
    }
    let detail = record
        .preflight
        .iter()
        .map(|check| {
            format!(
                "  preflight {}: {:?} - {}",
                check.check_id, check.status, check.detail
            )
        })
        .collect::<Vec<_>>();
    finish(&args, response, &detail)
}
