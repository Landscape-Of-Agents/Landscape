// Applicability and dependencies. A candidate applies only where its episode was observed: the
// environments are exactly those of the observed records (none observed means none applicable),
// the task class is the requester's declaration, and generalizing beyond that one episode is an
// unresolved assumption that Component 2 surfaces as a warning to acknowledge. Prerequisites are
// inferred, declared conditions. Dependencies reference the Landscape zones and flows and the
// Crane policies the evidence ran under, pinned to the versions it saw, and the Evidence Pack by
// digest; nothing from those sources is copied. Risk is the requester's, raised (never lowered)
// when the procedure changes state.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::extract::Extraction;
use super::reconstruct::Reconstruction;
use super::verify::{Gap, GapKind};
use super::{Basis, CompileRequest};
use crate::capability::intake::CapabilityIntake;
use crate::capability::model::{
    ApplicabilityContract, ClaimStatus, Condition, DependencyKind, DependencyReference,
    ReviewLevel, RiskLevel, VersionConstraint,
};
use crate::capture::resources::scope::VersionedRef;

/** The basis of one applicability element
 * Fields
    - element: String - what it is (environment, task_class, precondition, risk, dependency)
    - value: String - its value
    - basis: Basis - how it is known
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ElementBasis {
    pub(crate) element: String,
    pub(crate) value: String,
    pub(crate) basis: Basis,
}

/** The applicability side of a candidate
 * Fields
    - applicability: ApplicabilityContract - contract
    - dependencies: Vec<DependencyReference> - dependencies
    - bases: Vec<ElementBasis> - how each element is known
    - gaps: Vec<Gap> - gaps (unknown versions, no environment)
    - warnings: Vec<String> - adjustments made
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContextPlan {
    pub(crate) applicability: ApplicabilityContract,
    pub(crate) dependencies: Vec<DependencyReference>,
    pub(crate) bases: Vec<ElementBasis>,
    pub(crate) gaps: Vec<Gap>,
    pub(crate) warnings: Vec<String>,
}

/** Build a declared condition
 * Input
    - id: &str - id
    - description: String - description
    - status: ClaimStatus - status
 * Output
    - Condition
*/
fn condition(id: &str, description: String, status: ClaimStatus) -> Condition {
    Condition {
        id: id.into(),
        description,
        check: None,
        status,
    }
}

/** Pin a governance reference to the version the evidence saw
 * Input
    - kind: DependencyKind - zone, flow, or policy
    - reference: &VersionedRef - reference
    - gaps: &mut Vec<Gap> - gaps to extend when the version is unknown
 * Output
    - DependencyReference
*/
fn pinned(
    kind: DependencyKind,
    reference: &VersionedRef,
    gaps: &mut Vec<Gap>,
) -> DependencyReference {
    let constraint = match &reference.version {
        Some(version) => VersionConstraint::Exact(version.clone()),
        None => {
            gaps.push(Gap {
                kind: GapKind::Unknown,
                detail: format!(
                    "the version of {kind:?} {} the evidence ran under is unknown",
                    reference.id
                ),
                refs: vec![reference.id.clone()],
                blocking: true,
            });
            VersionConstraint::Unpinned
        }
    };
    DependencyReference {
        kind,
        target_id: reference.id.clone(),
        required: true,
        constraint,
        reason: "the source episode ran under this version".into(),
        applicability: Vec::new(),
    }
}

