// The validation engine: versioned validation profiles and six separate gates, each producing a
// structured result (gate and version, status, findings with reasons and evidence references,
// method, timestamp, validator). Gates 1-5 run in a validation run; gate 6 (publication
// eligibility) runs at publication. A warning is never a pass: publication needs every warning
// acknowledged explicitly. Claim statuses written by the author are not trusted: gate 4
// recomputes each claim's effective status from the referenced evidence and assertions, so a
// hypothesis, an agent's statement, or CI alone cannot verify a business invariant. Each run
// records the basis it validated against (digests and versions), so a later change marks it stale.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::{
    CapabilityDefinition, ClaimKind, ClaimStatus, CreationMethod, DependencyKind, PackRef,
    ReviewLevel, RiskLevel, VerificationClaim, VersionConstraint, CAPABILITY_SCHEMA_VERSION,
    PROCEDURE_VERSION,
};
use super::refs::{RefOutcome, Sources};
use super::registry::RegistryState;
use super::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capture::correlate::assertions::{AssertionStatus, DerivationMethod};
use crate::capture::format::record::EvidenceType;
use crate::capture::resources::registry::valid_identifier;
use crate::capture::{stable_id, Actor, Truth};

/** Version of every gate's rules */
pub(crate) const GATE_VERSION: u32 = 1;

/** A validation profile: explicit, versioned requirements by purpose and risk
 * Fields
    - profile_id: String - identifier
    - version: u32 - version
    - description: String - purpose
    - max_risk: RiskLevel - the highest risk it may be used for
    - require_evidence_backed: bool - the definition must come from Work Capture evidence
    - require_authoritative_objective: bool - the business objective must be mapped or confirmed
    - require_acceptance_criteria: bool - acceptance criteria are required
    - require_invariants: bool - authoritative business invariants are required and verified
    - allow_missing_evidence: bool - missing required evidence may remain (as a limitation)
    - require_independent_publisher: bool - the publisher must not be the author
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ValidationProfile {
    pub(crate) profile_id: String,
    pub(crate) version: u32,
    pub(crate) description: String,
    pub(crate) max_risk: RiskLevel,
    pub(crate) require_evidence_backed: bool,
    pub(crate) require_authoritative_objective: bool,
    pub(crate) require_acceptance_criteria: bool,
    pub(crate) require_invariants: bool,
    pub(crate) allow_missing_evidence: bool,
    pub(crate) require_independent_publisher: bool,
}

/** The built-in profiles: "conservative" (the default, for any risk) and "low-risk-technical"
 * (technical acceptance criteria only, for low-risk capabilities)
 * Input
    - None
 * Output
    - Vec<ValidationProfile>
*/
pub(crate) fn profiles() -> Vec<ValidationProfile> {
    vec![
        ValidationProfile {
            profile_id: "conservative".into(),
            version: 1,
            description: "Default: evidence-backed, authoritative business objective and invariants, complete evidence".into(),
            max_risk: RiskLevel::Unknown,
            require_evidence_backed: true,
            require_authoritative_objective: true,
            require_acceptance_criteria: true,
            require_invariants: true,
            allow_missing_evidence: false,
            require_independent_publisher: true,
        },
        ValidationProfile {
            profile_id: "low-risk-technical".into(),
            version: 1,
            description: "Low-risk, read-only or reversible helpers: technical acceptance criteria; missing evidence must be declared as a limitation".into(),
            max_risk: RiskLevel::Low,
            require_evidence_backed: true,
            require_authoritative_objective: false,
            require_acceptance_criteria: true,
            require_invariants: false,
            allow_missing_evidence: true,
            require_independent_publisher: true,
        },
    ]
}

/** Select a profile: the named one, or the default for the risk; a profile weaker than the risk
 * requires is refused
 * Input
    - requested: Option<&str> - profile id, None for the default
    - risk: RiskLevel - risk
 * Output
    - CapabilityResult<ValidationProfile>
    - Error (InvalidSchema) for an unknown profile or one too weak for the risk
*/
pub(crate) fn select_profile(
    requested: Option<&str>,
    risk: RiskLevel,
) -> CapabilityResult<ValidationProfile> {
    let id = requested.unwrap_or("conservative");
    let profile = profiles()
        .into_iter()
        .find(|profile| profile.profile_id == id)
        .ok_or_else(|| {
            CapabilityError::new(
                ErrorCode::InvalidSchema,
                format!("unknown validation profile '{id}'"),
            )
        })?;
    if risk > profile.max_risk {
        return Err(CapabilityError::new(
            ErrorCode::InvalidSchema,
            format!("profile {id} cannot be used for {risk:?} risk"),
        ));
    }
    Ok(profile)
}

