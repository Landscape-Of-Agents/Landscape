// The CapabilityIntake contract (INTAKE_SCHEMA_VERSION): the traceable handoff from Work Capture.
// It is not an Evidence Pack and not a capability. The caller supplies only identities (episode,
// pack, the digest it expects) and enterprise intent; everything else is built from the canonical
// Work Capture records as references (evidence ids, artifact hashes, assertion ids, zone and flow
// versions), never copies of payloads. Building an intake:
//
//   1. checks the request's schema version and the pack's schema version
//   2. resolves the work episode and the Evidence Pack (missing ones are an error, not inferred)
//   3. recomputes the pack manifest and compares it with the recorded and the expected digest
//      (a match proves integrity of the content, not the truth of its claims)
//   4. resolves every evidence record and assertion for the submitting reader (unreadable,
//      missing, and changed records are recorded with their exact condition)
//   5. compares the zone, flow, and policy versions the pack used with the current ones
//   6. classifies business mappings (an inferred mapping never becomes authoritative)
//   7. collects completeness and uncertainty, and evaluates the validation profile
//
// An incomplete intake is still stored for review; storage is not readiness.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{MappingStatus, ReviewLevel, RiskLevel};
use super::refs::{RefOutcome, ReferenceCheck, Sources};
use super::validate::{select_profile, Severity, ValidationFinding, ValidationProfile};
use super::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capture::correlate::assertions::{AssertionStatus, DerivationMethod};
use crate::capture::correlate::model::RelationType;
use crate::capture::format::record::{EvidenceType, ParseStatus};
use crate::capture::ingest::episode::EpisodeOutcome;
use crate::capture::ingest::pack::{EvidencePack, MissingEvidence, SourceState};
use crate::capture::resources::scope::VersionedRef;
use crate::capture::{stable_id, Actor, Truth};
use crate::trust::crypto::digest_json;

/** Version of the intake schema */
pub(crate) const INTAKE_SCHEMA_VERSION: u32 = 1;

/** A business reference: an objective, criterion, invariant, or measure, with its authority
 * Fields
    - id: String - stable identifier
    - statement: String - the objective, criterion, invariant, or measure
    - authority: Option<String> - authoritative system or approved declaration
    - mapping: MappingStatus - how it is known to apply
    - confirmed_by: Option<String> - who confirmed it (human-confirmed mappings)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BusinessRef {
    pub(crate) id: String,
    pub(crate) statement: String,
    #[serde(default)]
    pub(crate) authority: Option<String>,
    pub(crate) mapping: MappingStatus,
    #[serde(default)]
    pub(crate) confirmed_by: Option<String>,
}

impl BusinessRef {
    /** The mapping status the reference actually supports: explicit needs an authority and
     * human-confirmed needs a confirmer; otherwise the claim is not authoritative
     * Input
        - None (uses self)
     * Output
        - MappingStatus
    */
    pub(crate) fn effective_mapping(&self) -> MappingStatus {
        match self.mapping {
            MappingStatus::ExplicitlyMapped if self.authority.is_none() => {
                MappingStatus::InferredUnverified
            }
            MappingStatus::HumanConfirmed if self.confirmed_by.is_none() => {
                MappingStatus::InferredUnverified
            }
            other => other,
        }
    }
}

