// Deterministic signal detection and qualification. Only independent trials of the exact analyzed
// digest are counted (resubmissions collapsed, blocked and unfinished invocations excluded), and
// they are partitioned by comparable context (environment, repository, and the governance
// versions current when they ran), so incomparable contexts are never pooled. In each context:
//
//   two or more established failures  -> repeated_failure (contradicting successes kept)
//   one established failure            -> diagnostic_failure (never a recurring pattern)
//   verification gaps                  -> verification_gap
//   contradictions without failures    -> contradictory_outcomes
//   only verified successes            -> verified_success_pattern (no change implied)
//
// plus one dependency_drift signal per observed drifted dependency, context_unavailable when a
// pinned source cannot be read, and no_longer_eligible for a withdrawn version. Qualification
// evaluates explicit rules (independent count against configurable thresholds raised for higher
// risk, verified outcomes only, comparability, no contradictions, failure share, not explained by
// infrastructure or changed dependencies, evidence readable); any rule not met leaves the signal
// with insufficient evidence. The rules are decision rules, not statistical confidence.

use std::collections::{BTreeMap, BTreeSet};

use super::models::{
    DriftFinding, DriftReport, DriftStatus, EvidenceAccess, EvidenceRef, Hypothesis, Observation,
    OutcomeClass, PartitionSummary, Qualification, RuleResult, RuleStatus, SignalKind, Thresholds,
};
use crate::capability::model::RiskLevel;

/** A detected, not yet qualified, signal
 * Fields
    - kind: SignalKind - kind
    - context_key: String - context
    - supporting, contradicting: Vec<String> - invocation ids
    - collapsed: Vec<String> - resubmissions collapsed into the supporting ones
    - affected_claims, affected_steps: Vec<String> - claims and steps
    - evidence: Vec<EvidenceRef> - evidence of the supporting and contradicting observations
    - failures, successes: usize - established failures and verified successes in the context
    - infrastructure: bool - a supporting failure names unavailable infrastructure
    - context_governance: BTreeMap<String, String> - governance versions of the context
    - drift: Vec<DriftFinding> - drift findings (drift signals)
*/
#[derive(Debug, Clone)]
pub(crate) struct Detected {
    pub(crate) kind: SignalKind,
    pub(crate) context_key: String,
    pub(crate) supporting: Vec<String>,
    pub(crate) contradicting: Vec<String>,
    pub(crate) collapsed: Vec<String>,
    pub(crate) affected_claims: Vec<String>,
    pub(crate) affected_steps: Vec<String>,
    pub(crate) evidence: Vec<EvidenceRef>,
    pub(crate) failures: usize,
    pub(crate) successes: usize,
    pub(crate) infrastructure: bool,
    pub(crate) context_governance: BTreeMap<String, String>,
    pub(crate) drift: Vec<DriftFinding>,
}

/** The independent trials of the exact digest, by comparable context
 * Input
    - observations: &[Observation] - observations (duplicates marked)
    - digest: &str - analyzed definition digest
 * Output
    - BTreeMap<String, Vec<&Observation>> context key to trials
*/
pub(crate) fn partition<'a>(
    observations: &'a [Observation],
    digest: &str,
) -> BTreeMap<String, Vec<&'a Observation>> {
    let mut partitions: BTreeMap<String, Vec<&Observation>> = BTreeMap::new();
    for observation in observations.iter().filter(|observation| {
        observation.duplicate_of.is_none()
            && observation.capability_digest == digest
            && observation.class.trial()
    }) {
        partitions
            .entry(observation.context.key())
            .or_default()
            .push(observation);
    }
    partitions
}

/** Summaries of the partitions
 * Input
    - partitions: &BTreeMap<String, Vec<&Observation>> - partitions
 * Output
    - Vec<PartitionSummary>
*/
pub(crate) fn summaries(partitions: &BTreeMap<String, Vec<&Observation>>) -> Vec<PartitionSummary> {
    partitions
        .iter()
        .map(|(key, members)| {
            let mut counts = BTreeMap::new();
            for member in members {
                *counts.entry(member.class).or_insert(0) += 1;
            }
            PartitionSummary {
                context_key: key.clone(),
                independent: members.len(),
                counts,
            }
        })
        .collect()
}

