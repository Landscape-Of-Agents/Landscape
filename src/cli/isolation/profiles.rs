// crane isolation profiles: every profile with what it enforces, partially enforces, or does not.

use serde_json::json;

use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::isolation::profiles as all;

/** Run the command
 * Input
    - args: &[String] - arguments after "profiles"
 * Output
    - Result<(), String>
*/
pub(super) fn profiles(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane isolation profiles")?;
    let profiles = all();
    let detail = profiles.iter().flat_map(super::lines).collect::<Vec<_>>();
    let available = profiles.iter().filter(|profile| profile.available).count();
    let response = Response::new(
        "isolation.profiles",
        Status::Ok,
        format!(
            "{} profiles, {available} available; none is a sandbox",
            profiles.len()
        ),
    )
    .data(json!({"profiles": profiles}));
    finish(&args, response, &detail)
}
