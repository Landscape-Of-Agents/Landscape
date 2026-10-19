// Registry queries: exact versions, the latest eligible version, filtered listings, version
// history, provenance traces down to the original evidence (checked against the reader's own
// access), dependency inspection, and deterministic applicability evaluation. Only published,
// current (not stale), non-withdrawn versions are eligible for new use; revoked and deprecated
// versions stay visible in history. Applicability compares exact, declared values: an unknown
// value never matches, and richer matching can be added behind the ApplicabilityMatcher interface.

use serde::{Deserialize, Serialize};

use super::lifecycle::{LifecycleState, LifecycleTransition};
use super::model::{CapabilityDefinition, DependencyReference};
use super::refs::{RefOutcome, ReferenceCheck, Sources};
use super::registry::{RegistryState, VersionRecord};
use super::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capture::Truth;

/** A registry listing filter (every field narrows; None does not filter)
 * Fields
    - capability_id: Option<String> - capability
    - owner: Option<String> - owner
    - state: Option<LifecycleState> - lifecycle state of the latest version
    - task_class: Option<String> - supported task class
    - environment: Option<String> - applicable environment
    - include_withdrawn: bool - include capabilities whose every version is revoked or rejected
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegistryQuery {
    pub(crate) capability_id: Option<String>,
    pub(crate) owner: Option<String>,
    pub(crate) state: Option<LifecycleState>,
    pub(crate) task_class: Option<String>,
    pub(crate) environment: Option<String>,
    pub(crate) include_withdrawn: bool,
}

/** One capability in a listing
 * Fields
    - capability_id: String - capability
    - name: String - name of the latest version
    - owner_ref: Option<String> - owner
    - versions: Vec<(u32, LifecycleState)> - every version and its state
    - latest_eligible: Option<u32> - latest version eligible for new use
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CapabilitySummary {
    pub(crate) capability_id: String,
    pub(crate) name: String,
    pub(crate) owner_ref: Option<String>,
    pub(crate) versions: Vec<(u32, LifecycleState)>,
    pub(crate) latest_eligible: Option<u32>,
}

/** Report whether a version is eligible for new use
 * Input
    - record: &VersionRecord - version
 * Output
    - bool
*/
pub(crate) fn eligible(record: &VersionRecord) -> bool {
    record.state == LifecycleState::Published && record.stale.is_empty()
}

/** The latest eligible version of a capability
 * Input
    - state: &RegistryState - registry
    - capability_id: &str - capability
 * Output
    - Option<&VersionRecord>
*/
pub(crate) fn latest_eligible<'a>(
    state: &'a RegistryState,
    capability_id: &str,
) -> Option<&'a VersionRecord> {
    state
        .versions
        .get(capability_id)?
        .values()
        .rev()
        .find(|record| eligible(record))
}

/** List capabilities matching a query
 * Input
    - state: &RegistryState - registry
    - query: &RegistryQuery - filter
 * Output
    - Vec<CapabilitySummary>
*/
pub(crate) fn list(state: &RegistryState, query: &RegistryQuery) -> Vec<CapabilitySummary> {
    state
        .versions
        .iter()
        .filter(|(id, _)| {
            query
                .capability_id
                .as_ref()
                .is_none_or(|wanted| wanted == *id)
        })
        .filter_map(|(id, versions)| {
            let latest = versions.values().next_back()?;
            let definition = &latest.definition;
            let withdrawn = versions.values().all(|record| {
                matches!(
                    record.state,
                    LifecycleState::Revoked | LifecycleState::Rejected
                )
            });
            let keep = (query.include_withdrawn || !withdrawn)
                && query
                    .owner
                    .as_ref()
                    .is_none_or(|owner| definition.owner_ref.as_ref() == Some(owner))
                && query.state.is_none_or(|wanted| latest.state == wanted)
                && query
                    .task_class
                    .as_ref()
                    .is_none_or(|class| definition.applicability.task_classes.contains(class))
                && query.environment.as_ref().is_none_or(|environment| {
                    definition.applicability.environments.contains(environment)
                });
            keep.then(|| CapabilitySummary {
                capability_id: id.clone(),
                name: definition.name.clone(),
                owner_ref: definition.owner_ref.clone(),
                versions: versions
                    .iter()
                    .map(|(version, record)| (*version, record.state))
                    .collect(),
                latest_eligible: latest_eligible(state, id).map(|record| record.definition.version),
            })
        })
        .collect()
}

