// crane skill retire: deprecate an exact version so it is not used for new work; history and
// evidence stay, and the deprecation is signed into the shared skill store.

use crate::capability::lifecycle::LifecycleState;
use crate::cli::capability::{withdraw_version, Context};
use crate::cli::family::exact;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Withdraw a version (shared by retire and revoke)
 * Input
    - args: &[String] - arguments after the command
    - command: &str - retire or revoke
    - to: LifecycleState - Deprecated or Revoked
 * Output
    - Result<(), String>
*/
pub(super) fn withdraw(args: &[String], command: &str, to: LifecycleState) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, &format!("crane skill {command} SKILL@N --reason TEXT"))?;
    let (id, version) = exact(&args.positional[0], command)?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("'--reason' is required")?;
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let (changed, shared) =
        withdraw_version(&mut context, &actor, &id, Some(version), to, &reason)?;
    let response = Response::new(
        &format!("skill.{command}"),
        Status::Ok,
        format!(
            "{id}@{version} is {to:?}{}",
            if shared > 0 {
                " here and in the shared skill store; commit .crane so every machine honors it"
            } else {
                " on this machine (it was not in the shared store)"
            }
        ),
    )
    .reference("skill_ref", format!("{id}@{version}"))
    .data(serde_json::json!({"changed": changed, "shared": shared, "reason": reason}));
    finish(&args, response, &[])
}

/** Run the command
 * Input
    - args: &[String] - arguments after "retire"
 * Output
    - Result<(), String>
*/
pub(super) fn retire(args: &[String]) -> Result<(), String> {
    withdraw(args, "retire", LifecycleState::Deprecated)
}
