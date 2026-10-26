// crane recovery: make partial failure and uncertain effects explicit, and move forward only safely:
// reconcile against the authoritative system before anything is retried, resume only what was
// reconciled and re-authorized, and abort without claiming a rollback. Works on skill invocations
// (inv_...) and runbook runs (rbr_...). One file per command: inspect, reconcile, resume, abort, test.

mod abort;
mod inspect;
mod reconcile;
mod resume;
mod test;

/** The command reference of 'crane recovery' */
const USAGE: &str = "usage: crane recovery inspect --run INVOCATION_OR_RUN_ID
       crane recovery reconcile --run INVOCATION_OR_RUN_ID [--step STEP]
       crane recovery resume --run INVOCATION_OR_RUN_ID [--from-step STEP]
       crane recovery abort --run INVOCATION_OR_RUN_ID --reason TEXT
       crane recovery test
  (every command takes --format json)";

/** Dispatch a recovery command
 * Input
    - args: &[String] - arguments after "recovery"
 * Output
    - Result<(), String>
*/
pub(crate) fn recovery(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("inspect") => inspect::inspect(rest),
        Some("reconcile") => reconcile::reconcile(rest),
        Some("resume") => resume::resume(rest),
        Some("abort") => abort::abort(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}
