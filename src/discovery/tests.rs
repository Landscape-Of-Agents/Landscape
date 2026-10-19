// Tests of the discovery engine: exact matches, unknown applicability, empty environment lists,
// stale dependencies, authorization and integrity of context, failed and inconclusive lab reports,
// revoked versions, empty registries, deterministic ranking, reproducible explanations, report
// storage, and the absence of registry changes.

use super::demo::{catalog, run, Catalog, NOW};
use super::evaluate::{ConditionStatus, Disposition};
use super::report::{discover, DiscoveryReport, DiscoveryStore};
use super::resolve::{ContextRef, DiscoveryRequest};
use super::DiscoveryOutcome;
use crate::capability::demo as capability_demo;
use crate::capability::refs::Sources;
use crate::capability::registry::CapabilityRegistry;
use crate::capability_lab::CheckStatus;
use crate::capture::{Actor, ActorKind};

/** The evaluation of one capability in a report
 * Input
    - report: &DiscoveryReport - report
    - id: &str - capability
 * Output
    - &super::evaluate::CandidateEvaluation
*/
fn candidate<'a>(
    report: &'a DiscoveryReport,
    id: &str,
) -> &'a super::evaluate::CandidateEvaluation {
    report
        .candidates
        .iter()
        .find(|candidate| candidate.capability_id == id)
        .unwrap_or_else(|| panic!("no candidate {id}"))
}

