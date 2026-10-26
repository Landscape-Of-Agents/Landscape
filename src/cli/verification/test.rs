// crane verification test: the evaluator must not be fooled, for this skill's claims: a claim marked
// verified with its evidence removed is not a pass, a claim citing evidence that does not exist is
// not a pass, a required unknown keeps the whole result from passing, and a failed required check
// fails it. Runs on copies; nothing is stored.

use serde_json::json;

use crate::capability::model::ClaimStatus;
use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill"], &[])?;
    args.expect_positional(0, "crane verification test --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let sources = local.context.sources();
    let mut stripped = record.definition.clone();
    for claim in &mut stripped.verification.claims {
        claim.evidence_refs.clear();
        claim.assertion_refs.clear();
        claim.status = ClaimStatus::Verified;
    }
    let mut forged = record.definition.clone();
    for claim in &mut forged.verification.claims {
        claim.evidence_refs = vec!["evd_does_not_exist".into()];
        claim.status = ClaimStatus::Verified;
    }
    let none_pass = |rows: &[serde_json::Value]| rows.iter().all(|row| row["result"] != "PASS");
    let row = |required: bool, result: &str| json!({"claim": "c", "required": required, "result": result});
    let mut cases = vec![
        (
            "claims marked verified without evidence are not passes",
            none_pass(&super::claims(&stripped, &sources)),
        ),
        (
            "claims citing evidence that does not exist are not passes",
            none_pass(&super::claims(&forged, &sources)),
        ),
        (
            "a required unknown keeps the result from passing",
            super::overall(&[row(true, "PASS"), row(true, "UNKNOWN")]) != Status::Passed,
        ),
        (
            "a failed required check fails the result",
            super::overall(&[row(true, "PASS"), row(true, "FAIL")]) == Status::Failed,
        ),
        (
            "no required check at all is not a pass",
            super::overall(&[row(false, "PASS")]) != Status::Passed,
        ),
    ];
    if record.definition.verification.claims.is_empty() {
        cases.push(("the skill declares at least one claim", false));
    }
    let failed = cases.iter().filter(|(_, passed)| !passed).count();
    let detail = cases
        .iter()
        .map(|(name, passed)| format!("  {:<5} {name}", if *passed { "PASS" } else { "FAIL" }))
        .collect::<Vec<_>>();
    let response = Response::new("verification.test", if failed == 0 { Status::Passed } else { Status::Failed }, format!("{}: {} cases, {failed} failed", reference(record), cases.len()))
        .reference("skill_ref", reference(record))
        .data(json!({"cases": cases.iter().map(|(name, passed)| json!({"case": name, "status": if *passed { "PASS" } else { "FAIL" }})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
