// crane precondition: the facts that must hold before a skill (or one of its steps) starts,
// evaluated by the precondition service (src/capability/preconditions.rs): a claim-backed fact holds
// only when its claim verifies from evidence; a manual fact is declared by a person, never observed.
// One file per command: inspect, validate, evaluate, test.

mod evaluate;
mod inspect;
mod test;
mod validate;

/** The command reference of 'crane precondition' */
const USAGE: &str = "usage: crane precondition inspect --skill SKILL[@N] [--step STEP]
       crane precondition validate --skill SKILL[@N]
       crane precondition evaluate --skill SKILL[@N] [--step STEP] [--confirm ID,ID]
       crane precondition test --skill SKILL[@N]
  (every command takes --format json)";

/** Dispatch a precondition command
 * Input
    - args: &[String] - arguments after "precondition"
 * Output
    - Result<(), String>
*/
pub(crate) fn precondition(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("inspect") => inspect::inspect(rest),
        Some("validate") => validate::validate(rest),
        Some("evaluate") => evaluate::evaluate(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}
