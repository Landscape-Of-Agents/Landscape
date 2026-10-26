// crane skill activate: publish an exact version after review, by a person other than its author
// holding the publisher role (an admin key), with every validation gate passed and warnings
// acknowledged; the version is signed into the shared skill store. Never activates another digest
// than the one that was validated.

use serde_json::json;

use crate::capability::lifecycle::PublicationRequest;
use crate::cli::capability::{publish_version, Context};
use crate::cli::family::{exact, list};
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "activate"
 * Output
    - Result<(), String>
*/
pub(super) fn activate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["review-ref", "acknowledge"], &[])?;
    args.expect_positional(
        1,
        "crane skill activate SKILL@N --review-ref REF [--acknowledge CODE,CODE]",
    )?;
    let (id, version) = exact(&args.positional[0], "activation")?;
    let review = args
        .value("review-ref")
        .filter(|value| !value.trim().is_empty())
        .ok_or("'--review-ref' (the review that approved this version) is required")?;
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let request = PublicationRequest {
        capability_id: id.clone(),
        version,
        reason: format!("activated after review {review}"),
        acknowledged_warnings: list(args.value("acknowledge")).into_iter().collect(),
    };
    let gate = publish_version(&mut context, &actor, &request)?;
    let digest = context
        .registry
        .state
        .record(&id, version)?
        .definition_digest
        .clone();
    let response = Response::new("skill.activate", Status::Ok, format!("activated {id}@{version} and signed it into .crane/skills.json; commit .crane so the team shares it"))
        .reference("skill_ref", format!("{id}@{version}"))
        .reference("digest", digest)
        .reference("review_ref", review)
        .data(json!(gate));
    finish(&args, response, &[])
}
