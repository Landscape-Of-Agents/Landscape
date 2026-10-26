// crane revocation: withdraw the authority to use a skill version, a runbook version, or an
// approval, and see what is withdrawn. Revocation is checked before every protected effect (skill
// preflight, every runbook step), reaches other machines through the signed shared skill store, and
// is never undone: a new version, or a new approval, is the way back. One file per command:
// inspect, revoke, test.

mod inspect;
mod revoke;
mod test;

/** The command reference of 'crane revocation' */
const USAGE: &str = "usage: crane revocation inspect --skill SKILL[@N] | --runbook RUNBOOK[@N] | --run RUN_ID | --actor PUBLIC_KEY
       crane revocation revoke --target skill:SKILL@N|runbook:RUNBOOK@N|approval:APPROVAL_REF --reason TEXT
       crane revocation test
  (every command takes --format json)";

/** Dispatch a revocation command
 * Input
    - args: &[String] - arguments after "revocation"
 * Output
    - Result<(), String>
*/
pub(crate) fn revocation(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("inspect") => inspect::inspect(rest),
        Some("revoke") => revoke::revoke(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}
