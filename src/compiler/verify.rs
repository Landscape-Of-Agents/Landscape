// Verification mapping and gap analysis. Claims come only from what can carry them: a supported
// Work Capture assertion (derived by Work Capture's versioned rules) becomes a verified technical
// claim with its assertion and evidence; an unknown, unresolved, contradicted, or negative result
// becomes an unresolved claim and a gap. Business invariants come from the intake's declared
// intent and are verified only by a check record that names the invariant and reports that it
// held, and only when the invariant's mapping is authoritative; technical results never stand in
// for them. Acceptance criteria stay declared. Hypotheses are listed, never turned into claims.
// Component 2 recomputes every claim again at validation; nothing here is trusted on its word.

use serde::{Deserialize, Serialize};

use super::Basis;
use crate::capability::intake::CapabilityIntake;
use crate::capability::model::{BusinessInvariant, ClaimKind, ClaimStatus, VerificationClaim};
use crate::capability::refs::Sources;
use crate::capture::correlate::assertions::{AssertionStatus, DerivationMethod, RULES_VERSION};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::episode::EpisodeOutcome;
use crate::capture::Truth;

/** Kinds of gaps
 * Variants
    - MissingEvidence - required evidence is missing or a result is unknown
    - Contradiction - evidence disagrees, or the episode observed a negative result
    - Untested - a requirement or case has no check
    - UnsupportedClaim - a claim lacks an authoritative mapping or evidence
    - Unknown - something could not be established (outcome, access, versions)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GapKind {
    MissingEvidence,
    Contradiction,
    Untested,
    UnsupportedClaim,
    Unknown,
}

/** A gap
 * Fields
    - kind: GapKind - kind
    - detail: String - what is missing or wrong
    - refs: Vec<String> - related claims, assertions, or evidence
    - blocking: bool - whether it keeps the candidate from being reported as generated
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Gap {
    pub(crate) kind: GapKind,
    pub(crate) detail: String,
    pub(crate) refs: Vec<String>,
    pub(crate) blocking: bool,
}

/** How a claim of the candidate is supported
 * Fields
    - claim_id: String - claim
    - basis: Basis - observed, derived, declared, or unknown
    - method: String - how it was mapped
    - refs: Vec<String> - assertions and evidence it relies on
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClaimBasis {
    pub(crate) claim_id: String,
    pub(crate) basis: Basis,
    pub(crate) method: String,
    pub(crate) refs: Vec<String>,
}

/** The verification side of a candidate
 * Fields
    - claims: Vec<VerificationClaim> - claims
    - invariants: Vec<BusinessInvariant> - business invariants
    - bases: Vec<ClaimBasis> - support of each claim
    - gaps: Vec<Gap> - gaps
    - negative_cases: Vec<String> - cases the episode did not exercise
    - failure_signals: Vec<String> - negative results showing the episode did not succeed
    - hypotheses: Vec<String> - hypothesis assertions found (never claims)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VerificationPlan {
    pub(crate) claims: Vec<VerificationClaim>,
    pub(crate) invariants: Vec<BusinessInvariant>,
    pub(crate) bases: Vec<ClaimBasis>,
    pub(crate) gaps: Vec<Gap>,
    pub(crate) negative_cases: Vec<String>,
    pub(crate) failure_signals: Vec<String>,
    pub(crate) hypotheses: Vec<String>,
}

/** The claim a Work Capture predicate maps to: id, statement, expected result */
const PREDICATES: &[(&str, &str, &str, &str)] = &[
    (
        "ci_passed",
        "ci-passed",
        "CI passes for the change",
        "CI reports success",
    ),
    (
        "tests_passed",
        "tests-passed",
        "The test suite passes for the change",
        "Tests report zero failures",
    ),
    (
        "deployment_succeeded",
        "deployment-succeeded",
        "The deployment succeeds and its checks report healthy",
        "The deployment and its checks report success",
    ),
    (
        "change_verified",
        "change-verified",
        "The change is verified end to end (derived from CI, tests, and deployment)",
        "Every input result is verified",
    ),
];

/** Build a claim
 * Input
    - id: &str - claim id
    - kind: ClaimKind - kind
    - statement: &str - statement
    - expected: &str - expected result
    - required: bool - required
    - status: ClaimStatus - status
    - environment: Option<String> - environment
 * Output
    - VerificationClaim
*/
fn claim(
    id: &str,
    kind: ClaimKind,
    statement: &str,
    expected: &str,
    required: bool,
    status: ClaimStatus,
    environment: Option<String>,
) -> VerificationClaim {
    VerificationClaim {
        claim_id: id.into(),
        kind,
        statement: statement.into(),
        expected_result: expected.into(),
        required,
        status,
        evidence_refs: Vec::new(),
        assertion_refs: Vec::new(),
        check_ref: None,
        invariant_ref: None,
        method: None,
        environment,
        verified_at: None,
    }
}

