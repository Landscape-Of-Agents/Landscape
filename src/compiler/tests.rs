// Tests of the Capability Compiler: supported extraction, missing and contradictory evidence,
// unsupported claims and proposals, invalid inputs, authorization and integrity failures, policy
// changes, determinism, schema compatibility, and the hand-off to Component 2. They use the
// successful-deployment fixture and the Work Capture failed-deployment fixture.

use serde_json::json;

use super::assemble::{compile, CompilationReport};
use super::demo::{compile_request, investigate, success_scenario};
use super::reconstruct::ORDERING_NOTE;
use super::verify::GapKind;
use super::{Basis, CompilationOutcome, CompileRequest, ProposedStep};
use crate::capability::demo::{self as capability_demo, Cast};
use crate::capability::lifecycle::{LifecycleState, PublicationRequest};
use crate::capability::model::{
    parse_definition, ClaimStatus, CreationMethod, MappingStatus, ProcedureStep, StepKind,
};
use crate::capability::refs::{CurrentVersions, Sources};
use crate::capability::registry::CapabilityRegistry;
use crate::capability::validate::RunOutcome;
use crate::capture::broker::run::EvidenceQueryBroker;
use crate::capture::demo::Scenario;
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::episode::EpisodeOutcome;
use crate::capture::ingest::pack::EvidencePack;
use crate::capture::{AccessPolicy, Actor, ActorKind};
use crate::trust::crypto::digest_json;

/** Fixed compilation time */
const NOW: u64 = 1_760_000_300_000;

/** A compilation world
 * Fields
    - capture: EvidenceQueryBroker - Work Capture with the episode's pack
    - pack: EvidencePack - the pack
    - registry: CapabilityRegistry - Component 2 registry
    - current: CurrentVersions - current Landscape and Crane versions
    - cast: Cast - people and agent
    - reader: AccessPolicy - the engineer's evidence access
*/
struct World {
    capture: EvidenceQueryBroker,
    pack: EvidencePack,
    registry: CapabilityRegistry,
    current: CurrentVersions,
    cast: Cast,
    reader: AccessPolicy,
}

impl World {
    /** A world over a scenario
     * Input
        - scenario: &Scenario - scenario
        - outcome: Option<EpisodeOutcome> - recorded outcome
     * Output
        - World
    */
    fn over(scenario: &Scenario, outcome: Option<EpisodeOutcome>) -> Self {
        let cast = capability_demo::cast();
        let (capture, pack) = investigate(scenario, outcome, &cast.engineer).unwrap();
        Self {
            capture,
            pack,
            registry: CapabilityRegistry::in_memory(capability_demo::grants(&cast)),
            current: capability_demo::current_versions(),
            reader: capability_demo::reader(&cast.engineer).unwrap(),
            cast,
        }
    }

    /** The successful episode, outcome recorded
     * Input
        - None
     * Output
        - World
    */
    fn success() -> Self {
        Self::over(
            &success_scenario().unwrap(),
            Some(EpisodeOutcome::Succeeded),
        )
    }

    /** Compile a request as the engineer
     * Input
        - request: &CompileRequest - request
     * Output
        - CompilationReport
    */
    fn compile(&mut self, request: &CompileRequest) -> CompilationReport {
        let sources = Sources {
            capture: &self.capture.store,
            current: &self.current,
            reader: &self.reader,
        };
        compile(
            &mut self.registry,
            &sources,
            &self.cast.engineer,
            request,
            NOW,
        )
    }
}

/** The success scenario changed
 * Input
    - change: impl FnOnce(&mut Scenario) - change
 * Output
    - Scenario
*/
fn changed(change: impl FnOnce(&mut Scenario)) -> Scenario {
    let mut scenario = success_scenario().unwrap();
    change(&mut scenario);
    scenario
}