/** The history of a capability
 * Fields
    - capability_id: String - capability
    - versions: Vec<(u32, Option<u32>, String, LifecycleState)> - version, predecessor, digest, state
    - transitions: Vec<LifecycleTransition> - lifecycle transitions, oldest first
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VersionHistory {
    pub(crate) capability_id: String,
    pub(crate) versions: Vec<(u32, Option<u32>, String, LifecycleState)>,
    pub(crate) transitions: Vec<LifecycleTransition>,
}

/** The version history of a capability
 * Input
    - state: &RegistryState - registry
    - capability_id: &str - capability
 * Output
    - CapabilityResult<VersionHistory>
    - Error (NotFound) when unknown
*/
pub(crate) fn history(
    state: &RegistryState,
    capability_id: &str,
) -> CapabilityResult<VersionHistory> {
    let versions = state.versions.get(capability_id).ok_or_else(|| {
        CapabilityError::new(
            ErrorCode::NotFound,
            format!("capability {capability_id} does not exist"),
        )
    })?;
    Ok(VersionHistory {
        capability_id: capability_id.into(),
        versions: versions
            .iter()
            .map(|(version, record)| {
                (
                    *version,
                    record.definition.previous_version_ref,
                    record.definition_digest.clone(),
                    record.state,
                )
            })
            .collect(),
        transitions: state
            .transitions
            .iter()
            .filter(|transition| transition.capability_id == capability_id)
            .cloned()
            .collect(),
    })
}

/** A provenance trace from a capability version to its original evidence
 * Fields
    - capability_id: String - capability
    - version: u32 - version
    - intakes: Vec<String> - source intakes
    - references: Vec<ReferenceCheck> - every pack, episode, evidence, and assertion reference,
      resolved for the reader
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProvenanceTrace {
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) intakes: Vec<String>,
    pub(crate) references: Vec<ReferenceCheck>,
}

/** Trace a version to its intakes, packs, episodes, and the evidence its claims cite; each
 * evidence record is resolved against the reader's own access policy
 * Input
    - state: &RegistryState - registry
    - capability_id: &str - capability
    - version: u32 - version
    - sources: &Sources - canonical sources and the reader
 * Output
    - CapabilityResult<ProvenanceTrace>
    - Error (NotFound) when unknown
*/
pub(crate) fn trace(
    state: &RegistryState,
    capability_id: &str,
    version: u32,
    sources: &Sources,
) -> CapabilityResult<ProvenanceTrace> {
    let definition = &state.record(capability_id, version)?.definition;
    let provenance = &definition.provenance;
    let mut references = Vec::new();
    for intake in &provenance.source_intake_refs {
        let outcome = if state.intakes.contains_key(intake) {
            RefOutcome::Resolved
        } else {
            RefOutcome::Missing
        };
        references.push(ReferenceCheck::new(
            "intake",
            intake,
            outcome,
            "capability intake",
        ));
    }
    for pack in &provenance.source_evidence_pack_refs {
        let outcome = match sources.capture.packs.get(&pack.pack_id) {
            Some(found)
                if found.manifest.digest == pack.manifest_digest
                    && found.verify_manifest().is_ok() =>
            {
                RefOutcome::Resolved
            }
            Some(_) => RefOutcome::Mismatch,
            None => RefOutcome::Missing,
        };
        references.push(ReferenceCheck::new(
            "evidence_pack",
            &format!("{}#{}", pack.pack_id, pack.manifest_digest),
            outcome,
            "Work Capture Evidence Pack",
        ));
    }
    for episode in &provenance.source_episode_refs {
        let outcome = if sources.capture.episodes.contains_key(episode) {
            RefOutcome::Resolved
        } else {
            RefOutcome::Missing
        };
        references.push(ReferenceCheck::new(
            "episode",
            episode,
            outcome,
            "Work Capture episode",
        ));
    }
    for claim in &definition.verification.claims {
        for evidence in &claim.evidence_refs {
            let (check, _) = sources.evidence(evidence);
            references.push(ReferenceCheck {
                detail: format!("claim {}: {}", claim.claim_id, check.detail),
                ..check
            });
        }
        for assertion in &claim.assertion_refs {
            let outcome = if sources.capture.assertions.contains_key(assertion) {
                RefOutcome::Resolved
            } else {
                RefOutcome::Missing
            };
            references.push(ReferenceCheck::new(
                "assertion",
                assertion,
                outcome,
                format!("claim {}", claim.claim_id),
            ));
        }
    }
    Ok(ProvenanceTrace {
        capability_id: capability_id.into(),
        version,
        intakes: provenance.source_intake_refs.clone(),
        references,
    })
}

