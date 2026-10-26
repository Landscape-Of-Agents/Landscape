// crane permission check: may one typed operation act on one resource (for a skill)? A decision
// with reason codes from the permission service; nothing is executed.

use serde_json::json;

use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::permission;
use crate::scope::Boundary;

/** Run the command
 * Input
    - args: &[String] - arguments after "check"
 * Output
    - Result<(), String>
*/
pub(super) fn check(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["operation", "resource", "skill"], &[])?;
    args.expect_positional(
        0,
        "crane permission check --operation ADAPTER/OPERATION --resource REF [--skill SKILL[@N]]",
    )?;
    let operation = args.value("operation").ok_or("'--operation' is required")?;
    let resource = args.value("resource").ok_or("'--resource' is required")?;
    let local = Local::open()?;
    let record = args
        .value("skill")
        .map(|reference| skill(&local, &reference))
        .transpose()?;
    let boundary = Boundary::load(local.context.workspace.clone())?;
    let answer = permission::check(
        &local.adapters,
        &boundary,
        &local.context.actor,
        &operation,
        &resource,
        record,
    );
    let detail = answer
        .reasons
        .iter()
        .map(|(code, message)| format!("  {code}: {message}"))
        .collect::<Vec<_>>();
    let mut response = Response::new("permission.check", Status::Ok, format!("{operation} on {resource}"))
        .decision(answer.decision)
        .reasons(answer.reasons.iter().map(|(code, _)| code.clone()))
        .reference("operation_ref", operation.clone())
        .reference("policy_digest", boundary.versions().manifest_head.unwrap_or_default())
        .data(json!({"effect": answer.effect, "reasons": answer.reasons.iter().map(|(code, message)| json!({"code": code, "message": message})).collect::<Vec<_>>()}));
    if let Some(record) = record {
        response = response.reference("skill_ref", reference(record));
    }
    finish(&args, response, &detail)
}
