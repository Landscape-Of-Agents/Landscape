// crane provenance test: tampering is detected, on copies of a record (a run of this repository with
// --run, otherwise one from the invocation demonstration's fixture world): an altered state, a
// changed skill digest, an approval for another plan, and reordered transitions each break a link,
// and an untouched record verifies. Nothing is stored.

use serde_json::json;

use crate::capability::demo as capability_demo;
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::demo::{diagnostics_request, diagnostics_world};
use crate::invocation::engine::InvocationRecord;
use crate::invocation::lifecycle::InvocationState;

/** Report whether a record's own lineage checks hold (record digest, plan, approvals, order)
 * Input
    - record: &InvocationRecord - record
 * Output
    - bool
*/
fn holds(record: &InvocationRecord) -> bool {
    let plan_ok = record.plan.as_ref().is_none_or(|plan| {
        plan.compute_digest() == plan.plan_digest
            && record
                .approvals
                .iter()
                .all(|approval| approval.plan_digest == plan.plan_digest)
    });
    let ordered = record
        .transitions
        .iter()
        .enumerate()
        .all(|(index, transition)| transition.sequence == index as u64 + 1)
        && record
            .transitions
            .windows(2)
            .all(|pair| pair[1].from == Some(pair[0].to));
    record.verify_digest() && plan_ok && ordered
}

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run"], &[])?;
    args.expect_positional(0, "crane provenance test [--run INVOCATION_ID]")?;
    let record = match args.value("run") {
        Some(run) => Local::open()?.ledger.get(&run)?,
        None => {
            let mut world = diagnostics_world().map_err(String::from)?;
            let reader = capability_demo::reader(&world.cast.engineer).map_err(String::from)?;
            let actor = world.cast.engineer.clone();
            let request = diagnostics_request(&world, "provenance-test");
            world
                .engine(&reader)
                .submit(&actor, request)
                .map_err(String::from)?
        }
    };
    let mut altered = record.clone();
    altered.state = if record.state == InvocationState::Succeeded {
        InvocationState::Failed
    } else {
        InvocationState::Succeeded
    };
    let mut digest = record.clone();
    digest.digest = "0".repeat(32);
    let mut reordered = record.clone();
    reordered.transitions.reverse();
    let mut cases = vec![
        ("the untouched record verifies", holds(&record)),
        ("an altered outcome is detected", !holds(&altered)),
        ("a changed skill digest is detected", !holds(&digest)),
        (
            "reordered transitions are detected",
            record.transitions.len() < 2 || !holds(&reordered),
        ),
    ];
    if let Some(plan) = &record.plan {
        let mut approved = record.clone();
        approved
            .approvals
            .push(crate::invocation::engine::Approval {
                approver: record.requester.clone(),
                plan_digest: format!("not-{}", plan.plan_digest),
                environment: None,
                repository: None,
                at: 0,
                revoked: None,
            });
        approved.seal();
        cases.push((
            "an approval for another plan is detected even when resealed",
            !holds(&approved),
        ));
    }
    let failed = cases.iter().filter(|(_, passed)| !passed).count();
    let detail = cases
        .iter()
        .map(|(name, passed)| format!("  {:<5} {name}", if *passed { "PASS" } else { "FAIL" }))
        .collect::<Vec<_>>();
    let response = Response::new("provenance.test", if failed == 0 { Status::Passed } else { Status::Failed }, format!("{} tamper cases on {}, {failed} failed", cases.len(), record.invocation_id))
        .data(json!({"cases": cases.iter().map(|(name, passed)| json!({"case": name, "status": if *passed { "PASS" } else { "FAIL" }})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
