// crane skill: the lifecycle of a skill, one file per command. A skill is a capability (a
// versioned procedure with typed inputs, registered typed operations, preconditions, approval,
// verification, and recovery); these commands are the department-neutral names for the capability
// pipeline's services, which they call without adding rules of their own:
//
//   create   - a draft from a reviewed template (grants nothing)
//   validate - the validation gates (does not activate)
//   inspect  - a plain-language summary and the effective contract
//   test     - the Validation Lab on the exact digest (nothing is executed)
//   plan     - an invocation plan without effects
//   activate - publication by a person other than the author, signed into the shared store
//   invoke   - an invocation through the engine (preflight, approval, verification)
//   retire   - deprecation (no new use), shared
//   revoke   - revocation (blocked everywhere it is pulled), shared

mod activate;
mod create;
mod inspect;
mod invoke;
mod plan;
mod retire;
mod revoke;
mod test;
mod validate;

/** The command reference of 'crane skill' */
const USAGE: &str = "usage: crane skill create --from-template TEMPLATE --name NAME [--department DEPT] [--id ID]
       crane skill validate SKILL[@N] [--dry-run]
       crane skill inspect SKILL[@N]
       crane skill test SKILL[@N] --suite PLAN_FILE
       crane skill plan SKILL@N --bindings FILE [--input FILE] [--confirm ID,ID] [--environment ENV] [--repository REPO] [--idempotency-key KEY]
       crane skill activate SKILL@N --review-ref REF [--acknowledge CODE,CODE]
       crane skill invoke SKILL@N --bindings FILE [--input FILE] [--confirm ID,ID] [--environment ENV] [--repository REPO] --idempotency-key KEY
       crane skill retire SKILL@N --reason TEXT
       crane skill revoke SKILL@N --reason TEXT
  (every command takes --format json; templates: read-only-lookup, repository-change, business-record-update)";

/** Dispatch a skill command
 * Input
    - args: &[String] - arguments after "skill"
 * Output
    - Result<(), String>
*/
pub(crate) fn skill(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("create") => create::create(rest),
        Some("validate") => validate::validate(rest),
        Some("inspect") => inspect::inspect(rest),
        Some("test") => test::test(rest),
        Some("plan") => plan::submit(rest, false),
        Some("activate") => activate::activate(rest),
        Some("invoke") => invoke::invoke(rest),
        Some("retire") => retire::retire(rest),
        Some("revoke") => revoke::revoke(rest),
        _ => Err(USAGE.into()),
    }
}