/** How serious a finding is
 * Variants
    - Error - the gate fails
    - Warning - the gate passes with a warning that must be acknowledged before publication
    - Info - informational
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
}

/** One finding
 * Fields
    - code: String - category
    - severity: Severity - error, warning, or info
    - message: String - reason
    - evidence_refs: Vec<String> - related evidence, assertions, or references
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ValidationFinding {
    pub(crate) code: String,
    pub(crate) severity: Severity,
    pub(crate) message: String,
    pub(crate) evidence_refs: Vec<String>,
}

/** The gates
 * Variants
    - Structural - schema, fields, enumerations, references, procedure structure
    - Provenance - intakes, packs, digests, lineage
    - Applicability - applicability, scope, exclusions, dependencies
    - Verification - claims and their evidence
    - Governance - Landscape and Crane references, approvals, restrictions
    - Publication - eligibility for publication
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GateId {
    Structural,
    Provenance,
    Applicability,
    Verification,
    Governance,
    Publication,
}

/** A gate's status
 * Variants
    - Passed - no error and no warning
    - Warning - no error, but warnings that must be acknowledged
    - Failed - at least one error
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GateStatus {
    Passed,
    Warning,
    Failed,
}

/** The result of one gate
 * Fields
    - gate: GateId - gate
    - gate_version: u32 - GATE_VERSION
    - status: GateStatus - status
    - findings: Vec<ValidationFinding> - findings
    - evidence_refs: Vec<String> - evidence the gate relied on
    - method: String - validation method
    - validated_at: u64 - Unix milliseconds
    - validator: Actor - who ran it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GateResult {
    pub(crate) gate: GateId,
    pub(crate) gate_version: u32,
    pub(crate) status: GateStatus,
    pub(crate) findings: Vec<ValidationFinding>,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) method: String,
    pub(crate) validated_at: u64,
    pub(crate) validator: Actor,
}

/** What a run validated against; a change to any of it makes the run stale
 * Fields
    - definition_digest: String - definition content
    - packs: Vec<PackRef> - Evidence Pack digests
    - governance: BTreeMap<String, String> - "zone:ID", "flow:ID", "policy:NAME" to the version seen
    - capabilities: BTreeMap<String, String> - dependency capability id to the version resolved
    - method: String - verification method and version
    - environments: Vec<String> - environments assumed
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ValidationBasis {
    pub(crate) definition_digest: String,
    pub(crate) packs: Vec<PackRef>,
    pub(crate) governance: BTreeMap<String, String>,
    pub(crate) capabilities: BTreeMap<String, String>,
    pub(crate) method: String,
    pub(crate) environments: Vec<String>,
}

/** The outcome of a validation run
 * Variants
    - Passed - gates 1-5 passed (possibly with warnings)
    - NeedsEvidence - evidence, references, or applicability information is missing
    - StructurallyInvalid - gate 1 failed
    - Failed - governance or another requirement failed
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunOutcome {
    Passed,
    NeedsEvidence,
    StructurallyInvalid,
    Failed,
}

/** A validation run
 * Fields
    - run_id: String - identifier
    - capability_id: String - capability
    - version: u32 - version
    - profile_id: String - profile
    - profile_version: u32 - profile version
    - gates: Vec<GateResult> - gate results 1-5
    - outcome: RunOutcome - outcome
    - basis: ValidationBasis - what it validated against
    - validator: Actor - who ran it
    - dry_run: bool - a dry run changes no lifecycle state
    - started_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ValidationRun {
    pub(crate) run_id: String,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) profile_id: String,
    pub(crate) profile_version: u32,
    pub(crate) gates: Vec<GateResult>,
    pub(crate) outcome: RunOutcome,
    pub(crate) basis: ValidationBasis,
    pub(crate) validator: Actor,
    pub(crate) dry_run: bool,
    pub(crate) started_at: u64,
}

impl ValidationRun {
    /** Every warning code a publisher must acknowledge
     * Input
        - None (uses self)
     * Output
        - BTreeSet<String>
    */
    pub(crate) fn warning_codes(&self) -> BTreeSet<String> {
        self.gates
            .iter()
            .flat_map(|gate| gate.findings.iter())
            .filter(|finding| finding.severity == Severity::Warning)
            .map(|finding| finding.code.clone())
            .collect()
    }
}

/** Collects the findings of one gate
 * Fields
    - findings: Vec<ValidationFinding> - findings
    - evidence: BTreeSet<String> - evidence relied on
*/
#[derive(Default)]
struct Gate {
    findings: Vec<ValidationFinding>,
    evidence: BTreeSet<String>,
}

impl Gate {
    /** Record an error
     * Input
        - code: &str - category
        - message: String - reason
        - refs: Vec<String> - references
     * Output
        - None
    */
    fn error(&mut self, code: &str, message: String, refs: Vec<String>) {
        self.findings.push(ValidationFinding {
            code: code.into(),
            severity: Severity::Error,
            message,
            evidence_refs: refs,
        });
    }

    /** Record a warning
     * Input
        - code: &str - category
        - message: String - reason
        - refs: Vec<String> - references
     * Output
        - None
    */
    fn warn(&mut self, code: &str, message: String, refs: Vec<String>) {
        self.findings.push(ValidationFinding {
            code: code.into(),
            severity: Severity::Warning,
            message,
            evidence_refs: refs,
        });
    }

    /** Finish the gate
     * Input
        - gate: GateId - gate
        - method: &str - method
        - validator: &Actor - validator
        - now: u64 - Unix milliseconds
     * Output
        - GateResult
    */
    fn finish(self, gate: GateId, method: &str, validator: &Actor, now: u64) -> GateResult {
        let status = if self
            .findings
            .iter()
            .any(|finding| finding.severity == Severity::Error)
        {
            GateStatus::Failed
        } else if self
            .findings
            .iter()
            .any(|finding| finding.severity == Severity::Warning)
        {
            GateStatus::Warning
        } else {
            GateStatus::Passed
        };
        GateResult {
            gate,
            gate_version: GATE_VERSION,
            status,
            findings: self.findings,
            evidence_refs: self.evidence.into_iter().collect(),
            method: method.into(),
            validated_at: now,
            validator: validator.clone(),
        }
    }
}

/** Report whether a value is a wildcard
 * Input
    - value: &str - value
 * Output
    - bool
*/
fn wildcard(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.is_empty()
        || lower.contains('*')
        || matches!(lower.as_str(), "any" | "all" | "every" | "universal")
}

