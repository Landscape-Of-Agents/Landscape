// crane isolation: what each execution path really confines (files, processes, network,
// credentials), from the isolation service (src/invocation/isolation.rs). Nothing here sandboxes
// anything; a profile that cannot be enforced blocks the skill instead of being reported as safe.
// One file per command: inspect, validate, test, profiles.

mod inspect;
mod profiles;
mod test;
mod validate;

/** The command reference of 'crane isolation' */
const USAGE: &str = "usage: crane isolation profiles
       crane isolation inspect --skill SKILL[@N]
       crane isolation validate --skill SKILL[@N] [--environment PROFILE]
       crane isolation test --profile PROFILE
  (profiles: agent-hooks, typed-adapters, sandbox (not available); every command takes --format json)";

/** Human lines for a profile
 * Input
    - profile: &crate::invocation::isolation::Profile - profile
 * Output
    - Vec<String>
*/
pub(super) fn lines(profile: &crate::invocation::isolation::Profile) -> Vec<String> {
    let name = crate::cli::protocol::name;
    vec![
        format!(
            "  {}{}",
            profile.id,
            if profile.available {
                ""
            } else {
                " (NOT AVAILABLE)"
            }
        ),
        format!(
            "    files: {} - {}",
            name(&profile.files.0),
            profile.files.1
        ),
        format!(
            "    processes: {} - {}",
            name(&profile.processes.0),
            profile.processes.1
        ),
        format!(
            "    network: {} - {}",
            name(&profile.network.0),
            profile.network.1
        ),
        format!(
            "    credentials: {} - {}",
            name(&profile.credentials.0),
            profile.credentials.1
        ),
    ]
}

/** Dispatch an isolation command
 * Input
    - args: &[String] - arguments after "isolation"
 * Output
    - Result<(), String>
*/
pub(crate) fn isolation(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("profiles") => profiles::profiles(rest),
        Some("inspect") => inspect::inspect(rest),
        Some("validate") => validate::validate(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}
