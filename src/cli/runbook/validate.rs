// crane runbook validate: the runbook's structure (ids, dependencies, cycles, branches, inputs,
// completion, retries, budget) and every step against the registry: the pinned skill exists with
// its pinned digest and is published and not withdrawn, every bound operation is registered, and
// every step can run under the typed-adapters isolation profile.

use serde_json::json;

use crate::cli::family::{registry_findings, runbooks};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, versioned, Response, Status};
use crate::runbook::model;

/** Run the command
 * Input
    - args: &[String] - arguments after "validate"
 * Output
    - Result<(), String>
*/
pub(super) fn validate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane runbook validate RUNBOOK[@N]")?;
    let (id, version) = versioned(&args.positional[0])?;
    let local = Local::open()?;
    let store = runbooks(&local)?;
    let record = store.runbook(&id, version)?;
    let mut findings = model::validate(&record.definition);
    findings.extend(registry_findings(&local, &record.definition));
    let reference = format!("{id}@{}", record.definition.version);
    let status = if findings.is_empty() {
        Status::Passed
    } else {
        Status::Failed
    };
    let detail = findings
        .iter()
        .map(|finding| {
            format!(
                "  {}: {}",
                finding["code"].as_str().unwrap_or_default(),
                finding["message"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "runbook.validate",
        status,
        format!(
            "runbook {reference}: {}",
            if findings.is_empty() {
                "valid"
            } else {
                "not valid"
            }
        ),
    )
    .reasons(
        findings
            .iter()
            .filter_map(|finding| finding["code"].as_str().map(str::to_string)),
    )
    .reference("runbook_ref", reference)
    .reference("digest", record.digest.clone())
    .data(json!({"findings": findings, "state": record.state}));
    finish(&args, response, &detail)
}
