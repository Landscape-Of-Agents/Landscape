// Source conformance: each compiled step is re-derived from the observed record it cites, with the
// compiler's own versioned rule, and must reproduce the same kind and description; the record must
// belong to the candidate's Evidence Pack and be readable by the requester. A step whose kind came
// from the conservative fallback (an unrecognized command treated as a change) is reported as
// inconclusive, not passed. A proposed (hypothesis) step has no observed record and stays
// inconclusive. Each claim's recorded basis is checked against its sources: derived claims against
// their Work Capture assertions, observed invariant claims against the check record naming the
// invariant; declared and unknown claims are reported as such.

use std::collections::BTreeSet;

use serde_json::json;

use super::{CheckCategory, CheckStatus, LabCheck};
use crate::capability::model::CapabilityDefinition;
use crate::capability::refs::{RefOutcome, Sources};
use crate::compiler::assemble::CompilationReport;
use crate::compiler::extract::{recognized, step_of};
use crate::compiler::Basis;

/** Build a conformance check
 * Input
    - subject: &str - step or claim id
    - status: CheckStatus - status
    - detail: impl Into<String> - explanation
 * Output
    - LabCheck
*/
fn check(subject: &str, status: CheckStatus, detail: impl Into<String>) -> LabCheck {
    LabCheck::new(
        format!("conformance:{subject}"),
        CheckCategory::Conformance,
        status,
        detail,
    )
}

