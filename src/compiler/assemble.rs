// Candidate assembly and the compilation report. compile() runs the stages in order and always
// returns a report with an explicit outcome:
//
//   compilation_failed           - the request is invalid, or the assembled definition fails
//                                  Component 2's structural gate (nothing is stored)
//   compilation_blocked          - authorization, integrity, references, or a policy change
//                                  observed in the episode prevent compilation (nothing stored)
//   insufficient_reusable_signal - the episode failed, or shows no reusable action (nothing stored)
//   needs_more_evidence          - a candidate is stored, but blocking gaps remain
//   candidate_generated          - a candidate is stored and every required claim maps to evidence
//
// The intake goes through Component 2's submit_intake, the definition is checked with Component
// 2's gates (a dry run, which records nothing) and stored with create_candidate or propose_version
// as a candidate. Compiling the same inputs again reuses the stored version instead of adding one.
// The report records versions, the pack digest, source references, the extraction method, the
// candidate digest, the basis of every claim, step, and element, gaps, warnings, unresolved
// questions, and lineage; its own digest makes it tamper-evident.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::context::{self, ElementBasis};
use super::extract::{self, BranchNote, ProposalDecision, StepBasis};
use super::reconstruct::{self, Reconstruction};
use super::verify::{self, ClaimBasis, Gap};
use super::{Basis, CompilationOutcome, CompileRequest, COMPILER_VERSION, REPORT_SCHEMA_VERSION};
use crate::capability::intake::{CapabilityIntake, INTAKE_SCHEMA_VERSION};
use crate::capability::model::{
    CapabilityDefinition, CreationMethod, ExecutionContract, PackRef, ParamKind, Parameter,
    Provenance, VerificationContract, CAPABILITY_SCHEMA_VERSION, PROCEDURE_VERSION,
};
use crate::capability::refs::Sources;
use crate::capability::registry::CapabilityRegistry;
use crate::capability::validate::{self, select_profile, GateId, GateStatus, ValidationRun};
use crate::capability::ErrorCode;
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::contributions::ContributionType;
use crate::capture::resources::registry::valid_identifier;
use crate::capture::{stable_id, Actor};
use crate::trust::crypto::digest_json;

