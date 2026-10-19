// Outcome normalization. Invocation records are read from Component 6's ledger and never
// changed. Each record becomes one observation, classified from its recorded state and its own
// verification results: a recorded success counts as verified only when every required claim the
// plan bound has a verified result and none failed or was contradicted; otherwise it is a declared
// success. Retries are attempts inside one record and never extra trials; a resubmission of the
// same trial (same exact capability, task, environment, repository, and inputs under a new
// idempotency key) is collapsed into one independent observation; DataSea lifecycle events are
// counted for correlation and never as trials. Evidence references are checked through the
// reader's own Work Capture access: a restricted record is counted, never retrieved.

use std::collections::{BTreeMap, BTreeSet};

use super::models::{
    digest_of, EvidenceAccess, EvidenceRef, EvidenceSource, Observation, ObservationContext,
    OutcomeClass,
};
use crate::capability::model::{CapabilityDefinition, DependencyKind};
use crate::capability::refs::{RefOutcome, Sources};
use crate::invocation::engine::{InvocationRecord, VerificationStatus};
use crate::invocation::lifecycle::{InvocationState, OperationStatus};

/** The governance keys a definition pins ("zone:ID", "flow:ID", "policy:ID")
 * Input
    - definition: &CapabilityDefinition - definition
 * Output
    - BTreeMap<String, String> key to pinned version (empty string when unpinned)
*/
pub(crate) fn pinned_governance(definition: &CapabilityDefinition) -> BTreeMap<String, String> {
    definition
        .dependencies
        .iter()
        .filter_map(|dependency| {
            let kind = match dependency.kind {
                DependencyKind::Zone => "zone",
                DependencyKind::Flow => "flow",
                DependencyKind::Policy => "policy",
                _ => return None,
            };
            let pinned = match &dependency.constraint {
                crate::capability::model::VersionConstraint::Exact(version) => version.clone(),
                _ => String::new(),
            };
            Some((format!("{kind}:{}", dependency.target_id), pinned))
        })
        .collect()
}

