// crane verification inspect: a skill's verification contract (claims, whether each is required,
// how it is verified, its declared status) or a run's recorded verification results.

use serde_json::json;

use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill", "run"], &[])?;
    args.expect_positional(
        0,
        "crane verification inspect --skill SKILL[@N] | --run INVOCATION_ID",
    )?;
    let local = Local::open()?;
    if let Some(run) = args.value("run") {
        let record = local.ledger.get(&run)?;
        let rows = super::results(&record);
        let response = Response::new(
            "verification.inspect",
            Status::Ok,
            format!(
                "run {run}: {} verification results",
                record.verifications.len()
            ),
        )
        .reference("run_ref", run.clone())
        .data(json!({"results": rows}));
        return finish(&args, response, &super::lines(&rows));
    }
    let record = skill(
        &local,
        &args
            .value("skill")
            .ok_or("'--skill' or '--run' is required")?,
    )?;
    let contract = &record.definition.verification;
    let detail = contract
        .claims
        .iter()
        .map(|claim| {
            format!(
                "  {}{} [{:?}, declared {:?}]: {} - expects {}",
                claim.claim_id,
                if claim.required { "" } else { " (optional)" },
                claim.kind,
                claim.status,
                claim.statement,
                claim.expected_result
            )
        })
        .chain(contract.invariants.iter().map(|invariant| {
            format!(
                "  invariant {} ({:?}): {}",
                invariant.invariant_id, invariant.mapping, invariant.statement
            )
        }))
        .collect::<Vec<_>>();
    let response = Response::new(
        "verification.inspect",
        Status::Ok,
        format!(
            "{}: {} claims, {} invariants (method {})",
            reference(record),
            contract.claims.len(),
            contract.invariants.len(),
            contract.method
        ),
    )
    .reference("skill_ref", reference(record))
    .data(json!(contract));
    finish(&args, response, &detail)
}
