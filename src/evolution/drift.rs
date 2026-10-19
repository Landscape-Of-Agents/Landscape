// Drift detection. A version's pinned dependencies are checked with Component 2's own dependency
// semantics (query::dependencies) against the current Landscape and Crane versions, the registry,
// and Work Capture's packs. Observed drift (a current version that differs, a target that is gone)
// is kept apart from suspected drift (invocations that ran under another version than the pin;
// timing alone never establishes that the change caused anything) and from unknown (the current
// value cannot be read, which is never reported as "no drift"). A withdrawn version is reported
// as no longer eligible, so it is not revised. Nothing here rewrites a constraint.

use super::models::{DriftFinding, DriftReport, DriftStatus, Observation};
use crate::capability::lifecycle::LifecycleState;
use crate::capability::model::{DependencyKind, VersionConstraint};
use crate::capability::query::dependencies;
use crate::capability::refs::{RefOutcome, Sources};
use crate::capability::registry::RegistryState;
use crate::capability::CapabilityResult;

/** The snake_case name of a dependency kind
 * Input
    - kind: DependencyKind - kind
 * Output
    - String
*/
pub(crate) fn kind_name(kind: DependencyKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/** Detect drift for an exact version
 * Input
    - state: &RegistryState - registry
    - capability_id: &str - capability
    - version: u32 - version
    - sources: &Sources - canonical sources and current versions
    - observations: &[Observation] - its observations (for suspected drift)
 * Output
    - CapabilityResult<DriftReport>
    - Error (NotFound) when the version does not exist
*/
pub(crate) fn detect(
    state: &RegistryState,
    capability_id: &str,
    version: u32,
    sources: &Sources,
    observations: &[Observation],
) -> CapabilityResult<DriftReport> {
    let record = state.record(capability_id, version)?;
    let mut findings = Vec::new();
    if matches!(
        record.state,
        LifecycleState::Revoked | LifecycleState::Deprecated | LifecycleState::Rejected
    ) {
        findings.push(DriftFinding {
            kind: "lifecycle".into(),
            target_id: format!("{capability_id}@{version}"),
            required: true,
            pinned: None,
            observed: Some(format!("{:?}", record.state).to_ascii_lowercase()),
            status: DriftStatus::NoLongerEligible,
            source: "capability registry".into(),
            impact: "the version is withdrawn; it is not revised automatically".into(),
            invocations: Vec::new(),
        });
    }
    for status in dependencies(state, capability_id, version, sources)? {
        let dependency = &status.dependency;
        let kind = kind_name(dependency.kind);
        let pinned = match &dependency.constraint {
            VersionConstraint::Exact(value) => Some(value.clone()),
            VersionConstraint::AtLeast(value) => Some(format!(">={value}")),
            VersionConstraint::Unpinned => None,
        };
        let (current, source) = match dependency.kind {
            DependencyKind::Zone => (
                sources.current.zones.get(&dependency.target_id),
                "landscape",
            ),
            DependencyKind::Flow => (
                sources.current.flows.get(&dependency.target_id),
                "landscape",
            ),
            DependencyKind::Policy => {
                (sources.current.policies.get(&dependency.target_id), "crane")
            }
            DependencyKind::Capability => (None, "capability registry"),
            DependencyKind::EvidencePack => (None, "work capture"),
            _ => (None, "no connector"),
        };
        let (drift, observed) = match status.outcome {
            RefOutcome::Resolved => (DriftStatus::Unchanged, status.resolved_version.clone()),
            RefOutcome::Stale | RefOutcome::Mismatch => (DriftStatus::Changed, current.cloned()),
            RefOutcome::Missing => (DriftStatus::Missing, None),
            RefOutcome::Unverifiable | RefOutcome::Unauthorized => (DriftStatus::Unknown, None),
        };
        let impact = match drift {
            DriftStatus::Unchanged => "current".to_string(),
            DriftStatus::Changed if dependency.required => format!("required {kind} {} moved; the version's validation basis no longer matches and it is not eligible until revalidated or revised", dependency.target_id),
            DriftStatus::Changed => format!("optional {kind} {} moved", dependency.target_id),
            DriftStatus::Missing => format!("{kind} {} no longer resolves", dependency.target_id),
            _ => format!("the current {kind} {} cannot be observed ({source}); drift is unknown, not absent", dependency.target_id),
        };
        findings.push(DriftFinding {
            kind: kind.clone(),
            target_id: dependency.target_id.clone(),
            required: dependency.required,
            pinned: pinned.clone(),
            observed,
            status: drift,
            source: source.into(),
            impact,
            invocations: Vec::new(),
        });
        if matches!(
            dependency.kind,
            DependencyKind::Zone | DependencyKind::Flow | DependencyKind::Policy
        ) {
            let key = format!("{kind}:{}", dependency.target_id);
            let ran_elsewhere = observations
                .iter()
                .filter(|observation| {
                    observation
                        .context
                        .governance
                        .get(&key)
                        .is_some_and(|seen| {
                            seen != "unknown" && Some(seen.as_str()) != pinned.as_deref()
                        })
                })
                .map(|observation| observation.invocation_id.clone())
                .collect::<Vec<_>>();
            if !ran_elsewhere.is_empty() {
                findings.push(DriftFinding {
                    kind,
                    target_id: dependency.target_id.clone(),
                    required: dependency.required,
                    pinned,
                    observed: None,
                    status: DriftStatus::SuspectedAtInvocation,
                    source: "invocation records".into(),
                    impact: "these invocations ran under another version than the pin; a relation to their outcomes is suspected, not established".into(),
                    invocations: ran_elsewhere,
                });
            }
        }
    }
    let changed = findings
        .iter()
        .filter(|finding| {
            matches!(
                finding.status,
                DriftStatus::Changed | DriftStatus::Missing | DriftStatus::NoLongerEligible
            )
        })
        .count();
    let unknown = findings
        .iter()
        .filter(|finding| finding.status == DriftStatus::Unknown)
        .count();
    let summary = match (changed, unknown) {
        (0, 0) => "no drift observed in any pinned dependency".to_string(),
        (0, unknown) => format!("no drift observed, but {unknown} dependency(ies) cannot be observed; drift is unknown there"),
        (changed, 0) => format!("{changed} observed change(s)"),
        (changed, unknown) => format!("{changed} observed change(s); {unknown} dependency(ies) unknown"),
    };
    Ok(DriftReport {
        capability_id: capability_id.into(),
        version,
        digest: record.definition_digest.clone(),
        state: record.state,
        landscape_known: sources.current.landscape_known,
        crane_known: sources.current.crane_known,
        findings,
        summary,
    })
}
