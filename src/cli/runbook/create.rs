// crane runbook create: store a draft runbook version, from a full definition file or as a linear
// sequence of exact skill versions with their reviewed binding sets. In a linear draft each step
// depends on the one before, reads the skill's inputs from run inputs of the same names, and the
// budget covers every planned operation. A draft grants nothing and cannot run.

use std::collections::BTreeMap;

use serde_json::json;

use crate::cli::capability::read_json;
use crate::cli::family::{exact, list, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::plan::BindingSet;
use crate::runbook::model::{self, Budget, FailurePolicy, InputSource, Runbook, Step};
use crate::runbook::store::summary;
use crate::runbook::RUNBOOK_SCHEMA_VERSION;

/** Build a linear runbook from skills and binding sets
 * Input
    - local: &Local - repository context
    - id: &str - runbook id
    - name: &str - name
    - skills: &[String] - "SKILL@N" in order
    - bindings: &[String] - binding set files, one per skill
 * Output
    - Result<Runbook, String>
*/
fn linear(
    local: &Local,
    id: &str,
    name: &str,
    skills: &[String],
    bindings: &[String],
) -> Result<Runbook, String> {
    if skills.is_empty() || skills.len() != bindings.len() {
        return Err(
            "a linear runbook needs --skills and --bindings with one binding set per skill".into(),
        );
    }
    let mut steps = Vec::new();
    let mut inputs = Vec::<String>::new();
    for (index, (reference, file)) in skills.iter().zip(bindings).enumerate() {
        let (skill_id, version) = exact(reference, "a runbook step")?;
        let record = local.context.registry.state.record(&skill_id, version)?;
        let set: BindingSet =
            serde_json::from_value(read_json(file)?).map_err(|error| format!("{file}: {error}"))?;
        let step_inputs = record
            .definition
            .applicability
            .inputs
            .iter()
            .map(|input| {
                (
                    input.name.clone(),
                    InputSource::RunInput {
                        name: input.name.clone(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        for name in step_inputs.keys() {
            if !inputs.contains(name) {
                inputs.push(name.clone());
            }
        }
        let preconditions = record
            .definition
            .applicability
            .preconditions
            .iter()
            .map(|condition| condition.id.clone())
            .collect();
        steps.push(Step {
            step_id: format!("step-{}", index + 1),
            skill_id,
            skill_version: version,
            skill_digest: record.definition_digest.clone(),
            bindings: set,
            inputs: step_inputs,
            preconditions,
            depends_on: if index == 0 {
                Vec::new()
            } else {
                vec![format!("step-{index}")]
            },
            when: None,
            approval: false,
            max_retries: 0,
            on_failure: FailurePolicy::Stop,
        });
    }
    let operations = steps.iter().map(Step::cost).sum();
    Ok(Runbook {
        runbook_id: id.into(),
        version: 1,
        schema_version: RUNBOOK_SCHEMA_VERSION,
        name: name.into(),
        description: format!("Runs {} in order", skills.join(", then ")),
        department: None,
        owner: None,
        environment: None,
        repository: None,
        inputs,
        steps,
        completion: Vec::new(),
        budget: Budget { operations },
    })
}

/** Run the command
 * Input
    - args: &[String] - arguments after "create"
 * Output
    - Result<(), String>
*/
pub(super) fn create(args: &[String]) -> Result<(), String> {
    let args = parse(
        args,
        &[
            "file",
            "from-template",
            "name",
            "skills",
            "bindings",
            "department",
            "id",
            "environment",
            "repository",
        ],
        &[],
    )?;
    args.expect_positional(0, "crane runbook create (--file FILE | --from-template linear --name NAME --skills SKILL@N,... --bindings FILE,...)")?;
    let local = Local::open()?;
    let mut definition = match (args.value("file"), args.value("from-template").as_deref()) {
        (Some(file), None) => model::parse(&read_json(&file)?)?,
        (None, Some("linear")) => {
            let name = args.value("name").ok_or("'--name' is required")?;
            let id = args.value("id").unwrap_or_else(|| {
                name.to_ascii_lowercase()
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("-")
            });
            linear(
                &local,
                &id,
                &name,
                &list(args.value("skills")),
                &list(args.value("bindings")),
            )?
        }
        (None, Some(other)) => {
            return Err(format!(
                "unknown runbook template '{other}'; templates: linear"
            ))
        }
        _ => return Err("give --file, or --from-template linear".into()),
    };
    if let Some(department) = args.value("department") {
        definition.department = Some(department);
    }
    if args.value("environment").is_some() {
        definition.environment = args.value("environment");
    }
    if args.value("repository").is_some() {
        definition.repository = args.value("repository");
    }
    let findings = model::validate(&definition);
    if !findings.is_empty() {
        let response = Response::new(
            "runbook.create",
            Status::Failed,
            format!(
                "runbook {} is not valid; nothing was stored",
                definition.runbook_id
            ),
        )
        .reasons(
            findings
                .iter()
                .filter_map(|finding| finding["code"].as_str().map(str::to_string)),
        )
        .data(json!({"findings": findings}));
        let detail = findings
            .iter()
            .map(|finding| format!("  {}", finding["message"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        return finish(&args, response, &detail);
    }
    let mut store = runbooks(&local)?;
    let record = store.create(
        definition,
        &local.context.actor,
        crate::platform::now_millis(),
    )?;
    store.save()?;
    let reference = format!(
        "{}@{}",
        record.definition.runbook_id, record.definition.version
    );
    let response = Response::new(
        "runbook.create",
        Status::Ok,
        format!(
            "drafted runbook {reference}; it cannot run until a person other than you activates it"
        ),
    )
    .reference("runbook_ref", reference.clone())
    .reference("digest", record.digest.clone())
    .data(summary(&record));
    finish(
        &args,
        response,
        &[format!(
            "  next: crane runbook validate {reference}, then crane runbook test {reference}"
        )],
    )
}
