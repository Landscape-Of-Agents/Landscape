// crane resident catalog | definition ... | create | validate | activate | renew | pause | resume |
// revoke | list | show | rca ... : definitions, assignments, and RCA graphs. Activation and renewal
// validate against the signed state of this moment and need an admin key; pausing and revoking
// never do (making things stricter is always allowed).

use serde_json::json;

use super::{open, open_read, read_json, reason, require_signer, versioned};
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::resident::authority;
use crate::resident::context::PackCache;
use crate::resident::model::{
    AssignmentDraft, Definition, DefinitionState, Lifecycle, TriggerSource,
};
use crate::resident::rca::Graph;
use crate::resident::{plan, registry, report, word};

/** Run 'crane resident catalog': the step-type registry and the trigger sources
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn catalog(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let steps = plan::catalog(None);
    let detail = steps
        .iter()
        .map(|step| {
            format!(
                "  {:<20} {:<9} {}",
                step["type"].as_str().unwrap_or_default(),
                step["effect"].as_str().unwrap_or_default(),
                step["unavailable_reason"]
                    .as_str()
                    .map(|reason| format!("unavailable: {reason}"))
                    .unwrap_or_else(|| "available".into())
            )
        })
        .chain(TriggerSource::ALL.iter().map(|source| {
            format!(
                "  trigger {:<18} {}",
                source.name(),
                source.delivery().unwrap_or("unsupported: no connector")
            )
        }))
        .collect::<Vec<_>>();
    let response = Response::new("resident.catalog", Status::Ok, format!("{} step types (registry v{}), {} trigger sources", steps.len(), plan::REGISTRY_VERSION, TriggerSource::ALL.len()))
        .data(json!({"steps": steps, "triggers": TriggerSource::ALL.iter().map(|source| json!({"source": source, "supported": source.delivery().is_some(), "delivery": source.delivery()})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}

/** Run 'crane resident definition save'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn save_definition(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["file"], &["dry-run"])?;
    let file = args.value("file").ok_or("--file is required")?;
    let definition: Definition = serde_json::from_value(read_json(&file)?)
        .map_err(|error| format!("{file} is not a definition: {error}"))?;
    let (problems, warnings) = registry::check_definition(&definition);
    let mut repo = open()?;
    let detail = problems
        .iter()
        .map(|(code, message)| format!("  problem {code}: {message}"))
        .chain(
            warnings
                .iter()
                .map(|(code, message)| format!("  warning {code}: {message}")),
        )
        .collect::<Vec<_>>();
    if args.flag("dry-run") || !problems.is_empty() {
        let response = Response::new(
            "resident.definition.save",
            if problems.is_empty() {
                Status::Passed
            } else {
                Status::Failed
            },
            if problems.is_empty() {
                "the definition is valid; nothing was saved (--dry-run)".to_string()
            } else {
                format!("{} problem(s); nothing was saved", problems.len())
            },
        )
        .reasons(problems.iter().map(|(code, _)| code.clone()))
        .data(json!({"problems": problems, "warnings": warnings, "digest": definition.digest()}));
        return finish(&args, response, &detail);
    }
    let record = registry::save_definition(
        &mut repo.store,
        definition,
        &repo.author.clone(),
        crate::platform::now_millis(),
    )?;
    repo.store.save()?;
    let reference = format!(
        "{}@{}",
        record.definition.definition_id, record.definition.version
    );
    let response = Response::new("resident.definition.save", Status::Ok, format!("saved {reference} as a draft; an admin activates it with 'crane resident definition activate {reference} --reason TEXT'"))
        .reference("definition_ref", reference)
        .data(json!({"digest": record.digest, "warnings": warnings}));
    finish(&args, response, &detail)
}

/** Run 'crane resident definition activate' or 'revoke'
 * Input
    - args: &[String] - arguments
    - activate: bool - activate (true) or revoke
 * Output
    - Result<(), String>
*/
pub(super) fn change_definition(args: &[String], activate: bool) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(
        1,
        "crane resident definition activate|revoke ID@N --reason TEXT",
    )?;
    let (id, version) = versioned(&args.positional[0])?;
    let reason = reason(&args)?;
    let mut repo = open()?;
    let version = match version {
        Some(version) => version,
        None => repo.store.definition(&id, None)?.definition.version,
    };
    if activate {
        require_signer(&repo, "activating a definition")?;
    }
    let to = if activate {
        DefinitionState::Active
    } else {
        DefinitionState::Revoked
    };
    let record = registry::change_definition(
        &mut repo.store,
        &id,
        version,
        to,
        &repo.author.clone(),
        &reason,
        crate::platform::now_millis(),
    )?;
    repo.store.save()?;
    let response = Response::new(if activate { "resident.definition.activate" } else { "resident.definition.revoke" }, Status::Ok, format!("{id}@{version} is {}{}", word(&record.state), if record.self_reviewed { " (activated by its author: self-reviewed)" } else { "" }))
        .reference("definition_ref", format!("{id}@{version}"))
        .data(json!({"state": record.state, "digest": record.digest, "self_reviewed": record.self_reviewed}));
    finish(&args, response, &[])
}

