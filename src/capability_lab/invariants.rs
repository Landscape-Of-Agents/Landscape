// Invariant verification. Every claim is recomputed from the current evidence with Component 2's
// own rule (effective_claim), so a status the candidate declares is never taken on its word:
// declared-verified claims that no longer verify fail with the reasons as counterexample; required
// claims that are not verified are blocked (they need evidence, which the lab cannot supply);
// optional declared claims stay inconclusive. Contradicted assertions fail. Every required
// business invariant must have a claim that verifies. Dependencies are resolved against the
// current Landscape and Crane versions: stale or missing required ones fail, unverifiable ones are
// inconclusive. The single-episode generalization stays an inconclusive check, so a fully passing
// run still states that applicability beyond the tested conditions is unproven.

use serde_json::json;

use super::{CheckCategory, CheckStatus, LabCheck};
use crate::capability::model::{CapabilityDefinition, ClaimKind, ClaimStatus};
use crate::capability::query::dependencies;
use crate::capability::refs::{RefOutcome, Sources};
use crate::capability::registry::RegistryState;
use crate::capability::validate::effective_claim;

/** Build an invariant check
 * Input
    - subject: &str - subject
    - status: CheckStatus - status
    - detail: impl Into<String> - explanation
 * Output
    - LabCheck
*/
fn check(subject: &str, status: CheckStatus, detail: impl Into<String>) -> LabCheck {
    LabCheck::new(
        format!("invariant:{subject}"),
        CheckCategory::Invariant,
        status,
        detail,
    )
}

/** Verify claims, invariants, contradictions, dependencies, and generalization
 * Input
    - definition: &CapabilityDefinition - candidate
    - state: &RegistryState - registry
    - sources: &Sources - canonical sources and current versions
 * Output
    - Vec<LabCheck>
*/
pub(crate) fn verify(
    definition: &CapabilityDefinition,
    state: &RegistryState,
    sources: &Sources,
) -> Vec<LabCheck> {
    let mut checks = Vec::new();
    for claim in &definition.verification.claims {
        let subject = format!("claim:{}", claim.claim_id);
        let (status, reasons, relied) = effective_claim(definition, claim, sources);
        let conflicted = claim
            .assertion_refs
            .iter()
            .filter_map(|id| sources.capture.assertions.get(id))
            .filter(|assertion| assertion.conflicted())
            .map(|assertion| assertion.assertion_id.clone())
            .collect::<Vec<_>>();
        let result = if !conflicted.is_empty() {
            check(
                &subject,
                CheckStatus::Failed,
                "it rests on contradictory evidence; neither side is chosen",
            )
            .with_counterexample(json!({"conflicted_assertions": conflicted}))
        } else {
            match (claim.status, status, claim.required) {
                (ClaimStatus::Verified, ClaimStatus::Verified, _) => check(
                    &subject,
                    CheckStatus::Passed,
                    format!(
                        "verifies from {} reference(s) in {}",
                        relied.len(),
                        claim
                            .environment
                            .clone()
                            .unwrap_or_else(|| "an unknown environment".into())
                    ),
                ),
                (ClaimStatus::Verified, _, _) => check(
                    &subject,
                    CheckStatus::Failed,
                    "declared verified, but the evidence does not verify it",
                )
                .with_counterexample(json!({"reasons": reasons})),
                (_, _, true) => check(
                    &subject,
                    CheckStatus::Blocked,
                    format!("required but {:?}: {}", claim.status, reasons.join("; ")),
                ),
                (_, _, false) => check(
                    &subject,
                    CheckStatus::Inconclusive,
                    format!("{:?} and optional; nothing verifies it", claim.status),
                ),
            }
        };
        let mut refs = claim.assertion_refs.clone();
        refs.extend(claim.evidence_refs.iter().cloned());
        checks.push(result.with_refs(refs));
    }
    for invariant in definition
        .verification
        .invariants
        .iter()
        .filter(|invariant| invariant.required)
    {
        let subject = format!("business-invariant:{}", invariant.invariant_id);
        let verified = definition
            .verification
            .claims
            .iter()
            .filter(|claim| {
                claim.kind == ClaimKind::BusinessInvariant
                    && claim.invariant_ref.as_deref() == Some(invariant.invariant_id.as_str())
            })
            .any(|claim| effective_claim(definition, claim, sources).0 == ClaimStatus::Verified);
        checks.push(if !invariant.mapping.authoritative() {
            check(
                &subject,
                CheckStatus::Failed,
                format!("{:?} is not an authoritative mapping", invariant.mapping),
            )
        } else if verified {
            check(
                &subject,
                CheckStatus::Passed,
                format!(
                    "\"{}\" is verified within the tested scope ({})",
                    invariant.statement,
                    invariant.authority.clone().unwrap_or_default()
                ),
            )
        } else {
            check(
                &subject,
                CheckStatus::Blocked,
                format!(
                    "no claim verifies \"{}\"; technical results do not stand in for it",
                    invariant.statement
                ),
            )
        });
    }
    match dependencies(
        state,
        &definition.capability_id,
        definition.version,
        sources,
    ) {
        Ok(statuses) => {
            for status in statuses {
                let subject = format!(
                    "dependency:{:?}:{}",
                    status.dependency.kind, status.dependency.target_id
                )
                .to_ascii_lowercase();
                let detail = format!(
                    "{:?} {} is {:?}",
                    status.dependency.kind, status.dependency.target_id, status.outcome
                );
                checks.push(match (status.outcome, status.dependency.required) {
                    (RefOutcome::Resolved, _) => check(&subject, CheckStatus::Passed, detail),
                    (RefOutcome::Unverifiable, _) | (_, false) => {
                        check(&subject, CheckStatus::Inconclusive, detail)
                    }
                    _ => check(&subject, CheckStatus::Failed, detail).with_counterexample(
                        json!({"dependency": status.dependency, "outcome": status.outcome}),
                    ),
                });
            }
        }
        Err(error) => checks.push(check(
            "dependencies",
            CheckStatus::Blocked,
            error.to_string(),
        )),
    }
    for assumption in definition
        .applicability
        .assumptions
        .iter()
        .filter(|assumption| assumption.status != ClaimStatus::Verified)
    {
        checks.push(check(
            &format!("assumption:{}", assumption.id),
            CheckStatus::Inconclusive,
            format!("{:?}: {}", assumption.status, assumption.description),
        ));
    }
    checks
}
