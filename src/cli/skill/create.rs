// crane skill create: a draft skill from a reviewed template, stored as a candidate. A draft grants
// nothing: no operation, credential, or authority comes with it.

use serde_json::json;

use crate::cli::capability::Context;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::platform::now_millis;

/** Derive a skill id from a name ("Refund Review" -> "refund-review")
 * Input
    - name: &str - name
 * Output
    - String
*/
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

/** Run the command
 * Input
    - args: &[String] - arguments after "create"
 * Output
    - Result<(), String>
*/
pub(super) fn create(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["from-template", "name", "department", "id"], &[])?;
    args.expect_positional(
        0,
        "crane skill create --from-template TEMPLATE --name NAME [--department DEPT] [--id ID]",
    )?;
    let template = args
        .value("from-template")
        .ok_or("'--from-template' is required")?;
    let name = args
        .value("name")
        .filter(|name| !name.trim().is_empty())
        .ok_or("'--name' is required")?;
    let id = args.value("id").unwrap_or_else(|| slug(&name));
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let now = now_millis();
    let definition = crate::capability::templates::draft(
        &template,
        &id,
        &name,
        args.value("department").as_deref(),
        &actor,
        now,
    )?;
    let record = context
        .registry
        .create_candidate(&actor, definition, None, now)?;
    let response = Response::new(
        "skill.create",
        Status::Ok,
        format!("drafted {id}@1 from template {template}; it grants nothing until it is validated, tested, and activated by another person"),
    )
    .reference("skill_ref", format!("{id}@1"))
    .reference("digest", record.definition_digest.clone())
    .data(json!({"skill": format!("{id}@1"), "digest": record.definition_digest, "state": record.state, "profile": record.profile_id, "definition": record.definition}));
    finish(
        &args,
        response,
        &[format!(
            "  next: crane skill inspect {id}@1, then crane skill validate {id}@1"
        )],
    )
}

#[cfg(test)]
mod tests {
    use super::slug;

    /** Names become valid ids
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn names_become_ids() {
        assert_eq!(slug("Refund Review"), "refund-review");
        assert_eq!(slug("  Q3: close books!  "), "q3-close-books");
    }
}
