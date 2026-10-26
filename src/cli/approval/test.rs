// crane approval test: the approval rules, exercised on the invocation demonstration's fixture
// world (in memory; no real system is touched and no authority is granted): the requester cannot
// approve, an agent cannot, a wrong digest is refused, a plan cannot run before approval, a revoked
// approval does not count, a denied invocation can never be approved, and a valid approval runs it.

use serde_json::json;

use crate::capability::demo as capability_demo;
use crate::capture::{Actor, ActorKind};
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::demo::{refund_request, refund_world};
use crate::invocation::lifecycle::InvocationState;

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane approval test")?;
    let mut world = refund_world().map_err(String::from)?;
    let requester = world.cast.engineer.clone();
    let lead = world.cast.lead.clone();
    let reader = capability_demo::reader(&requester).map_err(String::from)?;
    let first = refund_request(&world, "approval-test-1").map_err(String::from)?;
    let second = refund_request(&world, "approval-test-2").map_err(String::from)?;
    let agent = Actor::new("agent-under-test", ActorKind::Agent);
    let mut engine = world.engine(&reader);
    let pending = engine.submit(&requester, first).map_err(String::from)?;
    let digest = pending
        .plan
        .as_ref()
        .map(|plan| plan.plan_digest.clone())
        .unwrap_or_default();
    let id = pending.invocation_id.clone();
    let mut cases = vec![
        (
            "the plan waits for approval",
            pending.state == InvocationState::AwaitingApproval,
        ),
        (
            "it cannot run before approval",
            engine.run(&requester, &id).is_err(),
        ),
        (
            "the requester cannot approve their own plan",
            engine.approve(&requester, &id, &digest).is_err(),
        ),
        (
            "an agent cannot approve",
            engine.approve(&agent, &id, &digest).is_err(),
        ),
        (
            "an approval naming another digest is refused",
            engine.approve(&lead, &id, "0000").is_err(),
        ),
    ];
    cases.push((
        "a person other than the requester can approve",
        engine.approve(&lead, &id, &digest).is_ok(),
    ));
    cases.push((
        "the approver can revoke it before it is used",
        engine.revoke_approval(&lead, &id, "test").is_ok(),
    ));
    cases.push((
        "a revoked approval does not let it run",
        engine.run(&requester, &id).is_err(),
    ));
    cases.push((
        "a new approval lets it run",
        engine.approve(&lead, &id, &digest).is_ok() && engine.run(&requester, &id).is_ok(),
    ));
    cases.push((
        "a used approval cannot be revoked",
        engine.revoke_approval(&lead, &id, "late").is_err(),
    ));
    let other = engine.submit(&requester, second).map_err(String::from)?;
    let other_digest = other
        .plan
        .as_ref()
        .map(|plan| plan.plan_digest.clone())
        .unwrap_or_default();
    cases.push((
        "the requester cannot deny their own plan",
        engine.deny(&requester, &other.invocation_id, "x").is_err(),
    ));
    cases.push((
        "another person can deny it",
        engine.deny(&lead, &other.invocation_id, "not now").is_ok(),
    ));
    cases.push((
        "a denied plan can never be approved",
        engine
            .approve(&lead, &other.invocation_id, &other_digest)
            .is_err(),
    ));
    let failed = cases.iter().filter(|(_, passed)| !passed).count();
    let detail = cases
        .iter()
        .map(|(name, passed)| format!("  {:<5} {name}", if *passed { "PASS" } else { "FAIL" }))
        .collect::<Vec<_>>();
    let response = Response::new("approval.test", if failed == 0 { Status::Passed } else { Status::Failed }, format!("{} approval cases on fixture systems, {failed} failed", cases.len()))
        .data(json!({"cases": cases.iter().map(|(name, passed)| json!({"case": name, "status": if *passed { "PASS" } else { "FAIL" }})).collect::<Vec<_>>(), "world": "fixture (invocation demonstration)"}));
    finish(&args, response, &detail)
}
