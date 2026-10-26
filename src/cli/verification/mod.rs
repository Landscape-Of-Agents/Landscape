// crane verification: what proves a skill worked, and what a run proved. Results are PASS, FAIL,
// UNKNOWN, BLOCKED, or INCONCLUSIVE; an agent's statement, a tool's success, or a claim marked
// verified without evidence is never a PASS. One file per command: evaluate, inspect, report, test.

mod evaluate;
mod inspect;
mod report;
mod test;

use serde_json::{json, Value};

use crate::capability::model::{CapabilityDefinition, ClaimStatus};
use crate::capability::refs::Sources;
use crate::capability::validate::effective_claim;
use crate::invocation::engine::{InvocationRecord, VerificationStatus};

/** The command reference of 'crane verification' */
const USAGE: &str = "usage: crane verification inspect --skill SKILL[@N] | --run INVOCATION_ID
       crane verification evaluate --skill SKILL[@N] | --run INVOCATION_ID
       crane verification report --run INVOCATION_ID
       crane verification test --skill SKILL[@N]
  (every command takes --format json)";

/** Judge a skill's claims from their evidence now
 * Input
    - definition: &CapabilityDefinition - skill
    - sources: &Sources - evidence, read through the caller's access
 * Output
    - Vec<Value> {claim, required, result, detail}
*/
pub(super) fn claims(definition: &CapabilityDefinition, sources: &Sources) -> Vec<Value> {
    definition
        .verification
        .claims
        .iter()
        .map(|claim| {
            let (status, reasons, relied) = effective_claim(definition, claim, sources);
            let result = if status == ClaimStatus::Verified { "PASS" } else { "UNKNOWN" };
            json!({"claim": claim.claim_id, "statement": claim.statement, "required": claim.required, "result": result, "detail": if relied.is_empty() { reasons.join("; ") } else { format!("evidence: {}", relied.join(", ")) }})
        })
        .collect()
}

/** Judge a run's recorded verification results
 * Input
    - record: &InvocationRecord - invocation
 * Output
    - Vec<Value> {claim, required, result, detail}
*/
pub(super) fn results(record: &InvocationRecord) -> Vec<Value> {
    let mut rows = record
        .verifications
        .iter()
        .map(|result| {
            let verdict = match result.result {
                VerificationStatus::Verified => "PASS",
                VerificationStatus::Failed => "FAIL",
                VerificationStatus::Unresolved => "UNKNOWN",
                VerificationStatus::Contradicted => "INCONCLUSIVE",
            };
            json!({"claim": result.claim_id, "required": result.required, "result": verdict, "method": result.method, "observed": result.observed, "detail": result.limitations.join("; "), "evidence": result.evidence_refs})
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        rows.push(json!({"claim": "(none)", "required": true, "result": if matches!(record.state, crate::invocation::lifecycle::InvocationState::Blocked | crate::invocation::lifecycle::InvocationState::Abandoned) { "BLOCKED" } else { "UNKNOWN" }, "detail": format!("no verification ran (the invocation is {:?})", record.state)}));
    }
    rows
}

/** The overall status of judged rows: passed only when every required row passed
 * Input
    - rows: &[Value] - judged rows
 * Output
    - crate::cli::protocol::Status
*/
pub(super) fn overall(rows: &[Value]) -> crate::cli::protocol::Status {
    use crate::cli::protocol::Status;
    let required = rows
        .iter()
        .filter(|row| row["required"] == true)
        .collect::<Vec<_>>();
    if required.iter().any(|row| row["result"] == "FAIL") {
        Status::Failed
    } else if required.iter().any(|row| row["result"] == "BLOCKED") {
        Status::Blocked
    } else if !required.is_empty() && required.iter().all(|row| row["result"] == "PASS") {
        Status::Passed
    } else {
        Status::Unknown
    }
}

/** Human lines for judged rows
 * Input
    - rows: &[Value] - rows
 * Output
    - Vec<String>
*/
pub(super) fn lines(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            format!(
                "  {:<12} {}{}: {}",
                row["result"].as_str().unwrap_or_default(),
                row["claim"].as_str().unwrap_or_default(),
                if row["required"] == true {
                    ""
                } else {
                    " (optional)"
                },
                row["detail"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/** Dispatch a verification command
 * Input
    - args: &[String] - arguments after "verification"
 * Output
    - Result<(), String>
*/
pub(crate) fn verification(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("evaluate") => evaluate::evaluate(rest),
        Some("inspect") => inspect::inspect(rest),
        Some("report") => report::report(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}