/** Run 'crane resident definition list'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn definitions(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let repo = open_read()?;
    let rows =
        report::build(&repo.store, None, crate::platform::now_millis())["definitions"].clone();
    let detail = rows
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "  {:<28} {:<8} {}",
                row["ref"].as_str().unwrap_or_default(),
                row["state"].as_str().unwrap_or_default(),
                row["name"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "resident.definition.list",
        Status::Ok,
        format!("{} definition version(s)", detail.len()),
    )
    .data(json!({"definitions": rows}));
    finish(&args, response, &detail)
}

/** Run 'crane resident definition show'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn show_definition(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane resident definition show ID[@N]")?;
    let (id, version) = versioned(&args.positional[0])?;
    let repo = open_read()?;
    let record = repo.store.definition(&id, version)?;
    let response = Response::new(
        "resident.definition.show",
        Status::Ok,
        format!(
            "{}@{} ({})",
            id,
            record.definition.version,
            word(&record.state)
        ),
    )
    .data(json!({"record": record, "steps": plan::catalog(Some(&record.definition))}));
    finish(&args, response, &[])
}

/** Run 'crane resident create'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn create(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["file"], &["dry-run"])?;
    let file = args.value("file").ok_or("--file is required")?;
    let draft: AssignmentDraft = serde_json::from_value(read_json(&file)?)
        .map_err(|error| format!("{file} is not an assignment: {error}"))?;
    let mut repo = open()?;
    let now = crate::platform::now_millis();
    let id = draft.assignment_id.clone();
    if args.flag("dry-run") {
        let mut preview = crate::resident::store::ResidentStore::in_memory();
        preview.state = repo.store.state.clone();
        registry::create_assignment(&mut preview, draft, &repo.author, now)?;
        let view = authority::load(&repo.workspace)?;
        let validation = registry::check_assignment(&preview, &view, preview.assignment(&id)?, now);
        // Nothing was created, so the preview's note must not become an event
        let _ = crate::platform::notes::drain();
        let response = Response::new(
            "resident.create",
            if validation.passed {
                Status::Passed
            } else {
                Status::Failed
            },
            format!(
                "{id} would be created as a draft; validation {} (nothing was saved)",
                if validation.passed { "passes" } else { "fails" }
            ),
        )
        .reasons(validation.problems.iter().map(|(code, _)| code.clone()))
        .data(json!({"assignment": preview.assignment(&id)?, "validation": validation}));
        return finish(&args, response, &[]);
    }
    let assignment =
        registry::create_assignment(&mut repo.store, draft, &repo.author.clone(), now)?;
    repo.store.save()?;
    let response = Response::new(
        "resident.create",
        Status::Ok,
        format!(
            "created {id} as a draft for {}; next: crane resident validate {id}",
            assignment.target.key()
        ),
    )
    .reference("assignment_ref", id)
    .data(json!({"assignment": assignment}));
    finish(&args, response, &[])
}

/** Run 'crane resident duplicate': copy an assignment as a new draft
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn duplicate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["as", "target"], &[])?;
    args.expect_positional(
        1,
        "crane resident duplicate ID --as NEW_ID [--target flow:ID|zone:ID]",
    )?;
    let source = args.positional[0].clone();
    let new_id = args.value("as").ok_or("--as is required")?;
    let target = args
        .value("target")
        .map(|text| crate::brick::model::Target::parse(&text))
        .transpose()?;
    let mut repo = open()?;
    let assignment = registry::duplicate_assignment(
        &mut repo.store,
        &source,
        &new_id,
        target,
        &repo.author.clone(),
        crate::platform::now_millis(),
    )?;
    repo.store.save()?;
    let response = Response::new("resident.duplicate", Status::Ok, format!("created {new_id} as a draft copy of {source} for {}; next: crane resident validate {new_id}", assignment.target.key()))
        .reference("assignment_ref", new_id)
        .data(json!({"assignment": assignment}));
    finish(&args, response, &[])
}

/** Run 'crane resident validate'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn validate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane resident validate ID")?;
    let id = args.positional[0].clone();
    let mut repo = open()?;
    let view = authority::load(&repo.workspace)?;
    let validation = registry::validate_assignment(
        &mut repo.store,
        &view,
        &id,
        &repo.author.clone(),
        crate::platform::now_millis(),
    )?;
    repo.store.save()?;
    let detail = validation
        .problems
        .iter()
        .map(|(code, message)| format!("  problem {code}: {message}"))
        .chain(
            validation
                .warnings
                .iter()
                .map(|(code, message)| format!("  warning {code}: {message}")),
        )
        .collect::<Vec<_>>();
    let response = Response::new(
        "resident.validate",
        if validation.passed {
            Status::Passed
        } else {
            Status::Failed
        },
        format!(
            "{id}: {} problem(s), {} warning(s){}",
            validation.problems.len(),
            validation.warnings.len(),
            if validation.passed {
                "; an admin activates it with 'crane resident activate'"
            } else {
                ""
            }
        ),
    )
    .reasons(validation.problems.iter().map(|(code, _)| code.clone()))
    .reference("assignment_ref", id)
    .data(json!({"validation": validation}));
    finish(&args, response, &detail)
}

/** Run 'crane resident activate | renew | pause | resume | revoke'
 * Input
    - args: &[String] - arguments
    - action: &str - which
 * Output
    - Result<(), String>
*/
pub(super) fn lifecycle(args: &[String], action: &str) -> Result<(), String> {
    let args = parse(args, &["reason"], &["dry-run"])?;
    args.expect_positional(1, &format!("crane resident {action} ID --reason TEXT"))?;
    let id = args.positional[0].clone();
    let reason = reason(&args)?;
    let mut repo = open()?;
    let now = crate::platform::now_millis();
    let to = match action {
        "activate" | "renew" | "resume" => Lifecycle::Active,
        "pause" => Lifecycle::Paused,
        _ => Lifecycle::Revoked,
    };
    let current = repo.store.assignment(&id)?.lifecycle;
    if matches!(action, "activate" | "renew") {
        require_signer(&repo, "activating or renewing a resident assignment")?;
        let expected = if action == "renew" {
            Lifecycle::Expired
        } else {
            Lifecycle::Validated
        };
        if current != expected {
            return Err(format!(
                "{id} is {}; '{action}' applies to a {} assignment",
                word(&current),
                word(&expected)
            ));
        }
    }
    if action == "resume" && current != Lifecycle::Paused {
        return Err(format!("{id} is {}, not paused", word(&current)));
    }
    let view = authority::load(&repo.workspace)?;
    if args.flag("dry-run") {
        let validation =
            registry::check_assignment(&repo.store, &view, repo.store.assignment(&id)?, now);
        let response = Response::new(
            &format!("resident.{action}"),
            if validation.passed {
                Status::Passed
            } else {
                Status::Failed
            },
            format!(
                "{id} would become {}; nothing was changed (--dry-run)",
                word(&to)
            ),
        )
        .reasons(validation.problems.iter().map(|(code, _)| code.clone()))
        .data(json!({"validation": validation}));
        return finish(&args, response, &[]);
    }
    let mut cache = PackCache::open(&repo.runtime);
    let assignment = registry::change_assignment(
        &mut repo.store,
        &mut cache,
        &view,
        &id,
        to,
        &repo.author.clone(),
        &reason,
        now,
    )?;
    repo.store.save()?;
    let response = Response::new(
        &format!("resident.{action}"),
        Status::Ok,
        format!(
            "{id} is {}{}",
            word(&assignment.lifecycle),
            if assignment.lifecycle == Lifecycle::Active {
                format!(
                    " until {} (idle until a trigger arrives; no model runs while idle)",
                    assignment.expires_at
                )
            } else {
                String::new()
            }
        ),
    )
    .reference("assignment_ref", id)
    .data(json!({"assignment": assignment}));
    finish(&args, response, &[])
}