/** The candidate a compilation stored
 * Fields
    - capability_id: String - capability
    - version: u32 - version
    - digest: String - definition digest
    - reused: bool - the same definition was already stored by an earlier compilation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CandidateRef {
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) reused: bool,
}

/** Where the candidate came from
 * Fields
    - intake_id: String - Component 2 intake
    - idempotency_key: String - its idempotency key
    - episode_id: String - work episode
    - pack_id: String - Evidence Pack
    - pack_digest: String - its verified manifest digest
    - query_id: String - the Work Capture investigation
    - recipe: Option<String> - its recipe id, version, and digest
    - source_refs: Vec<String> - evidence and assertions the candidate relies on
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Lineage {
    pub(crate) intake_id: String,
    pub(crate) idempotency_key: String,
    pub(crate) episode_id: String,
    pub(crate) pack_id: String,
    pub(crate) pack_digest: String,
    pub(crate) query_id: String,
    pub(crate) recipe: Option<String>,
    pub(crate) source_refs: Vec<String>,
}

/** The compilation report (separate from the candidate)
 * Fields
    - report_id: String - stable identifier (from the digest)
    - report_schema_version: u32 - REPORT_SCHEMA_VERSION
    - compiler_version: String - COMPILER_VERSION
    - capability_schema_version: u32 - Capability schema the candidate uses
    - intake_schema_version: u32 - intake schema used
    - outcome: CompilationOutcome - outcome
    - reasons: Vec<String> - why this outcome
    - actor: Actor - who compiled
    - capability_id: String - capability requested
    - extraction_method: String - how steps were extracted
    - lineage: Option<Lineage> - source lineage (None when blocked before the intake resolved)
    - candidate: Option<CandidateRef> - stored candidate
    - reconstruction: Option<Reconstruction> - reconstructed episode
    - step_bases: Vec<StepBasis> - support of each step
    - claim_bases: Vec<ClaimBasis> - support of each claim
    - element_bases: Vec<ElementBasis> - support of each applicability element
    - branches: Vec<BranchNote> - branch notes for review
    - proposals: Vec<ProposalDecision> - decisions on model proposals
    - hypotheses: Vec<String> - hypothesis assertions and steps (never claims)
    - gaps: Vec<Gap> - gaps
    - warnings: Vec<String> - warnings
    - unresolved_questions: Vec<String> - questions to resolve
    - validation: Option<ValidationRun> - Component 2's dry-run result (recorded nowhere else)
    - created_at: u64 - Unix milliseconds
    - digest: String - digest of the report without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompilationReport {
    pub(crate) report_id: String,
    pub(crate) report_schema_version: u32,
    pub(crate) compiler_version: String,
    pub(crate) capability_schema_version: u32,
    pub(crate) intake_schema_version: u32,
    pub(crate) outcome: CompilationOutcome,
    pub(crate) reasons: Vec<String>,
    pub(crate) actor: Actor,
    pub(crate) capability_id: String,
    pub(crate) extraction_method: String,
    pub(crate) lineage: Option<Lineage>,
    pub(crate) candidate: Option<CandidateRef>,
    pub(crate) reconstruction: Option<Reconstruction>,
    pub(crate) step_bases: Vec<StepBasis>,
    pub(crate) claim_bases: Vec<ClaimBasis>,
    pub(crate) element_bases: Vec<ElementBasis>,
    pub(crate) branches: Vec<BranchNote>,
    pub(crate) proposals: Vec<ProposalDecision>,
    pub(crate) hypotheses: Vec<String>,
    pub(crate) gaps: Vec<Gap>,
    pub(crate) warnings: Vec<String>,
    pub(crate) unresolved_questions: Vec<String>,
    pub(crate) validation: Option<ValidationRun>,
    pub(crate) created_at: u64,
    pub(crate) digest: String,
}

impl CompilationReport {
    /** An empty report for a request
     * Input
        - request: &CompileRequest - request
        - actor: &Actor - compiler actor
        - now: u64 - Unix milliseconds
     * Output
        - CompilationReport
    */
    fn new(request: &CompileRequest, actor: &Actor, now: u64) -> Self {
        Self {
            report_id: String::new(),
            report_schema_version: REPORT_SCHEMA_VERSION,
            compiler_version: COMPILER_VERSION.into(),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION,
            intake_schema_version: INTAKE_SCHEMA_VERSION,
            outcome: CompilationOutcome::CompilationFailed,
            reasons: Vec::new(),
            actor: actor.clone(),
            capability_id: request.capability_id.clone(),
            extraction_method: format!(
                "deterministic-rules@{COMPILER_VERSION}{}",
                if request.proposals.is_empty() {
                    ""
                } else {
                    "+validated-model-proposals"
                }
            ),
            lineage: None,
            candidate: None,
            reconstruction: None,
            step_bases: Vec::new(),
            claim_bases: Vec::new(),
            element_bases: Vec::new(),
            branches: Vec::new(),
            proposals: Vec::new(),
            hypotheses: Vec::new(),
            gaps: Vec::new(),
            warnings: Vec::new(),
            unresolved_questions: Vec::new(),
            validation: None,
            created_at: now,
            digest: String::new(),
        }
    }

    /** Set the outcome, compute the digest and identifier, and return the report
     * Input
        - outcome: CompilationOutcome - outcome
        - reasons: Vec<String> - why
     * Output
        - CompilationReport
    */
    fn finish(mut self, outcome: CompilationOutcome, reasons: Vec<String>) -> Self {
        self.outcome = outcome;
        self.reasons.extend(reasons);
        self.digest = String::new();
        self.report_id = String::new();
        let digest = digest_json("compiler/report/v1", &json!(self))[..32].to_string();
        self.report_id = stable_id("cmp", &[&digest]);
        self.digest = digest;
        self
    }

    /** Check that the report's content still matches its digest and identifier
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn verify_digest(&self) -> bool {
        let mut content = self.clone();
        content.digest = String::new();
        content.report_id = String::new();
        let digest = digest_json("compiler/report/v1", &json!(content))[..32].to_string();
        digest == self.digest && self.report_id == stable_id("cmp", &[&digest])
    }
}

/** Check a request's own fields before anything is touched
 * Input
    - request: &CompileRequest - request
 * Output
    - Vec<String> problems, empty when valid
*/
fn check_request(request: &CompileRequest) -> Vec<String> {
    let mut problems = Vec::new();
    if !valid_identifier(&request.capability_id) {
        problems.push(format!(
            "capability id '{}' must be 1-64 lowercase letters, digits, '.', '_' or '-'",
            request.capability_id
        ));
    }
    for (field, value) in [
        ("name", &request.name),
        ("purpose", &request.purpose),
        ("task_class", &request.task_class),
    ] {
        let lower = value.trim().to_ascii_lowercase();
        if lower.is_empty()
            || lower.contains('*')
            || matches!(lower.as_str(), "any" | "all" | "every" | "universal")
        {
            problems.push(format!("{field} must be explicit and non-empty"));
        }
    }
    problems
}