/** Classify a record from its recorded state and verification results
 * Input
    - record: &InvocationRecord - record
 * Output
    - (OutcomeClass, verified, failed, unresolved, contradicted claim ids)
*/
fn classify(
    record: &InvocationRecord,
) -> (
    OutcomeClass,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
) {
    let mut by_claim: BTreeMap<&str, Vec<VerificationStatus>> = BTreeMap::new();
    for result in &record.verifications {
        by_claim
            .entry(&result.claim_id)
            .or_default()
            .push(result.result);
    }
    let mut verified = Vec::new();
    let mut failed = Vec::new();
    let mut unresolved = Vec::new();
    let mut contradicted = Vec::new();
    for (claim, statuses) in &by_claim {
        let claim = claim.to_string();
        if statuses.contains(&VerificationStatus::Contradicted)
            || (statuses.contains(&VerificationStatus::Verified)
                && statuses.contains(&VerificationStatus::Failed))
        {
            contradicted.push(claim);
        } else if statuses.contains(&VerificationStatus::Failed) {
            failed.push(claim);
        } else if statuses.contains(&VerificationStatus::Unresolved) {
            unresolved.push(claim);
        } else {
            verified.push(claim);
        }
    }
    let required_bound = record
        .plan
        .as_ref()
        .map(|plan| {
            plan.verifications
                .iter()
                .filter(|verification| verification.required)
                .map(|verification| verification.claim_id.clone())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let all_required_verified =
        !required_bound.is_empty() && required_bound.iter().all(|claim| verified.contains(claim));
    let class = match record.state {
        state
            if !contradicted.is_empty()
                && (state.terminal() || state == InvocationState::Unknown) =>
        {
            OutcomeClass::Contradicted
        }
        InvocationState::Succeeded if all_required_verified && failed.is_empty() => {
            OutcomeClass::VerifiedSuccess
        }
        InvocationState::Succeeded => OutcomeClass::DeclaredSuccessUnverified,
        InvocationState::Failed if !failed.is_empty() => OutcomeClass::FailedVerification,
        InvocationState::Failed
            if record
                .operations
                .iter()
                .any(|operation| operation.status == OperationStatus::Failed) =>
        {
            OutcomeClass::VerifiedFailure
        }
        InvocationState::Failed => OutcomeClass::Unknown,
        InvocationState::PartiallyVerified => OutcomeClass::PartiallyVerified,
        InvocationState::Unknown => OutcomeClass::Unknown,
        InvocationState::ReconciliationRequired => OutcomeClass::ReconciliationUnresolved,
        InvocationState::Abandoned => OutcomeClass::Abandoned,
        InvocationState::Blocked => OutcomeClass::BlockedBeforeExecution,
        _ => OutcomeClass::InProgress,
    };
    (class, verified, failed, unresolved, contradicted)
}

/** The key under which resubmissions of one trial collapse
 * Input
    - record: &InvocationRecord - record
 * Output
    - String
*/
fn independence_key(record: &InvocationRecord) -> String {
    let request = &record.request;
    digest_of(
        "evolution/trial/v1",
        &(
            &record.capability_id,
            record.version,
            &record.digest,
            &request.task_id,
            &request.episode_id,
            &request.environment,
            &request.repository,
            &request.inputs,
        ),
    )
}

/** Check an evidence reference through the reader's access
 * Input
    - ref_id: &str - reference
    - invocation_id: &str - invocation
    - sources: &Sources - canonical sources and the reader's access
 * Output
    - EvidenceRef
*/
fn evidence_ref(ref_id: &str, invocation_id: &str, sources: &Sources) -> EvidenceRef {
    let (source, access) = if sources.capture.evidence.contains_key(ref_id) {
        let (check, _) = sources.evidence(ref_id);
        let access = match check.outcome {
            RefOutcome::Resolved => EvidenceAccess::Readable,
            RefOutcome::Unauthorized => EvidenceAccess::Restricted,
            _ => EvidenceAccess::Missing,
        };
        (EvidenceSource::WorkCapture, access)
    } else {
        (
            EvidenceSource::InvocationVerification,
            EvidenceAccess::RecordedByEngine,
        )
    };
    EvidenceRef {
        ref_id: ref_id.into(),
        source,
        access,
        invocation_id: invocation_id.into(),
    }
}

/** Normalize one record into an observation (the record is not changed)
 * Input
    - record: &InvocationRecord - record (its digest already verified)
    - definition: &CapabilityDefinition - the analyzed definition (for the pinned governance keys)
    - sources: &Sources - canonical sources and the reader's access
    - events: usize - DataSea events correlated with it
    - synthetic: bool - the record comes from a labelled fixture
 * Output
    - Observation
*/
pub(crate) fn normalize(
    record: &InvocationRecord,
    definition: &CapabilityDefinition,
    sources: &Sources,
    events: usize,
    synthetic: bool,
) -> Observation {
    let (class, verified, failed, unresolved, contradicted) = classify(record);
    let pins = pinned_governance(definition);
    let governance = pins
        .keys()
        .map(|key| {
            let (kind, id) = key.split_once(':').unwrap_or(("policy", key));
            let table = match kind {
                "zone" => &record.governance.zones,
                "flow" => &record.governance.flows,
                _ => &record.governance.policies,
            };
            let known = match kind {
                "zone" | "flow" => record.governance.landscape_known,
                _ => record.governance.crane_known,
            };
            let version = if known {
                table.get(id).cloned().unwrap_or_else(|| "missing".into())
            } else {
                "unknown".into()
            };
            (key.clone(), version)
        })
        .collect();
    let mut evidence = record
        .verifications
        .iter()
        .flat_map(|verification| verification.evidence_refs.iter())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|id| evidence_ref(id, &record.invocation_id, sources))
        .collect::<Vec<_>>();
    if events > 0 {
        evidence.push(EvidenceRef {
            ref_id: format!("datasea:capability.invocation:{}", record.invocation_id),
            source: EvidenceSource::DataSeaEvent,
            access: EvidenceAccess::Readable,
            invocation_id: record.invocation_id.clone(),
        });
    }
    let failed_steps = record
        .operations
        .iter()
        .filter(|operation| {
            matches!(
                operation.status,
                OperationStatus::Failed | OperationStatus::NotStarted | OperationStatus::TimedOut
            )
        })
        .map(|operation| operation.step_id.clone())
        .collect();
    let infrastructure = record.failures.iter().any(|failure| {
        let failure = failure.to_ascii_lowercase();
        failure.contains("unavailable")
            || failure.contains("not healthy")
            || failure.contains("unhealthy")
    });
    Observation {
        invocation_id: record.invocation_id.clone(),
        record_digest: record.record_digest.clone(),
        capability_id: record.capability_id.clone(),
        version: record.version,
        capability_digest: record.digest.clone(),
        plan_digest: record.plan.as_ref().map(|plan| plan.plan_digest.clone()),
        independence_key: independence_key(record),
        state: record.state,
        class,
        context: ObservationContext {
            environment: record.request.environment.clone(),
            repository: record.request.repository.clone(),
            governance,
        },
        verified_claims: verified,
        failed_claims: failed,
        unresolved_claims: unresolved,
        contradicted_claims: contradicted,
        failed_steps,
        infrastructure,
        attempts: record
            .operations
            .iter()
            .map(|operation| operation.attempts)
            .sum(),
        events,
        evidence,
        synthetic,
        duplicate_of: None,
        at: record.updated_at,
    }
}

/** Collapse resubmissions of one trial: the most recent trial-like record represents the group,
 * the others point to it
 * Input
    - observations: &mut [Observation] - observations
 * Output
    - usize the number collapsed
*/
pub(crate) fn deduplicate(observations: &mut [Observation]) -> usize {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, observation) in observations.iter().enumerate() {
        groups
            .entry(observation.independence_key.clone())
            .or_default()
            .push(index);
    }
    let mut collapsed = 0;
    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        let representative = *members
            .iter()
            .max_by_key(|index| {
                let observation = &observations[**index];
                (
                    observation.class.trial(),
                    observation.at,
                    observation.invocation_id.clone(),
                )
            })
            .unwrap_or(&members[0]);
        let id = observations[representative].invocation_id.clone();
        for index in members {
            if *index != representative {
                observations[*index].duplicate_of = Some(id.clone());
                collapsed += 1;
            }
        }
    }
    collapsed
}

/** Count DataSea lifecycle events per invocation from the event log's payloads
 * Input
    - events: &[crate::telemetry::events::Event] - events
 * Output
    - BTreeMap<String, usize> invocation id to events
*/
pub(crate) fn count_events(events: &[crate::telemetry::events::Event]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for event in events
        .iter()
        .filter(|event| event.event_type.starts_with("capability.invocation."))
    {
        if let Some(id) = event
            .payload
            .get("invocation_id")
            .and_then(|id| id.as_str())
        {
            *counts.entry(id.to_string()).or_insert(0) += 1;
        }
    }
    counts
}
