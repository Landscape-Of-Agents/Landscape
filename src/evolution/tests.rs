// Unit tests of the evolution engine: lifecycles, outcome normalization and deduplication, signal
// detection and qualification, drift, proposals and their gate, regression, authorization, and
// store integrity. Worlds come from the deterministic demonstration fixtures.

use super::demo::{self, seeded, Bench};
use super::models::{
    advance, Change, ChangeKind, Lifecycle, OutcomeClass, ProposalState, RunState, Sealed,
    SignalKind, SignalState, Thresholds,
};
use super::reports::{parse, EvolutionStore};
use crate::capability::lifecycle::{LifecycleState, PublicationRequest};
use crate::capability::model::RiskLevel;
use crate::capability::ErrorCode;
use crate::capability_lab::scenarios::TestPlan;
use crate::capture::{AccessPolicy, Actor, ActorKind};
use crate::invocation::lifecycle::InvocationState;

/** The capability the fixtures evolve */
const CAPABILITY: &str = "deployment-failure-diagnostics";

/** Analyze version 1 as the engineer
 * Input
    - bench: &mut Bench - bench
 * Output
    - super::workflow::Analysis
*/
fn analyze(bench: &mut Bench) -> super::workflow::Analysis {
    let engineer = bench.world.cast.engineer.clone();
    bench
        .evolution()
        .analyze(&engineer, CAPABILITY, Some(1), Thresholds::default())
        .unwrap()
}

/** The repeated-failure signal of a repository
 * Input
    - analysis: &super::workflow::Analysis - analysis
    - repository: &str - repository
 * Output
    - super::models::Signal
*/
fn repeated(analysis: &super::workflow::Analysis, repository: &str) -> super::models::Signal {
    analysis
        .signals
        .iter()
        .find(|signal| {
            signal.kind == SignalKind::RepeatedFailure
                && signal
                    .context_key
                    .contains(&format!("repository={repository};"))
        })
        .cloned()
        .unwrap()
}

/** A suggestion citing the signal's supporting invocations
 * Input
    - change: ChangeKind - change
    - signal: &super::models::Signal - signal
 * Output
    - Change
*/
fn suggest(change: ChangeKind, signal: &super::models::Signal) -> Change {
    Change {
        change,
        rationale: "test".into(),
        evidence: signal.supporting.clone(),
        origin: "suggestion".into(),
    }
}

