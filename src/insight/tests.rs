// Tests of Insight over the deterministic fixture scenario of Work Capture: the learning loop from
// explorations and feedback to an accepted recipe, minimum support and the viewer's access, zone
// context proposed through the contribution ledger, recurring agent work as a capability
// candidate, and the outcome comparison.

use std::collections::BTreeSet;

use super::miners::fails_clearly_more;
use super::propose::{propose, Proposed};
use super::{report, Destination, PatternKind, MIN_SUPPORT};
use crate::capture::broker::proposals::ProposalStatus;
use crate::capture::broker::run::{EvidenceQueryBroker, EvidenceQueryRequest};
use crate::capture::demo::{self, Scenario};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::contributions::{ContributionStatus, ContributionType, Namespace};
use crate::capture::ingest::episode::EpisodeOutcome;
use crate::capture::ingest::pack::InvestigationStatus;
use crate::capture::{Actor, ActorKind, Clock, Truth};

/** A broker over the scenario with a fixed clock
 * Input
    - scenario: &Scenario - scenario
 * Output
    - EvidenceQueryBroker
*/
fn broker(scenario: &Scenario) -> EvidenceQueryBroker {
    demo::broker(scenario, Clock::fixed(demo::DEMO_EPOCH)).expect("broker builds")
}

/** The question classes the broker's recipes serve
 * Input
    - broker: &EvidenceQueryBroker - broker
 * Output
    - BTreeSet<String>
*/
fn served(broker: &EvidenceQueryBroker) -> BTreeSet<String> {
    broker
        .recipes
        .all()
        .flat_map(|recipe| recipe.investigation_classes.iter().cloned())
        .collect()
}

/** A request in the refund-check class, which no built-in recipe serves
 * Input
    - scenario: &Scenario - scenario
    - n: usize - which asking (each asking is its own question)
 * Output
    - EvidenceQueryRequest
*/
fn refund_check(scenario: &Scenario, n: usize) -> EvidenceQueryRequest {
    let mut request = demo::request(scenario);
    request.recipe = None;
    request.investigation_class = "refund-check".into();
    request.question = format!("Are refunds safe after PAY-142? ({n})");
    request
}

/** The person giving feedback in these tests
 * Input
    - None
 * Output
    - Actor
*/
fn person() -> Actor {
    Actor::new("oncall@acme.example", ActorKind::Human)
}