/** Build the definition for a version
 * Input
    - parts: &Parts - assembled sections
    - actor: &Actor - author
    - version: u32 - version
    - previous: Option<u32> - predecessor
    - change_reason: Option<String> - change reason
 * Output
    - CapabilityDefinition
*/
fn definition(
    parts: &Parts,
    actor: &Actor,
    version: u32,
    previous: Option<u32>,
    change_reason: Option<String>,
) -> CapabilityDefinition {
    let intake = &parts.intake;
    CapabilityDefinition {
        capability_id: parts.request.capability_id.clone(),
        version,
        schema_version: CAPABILITY_SCHEMA_VERSION,
        name: parts.request.name.clone(),
        description: parts.request.purpose.clone(),
        owner_ref: intake.intent.owner.clone(),
        previous_version_ref: previous,
        provenance: Provenance {
            creation_method: CreationMethod::Compiler,
            created_by: actor.clone(),
            created_at: intake.created_at,
            source_intake_refs: vec![intake.intake_id.clone()],
            source_episode_refs: vec![intake.work.episode_id.clone()],
            source_evidence_pack_refs: vec![PackRef {
                pack_id: intake.work.pack_id.clone(),
                manifest_digest: intake.work.pack_digest.clone(),
            }],
            change_reason,
        },
        applicability: parts.context.applicability.clone(),
        dependencies: parts.context.dependencies.clone(),
        execution: ExecutionContract {
            procedure_version: PROCEDURE_VERSION,
            inputs: parts.extraction.inputs.clone(),
            preconditions: parts
                .context
                .applicability
                .preconditions
                .iter()
                .map(|condition| condition.id.clone())
                .collect(),
            steps: parts.extraction.steps.clone(),
            effects: parts.extraction.effects.clone(),
            state_transitions: parts.extraction.state_transitions.clone(),
            postconditions: parts
                .verification
                .claims
                .iter()
                .filter(|claim| claim.required)
                .map(|claim| claim.statement.clone())
                .collect(),
            outputs: parts
                .extraction
                .steps
                .last()
                .map(|step| {
                    vec![Parameter {
                        name: step.outputs[0].clone(),
                        kind: ParamKind::Json,
                        required: true,
                        description: "Result of the final step".into(),
                    }]
                })
                .unwrap_or_default(),
            failure_conditions: parts.verification.failure_signals.clone(),
            checkpoints: parts
                .verification
                .claims
                .iter()
                .filter(|claim| claim.required)
                .map(|claim| claim.claim_id.clone())
                .collect(),
            recovery: parts.extraction.recovery.clone(),
            side_effects: parts.extraction.side_effects.clone(),
        },
        verification: VerificationContract {
            method: "work-capture-evidence".into(),
            method_version: COMPILER_VERSION.into(),
            scope_environments: parts.context.applicability.environments.clone(),
            claims: parts.verification.claims.clone(),
            invariants: parts.verification.invariants.clone(),
            negative_cases: parts.verification.negative_cases.clone(),
            limitations: parts.context.applicability.limitations.clone(),
            unresolved_assumptions: parts
                .context
                .applicability
                .assumptions
                .iter()
                .filter(|assumption| {
                    assumption.status != crate::capability::model::ClaimStatus::Verified
                })
                .map(|assumption| assumption.description.clone())
                .collect(),
        },
        known_failure_cases: parts.verification.failure_signals.clone(),
        proposed_improvements: Vec::new(),
    }
}

/** The assembled sections of a candidate
 * Fields
    - request: &'a CompileRequest - request
    - intake: CapabilityIntake - intake
    - extraction: extract::Extraction - procedure
    - context: context::ContextPlan - applicability and dependencies
    - verification: verify::VerificationPlan - claims and gaps
*/
struct Parts<'a> {
    request: &'a CompileRequest,
    intake: CapabilityIntake,
    extraction: extract::Extraction,
    context: context::ContextPlan,
    verification: verify::VerificationPlan,
}

