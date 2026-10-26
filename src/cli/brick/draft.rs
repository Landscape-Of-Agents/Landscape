// crane brick create | edit: save an unsigned draft from options or a JSON file (the form the
// extension and other surfaces write). A draft grants nothing. It is validated when saved so the
// author sees problems at once; that result is informational, because activation validates again.
// Editing an activated Brick starts the next version's draft: published versions never change.

use serde_json::{json, Value};

use super::{open, text_option, Repo};
use crate::brick::model::{Brick, BrickRef, Sensitivity};
use crate::brick::store::{self, BrickDraft};
use crate::brick::types::BrickType;
use crate::brick::validate;
use crate::cli::args::Args;
use crate::cli::protocol::{finish, parse, Response, Status};

/** The options create and edit accept */
const KEYS: &[&str] = &[
    "file",
    "type",
    "name",
    "id",
    "intent",
    "description",
    "owner",
    "departments",
    "config",
    "context",
    "context-file",
    "sensitivity",
    "depends",
];

/** Make an identifier from a name ("Production config guard" -> "production-config-guard")
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
    let slug = slug
        .trim_end_matches('-')
        .chars()
        .take(64)
        .collect::<String>();
    if slug.starts_with(|character: char| character.is_ascii_alphabetic()) {
        slug
    } else {
        format!("b-{slug}").chars().take(64).collect()
    }
}

/** Read a configuration given inline (JSON starting with '{') or as a file
 * Input
    - text: &str - JSON or path
 * Output
    - Result<Value, String>
*/
fn config_value(text: &str) -> Result<Value, String> {
    let source = if text.trim_start().starts_with('{') {
        text.to_string()
    } else {
        std::fs::read_to_string(text).map_err(|error| format!("could not read {text}: {error}"))?
    };
    serde_json::from_str(&source).map_err(|error| format!("--config is not a JSON object: {error}"))
}

/** Apply the fields of a JSON draft file (any subset of the definition) to a Brick
 * Input
    - brick: &mut Brick - definition
    - value: &Value - file content
 * Output
    - Result<(), String>
*/
fn apply_file(brick: &mut Brick, value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("the draft file must hold a JSON object")?;
    for key in object.keys() {
        if ![
            "brick_id",
            "type",
            "name",
            "description",
            "intent",
            "owner",
            "departments",
            "config",
            "common_context",
            "dependencies",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("the draft file has an unknown field '{key}'"));
        }
    }
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
    if let Some(name) = text("name") {
        brick.name = name;
    }
    if let Some(description) = text("description") {
        brick.description = description;
    }
    if let Some(intent) = text("intent") {
        brick.intent = intent;
    }
    if object.contains_key("owner") {
        brick.owner = text("owner").filter(|owner| !owner.is_empty());
    }
    if let Some(departments) = object.get("departments") {
        brick.departments = serde_json::from_value(departments.clone())
            .map_err(|error| format!("departments: {error}"))?;
    }
    if let Some(config) = object.get("config") {
        brick.config = config.clone();
    }
    if let Some(context) = object.get("common_context") {
        brick.common_context = serde_json::from_value(context.clone())
            .map_err(|error| format!("common_context: {error}"))?;
    }
    if let Some(dependencies) = object.get("dependencies") {
        brick.dependencies = serde_json::from_value::<Vec<String>>(dependencies.clone())
            .map_err(|error| format!("dependencies: {error}"))?
            .iter()
            .map(|text| reference(text))
            .collect::<Result<_, _>>()?;
    }
    Ok(())
}

/** Parse an exact Brick reference
 * Input
    - text: &str - "ID@N"
 * Output
    - Result<BrickRef, String>
*/
fn reference(text: &str) -> Result<BrickRef, String> {
    match crate::cli::protocol::versioned(text.trim())? {
        (brick_id, Some(version)) => Ok(BrickRef { brick_id, version }),
        (id, None) => Err(format!("dependency {id} needs an exact version: {id}@N")),
    }
}

