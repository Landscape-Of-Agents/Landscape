// crane scope: the effective boundary agents work within, read through the shared scope engine
// (src/scope), one file per command: inspect (the boundary, how it is enforced, and what is
// unresolved), resolve (a proposed action decided by the guard and recorded), explain (a recorded
// decision and whether it still applies), diff (the boundary against the baseline recorded when the
// rules were last signed), test (positive and adversarial probes), and audit (the boundary against
// recorded attempts and effects). These commands parse and print; every decision comes from the
// engine. They only read the rules and, like every Crane command, run only for a person.

mod audit;
mod diff;
mod explain;
mod inspect;
mod resolve;
mod skill;
mod test;

use crate::governance::workspace::Workspace;
use crate::scope::Boundary;

/** The command reference of 'crane scope' */
const USAGE: &str = "usage: crane scope inspect [PATH ...] | --skill SKILL[@N]
       crane scope resolve read|write|delete|execute|network TARGET [lines A-B] [mode MODE] | --resource REF
       crane scope explain DECISION_ID|TOOL_CALL_ID | --operation ADAPTER/OPERATION --resource REF [--skill SKILL[@N]]
       crane scope diff [--record] | --skill SKILL@N [--against SKILL@N|current]
       crane scope test [--skill SKILL[@N]]
       crane scope audit [session SESSION_ID] [days N] [--since 7d] | --skill SKILL | --run INVOCATION_ID
  (every command takes --json or --format json)";

/** Dispatch a scope command
 * Input
    - args: &[String] - arguments after "scope"
 * Output
    - Result<(), String>
*/
pub(crate) fn scope(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("inspect") => inspect::inspect(rest),
        Some("resolve") => resolve::resolve(rest),
        Some("explain") => explain::explain(rest),
        Some("diff") => diff::diff(rest),
        Some("test") => test::test(rest),
        Some("audit") => audit::audit(rest),
        _ => Err(USAGE.into()),
    }
}

/** Load the boundary of the repository in the current directory
 * Input
    - None
 * Output
    - Result<Boundary, String>
*/
fn boundary() -> Result<Boundary, String> {
    Boundary::load(Workspace::locate()?)
}

/** Print a value as pretty JSON
 * Input
    - value: &impl serde::Serialize - value
 * Output
    - Result<(), String>
*/
fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

/** The serialized name of an enumeration value, such as "REQUIRE_APPROVAL"
 * Input
    - value: &impl serde::Serialize - value
 * Output
    - String
*/
fn name(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/** Print the rule codes of a decision with their Crane operation domain and meaning
 * Input
    - rules: &[String] - rule codes
    - reasons: &[String] - the guard's explanations, in the same order where known
 * Output
    - None
*/
fn print_rules(rules: &[String], reasons: &[String]) {
    for (index, code) in rules.iter().enumerate() {
        let (domain, meaning) = crate::scope::rule(code);
        println!("  - [{domain}] {code}: {meaning}");
        if let Some(reason) = reasons.get(index) {
            println!("      {reason}");
        }
    }
    for reason in reasons.iter().skip(rules.len()) {
        println!("  - {reason}");
    }
}