/** The exact match is recommended with its digest, and every alternative is explained
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn recommends_exact_matches_and_explains_alternatives() {
    let catalog = catalog().unwrap();
    let report = catalog.discover(&catalog.request()).unwrap();
    assert_eq!(report.outcome, DiscoveryOutcome::Recommended);
    assert_eq!(report.recommendations.len(), 1);
    let recommended = &report.recommendations[0];
    assert_eq!(
        (
            recommended.capability_id.as_str(),
            recommended.version,
            recommended.rank
        ),
        ("refund-change-with-verified-deploy", 1, 1)
    );
    assert_eq!(
        recommended.digest,
        catalog
            .world
            .registry
            .state
            .record("refund-change-with-verified-deploy", 1)
            .unwrap()
            .definition_digest
    );
    let chosen = candidate(&report, "refund-change-with-verified-deploy");
    assert!(chosen
        .conditions
        .iter()
        .all(|condition| condition.status == ConditionStatus::Satisfied
            || condition.gate == "dependency"
            || condition.gate == "scope"));
    assert!(chosen
        .conditions
        .iter()
        .any(|condition| condition.gate == "scope"
            && condition.status == ConditionStatus::Satisfied
            && condition.detail.contains("v3")));
    assert_eq!(
        candidate(&report, "refund-change-legacy").disposition,
        Disposition::Rejected
    );
    assert!(candidate(&report, "refund-change-legacy").reasons[0].contains("Revoked"));
    assert!(candidate(&report, "refund-change-draft").reasons[0].contains("Candidate"));
    assert!(candidate(&report, "refund-reporting").reasons[0].contains("task class"));
    let context = report.context.as_ref().unwrap();
    assert_eq!(context.environment.as_deref(), Some("production"));
    assert!(context.values.iter().any(
        |value| value.field == "environment" && value.basis == crate::compiler::Basis::Observed
    ));
}

/** Unknown applicability never counts as satisfied: missing environment, unconfirmed
 * preconditions, and contradicting declared and observed values leave candidates unresolved
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn unknown_conditions_are_not_satisfied() {
    let catalog = catalog().unwrap();
    let unknown = catalog
        .discover(&DiscoveryRequest {
            context: None,
            ..catalog.request()
        })
        .unwrap();
    assert_eq!(unknown.outcome, DiscoveryOutcome::InsufficientContext);
    assert_eq!(
        candidate(&unknown, "refund-change-with-verified-deploy").disposition,
        Disposition::Unresolved
    );
    let unconfirmed = catalog
        .discover(&DiscoveryRequest {
            confirmed_preconditions: vec!["repository-access".into()],
            ..catalog.request()
        })
        .unwrap();
    assert_eq!(unconfirmed.outcome, DiscoveryOutcome::InsufficientContext);
    assert!(
        candidate(&unconfirmed, "refund-change-with-verified-deploy")
            .conditions
            .iter()
            .any(
                |condition| condition.condition == "precondition deployment-target-known"
                    && condition.status == ConditionStatus::Unknown
            )
    );
    let conflicting = catalog
        .discover(&DiscoveryRequest {
            environment: Some("staging".into()),
            ..catalog.request()
        })
        .unwrap();
    assert_eq!(conflicting.outcome, DiscoveryOutcome::InsufficientContext);
    assert!(conflicting.unresolved.iter().any(|item| item
        .contains("the request says staging but the Work Capture context says production")));
    let staging = catalog
        .discover(&DiscoveryRequest {
            context: None,
            environment: Some("staging".into()),
            ..catalog.request()
        })
        .unwrap();
    assert_eq!(staging.outcome, DiscoveryOutcome::NoMatch);
}

/** An empty environment list means no known environment, never every environment
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn empty_environment_lists_never_match() {
    let mut catalog = catalog().unwrap();
    let versions = catalog
        .world
        .registry
        .state
        .versions
        .get_mut("refund-change-with-verified-deploy")
        .unwrap();
    versions
        .get_mut(&1)
        .unwrap()
        .definition
        .applicability
        .environments
        .clear();
    let report = catalog.discover(&catalog.request()).unwrap();
    let evaluation = candidate(&report, "refund-change-with-verified-deploy");
    assert_eq!(evaluation.disposition, Disposition::Rejected);
    assert!(evaluation
        .conditions
        .iter()
        .any(|condition| condition.condition == "environment"
            && condition.status == ConditionStatus::Unsatisfied
            && condition.detail.contains("no known environment")));
}

/** A dependency that changed since validation blocks recommendation without touching the registry
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn stale_dependencies_block_without_changing_the_registry() {
    let mut catalog = catalog().unwrap();
    let before = catalog.world.registry.state.clone();
    catalog
        .world
        .current
        .flows
        .insert("refund-flow".into(), "v3".into());
    let report = catalog.discover(&catalog.request()).unwrap();
    assert_eq!(report.outcome, DiscoveryOutcome::Blocked);
    let evaluation = candidate(&report, "refund-change-with-verified-deploy");
    assert_eq!(evaluation.disposition, Disposition::BlockedByDependency);
    assert!(evaluation
        .reasons
        .iter()
        .any(|reason| reason.contains("not yet marked")));
    assert_eq!(
        catalog.world.registry.state, before,
        "discovery never marks, validates, or changes anything"
    );
    assert_eq!(report.governance.flows["refund-flow"], "v3");
}

/** Authorization comes before retrieval; Work Capture context must be readable and intact
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn authorizes_before_retrieval() {
    let catalog = catalog().unwrap();
    let stranger = Actor::new("stranger@acme.example", ActorKind::Human);
    let reader = capability_demo::reader(&stranger).unwrap();
    let sources = Sources {
        capture: &catalog.world.capture.store,
        current: &catalog.world.current,
        reader: &reader,
    };
    let refused = discover(
        &catalog.world.registry,
        &sources,
        &stranger,
        &catalog.request(),
        &catalog.lab_reports,
        NOW,
    );
    assert_eq!(refused.outcome, DiscoveryOutcome::Blocked);
    assert!(
        refused.candidates.is_empty() && refused.context.is_none(),
        "nothing is retrieved for an unauthorized requester"
    );
    let engineer = capability_demo::reader(&catalog.world.cast.engineer).unwrap();
    let mut narrow = engineer.clone();
    narrow.repositories.clear();
    let sources = Sources {
        reader: &narrow,
        ..Sources {
            capture: &catalog.world.capture.store,
            current: &catalog.world.current,
            reader: &engineer,
        }
    };
    let report = discover(
        &catalog.world.registry,
        &sources,
        &catalog.world.cast.engineer,
        &catalog.request(),
        &catalog.lab_reports,
        NOW,
    );
    assert_eq!(report.outcome, DiscoveryOutcome::Blocked);
    assert!(report.reasons[0].contains("may not read"));
    let tampered = DiscoveryRequest {
        context: Some(ContextRef {
            pack_id: catalog.pack.0.clone(),
            pack_digest: "0".repeat(32),
        }),
        ..catalog.request()
    };
    let report = catalog.discover(&tampered).unwrap();
    assert!(report.candidates.is_empty() && report.reasons[0].contains("integrity_mismatch"));
}

/** A failed lab report blocks automatic recommendation; an inconclusive one earns no credit; a
 * passed one ranks first; reports for another digest are ignored
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn surfaces_lab_reports_separately() {
    let catalog: Catalog = catalog().unwrap();
    let report = catalog.discover(&catalog.request()).unwrap();
    let unchecked = candidate(&report, "refund-change-unchecked");
    assert_eq!(unchecked.disposition, Disposition::BlockedByLab);
    assert_eq!(
        unchecked.lab[0].failed_checks,
        vec!["scenario:not-in-staging"]
    );
    let verified = candidate(&report, "refund-change-with-verified-deploy");
    assert_eq!(verified.lab[0].overall, CheckStatus::Inconclusive);
    assert!(verified.lab[0].note.contains("not counted as success"));
    let without_failure = catalog
        .lab_reports
        .iter()
        .filter(|lab| lab.overall != CheckStatus::Failed)
        .cloned()
        .collect::<Vec<_>>();
    let reader = capability_demo::reader(&catalog.world.cast.engineer).unwrap();
    let sources = Sources {
        capture: &catalog.world.capture.store,
        current: &catalog.world.current,
        reader: &reader,
    };
    let two = discover(
        &catalog.world.registry,
        &sources,
        &catalog.world.cast.engineer,
        &catalog.request(),
        &without_failure,
        NOW,
    );
    let order = two
        .recommendations
        .iter()
        .map(|recommendation| recommendation.capability_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        vec![
            "refund-change-unchecked",
            "refund-change-with-verified-deploy"
        ],
        "an inconclusive report earns no ranking credit"
    );
    assert!(candidate(&two, "refund-change-with-verified-deploy")
        .ranking_reasons
        .iter()
        .any(|reason| reason.contains("not counted")));
    let mut passed = without_failure.clone();
    passed[0].overall = CheckStatus::Passed;
    let ranked = discover(
        &catalog.world.registry,
        &sources,
        &catalog.world.cast.engineer,
        &catalog.request(),
        &passed,
        NOW,
    );
    assert_eq!(
        ranked.recommendations[0].capability_id,
        "refund-change-with-verified-deploy"
    );
    let mut elsewhere = catalog.lab_reports.clone();
    for lab in &mut elsewhere {
        lab.candidate_digest = "f".repeat(32);
    }
    let ignored = discover(
        &catalog.world.registry,
        &sources,
        &catalog.world.cast.engineer,
        &catalog.request(),
        &elsewhere,
        NOW,
    );
    assert!(
        ignored
            .candidates
            .iter()
            .all(|candidate| candidate.lab.is_empty()),
        "reports are matched by exact digest"
    );
}

/** An empty registry is no match, not an error
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn empty_registries_have_no_match() {
    let catalog = catalog().unwrap();
    let empty = CapabilityRegistry::in_memory(capability_demo::grants(&catalog.world.cast));
    let reader = capability_demo::reader(&catalog.world.cast.engineer).unwrap();
    let sources = Sources {
        capture: &catalog.world.capture.store,
        current: &catalog.world.current,
        reader: &reader,
    };
    let report = discover(
        &empty,
        &sources,
        &catalog.world.cast.engineer,
        &catalog.request(),
        &[],
        NOW,
    );
    assert_eq!(report.outcome, DiscoveryOutcome::NoMatch);
    assert!(report.reasons[0].contains("holds no capability"));
}

/** The same inputs give the same report, digest, and explanations; stored reports are verified
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn explanations_are_reproducible_and_protected() {
    let first = run().unwrap();
    let second = run().unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    assert_eq!(
        (
            first.with_context.outcome,
            first.unknown_environment.outcome,
            first.staging.outcome,
            first.stale.outcome
        ),
        (
            DiscoveryOutcome::Recommended,
            DiscoveryOutcome::InsufficientContext,
            DiscoveryOutcome::NoMatch,
            DiscoveryOutcome::Blocked
        )
    );
    assert!(first.with_context.verify_digest());
    let directory = tempfile::tempdir().unwrap();
    let store = DiscoveryStore::open(directory.path());
    store.append(&first.with_context).unwrap();
    assert_eq!(store.load().unwrap()[0], first.with_context);
    let mut forged = first.with_context.clone();
    forged.recommendations.clear();
    assert!(store.append(&forged).is_err());
    let path = directory.path().join("discovery").join("reports.jsonl");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"outcome\":\"recommended\"", "\"outcome\":\"no_match\"");
    std::fs::write(&path, text).unwrap();
    assert!(store.load().is_err());
}
