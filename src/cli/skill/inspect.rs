// crane skill inspect: what a skill does, in plain words, and its effective contract: steps (which
// change something), inputs, preconditions, the approval it needs, verification claims, side
// effects, lifecycle on this machine and in the shared store, and its digest. Context text is
// guidance; nothing shown here is authority.

use serde_json::json;

use crate::capability::model::StepKind;
use crate::capability::shared::{key, SharedView};
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
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane skill inspect SKILL[@N]")?;
    let local = Local::open()?;
    let record = skill(&local, &args.positional[0])?;
    let definition = &record.definition;
    let applicability = &definition.applicability;
    let shared = match &local.shared {
        SharedView::Absent => "no shared store".to_string(),
        SharedView::Unverified(reason) => format!("shared store UNVERIFIED ({reason})"),
        SharedView::Verified(entries) => entries
            .get(&key(&definition.capability_id, definition.version))
            .map_or("not in the shared store".into(), |entry| {
                format!("{:?} in the shared store", entry.state).to_lowercase()
            }),
    };
    let changes = definition
        .execution
        .steps
        .iter()
        .filter(|step| step.kind.mutating())
        .count();
    let mut detail = vec![
        format!("  purpose: {}", applicability.purpose),
        format!(
            "  department: {}",
            applicability
                .business_domain
                .clone()
                .unwrap_or_else(|| "not set".into())
        ),
        format!(
            "  owner: {}",
            definition
                .owner_ref
                .clone()
                .unwrap_or_else(|| "not set".into())
        ),
        format!(
            "  risk {:?}, review by {:?}; lifecycle {:?}; {shared}",
            applicability.risk, applicability.review_level, record.state
        )
        .to_lowercase(),
        format!("  steps ({changes} change something):"),
    ];
    for step in &definition.execution.steps {
        let marker = match step.kind {
            kind if kind.mutating() => "CHANGES",
            StepKind::Approval => "PERSON ",
            _ => "reads  ",
        };
        detail.push(format!(
            "    {marker} {} ({:?}): {}",
            step.step_id, step.kind, step.description
        ));
    }
    detail.push(format!(
        "  inputs: {}",
        applicability
            .inputs
            .iter()
            .map(|input| input.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for claim in &definition.verification.claims {
        detail.push(format!(
            "  proves{}: {} ({:?})",
            if claim.required { "" } else { " (optional)" },
            claim.statement,
            claim.status
        ));
    }
    for effect in &definition.execution.side_effects {
        detail.push(format!(
            "  side effect on {}: {} (reversible: {})",
            effect.system,
            effect.description,
            effect
                .reversible
                .map_or("unknown".into(), |value| value.to_string())
        ));
    }
    detail.push(format!("  digest {}", record.definition_digest));
    let response = Response::new("skill.inspect", Status::Ok, format!("{} ({}): {}", reference(record), definition.name, definition.description))
        .reference("skill_ref", reference(record))
        .reference("digest", record.definition_digest.clone())
        .data(json!({"skill": reference(record), "state": record.state, "shared": shared, "digest": record.definition_digest, "stale": record.stale, "definition": definition}));
    finish(&args, response, &detail)
}