/** The kinds of gaps of a report
 * Input
    - report: &CompilationReport - report
 * Output
    - Vec<(GapKind, bool)>
*/
fn gap_kinds(report: &CompilationReport) -> Vec<(GapKind, bool)> {
    report
        .gaps
        .iter()
        .map(|gap| (gap.kind, gap.blocking))
        .collect()
}

/** A successful episode compiles into a candidate whose steps, claims, and applicability are
 * all traceable, stored as a candidate in the existing schema
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn extracts_a_supported_procedure() {
    let mut world = World::success();
    let report = world.compile(&compile_request(&world.pack));
    assert_eq!(
        report.outcome,
        CompilationOutcome::CandidateGenerated,
        "{:?}",
        report.reasons
    );
    let candidate = report.candidate.clone().unwrap();
    let record = world
        .registry
        .state
        .record(&candidate.capability_id, 1)
        .unwrap();
    assert_eq!(
        (record.state, record.definition_digest.clone()),
        (LifecycleState::Candidate, candidate.digest)
    );
    let definition = &record.definition;
    let kinds = definition
        .execution
        .steps
        .iter()
        .map(|step| step.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            StepKind::Change,
            StepKind::Test,
            StepKind::Change,
            StepKind::Approval,
            StepKind::Test,
            StepKind::Test,
            StepKind::Deploy,
            StepKind::Verify,
            StepKind::Verify
        ]
    );
    assert!(report
        .step_bases
        .iter()
        .all(|basis| basis.basis == Basis::Observed
            && basis.kind_basis == Basis::Inferred
            && basis.evidence_refs.len() == 1));
    for step in &report.step_bases {
        assert!(
            world
                .pack
                .evidence
                .iter()
                .any(|item| item.evidence_id == step.evidence_refs[0]),
            "steps come from the pack"
        );
    }
    let claim = |id: &str| {
        definition
            .verification
            .claims
            .iter()
            .find(|claim| claim.claim_id == id)
            .unwrap()
    };
    for id in [
        "ci-passed",
        "tests-passed",
        "deployment-succeeded",
        "change-verified",
    ] {
        assert_eq!(claim(id).status, ClaimStatus::Verified, "{id}");
        assert!(!claim(id).assertion_refs.is_empty());
    }
    let invariant = claim("invariant-inv-no-double-refund");
    assert_eq!(invariant.status, ClaimStatus::Verified);
    assert_eq!(
        world.capture.store.evidence[&invariant.evidence_refs[0]]
            .field("check")
            .as_deref(),
        Some("refund-reconciliation")
    );
    assert_eq!(claim("acceptance-ac-1").status, ClaimStatus::Declared);
    assert_eq!(
        definition.provenance.creation_method,
        CreationMethod::Compiler
    );
    assert_eq!(
        report.validation.as_ref().unwrap().outcome,
        RunOutcome::Passed
    );
    assert!(
        report.validation.as_ref().unwrap().dry_run,
        "the compiler's validation is a dry run"
    );
}

/** Missing evidence leaves the candidate in need of evidence, with the claim unresolved
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn missing_evidence_needs_more_evidence() {
    let scenario = changed(|scenario| {
        let ci = scenario
            .resources
            .iter_mut()
            .find(|resource| resource.resource_id == "ci")
            .unwrap();
        ci.records
            .retain(|record| record.evidence_type != EvidenceType::TestReport);
    });
    let mut world = World::over(&scenario, Some(EpisodeOutcome::Succeeded));
    let report = world.compile(&compile_request(&world.pack));
    assert_eq!(report.outcome, CompilationOutcome::NeedsMoreEvidence);
    assert!(gap_kinds(&report).contains(&(GapKind::MissingEvidence, true)));
    let definition = &world
        .registry
        .state
        .record("refund-change-with-verified-deploy", 1)
        .unwrap()
        .definition;
    let tests = definition
        .verification
        .claims
        .iter()
        .find(|claim| claim.claim_id == "tests-passed")
        .unwrap();
    assert_eq!(tests.status, ClaimStatus::Unresolved);
    assert!(
        tests.evidence_refs.is_empty(),
        "an unknown result cites no evidence as support"
    );
    assert!(report
        .unresolved_questions
        .iter()
        .any(|question| question.contains("tests_passed")));
}

/** An unrecorded outcome is unknown, never success
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn unrecorded_outcome_needs_more_evidence() {
    let mut world = World::over(&success_scenario().unwrap(), None);
    let report = world.compile(&compile_request(&world.pack));
    assert_eq!(report.outcome, CompilationOutcome::NeedsMoreEvidence);
    assert!(report
        .gaps
        .iter()
        .any(|gap| gap.kind == GapKind::Unknown && gap.detail.contains("not recorded")));
    let reconstruction = report.reconstruction.unwrap();
    assert!(reconstruction
        .items
        .iter()
        .any(|item| item.basis == Basis::Unknown && item.description.contains("not recorded")));
}

/** Contradictory evidence is kept on both sides and blocks the candidate
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn contradictions_are_not_resolved_silently() {
    let scenario = changed(|scenario| {
        let deploy = scenario
            .resources
            .iter_mut()
            .find(|resource| resource.resource_id == "deploy")
            .unwrap();
        let mut canary = deploy.records[1].clone();
        canary.record_id = "check-canary-7802".into();
        canary.content = json!({"deployment": "dep-7802", "check": "canary", "healthy": false, "environment": "production"});
        deploy.records.push(canary);
    });
    let mut world = World::over(&scenario, Some(EpisodeOutcome::Succeeded));
    let report = world.compile(&compile_request(&world.pack));
    assert_eq!(report.outcome, CompilationOutcome::NeedsMoreEvidence);
    let contradiction = report
        .gaps
        .iter()
        .find(|gap| gap.kind == GapKind::Contradiction)
        .unwrap();
    assert!(
        contradiction.blocking && contradiction.refs.len() >= 3,
        "both sides are referenced"
    );
    let deployment = report
        .claim_bases
        .iter()
        .find(|basis| basis.claim_id == "deployment-succeeded")
        .unwrap();
    assert_eq!(deployment.basis, Basis::Unknown);
}

/** A failed episode is not compiled into a reusable procedure
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn failed_episodes_have_insufficient_signal() {
    let cast = capability_demo::cast();
    let (capture, pack) = capability_demo::captured().unwrap();
    let mut registry = CapabilityRegistry::in_memory(capability_demo::grants(&cast));
    let current = capability_demo::current_versions();
    let reader = capability_demo::reader(&cast.engineer).unwrap();
    let sources = Sources {
        capture: &capture.store,
        current: &current,
        reader: &reader,
    };
    let report = compile(
        &mut registry,
        &sources,
        &cast.engineer,
        &compile_request(&pack),
        NOW,
    );
    assert_eq!(
        report.outcome,
        CompilationOutcome::InsufficientReusableSignal
    );
    assert!(report
        .reasons
        .iter()
        .any(|reason| reason.contains("deployment_succeeded")));
    assert!(report.candidate.is_none() && registry.state.versions.is_empty());
    let scenario = changed(|_| {});
    let mut abandoned = World::over(&scenario, Some(EpisodeOutcome::Abandoned));
    assert_eq!(
        abandoned.compile(&compile_request(&abandoned.pack)).outcome,
        CompilationOutcome::InsufficientReusableSignal
    );
}

/** Model proposals are validated: unsupported or policy-touching ones are rejected, accepted ones
 * stay hypotheses; an unsupported business mapping is a blocking gap
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn validates_proposals_and_unsupported_claims() {
    let mut world = World::success();
    let deployment = world
        .pack
        .evidence
        .iter()
        .find(|item| item.evidence_type == EvidenceType::Deployment)
        .unwrap()
        .evidence_id
        .clone();
    let step = |id: &str, kind: StepKind, description: &str| ProcedureStep {
        step_id: id.into(),
        kind,
        description: description.into(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        preconditions: Vec::new(),
        verifies: Vec::new(),
    };
    let mut request = compile_request(&world.pack);
    request.proposals = vec![
        ProposedStep {
            step: step(
                "monitor-refunds",
                StepKind::Diagnostic,
                "Watch refund error rates for an hour",
            ),
            rationale: "common practice".into(),
            evidence_refs: vec!["evd_invented".into()],
            producer: "model-x".into(),
        },
        ProposedStep {
            step: step(
                "relax-policy",
                StepKind::Change,
                "Edit .crane/policies/refunds.crane to allow retries",
            ),
            rationale: "faster".into(),
            evidence_refs: vec![deployment.clone()],
            producer: "model-x".into(),
        },
        ProposedStep {
            step: step(
                "announce",
                StepKind::Diagnostic,
                "Post the deployment result to the payments channel",
            ),
            rationale: "the deployment was observed".into(),
            evidence_refs: vec![deployment],
            producer: "model-x".into(),
        },
    ];
    let report = world.compile(&request);
    let decision = |id: &str| {
        report
            .proposals
            .iter()
            .find(|decision| decision.step_id == id)
            .unwrap()
    };
    assert!(
        !decision("monitor-refunds").accepted
            && decision("monitor-refunds").reasons[0].contains("outside the pack")
    );
    assert!(!decision("relax-policy").accepted);
    assert!(decision("announce").accepted);
    assert_eq!(
        report
            .step_bases
            .iter()
            .find(|basis| basis.step_id == "announce")
            .unwrap()
            .basis,
        Basis::Hypothesis
    );
    assert!(report
        .hypotheses
        .iter()
        .any(|hypothesis| hypothesis.contains("announce")));
    let definition = &world
        .registry
        .state
        .record("refund-change-with-verified-deploy", 1)
        .unwrap()
        .definition;
    assert!(definition
        .applicability
        .assumptions
        .iter()
        .any(|assumption| assumption.id == "proposed-announce"
            && assumption.status == ClaimStatus::Unresolved));
    assert!(report
        .extraction_method
        .contains("validated-model-proposals"));
    let mut unsupported = World::success();
    let mut request = compile_request(&unsupported.pack);
    request.intake.intent.invariants[0].authority = None;
    let report = unsupported.compile(&request);
    assert_eq!(report.outcome, CompilationOutcome::NeedsMoreEvidence);
    assert!(gap_kinds(&report).contains(&(GapKind::UnsupportedClaim, true)));
    let mut inferred = World::success();
    let mut request = compile_request(&inferred.pack);
    request.intake.intent.invariants[0].mapping = MappingStatus::InferredUnverified;
    request.intake.intent.invariants[0].authority = None;
    let definition = inferred.compile(&request);
    assert_eq!(
        definition.outcome,
        CompilationOutcome::NeedsMoreEvidence,
        "an inferred invariant never becomes authoritative"
    );
}

/** Invalid inputs fail before anything is stored
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_invalid_inputs() {
    let mut world = World::success();
    for change in [
        Box::new(|request: &mut CompileRequest| request.capability_id = "Bad Id".into())
            as Box<dyn Fn(&mut CompileRequest)>,
        Box::new(|request: &mut CompileRequest| request.task_class = "any".into()),
        Box::new(|request: &mut CompileRequest| request.purpose = " ".into()),
    ] {
        let mut request = compile_request(&world.pack);
        change(&mut request);
        assert_eq!(
            world.compile(&request).outcome,
            CompilationOutcome::CompilationFailed
        );
    }
    assert!(world.registry.state.intakes.is_empty() && world.registry.state.versions.is_empty());
    let mut request = compile_request(&world.pack);
    request.intake.schema_version = 9;
    assert_eq!(
        world.compile(&request).outcome,
        CompilationOutcome::CompilationBlocked
    );
}

/** Authorization, integrity, and policy changes block compilation without storing anything
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn blocks_unauthorized_and_tampered_inputs() {
    let mut world = World::success();
    let stranger = Actor::new("stranger@acme.example", ActorKind::Human);
    let reader = AccessPolicy {
        actor: stranger.clone(),
        ..world.reader.clone()
    };
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let report = compile(
        &mut world.registry,
        &sources,
        &stranger,
        &compile_request(&world.pack),
        NOW,
    );
    assert_eq!(report.outcome, CompilationOutcome::CompilationBlocked);
    assert!(report.reasons[0].contains("unauthorized"));
    let mut tampered = compile_request(&world.pack);
    tampered.intake.pack_digest = "0".repeat(32);
    let report = world.compile(&tampered);
    assert_eq!(report.outcome, CompilationOutcome::CompilationBlocked);
    assert!(report.reasons[0].contains("integrity_mismatch"));
    assert!(world.registry.state.versions.is_empty());
    let scenario = changed(|scenario| {
        let datasea = scenario
            .resources
            .iter_mut()
            .find(|resource| resource.resource_id == "datasea")
            .unwrap();
        datasea.records[0].content["resource"] = json!(".crane/policies/refunds.crane");
    });
    let mut policy = World::over(&scenario, Some(EpisodeOutcome::Succeeded));
    let report = policy.compile(&compile_request(&policy.pack));
    assert_eq!(report.outcome, CompilationOutcome::CompilationBlocked);
    assert!(report.reasons[0].contains("signed workflow"));
    assert!(policy.registry.state.versions.is_empty());
}

/** The same inputs give the same candidate and report; compiling again reuses the version
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn compiles_deterministically() {
    let mut first = World::success();
    let mut second = World::success();
    let a = first.compile(&compile_request(&first.pack));
    let b = second.compile(&compile_request(&second.pack));
    assert_eq!(a, b);
    let again = first.compile(&compile_request(&first.pack));
    let candidate = again.candidate.unwrap();
    assert!(candidate.reused);
    assert_eq!(
        (candidate.version, candidate.digest),
        (1, a.candidate.clone().unwrap().digest)
    );
    assert_eq!(
        first.registry.state.versions["refund-change-with-verified-deploy"].len(),
        1
    );
    let mut without = a.clone();
    without.digest = String::new();
    without.report_id = String::new();
    assert_eq!(
        digest_json("compiler/report/v1", &json!(without))[..32],
        a.digest,
        "the report digest covers its content"
    );
}

/** The candidate is the existing schema, handed off as a candidate only; Component 2 decides the
 * rest, and a changed compilation becomes a new linked version
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn hands_off_to_component_two() {
    let mut world = World::success();
    let report = world.compile(&compile_request(&world.pack));
    let id = report.candidate.unwrap().capability_id;
    let definition = world
        .registry
        .state
        .record(&id, 1)
        .unwrap()
        .definition
        .clone();
    assert_eq!(parse_definition(&json!(definition)).unwrap(), definition);
    let request = PublicationRequest {
        capability_id: id.clone(),
        version: 1,
        reason: "ship".into(),
        acknowledged_warnings: Vec::new(),
    };
    assert!(
        world
            .registry
            .publish(&world.cast.lead, &request, NOW + 1)
            .is_err(),
        "a compiled candidate is not publishable before validation"
    );
    let lead = capability_demo::reader(&world.cast.lead).unwrap();
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &lead,
    };
    let run = world
        .registry
        .validate(&world.cast.lead, &id, 1, &sources, false, NOW + 2)
        .unwrap();
    assert_eq!(run.outcome, RunOutcome::Passed);
    let mut revised = compile_request(&world.pack);
    revised.purpose = "Change refund handling and verify no double refunds after deploy".into();
    let report = world.compile(&revised);
    let candidate = report.candidate.unwrap();
    assert_eq!((candidate.version, candidate.reused), (2, false));
    let second = &world.registry.state.record(&id, 2).unwrap().definition;
    assert_eq!(second.previous_version_ref, Some(1));
    assert!(second
        .provenance
        .change_reason
        .as_deref()
        .unwrap()
        .starts_with("Recompiled from intake"));
    assert_eq!(
        world.registry.state.record(&id, 1).unwrap().state,
        LifecycleState::Validated,
        "the earlier version is untouched"
    );
}

/** Observed, declared, derived, inferred, hypothesis, and unknown stay distinct, and order is
 * never causation; one episode is not generalized
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn keeps_bases_distinct_and_scope_narrow() {
    let mut world = World::success();
    let report = world.compile(&compile_request(&world.pack));
    let reconstruction = report.reconstruction.as_ref().unwrap();
    assert_eq!(reconstruction.ordering_note, ORDERING_NOTE);
    for basis in [Basis::Observed, Basis::Declared, Basis::Derived] {
        assert!(
            reconstruction.items.iter().any(|item| item.basis == basis),
            "{basis:?}"
        );
    }
    assert!(reconstruction
        .items
        .iter()
        .any(|item| item.basis == Basis::Declared
            && item.description.contains("approved pull request 431")));
    let definition = &world
        .registry
        .state
        .record("refund-change-with-verified-deploy", 1)
        .unwrap()
        .definition;
    assert_eq!(definition.applicability.environments, vec!["production"]);
    assert_eq!(
        definition.verification.scope_environments,
        vec!["production"]
    );
    let single = definition
        .applicability
        .assumptions
        .iter()
        .find(|assumption| assumption.id == "single-episode")
        .unwrap();
    assert_eq!(single.status, ClaimStatus::Unresolved);
    assert!(report
        .element_bases
        .iter()
        .any(|basis| basis.element == "task_class" && basis.basis == Basis::Declared));
    assert!(report
        .element_bases
        .iter()
        .any(|basis| basis.element == "environment" && basis.basis == Basis::Observed));
    assert!(report.branches.iter().all(
        |branch| branch.source.contains("not the procedure") || branch.basis == Basis::Inferred
    ));
    let lineage = report.lineage.as_ref().unwrap();
    assert_eq!(
        (lineage.pack_id.as_str(), lineage.pack_digest.as_str()),
        (
            world.pack.pack_id.as_str(),
            world.pack.manifest.digest.as_str()
        )
    );
    assert!(lineage
        .recipe
        .as_deref()
        .unwrap()
        .starts_with("failed-deployment@1#"));
    assert!(!lineage.source_refs.is_empty());
    assert_eq!(
        (
            report.compiler_version.as_str(),
            report.capability_schema_version,
            report.report_schema_version
        ),
        ("1", 1, 1)
    );
    let risk = definition.applicability.risk;
    let mut low = World::success();
    let mut request = compile_request(&low.pack);
    request.intake.risk = crate::capability::model::RiskLevel::Low;
    request.intake.profile = None;
    let raised = low.compile(&request);
    assert!(raised
        .warnings
        .iter()
        .any(|warning| warning.contains("raised to medium")));
    assert_eq!(risk, crate::capability::model::RiskLevel::Medium);
}

/** The demonstration runs deterministically
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn runs_the_demonstration() {
    let report = super::demo::run().unwrap();
    assert_eq!(
        report.success.outcome,
        CompilationOutcome::CandidateGenerated
    );
    assert!(report.recompiled_reused);
    assert_eq!(
        report.state_after_validation,
        Some(LifecycleState::Validated)
    );
    assert_eq!(
        report.failed.outcome,
        CompilationOutcome::InsufficientReusableSignal
    );
    assert_eq!(
        serde_json::to_value(super::demo::run().unwrap()).unwrap(),
        serde_json::to_value(&report).unwrap()
    );
}