/** Apply the options of create and edit to a Brick
 * Input
    - brick: &mut Brick - definition
    - args: &Args - parsed arguments
 * Output
    - Result<(), String>
*/
fn apply_options(brick: &mut Brick, args: &Args) -> Result<(), String> {
    if let Some(name) = args.value("name") {
        brick.name = name;
    }
    if let Some(intent) = args.value("intent") {
        brick.intent = intent;
    }
    if let Some(description) = args.value("description") {
        brick.description = description;
    }
    if let Some(owner) = args.value("owner") {
        brick.owner = Some(owner).filter(|owner| !owner.is_empty());
    }
    if let Some(departments) = args.value("departments") {
        brick.departments = crate::cli::family::list(Some(departments));
    }
    if let Some(config) = args.value("config") {
        brick.config = config_value(&config)?;
    }
    if let Some(text) = text_option(args, "context", "context-file")? {
        brick.common_context.text = text;
    }
    if let Some(sensitivity) = args.value("sensitivity") {
        brick.common_context.sensitivity = match sensitivity.as_str() {
            "internal" => Sensitivity::Internal,
            "restricted" => Sensitivity::Restricted,
            other => {
                return Err(format!(
                    "sensitivity must be internal or restricted, got '{other}'"
                ))
            }
        };
    }
    if let Some(depends) = args.value("depends") {
        brick.dependencies = crate::cli::family::list(Some(depends))
            .iter()
            .map(|text| reference(text))
            .collect::<Result<_, _>>()?;
    }
    Ok(())
}

/** Validate, save, and report a draft
 * Input
    - repo: &Repo - repository
    - args: &Args - parsed arguments
    - command: &str - "brick.create" or "brick.edit"
    - brick: Brick - definition
 * Output
    - Result<(), String>
*/
fn save(repo: &Repo, args: &Args, command: &str, mut brick: Brick) -> Result<(), String> {
    let by = store::author(&repo.workspace);
    brick.created_by = by.clone();
    brick.created_at = crate::platform::now_millis();
    let validation = validate::brick(&brick, &repo.store, &repo.workspace.root);
    let draft = BrickDraft {
        brick,
        validation: validation.clone(),
        updated_by: by,
        updated_at: crate::platform::now_millis(),
    };
    store::save_brick_draft(&repo.workspace, &draft)?;
    let reference = format!("{}@{}", draft.brick.brick_id, draft.brick.version);
    crate::platform::notes::note(
        "BRICK_DRAFT_SAVED",
        json!({"brick": reference, "type": draft.brick.brick_type, "validation": validation.status, "problems": validation.problems.len()}),
    );
    let detail = validation
        .problems
        .iter()
        .map(|problem| format!("  {}: {}", problem.code, problem.message))
        .collect::<Vec<_>>();
    let response = Response::new(
        command,
        Status::Ok,
        format!(
            "saved draft {reference} ({}); validation {}{}",
            draft.brick.brick_type.name(),
            crate::cli::protocol::name(&validation.status),
            if validation.problems.is_empty() {
                "; an admin activates it with 'crane brick activate'".to_string()
            } else {
                format!(", {} problem(s)", validation.problems.len())
            }
        ),
    )
    .reference("brick_ref", reference)
    .reference("digest", draft.brick.digest())
    .data(json!({
        "state": "draft",
        "definition": draft.brick,
        "validation": validation,
        "path": format!(".crane/drafts/bricks/{}.json", draft.brick.brick_id),
    }));
    finish(args, response, &detail)
}

