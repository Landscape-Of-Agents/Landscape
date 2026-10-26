// crane precondition test: the evaluator must fail closed for this skill: nothing confirmed leaves
// every manual precondition unresolved; an agent's confirmation never counts; a person's
// confirmation is declared, not observed; a claim stripped of its evidence (a spoofed "verified")
// does not hold; and a check that names no claim stays unresolved. Runs on copies; nothing is stored.

use serde_json::json;

use crate::capability::model::{ClaimStatus, Condition};
use crate::capability::preconditions::{evaluate as judge, Source, Truth};
use crate::capture::{Actor, ActorKind};
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
    args.expect_positional(0, "crane precondition test --skill SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.value("skill").ok_or("'--skill' is required")?)?;
    let sources = local.context.sources();
    let person = local.context.actor.clone();
    let agent = Actor::new("agent-under-test", ActorKind::Agent);
    // A copy with one condition of each kind added, so every rule is exercised for any skill
    let mut probe = record.definition.clone();
    let claim = probe.verification.claims.first().cloned();
    probe.applicability.preconditions.push(Condition {
        id: "probe-manual".into(),
        description: "a manual fact".into(),
        check: None,
        status: ClaimStatus::Declared,
    });
    probe.applicability.preconditions.push(Condition {
        id: "probe-unregistered".into(),
        description: "a check nobody registered".into(),
        check: Some("no-such-check".into()),
        status: ClaimStatus::Verified,
    });
    if let Some(claim) = &claim {
        probe.applicability.preconditions.push(Condition {
            id: "probe-spoofed".into(),
            description: "a claim without evidence".into(),
            check: Some(claim.claim_id.clone()),
            status: ClaimStatus::Verified,
        });
        for stored in probe
            .verification
            .claims
            .iter_mut()
            .filter(|stored| stored.claim_id == claim.claim_id)
        {
            stored.evidence_refs.clear();
            stored.assertion_refs.clear();
            stored.status = ClaimStatus::Verified;
        }
    }
    let ids = |evaluations: &[crate::capability::preconditions::Evaluation], id: &str| {
        evaluations
            .iter()
            .find(|evaluation| evaluation.id == id)
            .map(|evaluation| evaluation.truth)
    };
    let nothing = judge(&probe, None, &sources, &[], &person)?;
    let by_agent = judge(
        &probe,
        None,
        &sources,
        &["probe-manual".to_string()],
        &agent,
    )?;
    let by_person = judge(
        &probe,
        None,
        &sources,
        &["probe-manual".to_string()],
        &person,
    )?;
    let mut cases = vec![
        (
            "with nothing confirmed, every manual precondition is unresolved",
            nothing
                .iter()
                .filter(|evaluation| evaluation.source == Source::Manual)
                .all(|evaluation| evaluation.truth == Truth::Unresolved),
        ),
        (
            "an agent's confirmation does not count",
            ids(&by_agent, "probe-manual") == Some(Truth::Unresolved),
        ),
        (
            "a person's confirmation is declared, not observed",
            ids(&by_person, "probe-manual") == Some(Truth::Declared),
        ),
        (
            "a check that names no claim stays unresolved",
            ids(&nothing, "probe-unregistered") == Some(Truth::Unresolved),
        ),
    ];
    if claim.is_some() {
        cases.push((
            "a claim marked verified without evidence does not hold",
            ids(&nothing, "probe-spoofed") == Some(Truth::Unresolved),
        ));
    }
    let failed = cases.iter().filter(|(_, passed)| !passed).count();
    let detail = cases
        .iter()
        .map(|(name, passed)| format!("  {:<5} {name}", if *passed { "PASS" } else { "FAIL" }))
        .collect::<Vec<_>>();
    let response = Response::new("precondition.test", if failed == 0 { Status::Passed } else { Status::Failed }, format!("{}: {} cases, {failed} failed", reference(record), cases.len()))
        .reference("skill_ref", reference(record))
        .data(json!({"cases": cases.iter().map(|(name, passed)| json!({"case": name, "status": if *passed { "PASS" } else { "FAIL" }})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