/** The demonstration matches every expected outcome
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn demo_matches_every_case() {
    let report = demo::run().unwrap();
    for case in report.cases.iter().filter(|case| !case.matched) {
        println!(
            "{} {}: expected {} got {}",
            case.case, case.name, case.expected, case.actual
        );
    }
    assert_eq!(report.cases.len(), 15);
    assert!(report.all_matched);
}

/** The three lifecycle tables allow only their transitions and terminal states end them
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn lifecycle_tables() {
    use ProposalState as P;
    use RunState as R;
    use SignalState as S;
    let signals = [
        S::SignalDetected,
        S::Qualifying,
        S::InsufficientEvidence,
        S::Qualified,
        S::Investigating,
        S::ProposalCreated,
        S::NoChangeRecommended,
        S::Dismissed,
        S::Superseded,
    ];
    assert!(S::SignalDetected.allows(S::Qualifying));
    assert!(
        !S::SignalDetected.allows(S::Qualified),
        "detected is never qualified directly"
    );
    assert!(!S::InsufficientEvidence.allows(S::Qualified));
    assert!(S::Investigating.allows(S::ProposalCreated));
    for state in signals.iter().filter(|state| state.terminal()) {
        assert!(signals.iter().all(|to| !state.allows(*to)), "{state:?}");
    }
    let proposals = [
        P::Draft,
        P::CandidateCreated,
        P::RegressionPending,
        P::RegressionPassed,
        P::RegressionFailed,
        P::Inconclusive,
        P::Blocked,
        P::SubmittedForReview,
        P::Rejected,
        P::Superseded,
        P::Abandoned,
    ];
    assert!(!P::Draft.allows(P::RegressionPassed));
    assert!(!P::Inconclusive.allows(P::SubmittedForReview));
    assert!(
        !P::RegressionFailed.allows(P::RegressionPassed),
        "a retry is a new run"
    );
    assert!(P::RegressionPassed.allows(P::SubmittedForReview));
    for state in proposals.iter().filter(|state| state.terminal()) {
        assert!(proposals.iter().all(|to| !state.allows(*to)), "{state:?}");
    }
    let runs = [
        R::Created,
        R::Preflight,
        R::Running,
        R::Passed,
        R::Failed,
        R::Inconclusive,
        R::Blocked,
        R::Cancelled,
    ];
    assert!(!R::Created.allows(R::Running));
    assert!(
        !R::Running.allows(R::Blocked),
        "blocked is decided in preflight"
    );
    for state in runs.iter().filter(|state| state.terminal()) {
        assert!(runs.iter().all(|to| !state.allows(*to)), "{state:?}");
    }
    let actor = Actor::new("someone", ActorKind::Human);
    let mut state = S::SignalDetected;
    let mut history = Vec::new();
    assert_eq!(
        advance(&mut state, &mut history, S::Qualified, &actor, "skip", 1)
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
    assert!(history.is_empty() && state == S::SignalDetected);
}

/** Every invocation state maps to an evolution class without changing the record; declared
 * success never becomes verified, contradictions stay explicit
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn outcome_normalization() {
    let bench = seeded().unwrap();
    let definition = bench
        .world
        .registry
        .state
        .record(CAPABILITY, 1)
        .unwrap()
        .definition
        .clone();
    let sources = crate::capability::refs::Sources {
        capture: &bench.world.capture.store,
        current: &bench.world.current,
        reader: &bench.reader,
    };
    let success = bench
        .records
        .iter()
        .find(|record| record.idempotency_key == "ra-4")
        .unwrap()
        .clone();
    let before = success.clone();
    let classify = |state: InvocationState, clear: bool| {
        let mut record = success.clone();
        record.state = state;
        if clear {
            record.verifications.clear();
        }
        super::outcomes::normalize(&record, &definition, &sources, 0, false).class
    };
    assert_eq!(
        classify(InvocationState::Succeeded, false),
        OutcomeClass::VerifiedSuccess
    );
    assert_eq!(
        classify(InvocationState::Succeeded, true),
        OutcomeClass::DeclaredSuccessUnverified
    );
    assert_eq!(
        classify(InvocationState::PartiallyVerified, false),
        OutcomeClass::PartiallyVerified
    );
    assert_eq!(
        classify(InvocationState::Unknown, true),
        OutcomeClass::Unknown
    );
    assert_eq!(
        classify(InvocationState::ReconciliationRequired, false),
        OutcomeClass::ReconciliationUnresolved
    );
    assert_eq!(
        classify(InvocationState::Abandoned, false),
        OutcomeClass::Abandoned
    );
    assert_eq!(
        classify(InvocationState::Blocked, false),
        OutcomeClass::BlockedBeforeExecution
    );
    assert_eq!(
        classify(InvocationState::Failed, true),
        OutcomeClass::Unknown,
        "a failure without a failed check or operation is not established"
    );
    for state in [
        InvocationState::Requested,
        InvocationState::Authorizing,
        InvocationState::Preflighting,
        InvocationState::PlanReady,
        InvocationState::AwaitingApproval,
        InvocationState::Executing,
        InvocationState::Verifying,
    ] {
        assert_eq!(classify(state, false), OutcomeClass::InProgress);
    }
    assert_eq!(success, before, "the source record is never changed");
    let failed = bench
        .records
        .iter()
        .find(|record| record.idempotency_key == "pay-1")
        .unwrap();
    assert_eq!(
        super::outcomes::normalize(failed, &definition, &sources, 0, false).class,
        OutcomeClass::FailedVerification
    );
    let contradicted = bench
        .records
        .iter()
        .find(|record| record.idempotency_key == "ra-5")
        .unwrap();
    let observation = super::outcomes::normalize(contradicted, &definition, &sources, 0, false);
    assert_eq!(observation.class, OutcomeClass::Contradicted);
    assert!(!observation.contradicted_claims.is_empty());
}

/** Retries, resubmissions, and events never inflate independent counts; incomparable contexts are
 * never pooled
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn deduplication_and_partitions() {
    let mut bench = seeded().unwrap();
    let analysis = analyze(&mut bench);
    let coverage = &analysis.report.coverage;
    assert_eq!(coverage.duplicates_collapsed, 2);
    assert!(coverage.events_correlated > coverage.records_visible);
    let payments = repeated(&analysis, "acme/payments");
    assert_eq!(payments.independent_count, 3);
    assert_eq!(payments.collapsed.len(), 2);
    let refunds = repeated(&analysis, "acme/refunds-api");
    assert_eq!(
        refunds.independent_count, 3,
        "the two contexts' failures are not pooled into six"
    );
    assert!(analysis.report.partitions.len() >= 3);
}

/** Restricted evidence is counted, never read, and leaves a pattern unqualified
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn restricted_evidence_is_not_a_finding() {
    let mut bench = seeded().unwrap();
    bench.reader = AccessPolicy {
        evidence_types: Some(Default::default()),
        ..bench.reader.clone()
    };
    let analysis = analyze(&mut bench);
    assert!(analysis.report.coverage.restricted_evidence > 0);
    let payments = repeated(&analysis, "acme/payments");
    assert_eq!(payments.state, SignalState::InsufficientEvidence);
    assert!(!payments.missing_evidence.is_empty());
}

/** A materially different evidence set gives a new signal and supersedes the old one; the old
 * report stays
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn new_evidence_supersedes_without_erasing() {
    let mut bench = seeded().unwrap();
    let first = analyze(&mut bench);
    let old = repeated(&first, "acme/payments");
    let reader = bench.reader.clone();
    let mut request = crate::invocation::demo::diagnostics_request(&bench.world, "pay-4");
    request.task_id = Some("PAY-204".into());
    request.repository = Some("acme/payments".into());
    request
        .inputs
        .insert("deployment_id".into(), "dep-8004".into());
    bench
        .world
        .set_status(crate::invocation::demo::status_fixture("failed", false));
    let actor = bench.world.cast.engineer.clone();
    bench.world.engine(&reader).submit(&actor, request).unwrap();
    let ledger = bench
        .world
        .ledger
        .state
        .records
        .values()
        .cloned()
        .collect::<Vec<_>>();
    bench
        .records
        .retain(|record| bench.info.synthetic.contains(&record.invocation_id));
    bench.records.extend(ledger);
    let second = analyze(&mut bench);
    let new = repeated(&second, "acme/payments");
    assert_ne!(new.signal_id, old.signal_id);
    assert_eq!(new.independent_count, 4);
    let stored = bench.store.signal(&old.signal_id).unwrap();
    assert_eq!(stored.state, SignalState::Superseded);
    assert_eq!(
        stored.superseded_by.as_deref(),
        Some(new.signal_id.as_str())
    );
    assert!(bench
        .store
        .state
        .analyses
        .contains_key(&first.report.report_id));
}

/** Thresholds are configurable and raised for higher risk
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn qualification_thresholds() {
    let mut bench = seeded().unwrap();
    let engineer = bench.world.cast.engineer.clone();
    let strict = Thresholds {
        min_independent: 4,
        ..Thresholds::default()
    };
    let analysis = bench
        .evolution()
        .analyze(&engineer, CAPABILITY, Some(1), strict)
        .unwrap();
    assert_eq!(
        repeated(&analysis, "acme/payments").state,
        SignalState::InsufficientEvidence
    );
    let detected = super::patterns::detect(
        &analysis.report.observations,
        &analysis.report.capability_digest,
        &analysis.report.drift,
    );
    let payments = detected
        .iter()
        .find(|signal| {
            signal.kind == SignalKind::RepeatedFailure
                && signal.context_key.contains("acme/payments")
        })
        .unwrap();
    let pins = Default::default();
    assert!(
        super::patterns::qualify(payments, RiskLevel::Low, &pins, Thresholds::default()).qualified
    );
    assert!(
        !super::patterns::qualify(payments, RiskLevel::Critical, &pins, Thresholds::default())
            .qualified
    );
}

/** Drift is reported without rewriting constraints; a policy change is never re-pinned
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn drift_and_policy_changes() {
    let mut bench = seeded().unwrap();
    bench
        .world
        .current
        .policies
        .insert("refund_safety".into(), "ffffffffffffffff".into());
    let parent = bench
        .world
        .registry
        .state
        .record(CAPABILITY, 1)
        .unwrap()
        .clone();
    let analysis = analyze(&mut bench);
    let policy = analysis
        .signals
        .iter()
        .find(|signal| {
            signal.kind == SignalKind::DependencyDrift && signal.context_key.contains("policy")
        })
        .unwrap()
        .clone();
    let engineer = bench.world.cast.engineer.clone();
    let proposal = bench
        .evolution()
        .propose(&engineer, &policy.signal_id, Vec::new())
        .unwrap();
    assert_eq!(proposal.state, ProposalState::Blocked);
    assert!(proposal.unsupported[0].contains("never edits or re-pins policies"));
    assert_eq!(
        bench.world.registry.state.record(CAPABILITY, 1).unwrap(),
        &parent
    );
    assert_eq!(bench.world.registry.state.versions[CAPABILITY].len(), 1);
}

/** The gate refuses risk downgrades, verification removal, self-verification, and changes
 * without the signal's evidence
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn gate_refuses_weakening() {
    let mut bench = seeded().unwrap();
    let analysis = analyze(&mut bench);
    let signal = repeated(&analysis, "acme/payments");
    let parent = bench
        .world
        .registry
        .state
        .record(CAPABILITY, 1)
        .unwrap()
        .definition
        .clone();
    let methods = Default::default();
    let context = super::proposals::GateContext {
        parent: &parent,
        signal: &signal,
        current: &bench.world.current,
        observed_methods: &methods,
    };
    let mut unsupported = vec![
        suggest(ChangeKind::LowerRisk { to: RiskLevel::Low }, &signal),
        suggest(
            ChangeKind::RemoveClaim {
                claim_id: "logs-retrieved".into(),
            },
            &signal,
        ),
        suggest(
            ChangeKind::MarkClaimVerified {
                claim_id: "logs-retrieved".into(),
            },
            &signal,
        ),
        suggest(
            ChangeKind::AddVerificationRequirement {
                claim_id: "new".into(),
                statement: "s".into(),
                expected_result: "r".into(),
                method: "invented".into(),
            },
            &signal,
        ),
        suggest(ChangeKind::RaiseRisk { to: RiskLevel::Low }, &signal),
    ];
    unsupported.push(Change {
        evidence: vec!["inv_unrelated".into()],
        ..suggest(ChangeKind::AddLimitation { text: "x".into() }, &signal)
    });
    unsupported.push(Change {
        evidence: Vec::new(),
        ..suggest(ChangeKind::AddLimitation { text: "x".into() }, &signal)
    });
    assert!(super::proposals::gate(&unsupported, &context)
        .iter()
        .all(|decision| !decision.allowed));
    let allowed = super::proposals::gate(
        &[suggest(
            ChangeKind::RaiseRisk {
                to: RiskLevel::High,
            },
            &signal,
        )],
        &context,
    );
    assert!(allowed[0].allowed);
    let mut weaker = parent.clone();
    weaker.applicability.risk = RiskLevel::Low;
    weaker.verification.claims.pop();
    weaker.applicability.environments.push("staging".into());
    let found = super::proposals::weakenings(&parent, &weaker);
    assert!(found.iter().any(|item| item.contains("claim")));
    assert!(found.iter().any(|item| item.contains("staging")));
}

/** A proposal creates a linked candidate through the registry, keeps provenance, leaves the
 * parent unchanged, and recovers from an interrupted creation without a duplicate
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn proposal_creates_candidate_and_recovers() {
    let mut bench = seeded().unwrap();
    let analysis = analyze(&mut bench);
    let signal = repeated(&analysis, "acme/payments");
    let parent = bench
        .world
        .registry
        .state
        .record(CAPABILITY, 1)
        .unwrap()
        .clone();
    let engineer = bench.world.cast.engineer.clone();
    let proposal = bench
        .evolution()
        .propose(&engineer, &signal.signal_id, Vec::new())
        .unwrap();
    let link = proposal.candidate.clone().unwrap();
    let candidate = bench
        .world
        .registry
        .state
        .record(CAPABILITY, link.version)
        .unwrap()
        .clone();
    assert_eq!(candidate.state, LifecycleState::Candidate);
    assert_eq!(candidate.definition.previous_version_ref, Some(1));
    assert_eq!(candidate.definition_digest, link.digest);
    assert_eq!(
        candidate.definition.provenance.source_evidence_pack_refs,
        parent.definition.provenance.source_evidence_pack_refs
    );
    assert!(candidate
        .definition
        .provenance
        .change_reason
        .as_deref()
        .unwrap()
        .contains(&proposal.proposal_id));
    assert_eq!(
        candidate.definition.applicability.risk,
        parent.definition.applicability.risk
    );
    assert_eq!(
        bench.world.registry.state.record(CAPABILITY, 1).unwrap(),
        &parent
    );
    let mut interrupted = proposal.clone();
    interrupted.state = ProposalState::Draft;
    interrupted.transitions.truncate(1);
    interrupted.candidate = None;
    interrupted.seal();
    bench
        .store
        .state
        .proposals
        .insert(interrupted.proposal_id.clone(), interrupted);
    let recovered = bench
        .evolution()
        .propose(&engineer, &signal.signal_id, Vec::new())
        .unwrap();
    assert_eq!(recovered.state, ProposalState::CandidateCreated);
    assert_eq!(recovered.candidate.unwrap().version, link.version);
    assert!(recovered
        .transitions
        .last()
        .unwrap()
        .reason
        .contains("recovered"));
    assert_eq!(bench.world.registry.state.versions[CAPABILITY].len(), 2);
}

/** Regression runs bind exact digests, retries are new linked runs, a passed run never publishes,
 * and a changed context invalidates it for review
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn regression_runs() {
    let mut bench = seeded().unwrap();
    let analysis = analyze(&mut bench);
    let signal = repeated(&analysis, "acme/payments");
    let engineer = bench.world.cast.engineer.clone();
    let proposal = bench
        .evolution()
        .propose(&engineer, &signal.signal_id, Vec::new())
        .unwrap();
    let (first, labs) = bench
        .evolution()
        .regression_run(&engineer, &proposal.proposal_id)
        .unwrap();
    for check in first.checks.iter().filter(|check| {
        check.required && check.status != crate::capability_lab::CheckStatus::Passed
    }) {
        println!("{} {:?} {}", check.check_id, check.status, check.detail);
    }
    assert_eq!(first.state, RunState::Passed, "{}", first.reason);
    assert_eq!(labs.len(), 2);
    assert_eq!(
        labs[1].candidate_digest,
        proposal.candidate.as_ref().unwrap().digest
    );
    assert_eq!(
        first.candidate_digest,
        proposal.candidate.as_ref().unwrap().digest
    );
    assert_eq!(first.parent_digest, proposal.parent_digest);
    assert!(first.verify());
    let (second, _) = bench
        .evolution()
        .regression_run(&engineer, &proposal.proposal_id)
        .unwrap();
    assert_ne!(second.run_id, first.run_id);
    assert_eq!(second.previous_run.as_deref(), Some(first.run_id.as_str()));
    assert_eq!(second.inputs_changed, Some(false));
    assert_eq!(
        bench.store.run(&first.run_id).unwrap(),
        first,
        "an earlier run is never changed"
    );
    assert_eq!(
        bench
            .world
            .registry
            .state
            .record(CAPABILITY, 2)
            .unwrap()
            .state,
        LifecycleState::Candidate
    );
    bench
        .world
        .current
        .flows
        .insert("refund-flow".into(), "v3".into());
    let refused = bench
        .evolution()
        .prepare_review(&engineer, &proposal.proposal_id)
        .unwrap_err();
    assert!(refused.message.contains("no longer matches"), "{refused}");
    let unknown = r#"{"plan_id":"p","version":1,"description":"d","scenarios":[{"scenario_id":"s","description":"d","check":{"kind":"execute","command":"rm -rf /"}}]}"#;
    assert!(TestPlan::parse(unknown).is_err());
}

/** Authorization: no role reads nothing; agents see only their own invocations, cannot dismiss,
 * and cannot publish what they propose
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn authorization_boundaries() {
    let mut bench = seeded().unwrap();
    let stranger = Actor::new("stranger@example", ActorKind::Human);
    let refused = bench
        .evolution()
        .analyze(&stranger, CAPABILITY, Some(1), Thresholds::default())
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Unauthorized);
    assert!(bench.store.state.analyses.is_empty());
    let agent = bench.world.cast.agent.clone();
    bench.reader = AccessPolicy {
        actor: agent.clone(),
        ..bench.reader.clone()
    };
    let analysis = bench
        .evolution()
        .analyze(&agent, CAPABILITY, Some(1), Thresholds::default())
        .unwrap();
    assert_eq!(analysis.report.coverage.records_visible, 0);
    assert!(analysis.report.coverage.records_not_visible > 0);
    let engineer = bench.world.cast.engineer.clone();
    bench.reader = AccessPolicy {
        actor: engineer.clone(),
        ..bench.reader.clone()
    };
    let full = analyze(&mut bench);
    let signal = repeated(&full, "acme/payments");
    assert_eq!(
        bench
            .evolution()
            .dismiss(&agent, &signal.signal_id, "noise")
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let proposal = bench
        .evolution()
        .propose(&agent, &signal.signal_id, Vec::new())
        .unwrap();
    let version = proposal.candidate.unwrap().version;
    assert!(bench
        .world
        .registry
        .publish(
            &agent,
            &PublicationRequest {
                capability_id: CAPABILITY.into(),
                version,
                reason: "self".into(),
                acknowledged_warnings: Vec::new(),
            },
            1,
        )
        .is_err());
    assert_eq!(
        bench
            .world
            .registry
            .state
            .record(CAPABILITY, version)
            .unwrap()
            .state,
        LifecycleState::Candidate
    );
}

/** Corrupt, incompatible, or tampered data fails closed: the store, a persisted file, and an
 * invocation record
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn integrity_fails_closed() {
    let mut bench = seeded().unwrap();
    analyze(&mut bench);
    let text = serde_json::to_string(&bench.store.state).unwrap();
    assert!(parse(&text).is_ok());
    let newer = text.replacen("\"schema_version\":1", "\"schema_version\":9", 1);
    assert_eq!(
        parse(&newer).unwrap_err().code,
        ErrorCode::UnsupportedSchemaVersion
    );
    assert_eq!(
        parse(&text[..text.len() / 2]).unwrap_err().code,
        ErrorCode::Persistence
    );
    let directory =
        std::env::temp_dir().join(format!("crane-evolution-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut store = EvolutionStore::open(&directory).unwrap();
    let state = bench.store.state.clone();
    store
        .transact(|stored| {
            *stored = state;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        EvolutionStore::open(&directory)
            .unwrap()
            .state
            .signals
            .len(),
        bench.store.state.signals.len()
    );
    let file = directory.join("state.json");
    let saved = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        saved.replacen("\"independent_count\": 3", "\"independent_count\": 9", 1),
    )
    .unwrap();
    assert_eq!(
        EvolutionStore::open(&directory).unwrap_err().code,
        ErrorCode::IntegrityMismatch
    );
    std::fs::remove_dir_all(&directory).ok();
    bench.records[0]
        .failures
        .push("edited outside Crane".into());
    let engineer = bench.world.cast.engineer.clone();
    let tampered = bench
        .evolution()
        .analyze(
            &engineer,
            CAPABILITY,
            Some(1),
            Thresholds {
                min_independent: 2,
                ..Thresholds::default()
            },
        )
        .unwrap_err();
    assert_eq!(tampered.code, ErrorCode::IntegrityMismatch);
}
