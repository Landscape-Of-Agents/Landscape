// The deterministic compiler demonstration ('crane compile demo'), from fixtures only:
//
//   1. Work Capture investigates a successful refund-retry change (src/compiler/fixtures/
//      successful_deployment.json): CI and 42 tests pass, the deployment succeeds, a smoke check
//      and a reconciliation check naming the business invariant report healthy; the engineer
//      records the outcome as succeeded with the deployment evidence
//   2. the compiler turns it into a candidate (candidate_generated), stored by Component 2 as a
//      candidate; a different person then validates it through Component 2
//   3. compiling the same inputs again reuses the stored version
//   4. the failed refund-worker deployment of the Work Capture demonstration compiles to
//      insufficient_reusable_signal: a failed episode is not a reusable procedure

use serde::Serialize;

use super::assemble::{compile, CompilationReport};
use super::{CompilationOutcome, CompileRequest};
use crate::capability::demo::{self as capability_demo, Cast};
use crate::capability::intake::{
    BusinessRef, EnterpriseIntent, IntakeRequest, INTAKE_SCHEMA_VERSION,
};
use crate::capability::lifecycle::LifecycleState;
use crate::capability::model::{MappingStatus, RiskLevel};
use crate::capability::refs::Sources;
use crate::capability::registry::CapabilityRegistry;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capture::broker::run::EvidenceQueryBroker;
use crate::capture::demo::{self as capture_demo, Scenario};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::episode::EpisodeOutcome;
use crate::capture::ingest::pack::EvidencePack;
use crate::capture::{Actor, Clock};

/** The successful-deployment fixture */
const SUCCESS: &str = include_str!("fixtures/successful_deployment.json");

/** Convert a Work Capture error
 * Input
    - error: crate::capture::CaptureError - error
 * Output
    - CapabilityError
*/
fn captured_error(error: crate::capture::CaptureError) -> CapabilityError {
    CapabilityError::new(ErrorCode::MissingSourceReference, error.to_string())
}

/** The successful-deployment scenario
 * Input
    - None
 * Output
    - CapabilityResult<Scenario>
*/
pub(crate) fn success_scenario() -> CapabilityResult<Scenario> {
    serde_json::from_str(SUCCESS).map_err(|error| {
        CapabilityError::new(
            ErrorCode::InvalidSchema,
            format!("compiler fixture: {error}"),
        )
    })
}

/** Investigate a scenario with Work Capture, requiring the checks and policies, and optionally
 * record the episode outcome (as the engineer, with the deployment and check evidence)
 * Input
    - scenario: &Scenario - scenario
    - outcome: Option<EpisodeOutcome> - outcome to record, None to leave it unrecorded
    - recorder: &Actor - who records the outcome
 * Output
    - CapabilityResult<(EvidenceQueryBroker, EvidencePack)>
*/
pub(crate) fn investigate(
    scenario: &Scenario,
    outcome: Option<EpisodeOutcome>,
    recorder: &Actor,
) -> CapabilityResult<(EvidenceQueryBroker, EvidencePack)> {
    let mut broker = capture_demo::broker(scenario, Clock::fixed(capture_demo::DEMO_EPOCH))
        .map_err(captured_error)?;
    let mut request = capture_demo::request(scenario);
    request.required_evidence = vec![EvidenceType::Verification, EvidenceType::Policy];
    let pack = broker
        .investigate(request, &capture_demo::engineer(scenario))
        .map_err(captured_error)?
        .pack;
    if let (Some(outcome), Some(episode_id)) = (outcome, &pack.episode_id) {
        let evidence = pack
            .evidence
            .iter()
            .filter(|item| {
                matches!(
                    item.evidence_type,
                    EvidenceType::Deployment | EvidenceType::Verification
                )
            })
            .map(|item| item.evidence_id.clone())
            .collect();
        broker
            .store
            .record_outcome(
                episode_id,
                recorder,
                false,
                outcome,
                evidence,
                capture_demo::DEMO_EPOCH + 10_000,
            )
            .map_err(captured_error)?;
    }
    Ok((broker, pack))
}

