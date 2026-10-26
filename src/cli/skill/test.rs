// crane skill test: the Validation Lab on the exact digest of a version with a reviewed test plan
// (preflight, conformance, scenarios, invariants). Nothing the skill describes is executed and no
// credential is used. Only a passing report is a pass; inconclusive, blocked, or not-run checks
// are not.

use serde_json::json;

use crate::capability_lab::preflight::Target;
use crate::capability_lab::report::run_lab;
use crate::capability_lab::scenarios::TestPlan;
use crate::capability_lab::store::LabStore;
use crate::capability_lab::CheckStatus;
use crate::cli::capability::{read_json, Context};
use crate::cli::protocol::{finish, parse, versioned, Response, Status};
use crate::platform::now_millis;

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["suite"], &[])?;
    args.expect_positional(1, "crane skill test SKILL[@N] --suite PLAN_FILE")?;
    let suite = args
        .value("suite")
        .ok_or("'--suite' (a lab test plan file) is required")?;
    let plan = TestPlan::parse(&read_json(&suite)?.to_string())?;
    let (id, version) = versioned(&args.positional[0])?;
    let context = Context::open()?;
    let version = match version {
        Some(version) => version,
        None => context
            .registry
            .state
            .versions
            .get(&id)
            .and_then(|versions| versions.keys().next_back().copied())
            .ok_or_else(|| format!("skill {id} does not exist"))?,
    };
    let digest = context
        .registry
        .state
        .record(&id, version)?
        .definition_digest
        .clone();
    let target = Target {
        capability_id: id.clone(),
        version,
        expected_digest: digest.clone(),
    };
    let sources = context.sources();
    let report = run_lab(
        &context.registry,
        &sources,
        &context.actor,
        &target,
        None,
        &plan,
        now_millis(),
    )?;
    LabStore::open(&context.runtime).append(&report)?;
    let status = match report.overall {
        CheckStatus::Passed => Status::Passed,
        CheckStatus::Failed => Status::Failed,
        CheckStatus::Blocked => Status::Blocked,
        _ => Status::Unknown,
    };
    let response = Response::new(
        "skill.test",
        status,
        format!(
            "{id}@{version}: lab {:?} (report {})",
            report.overall, report.report_id
        ),
    )
    .reference("skill_ref", format!("{id}@{version}"))
    .reference("digest", digest)
    .reference("evidence_ref", report.report_id.clone())
    .data(json!(report));
    finish(&args, response, &[])
}
