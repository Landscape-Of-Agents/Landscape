// crane recovery test: the recovery rules, on the invocation demonstration's fixture systems (no real
// system is touched): a timeout after the external effect is reconciled, not retried (the release
// is dispatched exactly once); repeating a request with the same key returns the same invocation
// without another dispatch; a failed verification never becomes success; and a revoked version is
// blocked before anything runs.

use serde_json::json;

use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::demo::run as demonstration;
use crate::invocation::lifecycle::InvocationState;

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane recovery test")?;
    let report = demonstration().map_err(String::from)?;
    let cases = [
        (
            "a timeout after the effect is reconciled, never retried (one dispatch)",
            report.release_dispatches == 1,
        ),
        (
            "the reconciled invocation ends verified",
            report.reconciled.state == InvocationState::Succeeded,
        ),
        (
            "a repeated request returns the same invocation",
            report.repeat_returns_same,
        ),
        (
            "a failed verification is not success",
            report.failed_verification != InvocationState::Succeeded,
        ),
        (
            "a revoked version is blocked before anything runs",
            report.revoked == InvocationState::Blocked,
        ),
        (
            "an unsupported operation is blocked without dispatching anything",
            report.unsupported.0 == InvocationState::Blocked && report.unsupported.2 == 0,
        ),
    ];
    let failed = cases.iter().filter(|(_, passed)| !passed).count();
    let detail = cases
        .iter()
        .map(|(name, passed)| format!("  {:<5} {name}", if *passed { "PASS" } else { "FAIL" }))
        .collect::<Vec<_>>();
    let response = Response::new("recovery.test", if failed == 0 { Status::Passed } else { Status::Failed }, format!("{} recovery cases on fixture systems, {failed} failed", cases.len()))
        .data(json!({"cases": cases.iter().map(|(name, passed)| json!({"case": name, "status": if *passed { "PASS" } else { "FAIL" }})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