/** The current status of one dependency
 * Fields
    - dependency: DependencyReference - declaration
    - outcome: RefOutcome - how it resolves now
    - resolved_version: Option<String> - the version it resolves to (never a substitute)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DependencyStatus {
    pub(crate) dependency: DependencyReference,
    pub(crate) outcome: RefOutcome,
    pub(crate) resolved_version: Option<String>,
}

/** Inspect the dependencies of a version
 * Input
    - state: &RegistryState - registry
    - capability_id: &str - capability
    - version: u32 - version
    - sources: &Sources - canonical sources
 * Output
    - CapabilityResult<Vec<DependencyStatus>>
    - Error (NotFound) when unknown
*/
pub(crate) fn dependencies(
    state: &RegistryState,
    capability_id: &str,
    version: u32,
    sources: &Sources,
) -> CapabilityResult<Vec<DependencyStatus>> {
    use super::model::{DependencyKind, VersionConstraint};
    let definition = &state.record(capability_id, version)?.definition;
    Ok(definition
        .dependencies
        .iter()
        .map(|dependency| {
            let exact = match &dependency.constraint {
                VersionConstraint::Exact(version) => Some(version.as_str()),
                _ => None,
            };
            let (outcome, resolved_version) = match dependency.kind {
                DependencyKind::Zone | DependencyKind::Flow | DependencyKind::Policy => {
                    let kind = format!("{:?}", dependency.kind).to_ascii_lowercase();
                    let check = sources.current.check(&kind, &dependency.target_id, exact);
                    (
                        check.outcome,
                        (check.outcome == RefOutcome::Resolved)
                            .then(|| exact.unwrap_or_default().to_string()),
                    )
                }
                DependencyKind::Capability => match state
                    .versions
                    .get(&dependency.target_id)
                    .and_then(|versions| {
                        versions.values().rev().find(|record| {
                            record.state.selectable_for_dependencies()
                                && dependency
                                    .constraint
                                    .admits(&record.definition.version.to_string())
                        })
                    }) {
                    Some(record) => (
                        RefOutcome::Resolved,
                        Some(record.definition.version.to_string()),
                    ),
                    None => (RefOutcome::Missing, None),
                },
                DependencyKind::EvidencePack => {
                    match sources.capture.packs.get(&dependency.target_id) {
                        Some(pack) if exact == Some(pack.manifest.digest.as_str()) => {
                            (RefOutcome::Resolved, Some(pack.manifest.digest.clone()))
                        }
                        Some(_) => (RefOutcome::Mismatch, None),
                        None => (RefOutcome::Missing, None),
                    }
                }
                _ => (RefOutcome::Unverifiable, None),
            };
            DependencyStatus {
                dependency: dependency.clone(),
                outcome,
                resolved_version,
            }
        })
        .collect())
}

/** A situation in which a capability might be used
 * Fields
    - task_class: Option<String> - task class, None when unknown
    - environment: Option<String> - environment, None when unknown
    - satisfied_preconditions: Vec<String> - precondition ids known to hold
    - entities: Vec<String> - entity or resource ids present
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ApplicabilityQuery {
    pub(crate) task_class: Option<String>,
    pub(crate) environment: Option<String>,
    pub(crate) satisfied_preconditions: Vec<String>,
    pub(crate) entities: Vec<String>,
}

/** An applicability decision
 * Fields
    - applicable: Truth - yes, no, or unknown (never yes on unknown inputs)
    - reasons: Vec<String> - why
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ApplicabilityDecision {
    pub(crate) applicable: Truth,
    pub(crate) reasons: Vec<String>,
}

/** Decides whether a definition applies to a situation (replaceable strategy) */
pub(crate) trait ApplicabilityMatcher {
    /** Evaluate applicability
     * Input
        - definition: &CapabilityDefinition - definition
        - query: &ApplicabilityQuery - situation
     * Output
        - ApplicabilityDecision
    */
    fn evaluate(
        &self,
        definition: &CapabilityDefinition,
        query: &ApplicabilityQuery,
    ) -> ApplicabilityDecision;
}

/** Exact matching of declared values: exclusions and mismatches say no; unknown task classes,
 * environments, and unconfirmed preconditions say unknown */
pub(crate) struct ExactMatcher;

impl ApplicabilityMatcher for ExactMatcher {
    /** Evaluate applicability by exact comparison
     * Input
        - definition: &CapabilityDefinition - definition
        - query: &ApplicabilityQuery - situation
     * Output
        - ApplicabilityDecision
    */
    fn evaluate(
        &self,
        definition: &CapabilityDefinition,
        query: &ApplicabilityQuery,
    ) -> ApplicabilityDecision {
        let contract = &definition.applicability;
        let mut no = Vec::new();
        let mut unknown = Vec::new();
        for exclusion in &contract.exclusions {
            let hit = match exclusion.kind.as_str() {
                "task_class" => query.task_class.as_ref() == Some(&exclusion.value),
                "environment" => query.environment.as_ref() == Some(&exclusion.value),
                "entity" => query.entities.contains(&exclusion.value),
                _ => false,
            };
            if hit {
                no.push(format!(
                    "excluded {} {}: {}",
                    exclusion.kind, exclusion.value, exclusion.reason
                ));
            }
        }
        match &query.task_class {
            None => unknown.push("the task class is unknown".to_string()),
            Some(class) if !contract.task_classes.contains(class) => {
                no.push(format!("task class {class} is not supported"))
            }
            _ => {}
        }
        match &query.environment {
            None => unknown.push("the environment is unknown".to_string()),
            Some(environment) if !contract.environments.contains(environment) => no.push(format!(
                "environment {environment} is not one it was established for"
            )),
            _ => {}
        }
        for condition in contract
            .preconditions
            .iter()
            .chain(&contract.required_state)
        {
            if !query.satisfied_preconditions.contains(&condition.id) {
                unknown.push(format!("precondition {} is not confirmed", condition.id));
            }
        }
        let applicable = if !no.is_empty() {
            Truth::No
        } else if !unknown.is_empty() {
            Truth::Unknown
        } else {
            Truth::Yes
        };
        no.extend(unknown);
        ApplicabilityDecision {
            applicable,
            reasons: no,
        }
    }
}