/** Run 'crane resident list'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn list(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let repo = open_read()?;
    let view = authority::load(&repo.workspace).ok();
    let report = report::build(&repo.store, view.as_ref(), crate::platform::now_millis());
    let detail = report["assignments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "  {:<20} {:<10} {:<20} {:<18} {}",
                row["assignment_id"].as_str().unwrap_or_default(),
                row["lifecycle"].as_str().unwrap_or_default(),
                row["status"].as_str().unwrap_or_default(),
                row["target"].as_str().unwrap_or_default(),
                row["definition"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new("resident.list", Status::Ok, format!("{} assignment(s); idle assignments make no model call", detail.len()))
        .data(json!({"assignments": report["assignments"], "definitions": report["definitions"], "health": report["health"]}));
    finish(&args, response, &detail)
}

/** Run 'crane resident show'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn show(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane resident show ID")?;
    let id = args.positional[0].clone();
    let repo = open_read()?;
    let view = authority::load(&repo.workspace).ok();
    let report = report::build(&repo.store, view.as_ref(), crate::platform::now_millis());
    let row = report["assignments"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["assignment_id"] == id.as_str())
        .cloned()
        .ok_or_else(|| format!("assignment {id} does not exist"))?;
    let activations = report["activations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|activation| activation["assignment"] == id.as_str())
        .cloned()
        .collect::<Vec<_>>();
    let response = Response::new(
        "resident.show",
        Status::Ok,
        format!(
            "{id}: {} ({})",
            row["status"].as_str().unwrap_or_default(),
            row["target"].as_str().unwrap_or_default()
        ),
    )
    .reference("assignment_ref", id)
    .data(json!({"assignment": row, "activations": activations}));
    finish(&args, response, &[])
}

/** Run 'crane resident rca save'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn save_rca(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["file"], &[])?;
    let file = args.value("file").ok_or("--file is required")?;
    let graph: Graph = serde_json::from_value(read_json(&file)?)
        .map_err(|error| format!("{file} is not an RCA graph: {error}"))?;
    let mut repo = open()?;
    let record = registry::save_rca(
        &mut repo.store,
        graph,
        &repo.author.clone(),
        crate::platform::now_millis(),
    )?;
    repo.store.save()?;
    let reference = format!("{}@{}", record.graph.rca_id, record.graph.version);
    let response = Response::new("resident.rca.save", Status::Ok, format!("saved {reference} as proposed; its remediations run only after 'crane resident rca review {reference} --accept'"))
        .reference("rca_ref", reference)
        .data(json!({"digest": record.digest}));
    finish(&args, response, &[])
}

/** Run 'crane resident rca review'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn review_rca(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &["accept", "reject"])?;
    args.expect_positional(
        1,
        "crane resident rca review ID@N (--accept | --reject) --reason TEXT",
    )?;
    let (id, version) = versioned(&args.positional[0])?;
    let version = version.ok_or("name the exact version: ID@N")?;
    let accept = match (args.flag("accept"), args.flag("reject")) {
        (true, false) => true,
        (false, true) => false,
        _ => return Err("give --accept or --reject".into()),
    };
    let reason = reason(&args)?;
    let mut repo = open()?;
    require_signer(&repo, "reviewing an RCA graph")?;
    let record = registry::review_rca(
        &mut repo.store,
        &id,
        version,
        accept,
        &repo.author.clone(),
        &reason,
        crate::platform::now_millis(),
    )?;
    repo.store.save()?;
    let response = Response::new(
        "resident.rca.review",
        Status::Ok,
        format!(
            "{id}@{version} is {}{}; node bases are unchanged (hypotheses stay hypotheses)",
            word(&record.state),
            if record.self_reviewed {
                " (reviewed by its author)"
            } else {
                ""
            }
        ),
    )
    .reference("rca_ref", format!("{id}@{version}"))
    .data(json!({"state": record.state, "self_reviewed": record.self_reviewed}));
    finish(&args, response, &[])
}

/** Run 'crane resident rca list'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn rcas(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let repo = open_read()?;
    let rows = report::build(&repo.store, None, crate::platform::now_millis())["rcas"].clone();
    let detail = rows
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "  {:<28} {:<9} {}",
                row["rca"].as_str().unwrap_or_default(),
                row["state"].as_str().unwrap_or_default(),
                row["target"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "resident.rca.list",
        Status::Ok,
        format!("{} RCA graph version(s)", detail.len()),
    )
    .data(json!({"rcas": rows}));
    finish(&args, response, &detail)
}

/** Run 'crane resident rca show'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn show_rca(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane resident rca show ID[@N]")?;
    let (id, version) = versioned(&args.positional[0])?;
    let repo = open_read()?;
    let record = repo.store.rca(&id, version)?;
    let response = Response::new(
        "resident.rca.show",
        Status::Ok,
        format!("{id}@{} ({})", record.graph.version, word(&record.state)),
    )
    .data(json!({"record": record, "problems": crate::resident::rca::validate(&record.graph)}));
    finish(&args, response, &[])
}