/** Sorted, unique strings
 * Input
    - items: impl Iterator<Item = String> - strings
 * Output
    - Vec<String>
*/
fn unique(items: impl Iterator<Item = String>) -> Vec<String> {
    items.collect::<BTreeSet<_>>().into_iter().collect()
}

/** Detect signals from the partitions and the drift report
 * Input
    - observations: &[Observation] - every observation (duplicates marked)
    - digest: &str - analyzed digest
    - drift: &DriftReport - drift report
 * Output
    - Vec<Detected>
*/
pub(crate) fn detect(
    observations: &[Observation],
    digest: &str,
    drift: &DriftReport,
) -> Vec<Detected> {
    let mut detected = Vec::new();
    for (key, members) in partition(observations, digest) {
        let of = |test: &dyn Fn(OutcomeClass) -> bool| {
            members
                .iter()
                .filter(|member| test(member.class))
                .copied()
                .collect::<Vec<_>>()
        };
        let failures = of(&|class| class.established_failure());
        let successes = of(&|class| class == OutcomeClass::VerifiedSuccess);
        let contradicted = of(&|class| class == OutcomeClass::Contradicted);
        let gaps = of(&|class| class.gap());
        let collapsed_into = |supporting: &[&Observation]| {
            unique(
                observations
                    .iter()
                    .filter(|observation| {
                        observation.duplicate_of.as_ref().is_some_and(|id| {
                            supporting.iter().any(|member| &member.invocation_id == id)
                        })
                    })
                    .map(|observation| observation.invocation_id.clone()),
            )
        };
        let build = |kind: SignalKind,
                     supporting: &[&Observation],
                     contradicting: &[&Observation],
                     claims: Vec<String>| Detected {
            kind,
            context_key: key.clone(),
            supporting: supporting
                .iter()
                .map(|member| member.invocation_id.clone())
                .collect(),
            contradicting: contradicting
                .iter()
                .map(|member| member.invocation_id.clone())
                .collect(),
            collapsed: collapsed_into(supporting),
            affected_claims: claims,
            affected_steps: unique(
                supporting
                    .iter()
                    .flat_map(|member| member.failed_steps.iter().cloned()),
            ),
            evidence: supporting
                .iter()
                .chain(contradicting)
                .flat_map(|member| member.evidence.iter().cloned())
                .collect(),
            failures: failures.len(),
            successes: successes.len(),
            infrastructure: supporting.iter().any(|member| member.infrastructure),
            context_governance: members
                .first()
                .map(|member| member.context.governance.clone())
                .unwrap_or_default(),
            drift: Vec::new(),
        };
        let mut against = successes.clone();
        against.extend(contradicted.iter().copied());
        let failed_claims = |members: &[&Observation]| {
            unique(members.iter().flat_map(|member| {
                member
                    .failed_claims
                    .iter()
                    .chain(&member.contradicted_claims)
                    .cloned()
            }))
        };
        match failures.len() {
            0 => {}
            1 => detected.push(build(
                SignalKind::DiagnosticFailure,
                &failures,
                &against,
                failed_claims(&failures),
            )),
            _ => detected.push(build(
                SignalKind::RepeatedFailure,
                &failures,
                &against,
                failed_claims(&failures),
            )),
        }
        if !gaps.is_empty() {
            let claims = unique(
                gaps.iter()
                    .flat_map(|member| member.unresolved_claims.iter().cloned()),
            );
            detected.push(build(SignalKind::VerificationGap, &gaps, &[], claims));
        }
        if !contradicted.is_empty() && failures.is_empty() {
            let claims = unique(
                contradicted
                    .iter()
                    .flat_map(|member| member.contradicted_claims.iter().cloned()),
            );
            detected.push(build(
                SignalKind::ContradictoryOutcomes,
                &contradicted,
                &successes,
                claims,
            ));
        }
        if !successes.is_empty()
            && failures.is_empty()
            && contradicted.is_empty()
            && gaps.is_empty()
        {
            detected.push(build(
                SignalKind::VerifiedSuccessPattern,
                &successes,
                &[],
                Vec::new(),
            ));
        }
    }
    let empty = |kind: SignalKind, key: String, findings: Vec<DriftFinding>| Detected {
        kind,
        context_key: key,
        supporting: unique(
            findings
                .iter()
                .flat_map(|finding| finding.invocations.iter().cloned()),
        ),
        contradicting: Vec::new(),
        collapsed: Vec::new(),
        affected_claims: Vec::new(),
        affected_steps: Vec::new(),
        evidence: Vec::new(),
        failures: 0,
        successes: 0,
        infrastructure: false,
        context_governance: BTreeMap::new(),
        drift: findings,
    };
    for finding in &drift.findings {
        match finding.status {
            DriftStatus::Changed | DriftStatus::Missing => {
                let suspected = drift
                    .findings
                    .iter()
                    .filter(|other| {
                        other.status == DriftStatus::SuspectedAtInvocation
                            && other.kind == finding.kind
                            && other.target_id == finding.target_id
                    })
                    .cloned();
                let findings = std::iter::once(finding.clone()).chain(suspected).collect();
                detected.push(empty(
                    SignalKind::DependencyDrift,
                    format!("dependency:{}:{}", finding.kind, finding.target_id),
                    findings,
                ));
            }
            DriftStatus::NoLongerEligible => detected.push(empty(
                SignalKind::NoLongerEligible,
                "lifecycle".into(),
                vec![finding.clone()],
            )),
            _ => {}
        }
    }
    let unknown = drift
        .findings
        .iter()
        .filter(|finding| {
            finding.status == DriftStatus::Unknown
                && matches!(finding.kind.as_str(), "zone" | "flow" | "policy")
        })
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        detected.push(empty(
            SignalKind::ContextUnavailable,
            "governance-context".into(),
            unknown,
        ));
    }
    detected
}

