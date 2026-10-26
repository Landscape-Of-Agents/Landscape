// crane constraint: the hard limits a skill runs within: the invocation engine's ceilings (which
// signed policy may narrow, never raise), the skill's own exclusions, environments, risk, review
// level, side effects, and pinned dependencies. One file per command: inspect, validate, test.

mod inspect;
mod test;
mod validate;

/** The command reference of 'crane constraint' */
const USAGE: &str = "usage: crane constraint inspect --skill SKILL[@N]
       crane constraint validate --skill SKILL[@N]
       crane constraint test --skill SKILL[@N]
  (every command takes --format json)";

/** Dispatch a constraint command
 * Input
    - args: &[String] - arguments after "constraint"
 * Output
    - Result<(), String>
*/
pub(crate) fn constraint(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("inspect") => inspect::inspect(rest),
        Some("validate") => validate::validate(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}
