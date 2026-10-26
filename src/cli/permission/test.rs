// crane permission test: the cases a skill's permissions must refuse, run through the permission
// service: an unknown operation id, an operation outside the skill's steps, Crane's own metadata,
// a path outside the repository, a business-system namespace no adapter declares, and an agent
// asking for more than contributing. Every case must be refused; nothing is executed.

use serde_json::json;

use crate::capability::Role;
use crate::capture::{Actor, ActorKind};
use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::permission;
use crate::scope::Boundary;
use crate::telemetry::model::Decision;

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill"], &[])?;
    args.expect_positional(0, "crane permission test --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let boundary = Boundary::load(local.context.workspace.clone())?;
    let actor = &local.context.actor;
    let operations = local.adapters.operations();
    let kinds = record
        .definition
        .execution
        .steps
        .iter()
        .map(|step| step.kind)
        .collect::<Vec<_>>();
    let mut cases = Vec::new();
    // Each case must be refused for its own reason, so one refusal cannot hide another
    let mut expect_refused = |name: String,
                              operation: &str,
                              resource: &str,
                              who: &Actor,
                              with_skill: bool,
                              reason: &str| {
        let answer = permission::check(
            &local.adapters,
            &boundary,
            who,
            operation,
            resource,
            with_skill.then_some(record),
        );
        let refused = matches!(answer.decision, Decision::Deny | Decision::Error)
            && answer.reasons.iter().any(|(code, _)| code == reason);
        cases.push(json!({"case": name, "status": if refused { "PASS" } else { "FAIL" }, "expected": reason, "decision": answer.decision, "reasons": answer.reasons.iter().map(|(code, _)| code.clone()).collect::<Vec<_>>()}));
    };
    expect_refused(
        "an unknown operation id".into(),
        "shell/exec",
        "repo:README.md",
        actor,
        false,
        "UNREGISTERED_OPERATION",
    );
    for op in operations
        .iter()
        .filter(|op| !op.step_kinds.iter().any(|kind| kinds.contains(kind)))
    {
        expect_refused(
            format!(
                "{}/{} implements none of its steps",
                op.adapter_id, op.operation_id
            ),
            &format!("{}/{}", op.adapter_id, op.operation_id),
            "repo:README.md",
            actor,
            true,
            "OPERATION_NOT_IN_SKILL",
        );
    }
    if let Some(op) = operations.first() {
        let id = format!("{}/{}", op.adapter_id, op.operation_id);
        // A read-only operation must not read the trust directory; any other must not write .crane
        if op.effect == crate::invocation::lifecycle::EffectClass::ReadOnly {
            if let Some(home) = &boundary.trust_home {
                let key = home.join("keys").join("repository.key");
                expect_refused(
                    format!("{id} on the trust directory's signing key"),
                    &id,
                    &format!("repo:{}", key.display()),
                    actor,
                    false,
                    "TRUST_DIRECTORY_ACCESS",
                );
            }
        } else {
            expect_refused(
                format!("{id} on Crane's signed rules"),
                &id,
                "repo:.crane/registry.json",
                actor,
                false,
                "CRANE_METADATA_WRITE",
            );
        }
        let outside = std::env::temp_dir().join("crane-permission-test.txt");
        expect_refused(
            format!("{id} outside the repository"),
            &id,
            &format!("repo:{}", outside.display()),
            actor,
            false,
            "RESOURCE_OUT_OF_SCOPE",
        );
        expect_refused(
            format!("{id} on a business system no adapter declares"),
            &id,
            "crm:account-1",
            actor,
            false,
            "RESOURCE_NAMESPACE_MISMATCH",
        );
    }
    let agent = Actor::new("agent-under-test", ActorKind::Agent);
    let escalated = local
        .context
        .registry
        .grants
        .clone()
        .with(&agent.id, &[Role::Publisher])
        .require(&agent, Role::Publisher)
        .is_err();
    cases.push(json!({"case": "an agent granted the publisher role still cannot publish", "status": if escalated { "PASS" } else { "FAIL" }}));
    let failed = cases.iter().filter(|case| case["status"] == "FAIL").count();
    let detail = cases
        .iter()
        .map(|case| {
            format!(
                "  {:<5} {} ({})",
                case["status"].as_str().unwrap_or_default(),
                case["case"].as_str().unwrap_or_default(),
                case["reasons"]
                    .as_array()
                    .map(|codes| codes
                        .iter()
                        .filter_map(|code| code.as_str())
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "permission.test",
        if failed == 0 {
            Status::Passed
        } else {
            Status::Failed
        },
        format!(
            "{}: {} cases, {failed} failed",
            reference(record),
            cases.len()
        ),
    )
    .reference("skill_ref", reference(record))
    .data(json!({"cases": cases}));
    finish(&args, response, &detail)
}
