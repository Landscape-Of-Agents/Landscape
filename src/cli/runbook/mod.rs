// crane runbook: versioned workflows over exact skill versions, one file per command. The runbook
// service (src/runbook) holds the rules; these commands parse, call it, and print:
//
//   create   - a draft from a file, or a linear draft from skills (grants nothing)
//   validate - structure, plus every pinned skill, digest, operation, and isolation profile
//   inspect  - the step map, branches, approval points, budget, and lifecycle
//   activate - review by a person other than the author (an addition to the command catalog: the
//              lifecycle needs reviewed activation before anything runs)
//   plan     - each step's invocation plan, cost, and approvals, with no effect
//   test     - the refusals a run must make (draft, revoked, missing or secret inputs) and the
//              plan's coverage of failure paths and budget, without running anything
//   run      - start a run through the orchestrator
//   status   - a run's steps, approvals, and outcome
//   cancel   - cancel a run safely
//   resume   - continue a paused run, or retry a step without effects
//   revoke   - stop a version: no new runs, and running runs stop at their next step

mod activate;
mod cancel;
mod create;
mod inspect;
mod plan;
mod resume;
mod revoke;
mod run;
mod status;
mod test;
mod validate;

/** The command reference of 'crane runbook' */
const USAGE: &str = "usage: crane runbook create (--file RUNBOOK.json | --from-template linear --name NAME --skills SKILL@N,SKILL@N --bindings FILE,FILE [--department DEPT] [--id ID] [--environment ENV] [--repository REPO])
       crane runbook validate RUNBOOK[@N]
       crane runbook inspect RUNBOOK[@N]
       crane runbook activate RUNBOOK@N --review-ref REF
       crane runbook plan RUNBOOK[@N] [--input FILE]
       crane runbook test RUNBOOK[@N]
       crane runbook run RUNBOOK@N [--input FILE] [--confirm ID,ID] --idempotency-key KEY
       crane runbook status RUN_ID
       crane runbook cancel RUN_ID --reason TEXT
       crane runbook resume RUN_ID [--from-step STEP]
       crane runbook revoke RUNBOOK@N --reason TEXT
  (every command takes --format json)";

/** Dispatch a runbook command
 * Input
    - args: &[String] - arguments after "runbook"
 * Output
    - Result<(), String>
*/
pub(crate) fn runbook(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("create") => create::create(rest),
        Some("validate") => validate::validate(rest),
        Some("inspect") => inspect::inspect(rest),
        Some("activate") => activate::activate(rest),
        Some("plan") => plan::plan(rest),
        Some("test") => test::test(rest),
        Some("run") => run::run(rest),
        Some("status") => status::status(rest),
        Some("cancel") => cancel::cancel(rest),
        Some("resume") => resume::resume(rest),
        Some("revoke") => revoke::revoke(rest),
        _ => Err(USAGE.into()),
    }
}