/** 1. The learning loop: a question no recipe serves is explored three times, people judge two
 * explorations sufficient (one still needing test reports), Insight reports the question and
 * proposes a recipe learned from those two, nothing changes until a reviewer accepts it, and then
 * the class is served by the learned recipe, whose requirements include what people said was
 * missing
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn explorations_and_feedback_become_a_reviewed_recipe() {
    let scenario = demo::scenario().unwrap();
    let policy = demo::engineer(&scenario);
    let mut broker = broker(&scenario);
    let packs = (0..3)
        .map(|n| {
            broker
                .investigate(refund_check(&scenario, n), &policy)
                .unwrap()
                .pack
                .pack_id
        })
        .collect::<Vec<_>>();
    broker
        .store
        .record_feedback(
            &packs[0],
            &person(),
            Truth::Yes,
            vec![EvidenceType::TestReport],
            "would also need the test report",
            1,
        )
        .unwrap();
    broker
        .store
        .record_feedback(&packs[1], &person(), Truth::Yes, Vec::new(), "", 2)
        .unwrap();
    let found = report(&broker.store, &policy, &served(&broker), 10);
    assert!(found.verifies());
    assert_eq!(
        found.digest,
        report(&broker.store, &policy, &served(&broker), 11).digest,
        "the same store and viewer give the same patterns"
    );
    let question = found
        .patterns
        .iter()
        .find(|pattern| pattern.kind == PatternKind::Question)
        .expect("the unserved question is reported");
    assert_eq!(
        (
            question.counts.matching,
            question.counts.total,
            question.counts.unknown
        ),
        (2, 3, 1)
    );
    assert_eq!(
        question.destination,
        Destination::Recipe {
            class: "refund-check".into()
        }
    );
    let mut basis = packs[..2].to_vec();
    basis.sort();
    assert_eq!(question.packs, basis);
    let Proposed::Recipe(id) = propose(&mut broker, question, 20).unwrap() else {
        panic!("a question pattern proposes a recipe");
    };
    let proposal = broker.store.recipe_proposals[&id].clone();
    assert_eq!(proposal.status, ProposalStatus::Proposed);
    assert_eq!(proposal.proposed_by.kind, ActorKind::System);
    assert_eq!(proposal.recipe.recipe_id, "learned-refund-check");
    assert_eq!(proposal.recipe.version, 1);
    assert_eq!(proposal.recipe.investigation_classes, vec!["refund-check"]);
    assert!(!proposal.recipe.steps.is_empty());
    let required = proposal
        .recipe
        .required
        .iter()
        .map(|item| item.evidence_type)
        .collect::<BTreeSet<_>>();
    assert!(
        required.contains(&EvidenceType::Deployment),
        "found by both"
    );
    assert!(required.contains(&EvidenceType::TestReport), "still needed");
    assert!(
        broker.recipes.for_class("refund-check").is_none(),
        "nothing changes until a person accepts it"
    );
    broker
        .review_recipe(
            &policy,
            &id,
            ProposalStatus::Accepted,
            "this is what we check",
        )
        .unwrap();
    let learned = broker
        .investigate(refund_check(&scenario, 3), &policy)
        .unwrap();
    assert_eq!(learned.pack.retrieval.recipe_id, "learned-refund-check");
    assert_eq!(learned.pack.status, InvestigationStatus::Incomplete);
    assert!(learned
        .pack
        .missing_evidence
        .iter()
        .any(|missing| missing.evidence_type == EvidenceType::TestReport));
    let next = report(&broker.store, &policy, &served(&broker), 30);
    assert!(
        next.patterns
            .iter()
            .all(|pattern| pattern.kind != PatternKind::Question),
        "a served class is no longer an unanswered question"
    );
}

/** 2. Minimum support and access: too few cases are suppressed, never reported; a viewer sees only
 * patterns over what they may open, and a kept report that was changed is refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn patterns_need_support_and_respect_access() {
    let scenario = demo::scenario().unwrap();
    let policy = demo::engineer(&scenario);
    let mut broker = broker(&scenario);
    for n in 0..MIN_SUPPORT - 1 {
        broker
            .investigate(refund_check(&scenario, n), &policy)
            .unwrap();
    }
    let found = report(&broker.store, &policy, &served(&broker), 10);
    assert!(found.patterns.is_empty(), "{:?}", found.patterns);
    assert!(found.suppressed > 0);
    let mut elsewhere = policy.clone();
    elsewhere.repositories = BTreeSet::from(["acme/other".to_string()]);
    let unseen = report(&broker.store, &elsewhere, &served(&broker), 10);
    assert_eq!(unseen.considered.packs, 0);
    assert_eq!(unseen.considered.audits, 0);
    let mut staging = policy.clone();
    staging.environments = BTreeSet::from(["staging".to_string()]);
    assert_eq!(
        report(&broker.store, &staging, &served(&broker), 10)
            .considered
            .packs,
        0,
        "packs about production are not a staging viewer's to count"
    );
    let directory = tempfile::tempdir().unwrap();
    super::store::keep(directory.path(), &found).unwrap();
    assert_eq!(
        super::store::reports(directory.path()).unwrap(),
        vec![found.clone()]
    );
    let mut changed = found;
    changed.suppressed += 1;
    super::store::keep(directory.path(), &changed).unwrap();
    assert!(super::store::reports(directory.path()).is_err());
}

/** 3. Evidence a zone's investigations keep lacking becomes system-derived context for the zone,
 * proposed in the contribution ledger, which a person then reviews; the zone's trust is reported
 * beside it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn zone_gaps_become_reviewed_context() {
    let scenario = demo::scenario().unwrap();
    let policy = demo::engineer(&scenario);
    let mut broker = broker(&scenario);
    for n in 0..MIN_SUPPORT {
        let mut request = demo::request(&scenario);
        request.question = format!("Why did dep-7731 fail? ({n})");
        broker.investigate(request, &policy).unwrap();
    }
    let found = report(&broker.store, &policy, &served(&broker), 10);
    let gap = found
        .patterns
        .iter()
        .find(|pattern| {
            pattern.kind == PatternKind::EvidenceGap
                && pattern.subject == "zone refunds: TestReport"
        })
        .expect("the missing test reports are a pattern");
    assert_eq!((gap.counts.matching, gap.counts.total), (3, 3));
    assert_eq!(
        gap.destination,
        Destination::Context {
            zone: "refunds".into(),
            version: Some("v3".into())
        }
    );
    assert!(!gap.evidence.is_empty());
    let trust = found
        .patterns
        .iter()
        .find(|pattern| pattern.kind == PatternKind::Trust)
        .expect("the zone's trust is reported");
    assert_eq!(trust.counts.matching, 0);
    assert_eq!(trust.destination, Destination::Report);
    let Proposed::Context(id) = propose(&mut broker, gap, 20).unwrap() else {
        panic!("a zone gap proposes context");
    };
    let contribution = broker.store.contributions.contributions[&id].clone();
    assert_eq!(contribution.namespace, Namespace::SystemDerived);
    assert_eq!(
        contribution.contribution_type,
        ContributionType::Observation
    );
    assert_eq!(contribution.status, ContributionStatus::Proposed);
    assert_eq!(contribution.contributor_id, "crane-insight");
    broker
        .store
        .review(&policy, &id, ContributionStatus::Accepted, "true", 30)
        .unwrap();
    assert_eq!(
        broker.store.contributions.contributions[&id].namespace,
        Namespace::HumanApproved
    );
    assert!(
        propose(&mut broker, trust, 40).is_err(),
        "a trust pattern is for people to act on"
    );
}

/** 4. An agent tool sequence that recurs across work episodes is a capability candidate, compiled
 * from an episode whose work succeeded; Insight itself proposes nothing for it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn recurring_agent_work_is_a_capability_candidate() {
    let scenario = {
        let mut scenario = demo::scenario().unwrap();
        let agent = scenario
            .resources
            .iter_mut()
            .find(|resource| resource.resource_id == "datasea")
            .unwrap();
        let mut read = agent.records[1].clone();
        read.record_id = "evt-903".into();
        read.content["tool"] = "Read".into();
        read.source_timestamp = read.source_timestamp.map(|at| at + 100_000);
        read.source_revision = Some("d-903".into());
        agent.records.push(read);
        scenario
    };
    let policy = demo::engineer(&scenario);
    let mut broker = broker(&scenario);
    let mut episodes = Vec::new();
    for n in 0..MIN_SUPPORT {
        let mut request = demo::request(&scenario);
        request.question = format!("What did the agent do? ({n})");
        request.episode_id = Some(format!("epi-work-{n}"));
        let result = broker.investigate(request, &policy).unwrap();
        episodes.push((result.pack.episode_id.clone().unwrap(), result.pack));
    }
    let (succeeded, pack) = &episodes[1];
    let deployment = pack
        .evidence
        .iter()
        .filter(|item| item.evidence_type == EvidenceType::Deployment)
        .map(|item| item.evidence_id.clone())
        .collect::<Vec<_>>();
    broker
        .store
        .record_outcome(
            succeeded,
            &person(),
            true,
            EpisodeOutcome::Succeeded,
            deployment,
            50,
        )
        .unwrap();
    let found = report(&broker.store, &policy, &served(&broker), 60);
    let work = found
        .patterns
        .iter()
        .find(|pattern| pattern.kind == PatternKind::Work)
        .expect("the recurring sequence is reported");
    assert_eq!(work.subject, "Edit > Bash > Read");
    assert_eq!(work.counts.matching, MIN_SUPPORT);
    assert_eq!(
        work.destination,
        Destination::Capability {
            episode: succeeded.clone()
        }
    );
    let refused = propose(&mut broker, work, 70).unwrap_err();
    assert!(refused.message.contains("crane compile episode"));
}

/** 5. An outcome pattern needs a failure rate clearly above the rate of all work
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn outcome_patterns_need_a_clear_difference() {
    assert!(fails_clearly_more(4, 6, 2, 10), "67% against 20%");
    assert!(
        !fails_clearly_more(2, 6, 2, 10),
        "33% against 20% is within the margin"
    );
    assert!(!fails_clearly_more(3, 10, 3, 10), "the same rate");
    assert!(
        fails_clearly_more(2, 5, 2, 10),
        "40% against 20% is exactly the margin"
    );
}