/** The intake request of the successful episode: an explicitly mapped objective, criteria, and
 * invariant, medium risk, conservative profile
 * Input
    - pack: &EvidencePack - Evidence Pack
 * Output
    - IntakeRequest
*/
pub(crate) fn intake_request(pack: &EvidencePack) -> IntakeRequest {
    let reference = |id: &str, statement: &str, authority: &str| BusinessRef {
        id: id.into(),
        statement: statement.into(),
        authority: Some(authority.into()),
        mapping: MappingStatus::ExplicitlyMapped,
        confirmed_by: None,
    };
    IntakeRequest {
        schema_version: INTAKE_SCHEMA_VERSION,
        episode_id: pack.episode_id.clone().unwrap_or_default(),
        pack_id: pack.pack_id.clone(),
        pack_digest: pack.manifest.digest.clone(),
        intent: EnterpriseIntent {
            objective: Some(reference(
                "obj-safe-refund-retries",
                "Failed refunds can be retried without paying twice",
                "payments-spec#refund-retries",
            )),
            process_domain: Some("payments/refunds".into()),
            acceptance_criteria: vec![reference(
                "ac-1",
                "A retried refund request never refunds twice",
                "tracker:PAY-150",
            )],
            invariants: vec![reference(
                "inv-no-double-refund",
                "A retried refund request never refunds twice",
                "payments-spec#INV-7",
            )],
            outcome_measures: Vec::new(),
            owner: Some("team:payments".into()),
            authoritative_systems: vec!["refunds-ledger".into()],
        },
        risk: RiskLevel::Medium,
        profile: Some("conservative".into()),
        idempotency_key: None,
        unresolved_assumptions: Vec::new(),
        human_interventions: vec!["lead@acme.example approved pull request 431".into()],
    }
}

/** The compilation request for a pack
 * Input
    - pack: &EvidencePack - Evidence Pack
 * Output
    - CompileRequest
*/
pub(crate) fn compile_request(pack: &EvidencePack) -> CompileRequest {
    CompileRequest {
        intake: intake_request(pack),
        capability_id: "refund-change-with-verified-deploy".into(),
        name: "Ship a refund change with a verified deployment".into(),
        purpose:
            "Change refund handling, test it, deploy it, and verify that no refund is paid twice"
                .into(),
        task_class: "refund-change".into(),
        proposals: Vec::new(),
    }
}

/** What the demonstration produced
 * Fields
    - success: CompilationReport - report of the successful episode
    - recompiled_reused: bool - compiling again reused the stored version
    - state_after_validation: Option<LifecycleState> - state after a person validated it
    - failed: CompilationReport - report of the failed episode
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DemoReport {
    pub(crate) success: CompilationReport,
    pub(crate) recompiled_reused: bool,
    pub(crate) state_after_validation: Option<LifecycleState>,
    pub(crate) failed: CompilationReport,
}

/** Run the demonstration
 * Input
    - None
 * Output
    - CapabilityResult<DemoReport>
*/
pub(crate) fn run() -> CapabilityResult<DemoReport> {
    let cast: Cast = capability_demo::cast();
    let current = capability_demo::current_versions();
    let reader = capability_demo::reader(&cast.engineer)?;
    let (capture, pack) = investigate(
        &success_scenario()?,
        Some(EpisodeOutcome::Succeeded),
        &cast.engineer,
    )?;
    let mut registry = CapabilityRegistry::in_memory(capability_demo::grants(&cast));
    let sources = Sources {
        capture: &capture.store,
        current: &current,
        reader: &reader,
    };
    let now = 1_760_000_300_000;
    let success = compile(
        &mut registry,
        &sources,
        &cast.engineer,
        &compile_request(&pack),
        now,
    );
    if success.outcome != CompilationOutcome::CandidateGenerated {
        return Err(CapabilityError::new(
            ErrorCode::IncompleteEvidence,
            format!(
                "the successful episode should compile: {:?} {:?}",
                success.outcome, success.reasons
            ),
        ));
    }
    let again = compile(
        &mut registry,
        &sources,
        &cast.engineer,
        &compile_request(&pack),
        now + 1,
    );
    let candidate = success
        .candidate
        .clone()
        .ok_or_else(|| CapabilityError::new(ErrorCode::NotFound, "no candidate"))?;
    let lead_reader = capability_demo::reader(&cast.lead)?;
    let lead_sources = Sources {
        reader: &lead_reader,
        ..sources
    };
    registry.validate(
        &cast.lead,
        &candidate.capability_id,
        candidate.version,
        &lead_sources,
        false,
        now + 2,
    )?;
    let state_after_validation = registry
        .state
        .record(&candidate.capability_id, candidate.version)
        .ok()
        .map(|record| record.state);
    let (failed_capture, failed_pack) = capability_demo::captured()?;
    let failed_sources = Sources {
        capture: &failed_capture.store,
        current: &current,
        reader: &reader,
    };
    let mut failed_request = compile_request(&failed_pack);
    failed_request.capability_id = "refund-worker-deploy".into();
    let failed = compile(
        &mut registry,
        &failed_sources,
        &cast.engineer,
        &failed_request,
        now + 3,
    );
    Ok(DemoReport {
        recompiled_reused: again
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.reused),
        success,
        state_after_validation,
        failed,
    })
}
