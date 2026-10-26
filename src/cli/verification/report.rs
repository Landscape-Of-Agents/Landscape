// crane verification report: the verification summary of a run, bound by a digest to the run's
// record (and so to its exact skill version, plan, and policy versions): every required check,
// its evidence, failures, unknowns, and unresolved effects. Success only when every required check
// passed and no effect is unresolved.

use serde_json::json;

use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::trust::crypto::digest_json;

/** Run the command
 * Input
    - args: &[String] - arguments after "report"
 * Output
    - Result<(), String>
*/
pub(super) fn report(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run"], &[])?;
    args.expect_positional(0, "crane verification report --run INVOCATION_ID")?;
    let run = args.value("run").ok_or("'--run' is required")?;
    let local = Local::open()?;
    let record = local.ledger.get(&run)?;
    let rows = super::results(&record);
    let mut status = super::overall(&rows);
    if status == Status::Passed && !record.unresolved_effects.is_empty() {
        status = Status::Unknown;
    }
    let body = json!({"run": run, "skill": format!("{}@{}", record.capability_id, record.version), "skill_digest": record.digest, "plan_digest": record.plan.as_ref().map(|plan| plan.plan_digest.clone()), "record_digest": record.record_digest, "state": record.state, "checks": rows, "unresolved_effects": record.unresolved_effects, "failures": record.failures, "governance": record.governance});
    let digest = digest_json("verification/report/v1", &body)[..32].to_string();
    let mut detail = super::lines(&rows);
    detail.extend(
        record
            .unresolved_effects
            .iter()
            .map(|effect| format!("  unresolved effect: {effect}")),
    );
    let response = Response::new(
        "verification.report",
        status,
        format!(
            "run {run} ({}@{}): {:?}, report {digest}",
            record.capability_id, record.version, record.state
        ),
    )
    .reference("run_ref", run.clone())
    .reference("provenance_ref", record.record_digest.clone())
    .reference("report_digest", digest.clone())
    .data(json!({"report": body, "report_digest": digest}));
    finish(&args, response, &detail)
}