/** Propose applicability and dependencies
 * Input
    - request: &CompileRequest - request
    - intake: &CapabilityIntake - intake
    - reconstruction: &Reconstruction - reconstructed episode
    - extraction: &Extraction - extracted procedure
    - commit: Option<String> - the commit the episode changed
 * Output
    - ContextPlan
*/
pub(crate) fn plan(
    request: &CompileRequest,
    intake: &CapabilityIntake,
    reconstruction: &Reconstruction,
    extraction: &Extraction,
    commit: Option<String>,
) -> ContextPlan {
    let mut bases = Vec::new();
    let mut gaps = Vec::new();
    let mut warnings = Vec::new();
    let environments = reconstruction
        .items
        .iter()
        .filter(|item| item.basis == Basis::Observed)
        .filter_map(|item| item.environment.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if environments.is_empty() {
        gaps.push(Gap {
            kind: GapKind::Unknown,
            detail: "no environment was observed, so no environment can be declared applicable"
                .into(),
            refs: Vec::new(),
            blocking: true,
        });
    }
    bases.extend(environments.iter().map(|environment| ElementBasis {
        element: "environment".into(),
        value: environment.clone(),
        basis: Basis::Observed,
    }));
    bases.push(ElementBasis {
        element: "task_class".into(),
        value: request.task_class.clone(),
        basis: Basis::Declared,
    });
    let mut risk = intake.risk;
    if extraction.mutating && risk == RiskLevel::Low {
        risk = RiskLevel::Medium;
        warnings.push("the declared risk was low but the procedure changes state; the candidate's risk was raised to medium".into());
    }
    bases.push(ElementBasis {
        element: "risk".into(),
        value: format!("{risk:?}"),
        basis: if risk == intake.risk {
            Basis::Declared
        } else {
            Basis::Inferred
        },
    });
    let review_level = match risk {
        RiskLevel::Low => ReviewLevel::Peer,
        RiskLevel::Medium => ReviewLevel::Owner,
        _ => ReviewLevel::Admin,
    };
    let access = if extraction.mutating {
        "read and change"
    } else {
        "read"
    };
    let mut preconditions = vec![condition(
        "repository-access",
        format!("The executor can {access} the repository"),
        ClaimStatus::Declared,
    )];
    if extraction.steps.iter().any(|step| {
        step.preconditions
            .iter()
            .any(|id| id == "deployment-target-known")
    }) {
        preconditions.push(condition(
            "deployment-target-known",
            "The target environment and deployment mechanism are known".into(),
            ClaimStatus::Declared,
        ));
    }
    bases.extend(preconditions.iter().map(|precondition| ElementBasis {
        element: "precondition".into(),
        value: precondition.id.clone(),
        basis: Basis::Inferred,
    }));
    let observed_in = if environments.is_empty() {
        "no observed environment".to_string()
    } else {
        environments.join(", ")
    };
    let mut assumptions = vec![condition("single-episode", format!("Established from one work episode ({}) in {observed_in}; behavior in other environments, revisions, or zone versions is unproven", intake.work.episode_id), ClaimStatus::Unresolved)];
    for decision in extraction
        .proposals
        .iter()
        .filter(|decision| decision.accepted)
    {
        assumptions.push(condition(
            &format!("proposed-{}", decision.step_id),
            format!(
                "Step {} was proposed by {} and is unverified",
                decision.step_id, decision.producer
            ),
            ClaimStatus::Unresolved,
        ));
    }
    let mut limitations = vec![format!(
        "Applicability is limited to what one episode observed: {observed_in}"
    )];
    limitations.extend(intake.completeness.missing.iter().map(|missing| {
        format!(
            "{:?} evidence was missing ({:?}) when compiled",
            missing.evidence_type, missing.state
        )
    }));
    let governance = &intake.governance;
    let mut dependencies = Vec::new();
    dependencies.extend(
        governance
            .zones
            .iter()
            .map(|zone| pinned(DependencyKind::Zone, zone, &mut gaps)),
    );
    dependencies.extend(
        governance
            .flows
            .iter()
            .map(|flow| pinned(DependencyKind::Flow, flow, &mut gaps)),
    );
    dependencies.extend(
        governance
            .policies
            .iter()
            .map(|policy| pinned(DependencyKind::Policy, policy, &mut gaps)),
    );
    dependencies.push(DependencyReference {
        kind: DependencyKind::EvidencePack,
        target_id: intake.work.pack_id.clone(),
        required: true,
        constraint: VersionConstraint::Exact(intake.work.pack_digest.clone()),
        reason: "the evidence the candidate was compiled from".into(),
        applicability: Vec::new(),
    });
    if let (Some(repository), Some(commit)) = (&intake.work.repository, commit) {
        dependencies.push(DependencyReference {
            kind: DependencyKind::Repository,
            target_id: repository.clone(),
            required: false,
            constraint: VersionConstraint::Exact(commit),
            reason: "the revision the episode changed".into(),
            applicability: Vec::new(),
        });
    }
    bases.extend(dependencies.iter().map(|dependency| ElementBasis {
        element: "dependency".into(),
        value: format!("{:?} {}", dependency.kind, dependency.target_id),
        basis: Basis::Observed,
    }));
    ContextPlan {
        applicability: ApplicabilityContract {
            purpose: request.purpose.clone(),
            task_classes: vec![request.task_class.clone()],
            environments,
            business_domain: intake.intent.process_domain.clone(),
            preconditions,
            required_state: Vec::new(),
            exclusions: Vec::new(),
            limitations,
            inputs: extraction.inputs.clone(),
            outputs: Vec::new(),
            assumptions,
            risk,
            review_level,
        },
        dependencies,
        bases,
        gaps,
        warnings,
    }
}
