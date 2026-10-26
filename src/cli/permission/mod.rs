// crane permission: what a skill may do and whether one typed operation may act on one resource,
// decided by the permission service (src/invocation/permission.rs), which uses the scope engine for
// repository resources. One file per command: inspect, check, test.

mod check;
mod inspect;
mod test;

/** The command reference of 'crane permission' */
const USAGE: &str = "usage: crane permission inspect --skill SKILL[@N]
       crane permission check --operation ADAPTER/OPERATION --resource REF [--skill SKILL[@N]]
       crane permission test --skill SKILL[@N]
  (REF: repo:PATH (or a path), or NAMESPACE:ID for a business system; every command takes --format json)";

/** Dispatch a permission command
 * Input
    - args: &[String] - arguments after "permission"
 * Output
    - Result<(), String>
*/
pub(crate) fn permission(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("inspect") => inspect::inspect(rest),
        Some("check") => check::check(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}
