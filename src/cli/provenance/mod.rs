// crane provenance: who and what produced a result (requester, exact skill version and digest, plan,
// approvals, transitions, policy versions, evidence), and whether that lineage still verifies.
// Provenance is evidence lineage, not authority. One file per command: inspect, verify, export, test.

mod export;
mod inspect;
mod test;
mod verify;

use serde_json::{json, Value};

use crate::cli::family::runbooks;
use crate::cli::invoke::Local;
use crate::invocation::engine::InvocationRecord;

/** The command reference of 'crane provenance' */
const USAGE: &str = "usage: crane provenance inspect --run INVOCATION_OR_RUN_ID
       crane provenance verify --run INVOCATION_OR_RUN_ID
       crane provenance export --run INVOCATION_OR_RUN_ID --destination stdout|runtime
       crane provenance test [--run INVOCATION_ID]
  (every command takes --format json)";

/** The checks of one invocation record's lineage
 * Input
    - local: &Local - repository context
    - record: &InvocationRecord - invocation
 * Output
    - Vec<(String, bool, String)> check, holds, detail
*/
pub(super) fn checks(local: &Local, record: &InvocationRecord) -> Vec<(String, bool, String)> {
    let mut checks = vec![(
        format!("{} record digest", record.invocation_id),
        record.verify_digest(),
        "the record matches its sealed digest".to_string(),
    )];
    match local
        .context
        .registry
        .state
        .record(&record.capability_id, record.version)
    {
        Ok(stored) => checks.push((
            format!("{} skill version", record.invocation_id),
            stored.definition_digest == record.digest,
            format!(
                "{}@{} is stored with digest {} (the record names {})",
                record.capability_id, record.version, stored.definition_digest, record.digest
            ),
        )),
        Err(error) => checks.push((
            format!("{} skill version", record.invocation_id),
            false,
            error.to_string(),
        )),
    }
    if let Some(plan) = &record.plan {
        checks.push((
            format!("{} plan digest", record.invocation_id),
            plan.compute_digest() == plan.plan_digest,
            "the plan matches its digest".into(),
        ));
        let bound = record
            .approvals
            .iter()
            .all(|approval| approval.plan_digest == plan.plan_digest);
        checks.push((
            format!("{} approvals bound to the plan", record.invocation_id),
            bound,
            format!("{} approvals", record.approvals.len()),
        ));
    }
    let ordered = record
        .transitions
        .iter()
        .enumerate()
        .all(|(index, transition)| transition.sequence == index as u64 + 1)
        && record
            .transitions
            .windows(2)
            .all(|pair| pair[0].at <= pair[1].at && pair[1].from == Some(pair[0].to));
    checks.push((
        format!("{} transitions in order", record.invocation_id),
        ordered,
        format!("{} transitions", record.transitions.len()),
    ));
    checks
}

/** Describe an invocation's lineage
 * Input
    - record: &InvocationRecord - invocation
 * Output
    - Value
*/
pub(super) fn lineage(record: &InvocationRecord) -> Value {
    json!({
        "invocation": record.invocation_id,
        "requester": record.requester,
        "skill": format!("{}@{}", record.capability_id, record.version),
        "skill_digest": record.digest,
        "plan_digest": record.plan.as_ref().map(|plan| plan.plan_digest.clone()),
        "approvals": record.approvals,
        "governance": record.governance,
        "transitions": record.transitions,
        "evidence": record.verifications.iter().flat_map(|result| result.evidence_refs.clone()).collect::<Vec<_>>(),
        "record_digest": record.record_digest,
    })
}

/** Collect the invocation records behind a run reference (an invocation, or a runbook run's steps)
 * Input
    - local: &Local - repository context
    - run: &str - INVOCATION_ID or RUN_ID
 * Output
    - Result<(Option<crate::runbook::store::RunRecord>, Vec<InvocationRecord>), String>
*/
pub(super) fn records(
    local: &Local,
    run: &str,
) -> Result<
    (
        Option<crate::runbook::store::RunRecord>,
        Vec<InvocationRecord>,
    ),
    String,
> {
    if run.starts_with("rbr_") {
        let store = runbooks(local)?;
        let record = store.run(run)?.clone();
        let invocations = record
            .steps
            .values()
            .filter_map(|step| step.invocation_id.as_ref())
            .map(|id| local.ledger.get(id).map_err(String::from))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((Some(record), invocations))
    } else {
        Ok((None, vec![local.ledger.get(run)?]))
    }
}

/** Dispatch a provenance command
 * Input
    - args: &[String] - arguments after "provenance"
 * Output
    - Result<(), String>
*/
pub(crate) fn provenance(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("inspect") => inspect::inspect(rest),
        Some("verify") => verify::verify(rest),
        Some("export") => export::export(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}