/** Build a rule result
 * Input
    - rule: &str - rule name
    - met: Option<bool> - Some(true) met, Some(false) not met, None unknown
    - detail: String - explanation
 * Output
    - RuleResult
*/
fn rule(rule: &str, met: Option<bool>, detail: String) -> RuleResult {
    RuleResult {
        rule: rule.into(),
        status: match met {
            Some(true) => RuleStatus::Met,
            Some(false) => RuleStatus::NotMet,
            None => RuleStatus::Unknown,
        },
        detail,
    }
}

/** Qualify a detected signal with the deterministic rules
 * Input
    - detected: &Detected - signal
    - risk: RiskLevel - capability risk
    - pins: &BTreeMap<String, String> - pinned governance versions of the definition
    - thresholds: Thresholds - thresholds
 * Output
    - Qualification
*/
pub(crate) fn qualify(
    detected: &Detected,
    risk: RiskLevel,
    pins: &BTreeMap<String, String>,
    thresholds: Thresholds,
) -> Qualification {
    let elevated = matches!(
        risk,
        RiskLevel::High | RiskLevel::Critical | RiskLevel::Unknown
    );
    let minimum = if elevated {
        thresholds.min_independent_elevated
    } else {
        thresholds.min_independent
    } as usize;
    let count = detected.supporting.len();
    let independent = |minimum: usize| {
        rule(
            "independent_observations",
            Some(count >= minimum),
            format!(
                "{count} independent invocation(s) (resubmissions collapsed: {}); at least {minimum} required for {risk:?} risk",
                detected.collapsed.len()
            ),
        )
    };
    let evidence_rule = || {
        let unreadable = detected
            .evidence
            .iter()
            .filter(|reference| {
                detected.supporting.contains(&reference.invocation_id)
                    && matches!(
                        reference.access,
                        EvidenceAccess::Missing | EvidenceAccess::Restricted
                    )
            })
            .map(|reference| reference.ref_id.clone())
            .collect::<Vec<_>>();
        if unreadable.is_empty() {
            rule(
                "evidence_readable",
                Some(true),
                "every supporting evidence reference is readable or recorded in an integrity-checked invocation record".into(),
            )
        } else {
            rule(
                "evidence_readable",
                None,
                format!("missing or restricted evidence: {}", unreadable.join(", ")),
            )
        }
    };
    let contradictions = || {
        rule(
            "no_contradictions",
            Some(detected.contradicting.is_empty()),
            if detected.contradicting.is_empty() {
                "no contradicting outcome in the same context".into()
            } else {
                format!(
                    "{} contradicting outcome(s) in the same context are kept: {}",
                    detected.contradicting.len(),
                    detected.contradicting.join(", ")
                )
            },
        )
    };
    let mut rules = Vec::new();
    match detected.kind {
        SignalKind::RepeatedFailure => {
            rules.push(independent(minimum));
            rules.push(rule("verified_outcomes", Some(true), "only established failures (failed verification or a definite failure from the system of record) are counted; declared outcomes are not".into()));
            rules.push(rule(
                "comparable_context",
                Some(true),
                format!("all supporting invocations share {}", detected.context_key),
            ));
            rules.push(contradictions());
            let share = 100 * detected.failures / (detected.failures + detected.successes).max(1);
            rules.push(rule(
                "failure_share",
                Some(share as u32 >= thresholds.min_failure_share_percent),
                format!(
                    "{share}% of established outcomes failed ({} failure(s), {} verified success(es)); at least {}% required",
                    detected.failures, detected.successes, thresholds.min_failure_share_percent
                ),
            ));
            let moved = detected
                .context_governance
                .iter()
                .filter(|(key, version)| pins.get(*key).is_some_and(|pinned| pinned != *version))
                .map(|(key, version)| format!("{key} ran at {version}, pinned {}", pins[key]))
                .collect::<Vec<_>>();
            rules.push(rule(
                "not_explained_by_context",
                Some(!detected.infrastructure && moved.is_empty()),
                if detected.infrastructure {
                    "a failure names unavailable or unhealthy infrastructure; it may not relate to the capability".into()
                } else if !moved.is_empty() {
                    format!("the invocations ran under changed dependency versions ({}); the failures may relate to the change", moved.join("; "))
                } else {
                    "no unavailable infrastructure and no changed dependency version in this context".into()
                },
            ));
            rules.push(evidence_rule());
        }
        SignalKind::DiagnosticFailure => {
            rules.push(rule(
                "independent_observations",
                Some(false),
                "a single established failure is diagnostic only; it is not a recurring pattern"
                    .into(),
            ));
            rules.push(contradictions());
            rules.push(evidence_rule());
        }
        SignalKind::VerificationGap => {
            rules.push(independent(
                (thresholds.min_gap_observations as usize).max(minimum),
            ));
            rules.push(rule("verified_outcomes", Some(true), "the outcomes were not established; they are counted as gaps, never as successes or failures".into()));
            rules.push(evidence_rule());
        }
        SignalKind::ContradictoryOutcomes => {
            rules.push(rule(
                "no_contradictions",
                Some(false),
                "checks of the same outcome disagree; the engine does not choose a side".into(),
            ));
        }
        SignalKind::VerifiedSuccessPattern => {
            rules.push(independent(minimum));
            rules.push(contradictions());
        }
        SignalKind::DependencyDrift => {
            let observed = detected.drift.iter().any(|finding| {
                matches!(finding.status, DriftStatus::Changed | DriftStatus::Missing)
            });
            rules.push(rule(
                "observed_drift",
                Some(observed),
                detected
                    .drift
                    .iter()
                    .map(|finding| format!("{:?}: {}", finding.status, finding.impact))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        SignalKind::ContextUnavailable => {
            rules.push(rule("context_observable", None, "the current Landscape or Crane versions cannot be read; drift is unknown, not absent".into()));
        }
        SignalKind::NoLongerEligible => {
            rules.push(rule(
                "withdrawn",
                Some(true),
                "the version is withdrawn; it is not revised".into(),
            ));
        }
    }
    let qualified = rules.iter().all(|rule| rule.status == RuleStatus::Met);
    let explanation = if qualified {
        "every deterministic rule is met".to_string()
    } else {
        format!(
            "not qualified: {}",
            rules
                .iter()
                .filter(|rule| rule.status != RuleStatus::Met)
                .map(|rule| rule.rule.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Qualification {
        qualified,
        rules,
        thresholds,
        explanation,
    }
}

/** The bounded hypothesis of a qualified signal (no causal claim)
 * Input
    - detected: &Detected - signal
    - capability: (&str, u32) - capability and version
 * Output
    - (Hypothesis, bool) the hypothesis and whether a revision may be proposed
*/
pub(crate) fn hypothesis(detected: &Detected, capability: (&str, u32)) -> (Hypothesis, bool) {
    let (id, version) = capability;
    let causality = "Not established: the outcomes co-occur with this version in this context; the order of events is not treated as a cause.".to_string();
    match detected.kind {
        SignalKind::RepeatedFailure => (
            Hypothesis {
                statement: format!(
                    "In {}, {} independent invocation(s) of {id} v{version} ended with established failures of {} (steps: {}).",
                    detected.context_key,
                    detected.supporting.len(),
                    if detected.affected_claims.is_empty() { "an operation".into() } else { detected.affected_claims.join(", ") },
                    if detected.affected_steps.is_empty() { "none failed to run".into() } else { detected.affected_steps.join(", ") }
                ),
                causality,
                assumptions: vec!["The recorded verification results are authoritative for these claims".into()],
                questions: vec![format!("What makes {} fail in this context?", detected.affected_claims.join(", "))],
                recommendation: "record the observed failure as a known failure case and negative case (a minimal, documentation-only revision)".into(),
            },
            true,
        ),
        SignalKind::VerificationGap => (
            Hypothesis {
                statement: format!(
                    "{} independent invocation(s) of {id} v{version} ended without established outcomes for {}.",
                    detected.supporting.len(),
                    detected.affected_claims.join(", ")
                ),
                causality,
                assumptions: Vec::new(),
                questions: vec!["Which authoritative source can verify these outcomes?".into()],
                recommendation: "add an evidence requirement as an unresolved assumption".into(),
            },
            true,
        ),
        SignalKind::DependencyDrift => {
            let finding = &detected.drift[0];
            let governance = finding.kind == "policy";
            (
                Hypothesis {
                    statement: format!(
                        "{} {} is pinned at {} and is now {}.",
                        finding.kind,
                        finding.target_id,
                        finding.pinned.as_deref().unwrap_or("unpinned"),
                        finding.observed.as_deref().unwrap_or("missing")
                    ),
                    causality: "Observed version change; whether it affects the procedure is not established.".into(),
                    assumptions: Vec::new(),
                    questions: vec![format!("Does the procedure still hold under {} {}?", finding.kind, finding.observed.as_deref().unwrap_or("missing"))],
                    recommendation: if governance {
                        "governance review: the engine never re-pins or edits policies".into()
                    } else {
                        "re-pin the dependency in a new candidate that must be revalidated".into()
                    },
                },
                finding.status == DriftStatus::Changed,
            )
        }
        SignalKind::VerifiedSuccessPattern => (
            Hypothesis {
                statement: format!("{} independent verified success(es) in {}.", detected.supporting.len(), detected.context_key),
                causality,
                assumptions: Vec::new(),
                questions: Vec::new(),
                recommendation: "no change: verified successes do not justify a revision".into(),
            },
            false,
        ),
        _ => (
            Hypothesis {
                statement: "The version is withdrawn.".into(),
                causality,
                assumptions: Vec::new(),
                questions: Vec::new(),
                recommendation: "no change: a withdrawn version is not revised".into(),
            },
            false,
        ),
    }
}