/** Why the work mattered (intent, never proof of outcome)
 * Fields
    - objective: Option<BusinessRef> - business objective
    - process_domain: Option<String> - business process or domain
    - acceptance_criteria: Vec<BusinessRef> - acceptance criteria (not proof they passed)
    - invariants: Vec<BusinessRef> - business invariants (not proof they held)
    - outcome_measures: Vec<BusinessRef> - KPIs or SLOs, where applicable
    - owner: Option<String> - responsible owner or team
    - authoritative_systems: Vec<String> - systems authoritative for the business claims
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnterpriseIntent {
    pub(crate) objective: Option<BusinessRef>,
    pub(crate) process_domain: Option<String>,
    pub(crate) acceptance_criteria: Vec<BusinessRef>,
    pub(crate) invariants: Vec<BusinessRef>,
    pub(crate) outcome_measures: Vec<BusinessRef>,
    pub(crate) owner: Option<String>,
    pub(crate) authoritative_systems: Vec<String>,
}

/** A submission request: identities and intent only
 * Fields
    - schema_version: u32 - INTAKE_SCHEMA_VERSION
    - episode_id: String - work episode
    - pack_id: String - Evidence Pack
    - pack_digest: String - manifest digest the submitter expects
    - intent: EnterpriseIntent - enterprise intent
    - risk: RiskLevel - risk of the intended capability (selects the minimum profile)
    - profile: Option<String> - validation profile, None for the default for the risk
    - idempotency_key: Option<String> - key, None to derive it from the source identity
    - unresolved_assumptions: Vec<String> - assumptions the submitter knows are unresolved
    - human_interventions: Vec<String> - human interventions during the work
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IntakeRequest {
    pub(crate) schema_version: u32,
    pub(crate) episode_id: String,
    pub(crate) pack_id: String,
    pub(crate) pack_digest: String,
    pub(crate) intent: EnterpriseIntent,
    pub(crate) risk: RiskLevel,
    #[serde(default)]
    pub(crate) profile: Option<String>,
    #[serde(default)]
    pub(crate) idempotency_key: Option<String>,
    #[serde(default)]
    pub(crate) unresolved_assumptions: Vec<String>,
    #[serde(default)]
    pub(crate) human_interventions: Vec<String>,
}

/** A reference to a Work Capture evidence record
 * Fields
    - evidence_id: String - evidence
    - evidence_type: EvidenceType - type
    - artifact_id: String - raw artifact
    - content_hash: Option<String> - artifact content hash
    - revision: Option<String> - source revision
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvidenceRef {
    pub(crate) evidence_id: String,
    pub(crate) evidence_type: EvidenceType,
    pub(crate) artifact_id: String,
    pub(crate) content_hash: Option<String>,
    pub(crate) revision: Option<String>,
}

/** A reference to a Work Capture assertion with its status
 * Fields
    - assertion_id: String - assertion
    - predicate: String - predicate
    - subject_key: String - subject
    - truth: Truth - claimed value
    - status: AssertionStatus - status
    - method: DerivationMethod - how it was produced
    - supporting: Vec<String> - supporting evidence
    - contradicting: Vec<String> - contradicting evidence
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AssertionRef {
    pub(crate) assertion_id: String,
    pub(crate) predicate: String,
    pub(crate) subject_key: String,
    pub(crate) truth: Truth,
    pub(crate) status: AssertionStatus,
    pub(crate) method: DerivationMethod,
    pub(crate) supporting: Vec<String>,
    pub(crate) contradicting: Vec<String>,
}

/** A source snapshot the pack's retrieval reflected
 * Fields
    - step_id: String - recipe step
    - resource_id: String - resource
    - snapshot: Option<String> - snapshot, None when the source cannot identify one
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceSnapshot {
    pub(crate) step_id: String,
    pub(crate) resource_id: String,
    pub(crate) snapshot: Option<String>,
}

/** What work occurred
 * Fields
    - episode_id: String - work episode
    - pack_id: String - Evidence Pack
    - pack_digest: String - verified manifest digest
    - pack_schema_version: u32 - Evidence Pack schema version
    - query_id: String - the pack's investigation (its audit record)
    - task_id: Option<String> - task
    - investigation: String - investigation question
    - outcome: Option<EpisodeOutcome> - recorded work outcome, None when not recorded
    - repository: Option<String> - repository
    - snapshots: Vec<SourceSnapshot> - source snapshots
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkIdentity {
    pub(crate) episode_id: String,
    pub(crate) pack_id: String,
    pub(crate) pack_digest: String,
    pub(crate) pack_schema_version: u32,
    pub(crate) query_id: String,
    pub(crate) task_id: Option<String>,
    pub(crate) investigation: String,
    pub(crate) outcome: Option<EpisodeOutcome>,
    pub(crate) repository: Option<String>,
    pub(crate) snapshots: Vec<SourceSnapshot>,
}

/** What was observed during the work (references)
 * Fields
    - code_changes: Vec<EvidenceRef> - commits and diffs
    - tool_calls: Vec<EvidenceRef> - agent tool calls and events (DataSea)
    - ci_and_tests: Vec<EvidenceRef> - CI runs and test reports
    - deployments: Vec<EvidenceRef> - deployments and verifications
    - runtime: Vec<EvidenceRef> - logs and metrics
    - assertions: Vec<AssertionRef> - assertions with their status
    - verified_claims: Vec<String> - assertions supported without contradiction
    - unresolved_claims: Vec<String> - assertions unresolved or only proposed
    - contradicted_claims: Vec<String> - assertions with contradicting evidence
    - state_transitions: Vec<String> - observed operational relations (TRIGGERED, RESULTED_IN,
      DEPLOYED_TO)
    - recovery: Vec<EvidenceRef> - rollback or recovery evidence
    - human_interventions: Vec<String> - human interventions
    - missing_evidence: Vec<MissingEvidence> - missing evidence as Work Capture reported it
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ObservedExecution {
    pub(crate) code_changes: Vec<EvidenceRef>,
    pub(crate) tool_calls: Vec<EvidenceRef>,
    pub(crate) ci_and_tests: Vec<EvidenceRef>,
    pub(crate) deployments: Vec<EvidenceRef>,
    pub(crate) runtime: Vec<EvidenceRef>,
    pub(crate) assertions: Vec<AssertionRef>,
    pub(crate) verified_claims: Vec<String>,
    pub(crate) unresolved_claims: Vec<String>,
    pub(crate) contradicted_claims: Vec<String>,
    pub(crate) state_transitions: Vec<String>,
    pub(crate) recovery: Vec<EvidenceRef>,
    pub(crate) human_interventions: Vec<String>,
    pub(crate) missing_evidence: Vec<MissingEvidence>,
}

/** Governance context (references and versions)
 * Fields
    - zones: Vec<VersionedRef> - Landscape zones
    - flows: Vec<VersionedRef> - Landscape flows
    - policies: Vec<VersionedRef> - Crane policies (name and version digest)
    - contracts: Vec<String> - Crane contract versions (registry generations)
    - validation_results: Vec<EvidenceRef> - Crane validation results
    - approval_level: ReviewLevel - approval required for the risk
    - environment: Option<String> - environment
    - access_limitations: Vec<String> - resources the investigation could not use, and why
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GovernanceRefs {
    pub(crate) zones: Vec<VersionedRef>,
    pub(crate) flows: Vec<VersionedRef>,
    pub(crate) policies: Vec<VersionedRef>,
    pub(crate) contracts: Vec<String>,
    pub(crate) validation_results: Vec<EvidenceRef>,
    pub(crate) approval_level: ReviewLevel,
    pub(crate) environment: Option<String>,
    pub(crate) access_limitations: Vec<String>,
}

/** Completeness and uncertainty, never collapsed into one flag
 * Fields
    - present: Vec<EvidenceType> - evidence types present and parsed
    - missing: Vec<MissingEvidence> - required evidence missing, with the reason
    - unavailable_sources: Vec<String> - sources that failed
    - unauthorized_sources: Vec<String> - sources or records not accessible
    - parse_failures: Vec<String> - records that could not be parsed
    - conflicting_assertions: Vec<String> - assertions with evidence on both sides
    - stale_revisions: Vec<ReferenceCheck> - references whose version changed or is unverifiable
    - unresolved_mappings: Vec<String> - business references without authoritative mapping
    - unresolved_assumptions: Vec<String> - assumptions not resolved
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Completeness {
    pub(crate) present: Vec<EvidenceType>,
    pub(crate) missing: Vec<MissingEvidence>,
    pub(crate) unavailable_sources: Vec<String>,
    pub(crate) unauthorized_sources: Vec<String>,
    pub(crate) parse_failures: Vec<String>,
    pub(crate) conflicting_assertions: Vec<String>,
    pub(crate) stale_revisions: Vec<ReferenceCheck>,
    pub(crate) unresolved_mappings: Vec<String>,
    pub(crate) unresolved_assumptions: Vec<String>,
}

/** Whether an intake is ready for its profile
 * Variants
    - Ready - every requirement of the profile is met
    - Incomplete - stored for review; not ready for compilation, verification, or publication
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IntakeStatus {
    Ready,
    Incomplete,
}

/** The validation of an intake
 * Fields
    - status: IntakeStatus - ready or incomplete
    - profile_id: String - validation profile
    - profile_version: u32 - its version
    - digest_verified: bool - the manifest was recomputed and matched
    - findings: Vec<ValidationFinding> - findings with reasons
    - references: Vec<ReferenceCheck> - every reference checked
    - validated_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IntakeValidationResult {
    pub(crate) status: IntakeStatus,
    pub(crate) profile_id: String,
    pub(crate) profile_version: u32,
    pub(crate) digest_verified: bool,
    pub(crate) findings: Vec<ValidationFinding>,
    pub(crate) references: Vec<ReferenceCheck>,
    pub(crate) validated_at: u64,
}

/** A capability intake
 * Fields
    - intake_id: String - stable identifier
    - schema_version: u32 - INTAKE_SCHEMA_VERSION
    - created_at: u64 - Unix milliseconds
    - created_by: Actor - submitter
    - idempotency_key: String - idempotency key
    - fingerprint: String - digest of the request content (idempotency conflicts)
    - status: IntakeStatus - ready or incomplete
    - risk: RiskLevel - risk declared for the intended capability
    - work: WorkIdentity - what work occurred
    - intent: EnterpriseIntent - why it mattered
    - observed: ObservedExecution - what was observed
    - governance: GovernanceRefs - governance context
    - completeness: Completeness - completeness and uncertainty
    - validation: IntakeValidationResult - validation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CapabilityIntake {
    pub(crate) intake_id: String,
    pub(crate) schema_version: u32,
    pub(crate) created_at: u64,
    pub(crate) created_by: Actor,
    pub(crate) idempotency_key: String,
    pub(crate) fingerprint: String,
    pub(crate) status: IntakeStatus,
    pub(crate) risk: RiskLevel,
    pub(crate) work: WorkIdentity,
    pub(crate) intent: EnterpriseIntent,
    pub(crate) observed: ObservedExecution,
    pub(crate) governance: GovernanceRefs,
    pub(crate) completeness: Completeness,
    pub(crate) validation: IntakeValidationResult,
}

/** The idempotency key (the source identity and digest, the profile, and the risk, unless the
 * caller supplies one) and the content fingerprint of a request
 * Input
    - request: &IntakeRequest - request
 * Output
    - (String, String) key and fingerprint
*/
pub(crate) fn identity(request: &IntakeRequest) -> (String, String) {
    let profile = request.profile.clone().unwrap_or_else(|| "default".into());
    let risk = format!("{:?}", request.risk);
    let key = request.idempotency_key.clone().unwrap_or_else(|| {
        stable_id(
            "itk",
            &[
                &request.episode_id,
                &request.pack_id,
                &request.pack_digest,
                &profile,
                &risk,
            ],
        )
    });
    let fingerprint =
        digest_json("capability/intake-request/v1", &json!(request))[..32].to_string();
    (key, fingerprint)
}