/** Run 'crane brick create'
 * Input
    - args: &[String] - arguments after "create"
 * Output
    - Result<(), String>
*/
pub(super) fn create(args: &[String]) -> Result<(), String> {
    let args = parse(args, KEYS, &[])?;
    args.expect_positional(
        0,
        "crane brick create (--file FILE | --type TYPE --name NAME ...)",
    )?;
    let file = args
        .value("file")
        .map(|path| {
            std::fs::read_to_string(&path)
                .map_err(|error| format!("could not read {path}: {error}"))
                .and_then(|text| {
                    serde_json::from_str::<Value>(&text).map_err(|error| format!("{path}: {error}"))
                })
        })
        .transpose()?;
    let type_text = args
        .value("type")
        .or_else(|| {
            file.as_ref()
                .and_then(|value| value["type"].as_str().map(str::to_string))
        })
        .ok_or("--type is required (the types are fixed: run 'crane brick types')")?;
    let brick_type = BrickType::parse(&type_text)?;
    let name = args
        .value("name")
        .or_else(|| {
            file.as_ref()
                .and_then(|value| value["name"].as_str().map(str::to_string))
        })
        .ok_or("--name is required")?;
    let id = args
        .value("id")
        .or_else(|| {
            file.as_ref()
                .and_then(|value| value["brick_id"].as_str().map(str::to_string))
        })
        .unwrap_or_else(|| slug(&name));
    crate::platform::validate_name("Brick", &id)?;
    let repo = open()?;
    if store::brick_draft(&repo.workspace, &id)?.is_some() {
        return Err(format!(
            "a draft of {id} already exists; change it with 'crane brick edit {id}'"
        ));
    }
    if let Some(latest) = repo.store.versions(&id).last() {
        if latest.brick.brick_type != brick_type {
            return Err(format!(
                "{id} is a {} Brick; a type never changes (choose another id)",
                latest.brick.brick_type.name()
            ));
        }
    }
    let version = repo.store.next_version(&id, false);
    let mut brick = store::new_brick(
        &id,
        version,
        brick_type,
        &name,
        store::author(&repo.workspace),
    );
    if let Some(file) = &file {
        apply_file(&mut brick, file)?;
    }
    apply_options(&mut brick, &args)?;
    save(&repo, &args, "brick.create", brick)
}

/** Run 'crane brick edit'
 * Input
    - args: &[String] - arguments after "edit"
 * Output
    - Result<(), String>
*/
pub(super) fn edit(args: &[String]) -> Result<(), String> {
    let args = parse(args, KEYS, &[])?;
    args.expect_positional(1, "crane brick edit ID [options]")?;
    let id = args.positional[0].clone();
    let repo = open()?;
    let mut brick = match store::brick_draft(&repo.workspace, &id)? {
        Some(draft) => draft.brick,
        None => {
            // Editing an activated Brick starts the next version from the newest one
            let latest = repo
                .store
                .versions(&id)
                .last()
                .map(|entry| entry.brick.clone())
                .ok_or_else(|| format!("no Brick '{id}' to edit; create it first"))?;
            Brick {
                version: repo.store.next_version(&id, false),
                ..latest
            }
        }
    };
    if let Some(text) = args.value("type") {
        if BrickType::parse(&text)? != brick.brick_type {
            return Err(format!(
                "{id} is a {} Brick; a type never changes",
                brick.brick_type.name()
            ));
        }
    }
    if args.value("id").is_some_and(|given| given != id) {
        return Err("an id never changes; create a new Brick instead".into());
    }
    if let Some(path) = args.value("file") {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("could not read {path}: {error}"))?;
        let value =
            serde_json::from_str::<Value>(&text).map_err(|error| format!("{path}: {error}"))?;
        if value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|text| BrickType::parse(text).ok() != Some(brick.brick_type))
        {
            return Err(format!(
                "{id} is a {} Brick; a type never changes",
                brick.brick_type.name()
            ));
        }
        if value
            .get("brick_id")
            .and_then(Value::as_str)
            .is_some_and(|given| given != id)
        {
            return Err("an id never changes; create a new Brick instead".into());
        }
        apply_file(&mut brick, &value)?;
    }
    apply_options(&mut brick, &args)?;
    save(&repo, &args, "brick.edit", brick)
}
