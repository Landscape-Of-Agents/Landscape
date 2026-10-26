// crane provenance inspect: the lineage of an invocation or a runbook run: who requested it, the
// exact skill version and digest, the plan, approvals, policy versions, transitions, and evidence.

use serde_json::json;

use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "receipt"], &[])?;
    args.expect_positional(0, "crane provenance inspect --run INVOCATION_OR_RUN_ID")?;
    let run = args
        .value("run")
        .or_else(|| args.value("receipt"))
        .ok_or("'--run' is required")?;
    let local = Local::open()?;
    let (runbook_run, invocations) = super::records(&local, &run)?;
    let mut detail = Vec::new();
    if let Some(record) = &runbook_run {
        detail.push(format!(
            "  runbook {}@{} (digest {}) started by {}",
            record.runbook_id, record.version, record.digest, record.started_by.id
        ));
    }
    for record in &invocations {
        detail.push(format!(
            "  {}: {}@{} digest {} requested by {}, {} approvals, {} transitions, now {:?}",
            record.invocation_id,
            record.capability_id,
            record.version,
            record.digest,
            record.requester.id,
            record.approvals.len(),
            record.transitions.len(),
            record.state
        ));
    }
    let response = Response::new("provenance.inspect", Status::Ok, format!("{run}: {} invocations", invocations.len()))
        .reference("run_ref", run.clone())
        .data(json!({"run": runbook_run.as_ref().map(crate::cli::family::run_view), "invocations": invocations.iter().map(super::lineage).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