/** Check steps and claims against their sources
 * Input
    - definition: &CapabilityDefinition - candidate
    - report: Option<&CompilationReport> - its compilation report, when usable
    - sources: &Sources - canonical sources and the requester's access
 * Output
    - Vec<LabCheck>
*/
pub(crate) fn conformance(
    definition: &CapabilityDefinition,
    report: Option<&CompilationReport>,
    sources: &Sources,
) -> Vec<LabCheck> {
    let Some(report) = report else {
        return vec![check("all", CheckStatus::NotRun, "no usable compilation report; steps and claims cannot be traced to their recorded bases")];
    };
    let pack_evidence = report
        .lineage
        .as_ref()
        .and_then(|lineage| sources.capture.packs.get(&lineage.pack_id))
        .map(|pack| {
            pack.evidence
                .iter()
                .map(|item| item.evidence_id.clone())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut checks = Vec::new();
    for step in &definition.execution.steps {
        let Some(basis) = report
            .step_bases
            .iter()
            .find(|basis| basis.step_id == step.step_id)
        else {
            checks.push(check(
                &step.step_id,
                CheckStatus::Failed,
                "the compilation report records no basis for this step",
            ));
            continue;
        };
        if basis.basis != Basis::Observed {
            checks.push(
                check(
                    &step.step_id,
                    CheckStatus::Inconclusive,
                    format!(
                        "the step is a {:?}; no observed record supports it",
                        basis.basis
                    ),
                )
                .with_basis(basis.basis)
                .with_refs(basis.evidence_refs.clone()),
            );
            continue;
        }
        let Some(cited) = basis.evidence_refs.first() else {
            checks.push(
                check(
                    &step.step_id,
                    CheckStatus::Failed,
                    "an observed step cites no record",
                )
                .with_basis(basis.basis),
            );
            continue;
        };
        let (reference, record) = sources.evidence(cited);
        let Some(record) = record else {
            let status = if reference.outcome == RefOutcome::Unauthorized {
                CheckStatus::Blocked
            } else {
                CheckStatus::Failed
            };
            checks.push(
                check(
                    &step.step_id,
                    status,
                    format!(
                        "its record {cited} is {:?}: {}",
                        reference.outcome, reference.detail
                    ),
                )
                .with_basis(basis.basis)
                .with_refs(vec![cited.clone()]),
            );
            continue;
        };
        if !pack_evidence.contains(cited) {
            checks.push(
                check(
                    &step.step_id,
                    CheckStatus::Failed,
                    format!("record {cited} is not part of the candidate's Evidence Pack"),
                )
                .with_basis(basis.basis)
                .with_refs(vec![cited.clone()]),
            );
            continue;
        }
        match step_of(record) {
            Some((kind, description)) if kind == step.kind && description == step.description => {
                if recognized(record) {
                    checks.push(
                        check(
                            &step.step_id,
                            CheckStatus::Passed,
                            format!("re-derived as {kind:?} \"{description}\" from {cited}"),
                        )
                        .with_basis(basis.basis)
                        .with_refs(vec![cited.clone()]),
                    );
                } else {
                    checks.push(check(&step.step_id, CheckStatus::Inconclusive, format!("its kind {kind:?} comes from the conservative rule for unrecognized commands, not from recognizing the action")).with_basis(basis.basis).with_refs(vec![cited.clone()]));
                }
            }
            derived => {
                let counterexample = json!({"compiled": {"kind": step.kind, "description": step.description}, "re_derived": derived.map(|(kind, description)| json!({"kind": kind, "description": description}))});
                checks.push(
                    check(
                        &step.step_id,
                        CheckStatus::Failed,
                        format!("record {cited} does not reproduce the compiled step"),
                    )
                    .with_basis(basis.basis)
                    .with_refs(vec![cited.clone()])
                    .with_counterexample(counterexample),
                );
            }
        }
    }
    for claim in &definition.verification.claims {
        let subject = format!("claim:{}", claim.claim_id);
        let Some(basis) = report
            .claim_bases
            .iter()
            .find(|basis| basis.claim_id == claim.claim_id)
        else {
            checks.push(check(
                &subject,
                CheckStatus::Failed,
                "the compilation report records no basis for this claim",
            ));
            continue;
        };
        let result = match basis.basis {
            Basis::Derived => {
                let missing = claim
                    .assertion_refs
                    .iter()
                    .filter(|id| !sources.capture.assertions.contains_key(*id))
                    .cloned()
                    .collect::<Vec<_>>();
                let mismatch = claim
                    .assertion_refs
                    .iter()
                    .filter_map(|id| sources.capture.assertions.get(id))
                    .any(|assertion| {
                        claim
                            .evidence_refs
                            .iter()
                            .any(|evidence| !assertion.supporting.contains(evidence))
                    });
                match (missing.is_empty(), mismatch) {
                    (false, _) => check(
                        &subject,
                        CheckStatus::Failed,
                        format!("its assertions do not exist: {}", missing.join(", ")),
                    ),
                    (true, true) => check(
                        &subject,
                        CheckStatus::Failed,
                        "it cites evidence its assertion does not rest on",
                    ),
                    (true, false) if claim.assertion_refs.is_empty() => check(
                        &subject,
                        CheckStatus::Failed,
                        "a derived claim cites no assertion",
                    ),
                    _ => check(
                        &subject,
                        CheckStatus::Passed,
                        "derived from the cited Work Capture assertion and its evidence",
                    ),
                }
            }
            Basis::Observed => {
                let named = claim
                    .evidence_refs
                    .iter()
                    .filter_map(|id| sources.evidence(id).1)
                    .all(|record| {
                        record.field("invariant").as_deref() == claim.invariant_ref.as_deref()
                    });
                if !claim.evidence_refs.is_empty() && named {
                    check(
                        &subject,
                        CheckStatus::Passed,
                        "observed by a check record that names this invariant",
                    )
                } else {
                    check(&subject, CheckStatus::Failed, "its observed records do not name the claimed invariant or are not readable")
                }
            }
            other => check(
                &subject,
                CheckStatus::Inconclusive,
                format!("the claim is {other:?}; there is nothing to trace beyond its declaration"),
            ),
        };
        let mut refs = claim.assertion_refs.clone();
        refs.extend(claim.evidence_refs.iter().cloned());
        checks.push(result.with_basis(basis.basis).with_refs(refs));
    }
    checks
}