/** Build a finding
 * Input
    - code: &str - category
    - severity: Severity - error, warning, or info
    - message: String - explanation
    - refs: Vec<String> - related references
 * Output
    - ValidationFinding
*/
fn finding(
    code: &str,
    severity: Severity,
    message: String,
    refs: Vec<String>,
) -> ValidationFinding {
    ValidationFinding {
        code: code.into(),
        severity,
        message,
        evidence_refs: refs,
    }
}

/** Build an intake from a request and the canonical Work Capture records (no persistence)
 * Input
    - request: &IntakeRequest - request
    - sources: &Sources - canonical sources and the submitter's access policy
    - actor: &Actor - submitter
    - now: u64 - Unix milliseconds
 * Output
    - CapabilityResult<CapabilityIntake>
    - Error for an unsupported schema version, a missing episode or pack, a digest mismatch, or a
      pack the submitter may not read
*/
pub(crate) fn build(
    request: &IntakeRequest,
    sources: &Sources,
    actor: &Actor,
    now: u64,
) -> CapabilityResult<CapabilityIntake> {
    if request.schema_version != INTAKE_SCHEMA_VERSION {
        return Err(CapabilityError::new(ErrorCode::UnsupportedSchemaVersion, format!("intake schema version {} is not supported (this Crane reads {INTAKE_SCHEMA_VERSION})", request.schema_version)));
    }
    let profile = select_profile(request.profile.as_deref(), request.risk)?;
    let capture = sources.capture;
    let episode = capture.episodes.get(&request.episode_id).ok_or_else(|| {
        CapabilityError::new(
            ErrorCode::MissingSourceReference,
            format!(
                "work episode '{}' does not exist in Work Capture",
                request.episode_id
            ),
        )
    })?;
    let pack = capture.packs.get(&request.pack_id).ok_or_else(|| {
        CapabilityError::new(
            ErrorCode::MissingSourceReference,
            format!(
                "Evidence Pack '{}' does not exist in Work Capture",
                request.pack_id
            ),
        )
    })?;
    if pack.episode_id.as_deref() != Some(episode.episode_id.as_str()) {
        return Err(CapabilityError::new(
            ErrorCode::IntegrityMismatch,
            "the Evidence Pack belongs to another work episode",
        ));
    }
    if pack.schema_version != crate::capture::SCHEMA_VERSION {
        return Err(CapabilityError::new(
            ErrorCode::UnsupportedSchemaVersion,
            format!(
                "Evidence Pack schema version {} is not supported",
                pack.schema_version
            ),
        ));
    }
    pack.verify_manifest().map_err(|error| {
        CapabilityError::new(
            ErrorCode::IntegrityMismatch,
            format!("the Evidence Pack content does not match its manifest: {error}"),
        )
    })?;
    if pack.manifest.digest != request.pack_digest {
        return Err(CapabilityError::new(
            ErrorCode::IntegrityMismatch,
            format!(
                "expected pack digest {} but the pack's manifest digest is {}",
                request.pack_digest, pack.manifest.digest
            ),
        ));
    }
    if let Some(repository) = &pack.scope.repository {
        if !sources.reader.permits_repository(Some(repository)) {
            return Err(CapabilityError::new(
                ErrorCode::Unauthorized,
                format!("the submitter may not read evidence of {repository}"),
            ));
        }
    }
    let mut references = vec![
        ReferenceCheck::new(
            "episode",
            &episode.episode_id,
            RefOutcome::Resolved,
            "work episode",
        ),
        ReferenceCheck::new(
            "evidence_pack",
            &format!("{}#{}", pack.pack_id, pack.manifest.digest),
            RefOutcome::Resolved,
            "manifest recomputed and matched",
        ),
    ];
    let mut findings = Vec::new();
    let mut completeness = Completeness::default();
    let mut observed = ObservedExecution {
        human_interventions: request.human_interventions.clone(),
        missing_evidence: pack.missing_evidence.clone(),
        ..ObservedExecution::default()
    };
    let mut validation_results = Vec::new();
    let mut policies = Vec::new();
    let mut present = BTreeSet::new();
    for item in &pack.evidence {
        let (check, record) = sources.evidence(&item.evidence_id);
        let check = match (record, &check.outcome) {
            (Some(record), _) if record.source_ref.artifact_id != item.artifact.artifact_id => {
                ReferenceCheck::new(
                    "evidence",
                    &item.evidence_id,
                    RefOutcome::Mismatch,
                    "its artifact differs from the pack's reference",
                )
            }
            (Some(_), _)
                if capture
                    .artifacts
                    .get(&item.artifact.artifact_id)
                    .map(|artifact| &artifact.content_hash)
                    != Some(&item.artifact.content_hash) =>
            {
                ReferenceCheck::new(
                    "evidence",
                    &item.evidence_id,
                    RefOutcome::Mismatch,
                    "its artifact content hash differs from the pack's reference",
                )
            }
            _ => check,
        };
        match check.outcome {
            RefOutcome::Unauthorized => completeness.unauthorized_sources.push(format!(
                "{}:{}",
                item.source_ref.resource_id, item.evidence_id
            )),
            RefOutcome::Missing | RefOutcome::Mismatch => findings.push(finding(
                "missing_source_reference",
                Severity::Error,
                format!(
                    "evidence {} is {:?}: {}",
                    item.evidence_id, check.outcome, check.detail
                ),
                vec![item.evidence_id.clone()],
            )),
            _ => {}
        }
        let resolved = check.outcome == RefOutcome::Resolved;
        references.push(check);
        if !resolved {
            continue;
        }
        if item.parse_status == ParseStatus::Failed {
            completeness.parse_failures.push(item.evidence_id.clone());
        } else {
            present.insert(item.evidence_type);
        }
        let reference = EvidenceRef {
            evidence_id: item.evidence_id.clone(),
            evidence_type: item.evidence_type,
            artifact_id: item.artifact.artifact_id.clone(),
            content_hash: item.artifact.content_hash.clone(),
            revision: item.source_ref.revision.clone(),
        };
        let status = capture
            .evidence
            .get(&item.evidence_id)
            .and_then(|record| record.field("status"))
            .unwrap_or_default();
        match item.evidence_type {
            EvidenceType::CodeChange | EvidenceType::PullRequest => {
                observed.code_changes.push(reference)
            }
            EvidenceType::ToolCall | EvidenceType::AgentSession | EvidenceType::AgentEvent => {
                observed.tool_calls.push(reference)
            }
            EvidenceType::CiRun | EvidenceType::TestReport => observed.ci_and_tests.push(reference),
            EvidenceType::Deployment
                if status.contains("rolled_back") || status.contains("rollback") =>
            {
                observed.recovery.push(reference)
            }
            EvidenceType::Deployment | EvidenceType::Verification => {
                observed.deployments.push(reference)
            }
            EvidenceType::RuntimeLog | EvidenceType::Metric => observed.runtime.push(reference),
            EvidenceType::Validation => validation_results.push(reference),
            EvidenceType::Policy => {
                if let Some(name) = capture
                    .evidence
                    .get(&item.evidence_id)
                    .and_then(|record| record.field("name"))
                {
                    policies.push(VersionedRef {
                        id: name,
                        version: item.source_ref.revision.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    completeness.present = present.into_iter().collect();
    for assertion in &pack.assertions {
        let current = capture.assertions.get(&assertion.assertion_id);
        let outcome = match current {
            None => RefOutcome::Missing,
            Some(current)
                if current.status != assertion.status || current.claim != assertion.claim =>
            {
                RefOutcome::Stale
            }
            Some(_) => RefOutcome::Resolved,
        };
        references.push(ReferenceCheck::new(
            "assertion",
            &assertion.assertion_id,
            outcome,
            assertion.statement.clone(),
        ));
        let truth = assertion.truth();
        observed.assertions.push(AssertionRef {
            assertion_id: assertion.assertion_id.clone(),
            predicate: assertion.claim.predicate.clone(),
            subject_key: assertion.claim.subject_key.clone(),
            truth,
            status: assertion.status,
            method: assertion.derivation.method,
            supporting: assertion.supporting.clone(),
            contradicting: assertion.contradicting.clone(),
        });
        if assertion.conflicted() || assertion.status == AssertionStatus::Contradicted {
            observed
                .contradicted_claims
                .push(assertion.assertion_id.clone());
            completeness
                .conflicting_assertions
                .push(assertion.assertion_id.clone());
        } else if assertion.status == AssertionStatus::Supported
            && truth != Truth::Unknown
            && assertion.derivation.method != DerivationMethod::AgentHypothesis
        {
            observed
                .verified_claims
                .push(assertion.assertion_id.clone());
        } else {
            observed
                .unresolved_claims
                .push(assertion.assertion_id.clone());
        }
    }
    observed.state_transitions = pack
        .context_view
        .relations
        .iter()
        .filter(|relation| {
            matches!(
                relation.relation_type,
                RelationType::Triggered | RelationType::ResultedIn | RelationType::DeployedTo
            )
        })
        .map(|relation| relation.relation_id.clone())
        .collect();
    for missing in &pack.missing_evidence {
        match missing.state {
            SourceState::Unavailable => completeness
                .unavailable_sources
                .push(format!("{:?}: {}", missing.evidence_type, missing.detail)),
            SourceState::Denied => completeness
                .unauthorized_sources
                .push(format!("{:?}: {}", missing.evidence_type, missing.detail)),
            SourceState::ParseFailed => completeness
                .parse_failures
                .push(format!("{:?}", missing.evidence_type)),
            _ => {}
        }
    }
    completeness.missing = pack.missing_evidence.clone();
    let governance_checks = pack
        .scope
        .zones
        .iter()
        .map(|zone| {
            sources
                .current
                .check("zone", &zone.id, zone.version.as_deref())
        })
        .chain(pack.scope.flows.iter().map(|flow| {
            sources
                .current
                .check("flow", &flow.id, flow.version.as_deref())
        }))
        .chain(policies.iter().map(|policy| {
            sources
                .current
                .check("policy", &policy.id, policy.version.as_deref())
        }))
        .collect::<Vec<_>>();
    for check in governance_checks {
        if check.outcome != RefOutcome::Resolved {
            completeness.stale_revisions.push(check.clone());
        }
        references.push(check);
    }
    let intent = &request.intent;
    let all_refs = intent
        .objective
        .iter()
        .chain(&intent.acceptance_criteria)
        .chain(&intent.invariants)
        .chain(&intent.outcome_measures);
    for reference in all_refs {
        let effective = reference.effective_mapping();
        if effective != reference.mapping {
            findings.push(finding("unsupported_mapping_claim", Severity::Error, format!("business reference {} claims {:?} without the authority or confirmation that requires; it is treated as inferred and unverified", reference.id, reference.mapping), vec![reference.id.clone()]));
        }
        if !effective.authoritative() {
            completeness.unresolved_mappings.push(reference.id.clone());
        }
    }
    completeness.unresolved_assumptions = request.unresolved_assumptions.clone();
    findings.extend(profile_findings(&profile, request, &completeness));
    let status = if findings
        .iter()
        .any(|finding| finding.severity == Severity::Error)
    {
        IntakeStatus::Incomplete
    } else {
        IntakeStatus::Ready
    };
    let (idempotency_key, fingerprint) = identity(request);
    let snapshots = capture
        .audit(&pack.retrieval.query_id)
        .map(|audit| {
            audit
                .steps
                .iter()
                .filter_map(|step| {
                    step.resource_id.clone().map(|resource_id| SourceSnapshot {
                        step_id: step.step_id.clone(),
                        resource_id,
                        snapshot: step.snapshot.clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let approval_level = match request.risk {
        RiskLevel::Low => ReviewLevel::Peer,
        RiskLevel::Medium => ReviewLevel::Owner,
        _ => ReviewLevel::Admin,
    };
    Ok(CapabilityIntake {
        intake_id: stable_id("int", &[&idempotency_key, &fingerprint]),
        schema_version: INTAKE_SCHEMA_VERSION,
        created_at: now,
        created_by: actor.clone(),
        idempotency_key,
        fingerprint,
        status,
        risk: request.risk,
        work: WorkIdentity {
            episode_id: episode.episode_id.clone(),
            pack_id: pack.pack_id.clone(),
            pack_digest: pack.manifest.digest.clone(),
            pack_schema_version: pack.schema_version,
            query_id: pack.retrieval.query_id.clone(),
            task_id: episode.task_id.clone(),
            investigation: pack.investigation.clone(),
            outcome: episode.outcome,
            repository: pack.scope.repository.clone(),
            snapshots,
        },
        intent: request.intent.clone(),
        observed,
        governance: GovernanceRefs {
            zones: pack.scope.zones.clone(),
            flows: pack.scope.flows.clone(),
            policies,
            contracts: contracts(pack, sources),
            validation_results,
            approval_level,
            environment: pack.scope.environment.clone(),
            access_limitations: pack
                .scope
                .denied
                .iter()
                .map(|denial| format!("{}: {}", denial.resource_id, denial.reason))
                .collect(),
        },
        completeness,
        validation: IntakeValidationResult {
            status,
            profile_id: profile.profile_id.clone(),
            profile_version: profile.version,
            digest_verified: true,
            findings,
            references,
            validated_at: now,
        },
    })
}

/** The contract versions (registry generations) the pack's evidence was decided under
 * Input
    - pack: &EvidencePack - pack
    - sources: &Sources - sources
 * Output
    - Vec<String>
*/
fn contracts(pack: &EvidencePack, sources: &Sources) -> Vec<String> {
    pack.evidence
        .iter()
        .filter_map(|item| sources.capture.evidence.get(&item.evidence_id))
        .flat_map(|record| record.refs.contract_versions.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/** The findings of a validation profile over an intake
 * Input
    - profile: &ValidationProfile - profile
    - request: &IntakeRequest - request
    - completeness: &Completeness - completeness
 * Output
    - Vec<ValidationFinding>
*/
fn profile_findings(
    profile: &ValidationProfile,
    request: &IntakeRequest,
    completeness: &Completeness,
) -> Vec<ValidationFinding> {
    let mut findings = Vec::new();
    let intent = &request.intent;
    let error = |code: &str, message: String, refs: Vec<String>| {
        finding(code, Severity::Error, message, refs)
    };
    if profile.require_authoritative_objective
        && !intent
            .objective
            .as_ref()
            .is_some_and(|objective| objective.effective_mapping().authoritative())
    {
        findings.push(error(
            "unresolved_business_mapping",
            format!(
                "profile {} requires an explicitly mapped or human-confirmed business objective",
                profile.profile_id
            ),
            Vec::new(),
        ));
    }
    if profile.require_acceptance_criteria && intent.acceptance_criteria.is_empty() {
        findings.push(error(
            "missing_acceptance_criteria",
            format!(
                "profile {} requires acceptance criteria",
                profile.profile_id
            ),
            Vec::new(),
        ));
    }
    if profile.require_invariants {
        if intent.invariants.is_empty() {
            findings.push(error(
                "missing_business_invariants",
                format!(
                    "profile {} requires business invariants",
                    profile.profile_id
                ),
                Vec::new(),
            ));
        }
        let unmapped = intent
            .invariants
            .iter()
            .filter(|invariant| !invariant.effective_mapping().authoritative())
            .map(|invariant| invariant.id.clone())
            .collect::<Vec<_>>();
        if !unmapped.is_empty() {
            findings.push(error("unresolved_business_mapping", "business invariants must trace to an authoritative reference or approved declaration".into(), unmapped));
        }
    }
    if !profile.allow_missing_evidence && !completeness.missing.is_empty() {
        let types = completeness
            .missing
            .iter()
            .map(|missing| format!("{:?} ({:?})", missing.evidence_type, missing.state))
            .collect::<Vec<_>>();
        findings.push(error(
            "incomplete_evidence",
            format!("required evidence is missing: {}", types.join(", ")),
            Vec::new(),
        ));
    }
    if !completeness.conflicting_assertions.is_empty() {
        findings.push(error(
            "conflicting_assertions",
            "assertions with contradicting evidence must be resolved under an explicit rule".into(),
            completeness.conflicting_assertions.clone(),
        ));
    }
    if !completeness.unauthorized_sources.is_empty() {
        findings.push(error(
            "unauthorized_reference",
            "some referenced evidence is not accessible to the submitter".into(),
            completeness.unauthorized_sources.clone(),
        ));
    }
    let stale = completeness
        .stale_revisions
        .iter()
        .map(|check| format!("{}:{} ({:?})", check.kind, check.reference, check.outcome))
        .collect::<Vec<_>>();
    if !stale.is_empty() {
        findings.push(error("stale_reference", "zone, flow, or policy versions changed or cannot be verified since the evidence was collected".into(), stale));
    }
    if !completeness.unresolved_assumptions.is_empty() {
        findings.push(finding(
            "unresolved_assumptions",
            Severity::Warning,
            "assumptions remain unresolved".into(),
            completeness.unresolved_assumptions.clone(),
        ));
    }
    findings
}