/** Build a gap
 * Input
    - kind: GapKind - kind
    - detail: String - detail
    - refs: Vec<String> - references
    - blocking: bool - blocking
 * Output
    - Gap
*/
fn gap(kind: GapKind, detail: String, refs: Vec<String>, blocking: bool) -> Gap {
    Gap {
        kind,
        detail,
        refs,
        blocking,
    }
}

/** Map an intake's assertions, intent, and completeness to claims and gaps
 * Input
    - intake: &CapabilityIntake - validated intake
    - sources: &Sources - canonical sources and the requester's access
    - environment: Option<String> - the environment the episode was observed in
 * Output
    - VerificationPlan
*/
pub(crate) fn plan(
    intake: &CapabilityIntake,
    sources: &Sources,
    environment: Option<String>,
) -> VerificationPlan {
    let mut plan = VerificationPlan::default();
    let method = format!("work-capture-rules@{RULES_VERSION}");
    for assertion in &intake.observed.assertions {
        if assertion.method == DerivationMethod::AgentHypothesis {
            plan.hypotheses.push(assertion.assertion_id.clone());
            continue;
        }
        let Some((_, base_id, statement, expected)) = PREDICATES
            .iter()
            .find(|(predicate, ..)| *predicate == assertion.predicate)
        else {
            continue;
        };
        let taken = plan
            .claims
            .iter()
            .filter(|existing| existing.claim_id.starts_with(base_id))
            .count();
        let claim_id = if taken == 0 {
            base_id.to_string()
        } else {
            format!("{base_id}-{}", taken + 1)
        };
        let mut item = claim(
            &claim_id,
            ClaimKind::Technical,
            statement,
            expected,
            true,
            ClaimStatus::Unresolved,
            environment.clone(),
        );
        item.assertion_refs = vec![assertion.assertion_id.clone()];
        item.evidence_refs.clone_from(&assertion.supporting);
        let mut refs = vec![assertion.assertion_id.clone()];
        refs.extend(assertion.supporting.iter().cloned());
        let conflicted = !assertion.contradicting.is_empty();
        let basis = match (assertion.status, assertion.truth, conflicted) {
            (_, _, true) => {
                refs.extend(assertion.contradicting.iter().cloned());
                plan.gaps.push(gap(
                    GapKind::Contradiction,
                    format!(
                        "evidence disagrees on {} for {}; no side is chosen",
                        assertion.predicate, assertion.subject_key
                    ),
                    refs.clone(),
                    true,
                ));
                Basis::Unknown
            }
            (AssertionStatus::Supported, Truth::Yes, false) => {
                item.status = ClaimStatus::Verified;
                item.method = Some(method.clone());
                Basis::Derived
            }
            (AssertionStatus::Supported, Truth::No, false) => {
                plan.failure_signals.push(format!(
                    "{} for {} was observed as no",
                    assertion.predicate, assertion.subject_key
                ));
                plan.gaps.push(gap(
                    GapKind::Contradiction,
                    format!(
                        "the episode observed {} = no for {}",
                        assertion.predicate, assertion.subject_key
                    ),
                    refs.clone(),
                    true,
                ));
                Basis::Derived
            }
            _ => {
                item.evidence_refs.clear();
                plan.gaps.push(gap(
                    GapKind::MissingEvidence,
                    format!(
                        "{} for {} could not be established",
                        assertion.predicate, assertion.subject_key
                    ),
                    refs.clone(),
                    true,
                ));
                Basis::Unknown
            }
        };
        plan.bases.push(ClaimBasis {
            claim_id,
            basis,
            method: method.clone(),
            refs,
        });
        plan.claims.push(item);
    }
    let checks = intake
        .observed
        .deployments
        .iter()
        .chain(&intake.observed.runtime)
        .filter(|reference| {
            matches!(
                reference.evidence_type,
                EvidenceType::Verification | EvidenceType::Metric
            )
        })
        .filter_map(|reference| sources.evidence(&reference.evidence_id).1)
        .collect::<Vec<_>>();
    for invariant in &intake.intent.invariants {
        let mapping = invariant.effective_mapping();
        plan.invariants.push(BusinessInvariant {
            invariant_id: invariant.id.clone(),
            statement: invariant.statement.clone(),
            authority: invariant.authority.clone(),
            mapping,
            required: true,
        });
        let claim_id = format!("invariant-{}", invariant.id);
        let mut item = claim(
            &claim_id,
            ClaimKind::BusinessInvariant,
            &invariant.statement,
            "A check naming the invariant reports that it held",
            true,
            ClaimStatus::Unresolved,
            environment.clone(),
        );
        item.invariant_ref = Some(invariant.id.clone());
        let found = checks
            .iter()
            .filter(|record| record.field("invariant").as_deref() == Some(invariant.id.as_str()))
            .collect::<Vec<_>>();
        let held = found
            .iter()
            .filter(|record| {
                record
                    .field("healthy")
                    .or_else(|| record.field("holds"))
                    .as_deref()
                    == Some("true")
            })
            .map(|record| record.evidence_id.clone())
            .collect::<Vec<_>>();
        let failed = found
            .iter()
            .filter(|record| {
                record
                    .field("healthy")
                    .or_else(|| record.field("holds"))
                    .as_deref()
                    == Some("false")
            })
            .map(|record| record.evidence_id.clone())
            .collect::<Vec<_>>();
        let basis = if !mapping.authoritative() {
            plan.gaps.push(gap(
                GapKind::UnsupportedClaim,
                format!("invariant {} has no authoritative mapping", invariant.id),
                vec![invariant.id.clone()],
                true,
            ));
            Basis::Declared
        } else if !failed.is_empty() {
            plan.gaps.push(gap(
                GapKind::Contradiction,
                format!(
                    "a check reported that invariant {} did not hold",
                    invariant.id
                ),
                failed.clone(),
                true,
            ));
            Basis::Observed
        } else if held.is_empty() {
            plan.gaps.push(gap(
                GapKind::Untested,
                format!(
                    "no check of invariant {} was observed; technical results do not verify it",
                    invariant.id
                ),
                vec![invariant.id.clone()],
                true,
            ));
            Basis::Declared
        } else {
            item.status = ClaimStatus::Verified;
            item.method = Some("declared-invariant-check".into());
            item.evidence_refs.clone_from(&held);
            Basis::Observed
        };
        let mut refs = held;
        refs.extend(failed);
        plan.bases.push(ClaimBasis {
            claim_id,
            basis,
            method: "declared-invariant-check".into(),
            refs,
        });
        plan.claims.push(item);
    }
    for criterion in &intake.intent.acceptance_criteria {
        let claim_id = format!("acceptance-{}", criterion.id);
        plan.claims.push(claim(
            &claim_id,
            ClaimKind::Acceptance,
            &criterion.statement,
            "The criterion is met",
            false,
            ClaimStatus::Declared,
            environment.clone(),
        ));
        plan.bases.push(ClaimBasis {
            claim_id,
            basis: Basis::Declared,
            method: "intake-intent".into(),
            refs: vec![criterion.id.clone()],
        });
        plan.gaps.push(gap(
            GapKind::Untested,
            format!(
                "acceptance criterion {} is declared but no check maps to it",
                criterion.id
            ),
            vec![criterion.id.clone()],
            false,
        ));
    }
    let completeness = &intake.completeness;
    for missing in &completeness.missing {
        plan.gaps.push(gap(
            GapKind::MissingEvidence,
            format!(
                "required {:?} evidence is missing ({:?}): {}",
                missing.evidence_type, missing.state, missing.requirement
            ),
            Vec::new(),
            true,
        ));
    }
    for source in &completeness.unauthorized_sources {
        plan.gaps.push(gap(
            GapKind::Unknown,
            format!("evidence not accessible to the requester: {source}"),
            vec![source.clone()],
            true,
        ));
    }
    for check in &completeness.stale_revisions {
        plan.gaps.push(gap(
            GapKind::Unknown,
            format!(
                "{} {} is {:?}: {}",
                check.kind, check.reference, check.outcome, check.detail
            ),
            vec![check.reference.clone()],
            true,
        ));
    }
    for mapping in &completeness.unresolved_mappings {
        plan.gaps.push(gap(
            GapKind::UnsupportedClaim,
            format!("business reference {mapping} is not authoritatively mapped"),
            vec![mapping.clone()],
            false,
        ));
    }
    match intake.work.outcome {
        None => plan.gaps.push(gap(
            GapKind::Unknown,
            "the episode's final outcome was not recorded".into(),
            Vec::new(),
            true,
        )),
        Some(outcome @ (EpisodeOutcome::Failed | EpisodeOutcome::Abandoned)) => plan
            .failure_signals
            .push(format!("the recorded outcome is {outcome:?}")),
        Some(outcome @ (EpisodeOutcome::Unknown | EpisodeOutcome::PartiallyVerified)) => {
            plan.gaps.push(gap(
                GapKind::Unknown,
                format!("the recorded outcome is {outcome:?}"),
                Vec::new(),
                true,
            ))
        }
        Some(EpisodeOutcome::Succeeded) => {}
    }
    if intake.observed.recovery.is_empty() {
        plan.negative_cases
            .push("Recovery or rollback was not exercised in the source episode".into());
        plan.gaps.push(gap(
            GapKind::Untested,
            "no recovery or rollback was observed".into(),
            Vec::new(),
            false,
        ));
    }
    if plan.failure_signals.is_empty() {
        plan.negative_cases
            .push("Behavior when a step fails was not observed".into());
    }
    plan
}
