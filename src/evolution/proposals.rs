// Revision proposals. A qualified signal yields a minimal, typed change set: a known failure case
// for repeated established failures, an evidence requirement for verification gaps, a re-pin for
// an observed zone or flow change. External suggestions (a person, or a future model-assisted
// analyzer) use the same typed changes and are hypotheses only. Every change passes a
// deterministic gate before anything is stored: it must cite evidence from the signal, stay inside
// the supported categories, and never weaken a safeguard (no policy edits, no risk or review
// downgrade, no claim removal or self-verification, no broadened applicability, no speculative
// steps). The gated changes are applied to an exact copy of the parent; the result is a new
// version (latest + 1, predecessor = parent) stored by Component 2's propose_version, which keeps
// its own authorization, schema, and immutability rules. The parent is never modified.

use std::collections::BTreeSet;

use super::models::{Change, ChangeKind, GateDecision, Signal, SignalKind};
use crate::capability::model::{
    CapabilityDefinition, ClaimKind, ClaimStatus, CreationMethod, DependencyKind, Exclusion,
    Improvement, VerificationClaim, VersionConstraint,
};
use crate::capability::refs::CurrentVersions;
use crate::capture::Actor;

/** Longest accepted text in a change */
const MAX_TEXT: usize = 600;

/** The changes a qualified signal justifies (deterministic; empty when none is supported)
 * Input
    - signal: &Signal - qualified signal
 * Output
    - Vec<Change>
*/
pub(crate) fn changes_for(signal: &Signal) -> Vec<Change> {
    let short = &signal.signal_id[..signal.signal_id.len().min(16)];
    match signal.kind {
        SignalKind::RepeatedFailure => vec![Change {
            change: ChangeKind::AddKnownFailureCase {
                text: format!(
                    "Observed failure: {} failed in {} independent invocations ({}); cause not established [{short}]",
                    if signal.affected_claims.is_empty() { "an operation".into() } else { signal.affected_claims.join(", ") },
                    signal.independent_count,
                    signal.context_key
                ),
            },
            rationale: "repeated established failures in one comparable context".into(),
            evidence: signal.supporting.clone(),
            origin: "detector".into(),
        }],
        SignalKind::VerificationGap => vec![Change {
            change: ChangeKind::AddUnresolvedAssumption {
                text: format!(
                    "Evidence requirement: outcomes of {} were not established in {} independent invocations; an authoritative verification source is needed [{short}]",
                    signal.affected_claims.join(", "),
                    signal.independent_count
                ),
            },
            rationale: "repeated verification gaps".into(),
            evidence: signal.supporting.clone(),
            origin: "detector".into(),
        }],
        SignalKind::DependencyDrift => signal
            .drift
            .iter()
            .filter(|finding| finding.status == super::models::DriftStatus::Changed)
            .take(1)
            .map(|finding| {
                let evidence = vec![format!("drift:{}:{}", finding.kind, finding.target_id)];
                let change = match finding.kind.as_str() {
                    "zone" | "flow" => ChangeKind::UpdateDependency {
                        dependency: if finding.kind == "zone" {
                            DependencyKind::Zone
                        } else {
                            DependencyKind::Flow
                        },
                        target_id: finding.target_id.clone(),
                        from: finding.pinned.clone().unwrap_or_default(),
                        to: finding.observed.clone().unwrap_or_default(),
                    },
                    _ => ChangeKind::ChangePolicy {
                        target_id: finding.target_id.clone(),
                        detail: format!(
                            "re-pin from {} to {}",
                            finding.pinned.as_deref().unwrap_or("unpinned"),
                            finding.observed.as_deref().unwrap_or("missing")
                        ),
                    },
                };
                Change {
                    change,
                    rationale: format!("observed drift: {}", finding.impact),
                    evidence,
                    origin: "detector".into(),
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/** What the gate needs besides the change set
 * Fields
    - parent: &'a CapabilityDefinition - exact parent
    - signal: &'a Signal - motivating signal
    - current: &'a CurrentVersions - current governance versions
    - observed_methods: &'a BTreeSet<String> - verification methods observed verifying in invocations
*/
pub(crate) struct GateContext<'a> {
    pub(crate) parent: &'a CapabilityDefinition,
    pub(crate) signal: &'a Signal,
    pub(crate) current: &'a CurrentVersions,
    pub(crate) observed_methods: &'a BTreeSet<String>,
}

/** Decide one change
 * Input
    - change: &Change - change
    - context: &GateContext - gate context
 * Output
    - Result<String, String> allowed with a reason, or refused with a reason
*/
fn decide(change: &Change, context: &GateContext) -> Result<String, String> {
    let parent = context.parent;
    let signal = context.signal;
    let grounded = signal
        .supporting
        .iter()
        .chain(&signal.contradicting)
        .cloned()
        .chain(
            signal
                .evidence
                .iter()
                .map(|reference| reference.ref_id.clone()),
        )
        .chain(
            signal
                .drift
                .iter()
                .map(|finding| format!("drift:{}:{}", finding.kind, finding.target_id)),
        )
        .collect::<BTreeSet<_>>();
    if change.evidence.is_empty() {
        return Err("a modification without evidence is refused".into());
    }
    if let Some(stray) = change
        .evidence
        .iter()
        .find(|reference| !grounded.contains(*reference))
    {
        return Err(format!(
            "{stray} is not evidence of signal {}; changes must cite the signal's own evidence",
            signal.signal_id
        ));
    }
    let text_ok = |text: &str| !text.trim().is_empty() && text.len() <= MAX_TEXT;
    match &change.change {
        ChangeKind::AddKnownFailureCase { text }
        | ChangeKind::AddLimitation { text }
        | ChangeKind::AddUnresolvedAssumption { text } => {
            if text_ok(text) {
                Ok("documents an observed condition; nothing is weakened".into())
            } else {
                Err(format!("text must be 1-{MAX_TEXT} characters"))
            }
        }
        ChangeKind::AddExclusion {
            exclusion_kind,
            value,
            reason,
        } => {
            if !matches!(exclusion_kind.as_str(), "task_class" | "environment" | "entity") {
                Err(format!("exclusion kind {exclusion_kind} is not supported by the applicability matcher"))
            } else if !text_ok(value) || !text_ok(reason) {
                Err("an exclusion needs a value and a reason".into())
            } else if !matches!(signal.kind, SignalKind::RepeatedFailure | SignalKind::DependencyDrift) {
                Err("tightening applicability needs a qualified failure or drift signal".into())
            } else {
                Ok("tightens applicability; justified by the cited failures (regression checks verified contexts)".into())
            }
        }
        ChangeKind::AddVerificationRequirement {
            claim_id,
            statement,
            expected_result,
            method,
        } => {
            if parent
                .verification
                .claims
                .iter()
                .any(|claim| &claim.claim_id == claim_id)
            {
                Err(format!("claim {claim_id} already exists"))
            } else if !context.observed_methods.contains(method) {
                Err(format!("verification method {method} was never observed verifying an outcome; the requirement would not be supported by the current verification interface"))
            } else if !text_ok(statement) || !text_ok(expected_result) {
                Err("a requirement needs a statement and an expected result".into())
            } else {
                Ok("adds a required claim that stays unresolved until validated".into())
            }
        }
        ChangeKind::UpdateDependency {
            dependency,
            target_id,
            from,
            to,
        } => {
            let pinned = parent.dependencies.iter().find(|existing| {
                existing.kind == *dependency
                    && &existing.target_id == target_id
                    && existing.constraint == VersionConstraint::Exact(from.clone())
            });
            let current = match dependency {
                DependencyKind::Zone if context.current.landscape_known => {
                    context.current.zones.get(target_id)
                }
                DependencyKind::Flow if context.current.landscape_known => {
                    context.current.flows.get(target_id)
                }
                DependencyKind::Zone | DependencyKind::Flow => None,
                _ => return Err(format!("{dependency:?} references are not re-pinned by evolution; only zone and flow pins are supported")),
            };
            match (pinned, current) {
                (None, _) => Err(format!("the parent does not pin {dependency:?} {target_id} at {from}")),
                (_, None) => Err(format!("the current version of {target_id} cannot be observed")),
                (_, Some(current)) if current != to => Err(format!("{target_id} is currently {current}, not {to}")),
                _ => Ok("re-pins to the observed current version; the candidate must be revalidated".into()),
            }
        }
        ChangeKind::ClarifyStep {
            step_id,
            description,
        } => {
            if !parent
                .execution
                .steps
                .iter()
                .any(|step| &step.step_id == step_id)
            {
                Err(format!("the parent has no step {step_id}"))
            } else if !text_ok(description) {
                Err("a clarified step needs a description".into())
            } else {
                Ok("rewords a step from observed evidence; its kind and contract are unchanged".into())
            }
        }
        ChangeKind::RaiseRisk { to } => {
            if *to > parent.applicability.risk {
                Ok("raises risk; never weakens a safeguard".into())
            } else {
                Err("risk can only be raised".into())
            }
        }
        ChangeKind::ChangePolicy { target_id, .. } => Err(format!(
            "policy {target_id}: evolution never edits or re-pins policies; signed governance changes go through Crane's policy workflow"
        )),
        ChangeKind::LowerRisk { .. } => Err("risk is never lowered by evolution".into()),
        ChangeKind::LowerReview { .. } => Err("the review level is never lowered by evolution".into()),
        ChangeKind::RemoveClaim { claim_id } => Err(format!(
            "claim {claim_id}: required verification is never removed by evolution"
        )),
        ChangeKind::MarkClaimVerified { claim_id } => Err(format!(
            "claim {claim_id}: only evidence verifies a claim, never an authored status"
        )),
        ChangeKind::AddEnvironment { environment } => Err(format!(
            "environment {environment}: broadening applicability is a speculative generalization"
        )),
        ChangeKind::AddTaskClass { task_class } => Err(format!(
            "task class {task_class}: broadening applicability is a speculative generalization"
        )),
        ChangeKind::AddStep { .. } => {
            Err("adding a step is speculative; a new procedure must be compiled from evidence".into())
        }
    }
}

/** Run the gate over a change set
 * Input
    - changes: &[Change] - change set
    - context: &GateContext - gate context
 * Output
    - Vec<GateDecision>
*/
pub(crate) fn gate(changes: &[Change], context: &GateContext) -> Vec<GateDecision> {
    changes
        .iter()
        .enumerate()
        .map(|(index, change)| match decide(change, context) {
            Ok(reason) => GateDecision {
                index,
                allowed: true,
                reason,
            },
            Err(reason) => GateDecision {
                index,
                allowed: false,
                reason,
            },
        })
        .collect()
}

/** Apply a change set to a copy of a definition (content only; identity untouched)
 * Input
    - definition: &CapabilityDefinition - definition
    - changes: &[Change] - changes (already gated)
 * Output
    - CapabilityDefinition
*/
pub(crate) fn apply_content(
    definition: &CapabilityDefinition,
    changes: &[Change],
) -> CapabilityDefinition {
    let mut next = definition.clone();
    let push = |list: &mut Vec<String>, text: &str| {
        if !list.iter().any(|existing| existing == text) {
            list.push(text.to_string());
        }
    };
    for change in changes {
        match &change.change {
            ChangeKind::AddKnownFailureCase { text } => {
                push(&mut next.known_failure_cases, text);
                push(&mut next.verification.negative_cases, text);
            }
            ChangeKind::AddLimitation { text } => push(&mut next.applicability.limitations, text),
            ChangeKind::AddUnresolvedAssumption { text } => {
                push(&mut next.verification.unresolved_assumptions, text)
            }
            ChangeKind::AddExclusion {
                exclusion_kind,
                value,
                reason,
            } => next.applicability.exclusions.push(Exclusion {
                kind: exclusion_kind.clone(),
                value: value.clone(),
                reason: reason.clone(),
            }),
            ChangeKind::AddVerificationRequirement {
                claim_id,
                statement,
                expected_result,
                method,
            } => next.verification.claims.push(VerificationClaim {
                claim_id: claim_id.clone(),
                kind: ClaimKind::Technical,
                statement: statement.clone(),
                expected_result: expected_result.clone(),
                required: true,
                status: ClaimStatus::Unresolved,
                evidence_refs: Vec::new(),
                assertion_refs: Vec::new(),
                check_ref: None,
                invariant_ref: None,
                method: Some(method.clone()),
                environment: None,
                verified_at: None,
            }),
            ChangeKind::UpdateDependency {
                dependency,
                target_id,
                to,
                ..
            } => {
                for existing in next.dependencies.iter_mut().filter(|existing| {
                    existing.kind == *dependency && &existing.target_id == target_id
                }) {
                    existing.constraint = VersionConstraint::Exact(to.clone());
                }
            }
            ChangeKind::ClarifyStep {
                step_id,
                description,
            } => {
                for step in next
                    .execution
                    .steps
                    .iter_mut()
                    .filter(|step| &step.step_id == step_id)
                {
                    step.description = description.clone();
                }
            }
            ChangeKind::RaiseRisk { to } => next.applicability.risk = *to,
            _ => {}
        }
    }
    next
}

/** Build the candidate definition: the parent's content with the changes, version latest + 1,
 * predecessor the parent, provenance kept and extended
 * Input
    - parent: &CapabilityDefinition - exact parent (the latest version)
    - changes: &[Change] - gated changes
    - actor: &Actor - proposer (the candidate's author)
    - reason: &str - change reason
    - now: u64 - Unix milliseconds
 * Output
    - CapabilityDefinition
*/
pub(crate) fn candidate(
    parent: &CapabilityDefinition,
    changes: &[Change],
    actor: &Actor,
    reason: &str,
    now: u64,
) -> CapabilityDefinition {
    let mut next = apply_content(parent, changes);
    next.version = parent.version + 1;
    next.previous_version_ref = Some(parent.version);
    next.provenance.creation_method = CreationMethod::Manual;
    next.provenance.created_by = actor.clone();
    next.provenance.created_at = now;
    next.provenance.change_reason = Some(reason.into());
    for change in changes {
        next.proposed_improvements.push(Improvement {
            description: format!("{} ({})", change.rationale, change.origin),
            proposed_by: actor.clone(),
            evidence_refs: change.evidence.clone(),
        });
    }
    next
}

/** The definition with identity, provenance, and improvement history cleared, for comparing
 * content
 * Input
    - definition: &CapabilityDefinition - definition
 * Output
    - CapabilityDefinition
*/
pub(crate) fn content(definition: &CapabilityDefinition) -> CapabilityDefinition {
    let mut copy = definition.clone();
    copy.version = 0;
    copy.previous_version_ref = None;
    copy.provenance.created_at = 0;
    copy.provenance.change_reason = None;
    copy.provenance.created_by = Actor::new("", crate::capture::ActorKind::System);
    copy.provenance.creation_method = CreationMethod::Manual;
    copy.proposed_improvements.clear();
    copy
}

/** Every way a candidate weakens its parent's safeguards (empty when none)
 * Input
    - parent: &CapabilityDefinition - parent
    - candidate: &CapabilityDefinition - candidate
 * Output
    - Vec<String> violations
*/
pub(crate) fn weakenings(
    parent: &CapabilityDefinition,
    candidate: &CapabilityDefinition,
) -> Vec<String> {
    let mut found = Vec::new();
    let (before, after) = (&parent.applicability, &candidate.applicability);
    if after.risk < before.risk {
        found.push(format!(
            "risk lowered from {:?} to {:?}",
            before.risk, after.risk
        ));
    }
    if after.review_level < before.review_level {
        found.push(format!(
            "review level lowered from {:?} to {:?}",
            before.review_level, after.review_level
        ));
    }
    let policies = |definition: &CapabilityDefinition| {
        definition
            .dependencies
            .iter()
            .filter(|dependency| dependency.kind == DependencyKind::Policy)
            .map(|dependency| format!("{}={:?}", dependency.target_id, dependency.constraint))
            .collect::<BTreeSet<_>>()
    };
    if policies(parent) != policies(candidate) {
        found.push("policy dependencies changed".into());
    }
    for dependency in &parent.dependencies {
        if !candidate.dependencies.iter().any(|other| {
            other.kind == dependency.kind
                && other.target_id == dependency.target_id
                && other.required == dependency.required
        }) {
            found.push(format!(
                "dependency {:?} {} removed or made optional",
                dependency.kind, dependency.target_id
            ));
        }
    }
    for exclusion in &before.exclusions {
        if !after.exclusions.contains(exclusion) {
            found.push(format!(
                "exclusion {}={} removed",
                exclusion.kind, exclusion.value
            ));
        }
    }
    for environment in after
        .environments
        .iter()
        .filter(|env| !before.environments.contains(env))
    {
        found.push(format!("environment {environment} added (broadened)"));
    }
    for class in after
        .task_classes
        .iter()
        .filter(|class| !before.task_classes.contains(class))
    {
        found.push(format!("task class {class} added (broadened)"));
    }
    for precondition in &before.preconditions {
        if !after
            .preconditions
            .iter()
            .any(|other| other.id == precondition.id)
        {
            found.push(format!("precondition {} removed", precondition.id));
        }
    }
    let (verify_before, verify_after) = (&parent.verification, &candidate.verification);
    if verify_before.method != verify_after.method
        || verify_before.method_version != verify_after.method_version
    {
        found.push("verification method changed".into());
    }
    for environment in verify_after
        .scope_environments
        .iter()
        .filter(|env| !verify_before.scope_environments.contains(env))
    {
        found.push(format!("verification scope broadened to {environment}"));
    }
    for claim in &verify_before.claims {
        match verify_after
            .claims
            .iter()
            .find(|other| other.claim_id == claim.claim_id)
        {
            None => found.push(format!("claim {} removed", claim.claim_id)),
            Some(other) => {
                if claim.required && !other.required {
                    found.push(format!("claim {} no longer required", claim.claim_id));
                }
                if other.status == ClaimStatus::Verified && claim.status != ClaimStatus::Verified {
                    found.push(format!(
                        "claim {} upgraded to verified by authorship",
                        claim.claim_id
                    ));
                }
                if other.evidence_refs != claim.evidence_refs || other.statement != claim.statement
                {
                    found.push(format!(
                        "claim {} statement or evidence changed",
                        claim.claim_id
                    ));
                }
            }
        }
    }
    for claim in verify_after.claims.iter().filter(|claim| {
        !verify_before
            .claims
            .iter()
            .any(|other| other.claim_id == claim.claim_id)
    }) {
        if claim.status != ClaimStatus::Unresolved {
            found.push(format!(
                "new claim {} is {:?}; new claims start unresolved",
                claim.claim_id, claim.status
            ));
        }
    }
    for invariant in &verify_before.invariants {
        if !verify_after.invariants.contains(invariant) {
            found.push(format!(
                "invariant {} removed or changed",
                invariant.invariant_id
            ));
        }
    }
    for case in &verify_before.negative_cases {
        if !verify_after.negative_cases.contains(case) {
            found.push(format!("negative case removed: {case}"));
        }
    }
    let shape = |definition: &CapabilityDefinition| {
        definition
            .execution
            .steps
            .iter()
            .map(|step| (step.step_id.clone(), step.kind))
            .collect::<Vec<_>>()
    };
    if shape(parent) != shape(candidate) {
        found.push("procedure steps added, removed, reordered, or changed kind".into());
    }
    found
}