/** Compile a work episode into a capability candidate
 * Input
    - registry: &mut CapabilityRegistry - Component 2 registry
    - sources: &Sources - canonical sources and the requester's access
    - actor: &Actor - requester (needs the Contributor role, like any contributor)
    - request: &CompileRequest - request
    - now: u64 - Unix milliseconds
 * Output
    - CompilationReport (always; failures are outcomes, not errors)
*/
pub(crate) fn compile(
    registry: &mut CapabilityRegistry,
    sources: &Sources,
    actor: &Actor,
    request: &CompileRequest,
    now: u64,
) -> CompilationReport {
    let mut report = CompilationReport::new(request, actor, now);
    let problems = check_request(request);
    if !problems.is_empty() {
        return report.finish(CompilationOutcome::CompilationFailed, problems);
    }
    let intake = match registry.submit_intake(&request.intake, sources, actor, now) {
        Ok(submission) => submission.intake,
        Err(error) => {
            let outcome = match error.code {
                ErrorCode::Unauthorized
                | ErrorCode::IntegrityMismatch
                | ErrorCode::MissingSourceReference
                | ErrorCode::UnsupportedSchemaVersion
                | ErrorCode::IdempotencyConflict => CompilationOutcome::CompilationBlocked,
                _ => CompilationOutcome::CompilationFailed,
            };
            return report.finish(outcome, vec![format!("the intake was refused: {error}")]);
        }
    };
    let audit = sources.capture.audit(&intake.work.query_id);
    report.lineage = Some(Lineage {
        intake_id: intake.intake_id.clone(),
        idempotency_key: intake.idempotency_key.clone(),
        episode_id: intake.work.episode_id.clone(),
        pack_id: intake.work.pack_id.clone(),
        pack_digest: intake.work.pack_digest.clone(),
        query_id: intake.work.query_id.clone(),
        recipe: audit.and_then(|audit| {
            audit.recipe.as_ref().map(|recipe| {
                format!(
                    "{}@{}#{}",
                    recipe.recipe_id,
                    recipe.version,
                    recipe.digest()
                )
            })
        }),
        source_refs: Vec::new(),
    });
    let reconstruction = reconstruct::reconstruct(&intake, sources);
    let environment = reconstruction
        .items
        .iter()
        .filter(|item| item.basis == Basis::Observed)
        .find_map(|item| item.environment.clone());
    let verification = verify::plan(&intake, sources, environment);
    report.claim_bases.clone_from(&verification.bases);
    report.gaps.clone_from(&verification.gaps);
    report.hypotheses.clone_from(&verification.hypotheses);
    report.reconstruction = Some(reconstruction.clone());
    let extraction = extract::extract(
        &reconstruction,
        &intake,
        sources,
        &verification,
        &request.proposals,
        intake.risk,
    );
    report.step_bases.clone_from(&extraction.bases);
    report.branches.clone_from(&extraction.branches);
    report.proposals.clone_from(&extraction.proposals);
    report.hypotheses.extend(
        extraction
            .proposals
            .iter()
            .filter(|decision| decision.accepted)
            .map(|decision| format!("proposed step {}", decision.step_id)),
    );
    report.warnings.extend(
        extraction
            .proposals
            .iter()
            .filter(|decision| !decision.accepted)
            .map(|decision| {
                format!(
                    "proposed step {} rejected: {}",
                    decision.step_id,
                    decision.reasons.join("; ")
                )
            }),
    );
    if !extraction.policy_touches.is_empty() {
        return report.finish(CompilationOutcome::CompilationBlocked, vec![format!("the episode changed Crane policy or governance ({}); policy changes go through Crane's signed workflow and are never compiled into a capability", extraction.policy_touches.join(", "))]);
    }
    if !verification.failure_signals.is_empty() {
        return report.finish(
            CompilationOutcome::InsufficientReusableSignal,
            verification
                .failure_signals
                .iter()
                .map(|signal| format!("the episode did not succeed: {signal}"))
                .collect(),
        );
    }
    if extraction
        .bases
        .iter()
        .all(|basis| basis.basis != Basis::Observed)
    {
        return report.finish(
            CompilationOutcome::InsufficientReusableSignal,
            vec!["the episode shows no observed action or check to form a procedure".into()],
        );
    }
    let commit = intake
        .observed
        .code_changes
        .iter()
        .filter(|reference| reference.evidence_type == EvidenceType::CodeChange)
        .find_map(|reference| {
            sources
                .capture
                .evidence
                .get(&reference.evidence_id)
                .and_then(|record| record.refs.commit.clone())
        });
    let context = context::plan(request, &intake, &reconstruction, &extraction, commit);
    report.element_bases.clone_from(&context.bases);
    report.gaps.extend(context.gaps.iter().cloned());
    report.warnings.extend(context.warnings.iter().cloned());
    let pack_questions = sources.capture.packs.get(&intake.work.pack_id).map(|pack| {
        pack.contributions
            .task_local
            .iter()
            .filter(|contribution| contribution.contribution_type == ContributionType::Question)
            .filter_map(|contribution| {
                contribution
                    .content
                    .get("question")
                    .and_then(|question| question.as_str())
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
    });
    report
        .unresolved_questions
        .extend(pack_questions.unwrap_or_default());
    report.unresolved_questions.extend(
        report
            .gaps
            .iter()
            .filter(|gap| gap.blocking)
            .map(|gap| format!("What evidence resolves: {}?", gap.detail)),
    );
    let mut sources_used = verification
        .bases
        .iter()
        .flat_map(|basis| basis.refs.iter().cloned())
        .chain(
            extraction
                .bases
                .iter()
                .flat_map(|basis| basis.evidence_refs.iter().cloned()),
        )
        .collect::<Vec<_>>();
    sources_used.sort();
    sources_used.dedup();
    if let Some(lineage) = report.lineage.as_mut() {
        lineage.source_refs = sources_used;
    }
    let parts = Parts {
        request,
        intake,
        extraction,
        context,
        verification,
    };
    let profile = match select_profile(
        request.intake.profile.as_deref(),
        parts.context.applicability.risk,
    ) {
        Ok(profile) => profile,
        Err(error) => {
            return report.finish(
                CompilationOutcome::CompilationFailed,
                vec![format!(
                    "the validation profile cannot govern the candidate: {}",
                    error.message
                )],
            )
        }
    };
    let existing = registry
        .state
        .versions
        .get(&request.capability_id)
        .and_then(|versions| versions.values().next_back())
        .map(|record| {
            (
                record.definition.version,
                record.definition.previous_version_ref,
                record.definition.provenance.change_reason.clone(),
                record.definition_digest.clone(),
            )
        });
    let (candidate, reused) = match existing {
        None => (definition(&parts, actor, 1, None, None), false),
        Some((version, previous, reason, digest)) => {
            let same = definition(&parts, actor, version, previous, reason);
            if same.digest() == digest {
                (same, true)
            } else {
                let reason = format!(
                    "Recompiled from intake {} (pack {}#{})",
                    parts.intake.intake_id,
                    parts.intake.work.pack_id,
                    parts.intake.work.pack_digest
                );
                (
                    definition(&parts, actor, version + 1, Some(version), Some(reason)),
                    false,
                )
            }
        }
    };
    let run = validate::run(
        &candidate,
        &profile,
        &registry.state,
        sources,
        actor,
        true,
        now,
    );
    let structural = run
        .gates
        .iter()
        .find(|gate| gate.gate == GateId::Structural)
        .is_some_and(|gate| gate.status == GateStatus::Failed);
    report.validation = Some(run);
    if structural {
        return report.finish(
            CompilationOutcome::CompilationFailed,
            vec![
                "the assembled candidate fails Component 2's structural gate; nothing was stored"
                    .into(),
            ],
        );
    }
    if !reused {
        let stored = if candidate.version == 1 {
            registry.create_candidate(actor, candidate.clone(), Some(&profile.profile_id), now)
        } else {
            registry.propose_version(actor, candidate.clone(), Some(&profile.profile_id), now)
        };
        if let Err(error) = stored {
            let outcome = if error.code == ErrorCode::Unauthorized {
                CompilationOutcome::CompilationBlocked
            } else {
                CompilationOutcome::CompilationFailed
            };
            return report.finish(
                outcome,
                vec![format!("Component 2 refused the candidate: {error}")],
            );
        }
    }
    report.candidate = Some(CandidateRef {
        capability_id: candidate.capability_id.clone(),
        version: candidate.version,
        digest: candidate.digest(),
        reused,
    });
    let blocking = report
        .gaps
        .iter()
        .filter(|gap| gap.blocking)
        .map(|gap| gap.detail.clone())
        .collect::<Vec<_>>();
    if blocking.is_empty() {
        report.finish(CompilationOutcome::CandidateGenerated, vec!["every required claim maps to evidence; the candidate awaits Component 2 validation and publication by authorized people".into()])
    } else {
        report.finish(CompilationOutcome::NeedsMoreEvidence, blocking)
    }
}