/** Gate 1: structural validation
 * Input
    - definition: &CapabilityDefinition - definition
 * Output
    - Gate
*/
fn structural(definition: &CapabilityDefinition) -> Gate {
    let mut gate = Gate::default();
    if definition.schema_version != CAPABILITY_SCHEMA_VERSION {
        gate.error(
            "unsupported_schema_version",
            format!(
                "schema version {} is not supported",
                definition.schema_version
            ),
            Vec::new(),
        );
    }
    if !valid_identifier(&definition.capability_id) {
        gate.error(
            "invalid_identifier",
            format!(
                "capability id '{}' must be 1-64 lowercase letters, digits, '.', '_' or '-'",
                definition.capability_id
            ),
            Vec::new(),
        );
    }
    match (definition.version, definition.previous_version_ref) {
        (0, _) => gate.error("invalid_version", "versions start at 1".into(), Vec::new()),
        (1, Some(_)) => gate.error(
            "invalid_lineage",
            "version 1 has no predecessor".into(),
            Vec::new(),
        ),
        (version, previous) if version > 1 && previous != Some(version - 1) => gate.error(
            "invalid_lineage",
            format!(
                "version {version} must name version {} as its predecessor",
                version - 1
            ),
            Vec::new(),
        ),
        _ => {}
    }
    for (field, value) in [
        ("name", &definition.name),
        ("description", &definition.description),
        ("applicability.purpose", &definition.applicability.purpose),
        ("verification.method", &definition.verification.method),
    ] {
        if value.trim().is_empty() {
            gate.error("missing_field", format!("{field} is required"), Vec::new());
        }
    }
    if definition.owner_ref.is_none() {
        gate.warn("unknown_owner", "the owner is not known".into(), Vec::new());
    }
    let execution = &definition.execution;
    if execution.procedure_version != PROCEDURE_VERSION {
        gate.error(
            "unsupported_procedure_version",
            format!(
                "procedure version {} is not supported",
                execution.procedure_version
            ),
            Vec::new(),
        );
    }
    if execution.steps.is_empty() {
        gate.error(
            "invalid_procedure",
            "the procedure has no steps".into(),
            Vec::new(),
        );
    }
    let applicability = &definition.applicability;
    let conditions = applicability
        .preconditions
        .iter()
        .chain(&applicability.required_state)
        .chain(&applicability.assumptions)
        .map(|condition| condition.id.as_str())
        .collect::<BTreeSet<_>>();
    let claims = definition
        .verification
        .claims
        .iter()
        .map(|claim| claim.claim_id.as_str())
        .collect::<BTreeSet<_>>();
    if claims.len() != definition.verification.claims.len() {
        gate.error(
            "duplicate_claim",
            "claim ids must be unique".into(),
            Vec::new(),
        );
    }
    let invariants = definition
        .verification
        .invariants
        .iter()
        .map(|invariant| invariant.invariant_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut available = execution
        .inputs
        .iter()
        .chain(&applicability.inputs)
        .map(|input| input.name.clone())
        .collect::<BTreeSet<_>>();
    let mut steps = BTreeSet::new();
    for step in &execution.steps {
        if !valid_identifier(&step.step_id) || !steps.insert(step.step_id.clone()) {
            gate.error(
                "invalid_procedure",
                format!("step id '{}' is invalid or repeated", step.step_id),
                Vec::new(),
            );
        }
        if step.description.trim().is_empty() {
            gate.error(
                "invalid_procedure",
                format!("step '{}' has no description", step.step_id),
                Vec::new(),
            );
        }
        for input in step
            .inputs
            .iter()
            .filter(|input| !available.contains(*input))
        {
            gate.error(
                "invalid_procedure",
                format!(
                    "step '{}' uses '{input}', which no input or earlier step provides",
                    step.step_id
                ),
                Vec::new(),
            );
        }
        for condition in step
            .preconditions
            .iter()
            .filter(|condition| !conditions.contains(condition.as_str()))
        {
            gate.error(
                "invalid_procedure",
                format!(
                    "step '{}' names unknown precondition '{condition}'",
                    step.step_id
                ),
                Vec::new(),
            );
        }
        for claim in step
            .verifies
            .iter()
            .filter(|claim| !claims.contains(claim.as_str()))
        {
            gate.error(
                "invalid_procedure",
                format!("step '{}' verifies unknown claim '{claim}'", step.step_id),
                Vec::new(),
            );
        }
        available.extend(step.outputs.iter().cloned());
    }
    for condition in execution
        .preconditions
        .iter()
        .filter(|condition| !conditions.contains(condition.as_str()))
    {
        gate.error(
            "invalid_reference",
            format!("execution precondition '{condition}' is not declared in applicability"),
            Vec::new(),
        );
    }
    for checkpoint in execution
        .checkpoints
        .iter()
        .filter(|checkpoint| !claims.contains(checkpoint.as_str()))
    {
        gate.error(
            "invalid_reference",
            format!("checkpoint '{checkpoint}' is not a verification claim"),
            Vec::new(),
        );
    }
    for claim in &definition.verification.claims {
        if claim.statement.trim().is_empty() || claim.expected_result.trim().is_empty() {
            gate.error(
                "invalid_claim",
                format!(
                    "claim '{}' needs a statement and an expected result",
                    claim.claim_id
                ),
                Vec::new(),
            );
        }
        if let Some(invariant) = &claim.invariant_ref {
            if !invariants.contains(invariant.as_str()) {
                gate.error(
                    "invalid_claim",
                    format!(
                        "claim '{}' names unknown invariant '{invariant}'",
                        claim.claim_id
                    ),
                    Vec::new(),
                );
            }
        }
        if claim.kind == ClaimKind::BusinessInvariant && claim.invariant_ref.is_none() {
            gate.error(
                "invalid_claim",
                format!(
                    "business invariant claim '{}' must name its invariant",
                    claim.claim_id
                ),
                Vec::new(),
            );
        }
    }
    let mut dependencies = BTreeSet::new();
    for dependency in &definition.dependencies {
        if dependency.target_id.trim().is_empty() || wildcard(&dependency.target_id) {
            gate.error(
                "invalid_dependency",
                format!("{:?} dependency needs an explicit target", dependency.kind),
                Vec::new(),
            );
        }
        if !dependencies.insert((dependency.kind, dependency.target_id.clone())) {
            gate.error(
                "invalid_dependency",
                format!(
                    "{:?} {} is listed twice",
                    dependency.kind, dependency.target_id
                ),
                Vec::new(),
            );
        }
        if dependency.kind == DependencyKind::Capability
            && dependency.target_id == definition.capability_id
        {
            gate.error(
                "invalid_dependency",
                "a capability cannot depend on itself".into(),
                Vec::new(),
            );
        }
        if dependency.reason.trim().is_empty() {
            gate.error(
                "invalid_dependency",
                format!(
                    "{:?} {} needs a reason",
                    dependency.kind, dependency.target_id
                ),
                Vec::new(),
            );
        }
    }
    for exclusion in &applicability.exclusions {
        let contradicts = match exclusion.kind.as_str() {
            "environment" => applicability.environments.contains(&exclusion.value),
            "task_class" => applicability.task_classes.contains(&exclusion.value),
            "entity" | "condition" => false,
            other => {
                gate.error(
                    "invalid_exclusion",
                    format!("unknown exclusion kind '{other}'"),
                    Vec::new(),
                );
                false
            }
        };
        if contradicts {
            gate.error(
                "inconsistent_applicability",
                format!(
                    "{} '{}' is both applicable and excluded",
                    exclusion.kind, exclusion.value
                ),
                Vec::new(),
            );
        }
    }
    gate
}

/** Gate 2: provenance validation
 * Input
    - definition: &CapabilityDefinition - definition
    - profile: &ValidationProfile - profile
    - state: &RegistryState - registry (intakes)
    - sources: &Sources - canonical sources
 * Output
    - Gate
*/
fn provenance(
    definition: &CapabilityDefinition,
    profile: &ValidationProfile,
    state: &RegistryState,
    sources: &Sources,
) -> Gate {
    let mut gate = Gate::default();
    let provenance = &definition.provenance;
    if !provenance.evidence_backed() {
        let message = format!(
            "created {:?} without Work Capture evidence; it is not evidence-verified",
            provenance.creation_method
        );
        if profile.require_evidence_backed {
            gate.error("not_evidence_backed", message, Vec::new());
        } else {
            gate.warn("not_evidence_backed", message, Vec::new());
        }
    }
    if provenance.creation_method == CreationMethod::Compiler
        && provenance.source_intake_refs.is_empty()
    {
        gate.error(
            "compiler_without_intake",
            "a compiler-created definition must name the intake it was compiled from".into(),
            Vec::new(),
        );
    }
    let mut pack_evidence = BTreeSet::new();
    for intake_id in &provenance.source_intake_refs {
        let Some(intake) = state.intakes.get(intake_id) else {
            gate.error(
                "missing_source_reference",
                format!("intake {intake_id} does not exist"),
                vec![intake_id.clone()],
            );
            continue;
        };
        if intake.validation.profile_id != profile.profile_id {
            gate.error(
                "intake_profile_mismatch",
                format!(
                    "intake {intake_id} was validated under {}, not {}",
                    intake.validation.profile_id, profile.profile_id
                ),
                vec![intake_id.clone()],
            );
        }
        if intake.validation.status != super::intake::IntakeStatus::Ready {
            let reasons = intake
                .validation
                .findings
                .iter()
                .filter(|finding| finding.severity == Severity::Error)
                .map(|finding| finding.code.clone())
                .collect::<Vec<_>>();
            gate.error(
                "incomplete_intake",
                format!(
                    "intake {intake_id} is incomplete for {}: {}",
                    profile.profile_id,
                    reasons.join(", ")
                ),
                vec![intake_id.clone()],
            );
        }
        if !provenance
            .source_episode_refs
            .contains(&intake.work.episode_id)
        {
            gate.error(
                "inconsistent_lineage",
                format!(
                    "intake {intake_id} comes from episode {}, which the definition does not list",
                    intake.work.episode_id
                ),
                vec![intake_id.clone()],
            );
        }
        if !provenance.source_evidence_pack_refs.iter().any(|pack| {
            pack.pack_id == intake.work.pack_id && pack.manifest_digest == intake.work.pack_digest
        }) {
            gate.error(
                "inconsistent_lineage",
                format!(
                    "intake {intake_id} references pack {}#{}, which the definition does not list",
                    intake.work.pack_id, intake.work.pack_digest
                ),
                vec![intake_id.clone()],
            );
        }
    }
    for reference in &provenance.source_evidence_pack_refs {
        match sources.capture.packs.get(&reference.pack_id) {
            None => gate.error(
                "missing_source_reference",
                format!("Evidence Pack {} does not exist", reference.pack_id),
                vec![reference.pack_id.clone()],
            ),
            Some(pack)
                if pack.manifest.digest != reference.manifest_digest
                    || pack.verify_manifest().is_err() =>
            {
                gate.error(
                    "integrity_mismatch",
                    format!(
                        "Evidence Pack {} does not match digest {}",
                        reference.pack_id, reference.manifest_digest
                    ),
                    vec![reference.pack_id.clone()],
                )
            }
            Some(pack) => {
                gate.evidence.insert(reference.pack_id.clone());
                pack_evidence.extend(pack.evidence.iter().map(|item| item.evidence_id.clone()));
                if !provenance
                    .source_episode_refs
                    .iter()
                    .any(|episode| pack.episode_id.as_deref() == Some(episode.as_str()))
                {
                    gate.error(
                        "inconsistent_lineage",
                        format!(
                            "Evidence Pack {} belongs to an episode the definition does not list",
                            reference.pack_id
                        ),
                        vec![reference.pack_id.clone()],
                    );
                }
            }
        }
    }
    for episode in &provenance.source_episode_refs {
        if !sources.capture.episodes.contains_key(episode) {
            gate.error(
                "missing_source_reference",
                format!("work episode {episode} does not exist"),
                vec![episode.clone()],
            );
        }
    }
    if provenance.evidence_backed() {
        let outside = definition
            .verification
            .claims
            .iter()
            .flat_map(|claim| claim.evidence_refs.iter())
            .filter(|id| !pack_evidence.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        if !outside.is_empty() {
            gate.error(
                "evidence_outside_provenance",
                "claims cite evidence that is not part of the declared Evidence Packs".into(),
                outside,
            );
        }
    }
    gate
}

/** Collect the capability dependency edges of the registry plus a definition
 * Input
    - definition: &CapabilityDefinition - definition being validated
    - state: &RegistryState - registry
 * Output
    - BTreeMap<String, BTreeSet<String>> capability id to the capabilities it requires
*/
fn capability_edges(
    definition: &CapabilityDefinition,
    state: &RegistryState,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for record in state
        .versions
        .values()
        .flat_map(|versions| versions.values())
    {
        if record.definition.capability_id == definition.capability_id
            || !record.state.selectable_for_dependencies()
        {
            continue;
        }
        let targets = record
            .definition
            .dependencies
            .iter()
            .filter(|dependency| {
                dependency.kind == DependencyKind::Capability && dependency.required
            })
            .map(|dependency| dependency.target_id.clone());
        edges
            .entry(record.definition.capability_id.clone())
            .or_default()
            .extend(targets);
    }
    let own = definition
        .dependencies
        .iter()
        .filter(|dependency| dependency.kind == DependencyKind::Capability && dependency.required)
        .map(|dependency| dependency.target_id.clone());
    edges
        .entry(definition.capability_id.clone())
        .or_default()
        .extend(own);
    edges
}

/** Find a dependency cycle through a capability (required capability dependencies must be acyclic,
 * because resolving them is recursive)
 * Input
    - start: &str - capability
    - edges: &BTreeMap<String, BTreeSet<String>> - required capability dependencies
 * Output
    - Option<Vec<String>> the cycle, starting and ending at start
*/
pub(crate) fn find_cycle(
    start: &str,
    edges: &BTreeMap<String, BTreeSet<String>>,
) -> Option<Vec<String>> {
    let mut stack = vec![(start.to_string(), vec![start.to_string()])];
    let mut seen = BTreeSet::new();
    while let Some((node, path)) = stack.pop() {
        for next in edges.get(&node).into_iter().flatten() {
            if next == start {
                let mut cycle = path.clone();
                cycle.push(start.into());
                return Some(cycle);
            }
            if seen.insert(next.clone()) {
                let mut extended = path.clone();
                extended.push(next.clone());
                stack.push((next.clone(), extended));
            }
        }
    }
    None
}

/** Gate 3: applicability and dependency validation
 * Input
    - definition: &CapabilityDefinition - definition
    - state: &RegistryState - registry
    - sources: &Sources - canonical sources
    - basis: &mut ValidationBasis - basis to record versions in
 * Output
    - Gate
*/
fn applicability(
    definition: &CapabilityDefinition,
    state: &RegistryState,
    sources: &Sources,
    basis: &mut ValidationBasis,
) -> Gate {
    let mut gate = Gate::default();
    let contract = &definition.applicability;
    if contract.task_classes.is_empty() {
        gate.error(
            "missing_applicability",
            "supported task classes are required".into(),
            Vec::new(),
        );
    }
    if contract.environments.is_empty() {
        gate.error("missing_applicability", "applicable environments are required; an empty list means none is known, not every environment".into(), Vec::new());
    }
    for value in contract.task_classes.iter().chain(&contract.environments) {
        if wildcard(value) {
            gate.error(
                "universal_applicability",
                format!("'{value}' is a wildcard; applicability must be explicit"),
                Vec::new(),
            );
        }
    }
    let unverified = contract
        .environments
        .iter()
        .filter(|environment| {
            !definition
                .verification
                .scope_environments
                .contains(environment)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !unverified.is_empty() {
        gate.error(
            "applicability_beyond_verification",
            format!(
                "applicable in environments it was never verified in: {}",
                unverified.join(", ")
            ),
            Vec::new(),
        );
    }
    if contract.risk == RiskLevel::Unknown {
        gate.error(
            "unassessed_risk",
            "the risk is unknown; assess it before validation".into(),
            Vec::new(),
        );
    }
    for condition in contract
        .preconditions
        .iter()
        .chain(&contract.required_state)
    {
        if condition.status == ClaimStatus::Unresolved {
            gate.error(
                "unresolved_precondition",
                format!("precondition '{}' is unresolved", condition.id),
                Vec::new(),
            );
        }
    }
    for assumption in contract
        .assumptions
        .iter()
        .filter(|assumption| assumption.status != ClaimStatus::Verified)
    {
        gate.warn(
            "unverified_assumption",
            format!("assumption '{}' is {:?}", assumption.id, assumption.status),
            Vec::new(),
        );
    }
    let edges = capability_edges(definition, state);
    if let Some(cycle) = find_cycle(&definition.capability_id, &edges) {
        gate.error(
            "dependency_cycle",
            format!(
                "required capability dependencies form a cycle: {}",
                cycle.join(" -> ")
            ),
            cycle,
        );
    }
    for dependency in &definition.dependencies {
        let label = format!("{:?} {}", dependency.kind, dependency.target_id);
        let problem = |gate: &mut Gate, code: &str, message: String| {
            if dependency.required {
                gate.error(code, message, vec![dependency.target_id.clone()]);
            } else {
                gate.warn(
                    code,
                    format!("optional: {message}"),
                    vec![dependency.target_id.clone()],
                );
            }
        };
        if dependency.required
            && dependency.kind.needs_pin()
            && dependency.constraint == VersionConstraint::Unpinned
        {
            gate.error(
                "unpinned_dependency",
                format!("{label} is required and must be pinned to a version"),
                vec![dependency.target_id.clone()],
            );
            continue;
        }
        match dependency.kind {
            DependencyKind::Zone | DependencyKind::Flow | DependencyKind::Policy => {
                let kind = match dependency.kind {
                    DependencyKind::Zone => "zone",
                    DependencyKind::Flow => "flow",
                    _ => "policy",
                };
                let version = match &dependency.constraint {
                    VersionConstraint::Exact(version) => Some(version.as_str()),
                    _ => None,
                };
                let check = sources.current.check(kind, &dependency.target_id, version);
                basis.governance.insert(format!("{kind}:{}", dependency.target_id), version.unwrap_or("unknown").to_string());
                match check.outcome {
                    RefOutcome::Resolved => {}
                    RefOutcome::Missing => problem(&mut gate, "missing_dependency", format!("{label} does not exist")),
                    outcome => problem(&mut gate, "stale_dependency", format!("{label} is {outcome:?}: {}", check.detail)),
                }
            }
            DependencyKind::Capability => {
                let resolved = state.versions.get(&dependency.target_id).and_then(|versions| {
                    versions.values().rev().find(|record| record.state.selectable_for_dependencies() && dependency.constraint.admits(&record.definition.version.to_string()))
                });
                match resolved {
                    Some(record) => {
                        basis.capabilities.insert(dependency.target_id.clone(), record.definition.version.to_string());
                    }
                    None => problem(&mut gate, "missing_dependency", format!("no usable version of capability {} satisfies {:?}", dependency.target_id, dependency.constraint)),
                }
            }
            DependencyKind::EvidencePack => match (sources.capture.packs.get(&dependency.target_id), &dependency.constraint) {
                (Some(pack), VersionConstraint::Exact(digest)) if &pack.manifest.digest == digest => {}
                (Some(_), _) => problem(&mut gate, "stale_dependency", format!("{label} does not match its pinned digest")),
                (None, _) => problem(&mut gate, "missing_dependency", format!("{label} does not exist")),
            },
            _ => gate.warn("unresolvable_dependency", format!("{label} is declared but this registry cannot resolve its kind; it is not verified"), vec![dependency.target_id.clone()]),
        }
    }
    gate
}

/** Recompute a claim's effective status from its references; returns the status and the
 * reasons it is not verified
 * Input
    - definition: &CapabilityDefinition - definition
    - claim: &VerificationClaim - claim
    - sources: &Sources - canonical sources
 * Output
    - (ClaimStatus, Vec<String>, Vec<String>) status, reasons, evidence relied on
*/
pub(crate) fn effective_claim(
    definition: &CapabilityDefinition,
    claim: &VerificationClaim,
    sources: &Sources,
) -> (ClaimStatus, Vec<String>, Vec<String>) {
    let mut reasons = Vec::new();
    let mut relied = Vec::new();
    if claim.status != ClaimStatus::Verified {
        return (
            claim.status,
            vec![format!("declared {:?}, not verified", claim.status)],
            relied,
        );
    }
    if claim
        .method
        .as_deref()
        .is_none_or(|method| method.trim().is_empty())
    {
        reasons.push("no verification method".into());
    }
    match &claim.environment {
        None => reasons.push("the verification environment is unknown".into()),
        Some(environment)
            if !definition
                .verification
                .scope_environments
                .contains(environment) =>
        {
            reasons.push(format!(
                "verified in {environment}, outside the verification scope"
            ))
        }
        _ => {}
    }
    if claim.evidence_refs.is_empty() && claim.assertion_refs.is_empty() {
        reasons.push("no evidence or assertion supports it".into());
    }
    let mut types = BTreeSet::new();
    for id in &claim.evidence_refs {
        let (check, record) = sources.evidence(id);
        match record {
            Some(record) => {
                types.insert(record.evidence_type);
                relied.push(id.clone());
            }
            None => reasons.push(format!(
                "evidence {id} is {:?}: {}",
                check.outcome, check.detail
            )),
        }
    }
    for id in &claim.assertion_refs {
        match sources.capture.assertions.get(id) {
            None => reasons.push(format!("assertion {id} does not exist")),
            Some(assertion) if assertion.derivation.method == DerivationMethod::AgentHypothesis => {
                reasons.push(format!(
                    "assertion {id} is a hypothesis; a hypothesis cannot verify a claim"
                ))
            }
            Some(assertion)
                if assertion.conflicted() || assertion.status != AssertionStatus::Supported =>
            {
                reasons.push(format!(
                    "assertion {id} is {:?}, not supported",
                    assertion.status
                ))
            }
            Some(assertion) if assertion.truth() == Truth::Unknown => {
                reasons.push(format!("assertion {id} establishes nothing (unknown)"))
            }
            Some(assertion) => {
                relied.push(id.clone());
                for evidence in &assertion.supporting {
                    if let Some(record) = sources.capture.evidence.get(evidence) {
                        types.insert(record.evidence_type);
                    }
                }
            }
        }
    }
    if claim.kind == ClaimKind::BusinessInvariant {
        let invariant = claim.invariant_ref.as_ref().and_then(|id| {
            definition
                .verification
                .invariants
                .iter()
                .find(|invariant| &invariant.invariant_id == id)
        });
        match invariant {
            Some(invariant)
                if !invariant.mapping.authoritative() || invariant.authority.is_none() =>
            {
                reasons.push(format!(
                    "invariant {} has no authoritative reference",
                    invariant.invariant_id
                ))
            }
            None => reasons.push("the invariant it verifies is not declared".into()),
            _ => {}
        }
        let technical = [
            EvidenceType::CiRun,
            EvidenceType::TestReport,
            EvidenceType::CodeChange,
            EvidenceType::PullRequest,
            EvidenceType::ToolCall,
            EvidenceType::AgentEvent,
            EvidenceType::AgentSession,
        ];
        if types.iter().all(|kind| technical.contains(kind)) {
            reasons.push("technical evidence (CI, tests, code, tool calls) alone does not verify a business invariant".into());
        }
    }
    if reasons.is_empty() {
        (ClaimStatus::Verified, reasons, relied)
    } else {
        (ClaimStatus::Unresolved, reasons, relied)
    }
}

/** Gate 4: verification validation
 * Input
    - definition: &CapabilityDefinition - definition
    - profile: &ValidationProfile - profile
    - sources: &Sources - canonical sources
 * Output
    - Gate
*/
fn verification(
    definition: &CapabilityDefinition,
    profile: &ValidationProfile,
    sources: &Sources,
) -> Gate {
    let mut gate = Gate::default();
    let contract = &definition.verification;
    if contract
        .claims
        .iter()
        .filter(|claim| claim.required)
        .count()
        == 0
    {
        gate.error(
            "missing_verification",
            "at least one required verification claim is needed".into(),
            Vec::new(),
        );
    }
    for claim in &contract.claims {
        let (status, reasons, relied) = effective_claim(definition, claim, sources);
        gate.evidence.extend(relied.iter().cloned());
        if status == ClaimStatus::Verified {
            continue;
        }
        let mut refs = claim.evidence_refs.clone();
        refs.extend(claim.assertion_refs.iter().cloned());
        let message = format!(
            "claim '{}' is {status:?}: {}",
            claim.claim_id,
            reasons.join("; ")
        );
        if claim.required {
            gate.error("unverified_claim", message, refs);
        } else {
            gate.warn("unverified_claim", message, refs);
        }
    }
    let mut invariant_claims = BTreeSet::new();
    for claim in contract
        .claims
        .iter()
        .filter(|claim| claim.kind == ClaimKind::BusinessInvariant)
    {
        invariant_claims.extend(claim.invariant_ref.iter().cloned());
    }
    for invariant in contract
        .invariants
        .iter()
        .filter(|invariant| invariant.required)
    {
        if !invariant_claims.contains(&invariant.invariant_id) {
            gate.error(
                "unverified_invariant",
                format!(
                    "required invariant '{}' has no verification claim",
                    invariant.invariant_id
                ),
                Vec::new(),
            );
        }
    }
    if profile.require_invariants
        && contract
            .invariants
            .iter()
            .all(|invariant| !invariant.required)
    {
        gate.error(
            "missing_business_invariants",
            format!(
                "profile {} requires at least one required business invariant",
                profile.profile_id
            ),
            Vec::new(),
        );
    }
    for assumption in &contract.unresolved_assumptions {
        gate.warn(
            "unresolved_assumption",
            format!("unresolved assumption: {assumption}"),
            Vec::new(),
        );
    }
    gate
}

/** Report whether text describes changing Crane/AgentScript policy or governance files
 * Input
    - text: &str - text
 * Output
    - bool
*/
pub(crate) fn touches_policy(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains(".crane/")
        || lower.contains("crane policy")
        || lower.contains("agentscript policy")
        || lower.contains("governance.yaml")
        || lower.contains("relax policy")
        || lower.contains("disable policy")
}

/** Gate 5: governance validation
 * Input
    - definition: &CapabilityDefinition - definition
    - profile: &ValidationProfile - profile
    - state: &RegistryState - registry (intakes)
 * Output
    - Gate
*/
fn governance(
    definition: &CapabilityDefinition,
    profile: &ValidationProfile,
    state: &RegistryState,
) -> Gate {
    let mut gate = Gate::default();
    let applicability = &definition.applicability;
    if applicability.risk > profile.max_risk {
        gate.error(
            "profile_too_weak",
            format!(
                "profile {} cannot govern {:?} risk",
                profile.profile_id, applicability.risk
            ),
            Vec::new(),
        );
    }
    let minimum = match applicability.risk {
        RiskLevel::Low => ReviewLevel::Peer,
        RiskLevel::Medium => ReviewLevel::Owner,
        _ => ReviewLevel::Admin,
    };
    if applicability.review_level < minimum {
        gate.error(
            "insufficient_review",
            format!(
                "{:?} risk needs at least {minimum:?} review",
                applicability.risk
            ),
            Vec::new(),
        );
    }
    for step in &definition.execution.steps {
        if touches_policy(&step.description) {
            gate.error("policy_change", format!("step '{}' changes Crane policy or governance; that goes through Crane's signed workflow, not a capability", step.step_id), Vec::new());
        }
        if applicability.risk == RiskLevel::Low && step.kind.mutating() {
            gate.error(
                "risk_mismatch",
                format!(
                    "step '{}' changes state, which a low-risk capability cannot do",
                    step.step_id
                ),
                Vec::new(),
            );
        }
    }
    for effect in &definition.execution.side_effects {
        if touches_policy(&effect.description)
            || matches!(
                effect.system.to_ascii_lowercase().as_str(),
                "crane" | "agentscript"
            )
        {
            gate.error(
                "policy_change",
                format!(
                    "side effect on {} would change policy outside Crane's signed workflow",
                    effect.system
                ),
                Vec::new(),
            );
        }
        if applicability.risk == RiskLevel::Low && effect.reversible != Some(true) {
            gate.error("risk_mismatch", format!("side effect on {} is not known to be reversible, which a low-risk capability requires", effect.system), Vec::new());
        }
        if effect.risk > applicability.risk {
            gate.error(
                "risk_mismatch",
                format!(
                    "side effect on {} is riskier ({:?}) than the capability ({:?})",
                    effect.system, effect.risk, applicability.risk
                ),
                Vec::new(),
            );
        }
    }
    let declared = definition
        .dependencies
        .iter()
        .map(|dependency| (dependency.kind, dependency.target_id.clone()))
        .collect::<BTreeSet<_>>();
    for intake in definition
        .provenance
        .source_intake_refs
        .iter()
        .filter_map(|id| state.intakes.get(id))
    {
        let governed = intake
            .governance
            .zones
            .iter()
            .map(|zone| (DependencyKind::Zone, zone.id.clone()))
            .chain(
                intake
                    .governance
                    .flows
                    .iter()
                    .map(|flow| (DependencyKind::Flow, flow.id.clone())),
            )
            .chain(
                intake
                    .governance
                    .policies
                    .iter()
                    .map(|policy| (DependencyKind::Policy, policy.id.clone())),
            );
        for (kind, id) in governed {
            if !declared.contains(&(kind, id.clone())) {
                gate.warn("undeclared_governance", format!("the evidence ran under {kind:?} {id}, which the definition does not declare as a dependency"), vec![id]);
            }
        }
    }
    gate
}

/** Run gates 1-5 over a definition
 * Input
    - definition: &CapabilityDefinition - definition
    - profile: &ValidationProfile - profile
    - state: &RegistryState - registry
    - sources: &Sources - canonical sources
    - validator: &Actor - validator
    - dry_run: bool - whether lifecycle state will change
    - now: u64 - Unix milliseconds
 * Output
    - ValidationRun
*/
pub(crate) fn run(
    definition: &CapabilityDefinition,
    profile: &ValidationProfile,
    state: &RegistryState,
    sources: &Sources,
    validator: &Actor,
    dry_run: bool,
    now: u64,
) -> ValidationRun {
    let mut basis = ValidationBasis {
        definition_digest: definition.digest(),
        packs: definition.provenance.source_evidence_pack_refs.clone(),
        method: format!(
            "{}@{}",
            definition.verification.method, definition.verification.method_version
        ),
        environments: definition.applicability.environments.clone(),
        ..ValidationBasis::default()
    };
    let method = format!("capability-gates/v{GATE_VERSION}");
    let gates = vec![
        structural(definition).finish(GateId::Structural, &method, validator, now),
        provenance(definition, profile, state, sources).finish(
            GateId::Provenance,
            &method,
            validator,
            now,
        ),
        applicability(definition, state, sources, &mut basis).finish(
            GateId::Applicability,
            &method,
            validator,
            now,
        ),
        verification(definition, profile, sources).finish(
            GateId::Verification,
            &method,
            validator,
            now,
        ),
        governance(definition, profile, state).finish(GateId::Governance, &method, validator, now),
    ];
    let failed = |id: GateId| {
        gates
            .iter()
            .any(|gate| gate.gate == id && gate.status == GateStatus::Failed)
    };
    let outcome = if failed(GateId::Structural) {
        RunOutcome::StructurallyInvalid
    } else if failed(GateId::Governance) {
        RunOutcome::Failed
    } else if failed(GateId::Provenance)
        || failed(GateId::Applicability)
        || failed(GateId::Verification)
    {
        RunOutcome::NeedsEvidence
    } else {
        RunOutcome::Passed
    };
    let started = now.to_string();
    let version = definition.version.to_string();
    ValidationRun {
        run_id: stable_id(
            "vrn",
            &[
                &definition.capability_id,
                &version,
                &basis.definition_digest,
                &started,
                &validator.id,
            ],
        ),
        capability_id: definition.capability_id.clone(),
        version: definition.version,
        profile_id: profile.profile_id.clone(),
        profile_version: profile.version,
        gates,
        outcome,
        basis,
        validator: validator.clone(),
        dry_run,
        started_at: now,
    }
}
