// crane provenance verify: every link of a result's lineage verifies: sealed record digests, the
// stored skill version's digest, the plan digest, approvals bound to that plan, transitions in
// order, and (for a runbook run) the run's own digest. A broken or missing link is reported plainly.

use serde_json::json;

use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "verify"
 * Output
    - Result<(), String>
*/
pub(super) fn verify(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "receipt"], &[])?;
    args.expect_positional(0, "crane provenance verify --run INVOCATION_OR_RUN_ID")?;
    let run = args
        .value("run")
        .or_else(|| args.value("receipt"))
        .ok_or("'--run' is required")?;
    let local = Local::open()?;
    let (runbook_run, invocations) = super::records(&local, &run)?;
    let mut checks = Vec::new();
    if let Some(record) = &runbook_run {
        checks.push((
            format!("{} run digest", record.run_id),
            record.verify_digest(),
            "the run matches its sealed digest".to_string(),
        ));
    }
    for record in &invocations {
        checks.extend(super::checks(&local, record));
    }
    let broken = checks.iter().filter(|(_, holds, _)| !holds).count();
    let detail = checks
        .iter()
        .map(|(name, holds, detail)| {
            format!(
                "  {:<6} {name}: {detail}",
                if *holds { "OK" } else { "BROKEN" }
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new("provenance.verify", if broken == 0 { Status::Passed } else { Status::Failed }, format!("{run}: {} links, {broken} broken", checks.len()))
        .reasons(checks.iter().filter(|(_, holds, _)| !holds).map(|(name, _, _)| format!("BROKEN:{name}")))
        .reference("run_ref", run.clone())
        .data(json!({"checks": checks.iter().map(|(name, holds, detail)| json!({"check": name, "holds": holds, "detail": detail})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
