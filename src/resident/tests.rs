// Unit tests of resident agents: definition and assignment validation, lifecycle and expiry, zero
// model calls while idle, trigger normalization and deduplication, filtering and cooldown, the
// event-to-idle round trip, restart recovery and leases, the context cache, authorization and
// revocation between planning and execution, approval binding, RCA evidence basis, plan
// validation, outcomes that need evidence, unavailable and uncertain effects, dry runs without side
// effects, and durable human waits. Every test uses a fake clock, fixtures, a counting fake model,
// and a fake Slack adapter: nothing reaches a real service.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use super::authority::{Authority, Decision, SignedAssignment, TargetFacts, Trust};
use super::context::{Cache, PackCache};
use super::model::{
    AssignmentDraft, Definition, DefinitionState, Lifecycle, Limits, MissingContext, RcaRef,
    StepPolicy, Subscription, SubscriptionScope, Timer, TriggerSource,
};
use super::plan::{self, Plan, StepType};
use super::rca::{
    self, Basis, CheckEnv, CheckSpec, Edge, Graph, GraphReasoner, Investigation, Node, NodeKind,
    RcaState, ReasonInput, Reasoner, Relation, SignalMatch,
};
use super::registry;
use super::store::{ActivationState, ResidentStore, StepState};
use super::trigger::{self, Trigger};
use super::worker::{self, ActionAdapter, Dispatch, Env, Via};
use crate::brick::context::Snapshot;
use crate::brick::model::Author;
use crate::governance::zones::{Ceiling, Criticality, Lifecycle as TargetLifecycle};

/** The fake clock's start (Unix milliseconds) */
const NOW: u64 = 1_800_000_000_000;

/** One minute in milliseconds */
const MINUTE: u64 = 60_000;

/** A person with a signing key
 * Input
    - name: &str - name
 * Output
    - Author
*/
fn person(name: &str) -> Author {
    Author {
        name: name.into(),
        key: Some(format!("key-{name}")),
    }
}

/** The signers of the fixture repository
 * Input
    - None
 * Output
    - Vec<String>
*/
fn signers() -> Vec<String> {
    vec!["key-alice".into(), "key-bob".into()]
}

/** A verified Crane view: claude is assigned to the delegated, routine flow:checkout
 * Input
    - None
 * Output
    - Authority
*/
fn authority() -> Authority {
    Authority {
        trust: Trust::Verified,
        generation: 7,
        governance_digest: "gov-1".into(),
        bricks_digest: "bricks-1".into(),
        assignments: vec![SignedAssignment {
            agent: "claude".into(),
            targets: vec!["flow:checkout".into()],
            active: true,
        }],
        targets: BTreeMap::from([(
            "flow:checkout".to_string(),
            TargetFacts {
                status: TargetLifecycle::Active,
                ceiling: Ceiling::Delegated,
                criticality: Criticality::Routine,
                approval: false,
                read_only: false,
                fail_closed: Vec::new(),
            },
        )]),
        connectors: BTreeMap::from([("slack".to_string(), true)]),
    }
}

/** A definition watching CI of the checkout flow
 * Input
    - auto_slack: bool - NOTIFY_SLACK may run without a per-activation approval
 * Output
    - Definition
*/
fn definition(auto_slack: bool) -> Definition {
    let policy = |step_type, auto| StepPolicy { step_type, auto };
    Definition {
        definition_id: "ci-watch".into(),
        version: 1,
        name: "Checkout CI watch".into(),
        purpose: "Respond to failed checkout CI runs.".into(),
        owner: "payments".into(),
        bricks: Vec::new(),
        triggers: vec![
            TriggerSource::Ci,
            TriggerSource::Webhook,
            TriggerSource::CraneEvent,
            TriggerSource::Timer,
        ],
        actions: vec![
            policy(StepType::ReadCheck, true),
            policy(StepType::NotifySlack, auto_slack),
            policy(StepType::Verify, true),
            policy(StepType::AskUser, false),
            policy(StepType::ProposeRca, true),
            policy(StepType::ProposeActionPlan, false),
            policy(StepType::RequestApproval, false),
            policy(StepType::WaitForEvent, true),
            policy(StepType::CreateTicket, true),
        ],
        verification_required: true,
        on_missing_context: MissingContext::Block,
        limits: Limits {
            cooldown_minutes: 0,
            max_model_calls: 2,
            ..Limits::default()
        },
        remote_decisions: false,
        distinct_approver: false,
        created_by: Author::default(),
        created_at: 0,
    }
}

/** A node
 * Input
    - id: &str - id
    - kind: NodeKind - kind
    - status: Basis - basis
 * Output
    - Node
*/
fn node(id: &str, kind: NodeKind, status: Basis) -> Node {
    Node {
        id: id.into(),
        kind,
        label: id.into(),
        status,
        basis: "fixture".into(),
        source: "test".into(),
        matches: None,
        check: None,
        steps: Vec::new(),
    }
}

/** An edge
 * Input
    - from: &str - from
    - to: &str - to
    - relation: Relation - relation
 * Output
    - Edge
*/
fn edge(from: &str, to: &str, relation: Relation) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        relation,
    }
}

/** The checkout CI graph: a failed run may come from a flaky test (confirmed when the job is
 * "tests", contradicted when infrastructure is down), mitigated by a Slack notice that is verified
 * by the signed state verifying
 * Input
    - None
 * Output
    - Graph
*/
fn checkout_graph() -> Graph {
    let mut signal = node("ci-failed", NodeKind::Signal, Basis::Observed);
    signal.matches = Some(SignalMatch {
        source: TriggerSource::Ci,
        kinds: vec!["ci.failed".into()],
    });
    let mut tests_job = node("tests-job", NodeKind::Check, Basis::Observed);
    tests_job.check = Some(CheckSpec::TriggerAttribute {
        key: "job".into(),
        equals: "tests".into(),
    });
    let mut infra = node("infra-down", NodeKind::Check, Basis::Observed);
    infra.check = Some(CheckSpec::TriggerAttribute {
        key: "infra".into(),
        equals: "down".into(),
    });
    let mut notify = node("notify", NodeKind::Remediation, Basis::Inferred);
    notify.steps = vec![rca::step(
        "notify",
        StepType::NotifySlack,
        &[(
            "text",
            "Checkout tests failed; likely the flaky test".into(),
        )],
        &[],
        String::new(),
    )];
    let mut verified = node("state-ok", NodeKind::Verification, Basis::Observed);
    verified.check = Some(CheckSpec::SignedStateVerifies);
    Graph {
        rca_id: "checkout-ci".into(),
        version: 1,
        name: "Checkout CI failures".into(),
        target: "flow:checkout".into(),
        nodes: vec![
            signal,
            node("flaky", NodeKind::Cause, Basis::Hypothesized),
            tests_job,
            infra,
            notify,
            verified,
        ],
        edges: vec![
            edge("ci-failed", "flaky", Relation::MayCause),
            edge("flaky", "tests-job", Relation::ConfirmedBy),
            edge("flaky", "infra-down", Relation::ContradictedBy),
            edge("flaky", "notify", Relation::MitigatedBy),
            edge("notify", "state-ok", Relation::VerifiedBy),
        ],
    }
}

/** A fake Slack adapter: records every message and answers from a queue (default: delivered)
 * Fields
    - sent: Rc<RefCell<Vec<String>>> - messages
    - answers: Rc<RefCell<VecDeque<Dispatch>>> - queued answers
*/
struct FakeSlack {
    sent: Rc<RefCell<Vec<String>>>,
    answers: Rc<RefCell<VecDeque<Dispatch>>>,
}

impl ActionAdapter for FakeSlack {
    /** NOTIFY_SLACK
     * Input
        - None (uses self)
     * Output
        - StepType
    */
    fn step_type(&self) -> StepType {
        StepType::NotifySlack
    }

    /** Record the message and answer
     * Input
        - text: &str - message
        - _key: &str - idempotency key
     * Output
        - Dispatch
    */
    fn dispatch(&self, text: &str, _key: &str) -> Dispatch {
        self.sent.borrow_mut().push(text.into());
        self.answers
            .borrow_mut()
            .pop_front()
            .unwrap_or(Dispatch::Done {
                external_ref: "slack:HTTP 200".into(),
                evidence: "delivered (HTTP 200)".into(),
            })
    }
}

/** A fake model-backed reasoner: the graph reasoner, counting one model call per investigation
 * Fields
    - calls: Rc<Cell<u64>> - model calls made
*/
struct CountingModel {
    calls: Rc<Cell<u64>>,
}

impl Reasoner for CountingModel {
    /** Name it
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn name(&self) -> String {
        "fake-model".into()
    }

    /** Count a model call, then investigate deterministically
     * Input
        - input: &ReasonInput - inputs
     * Output
        - Result<Investigation, String>
    */
    fn investigate(&mut self, input: &ReasonInput) -> Result<Investigation, String> {
        self.calls.set(self.calls.get() + 1);
        GraphReasoner.investigate(input)
    }

    /** The calls made
     * Input
        - None (uses self)
     * Output
        - u64
    */
    fn model_calls(&self) -> u64 {
        self.calls.get()
    }

    /** The fake model
     * Input
        - None (uses self)
     * Output
        - Option<String>
    */
    fn model(&self) -> Option<String> {
        Some("fake/model".into())
    }
}

/** The fixture world: a store, an environment, and handles to change the outside
 * Fields
    - store: ResidentStore - store
    - env: Env - environment
    - authority: Rc<RefCell<Authority>> - the Crane view the worker reads
    - sent: Rc<RefCell<Vec<String>>> - Slack messages
    - answers: Rc<RefCell<VecDeque<Dispatch>>> - Slack answers
    - calls: Rc<Cell<u64>> - model calls
    - snapshot: Rc<RefCell<String>> - the context snapshot digest
*/
struct World {
    store: ResidentStore,
    env: Env,
    authority: Rc<RefCell<Authority>>,
    sent: Rc<RefCell<Vec<String>>>,
    answers: Rc<RefCell<VecDeque<Dispatch>>>,
    calls: Rc<Cell<u64>>,
    snapshot: Rc<RefCell<String>>,
}

/** An environment over shared fixture handles
 * Input
    - authority: &Rc<RefCell<Authority>> - Crane view
    - sent, answers - Slack fixture
    - calls: &Rc<Cell<u64>> - model calls
    - snapshot: &Rc<RefCell<String>> - snapshot digest
 * Output
    - Env
*/
fn env_of(
    authority: &Rc<RefCell<Authority>>,
    sent: &Rc<RefCell<Vec<String>>>,
    answers: &Rc<RefCell<VecDeque<Dispatch>>>,
    calls: &Rc<Cell<u64>>,
    snapshot: &Rc<RefCell<String>>,
) -> Env {
    let view = authority.clone();
    let digest = snapshot.clone();
    Env {
        authority: Box::new(move || Ok(view.borrow().clone())),
        snapshot: Box::new(move |assignment| Snapshot {
            version: 1,
            targets: vec![assignment.target.key()],
            digest: digest.borrow().clone(),
            generation: 7,
            store_digest: "bricks-1".into(),
            bricks: Vec::new(),
            sections: Vec::new(),
            conflicts: Vec::new(),
            gaps: Vec::new(),
        }),
        checks: Box::new(|_| CheckEnv {
            root: None,
            signed_state: Some(true),
            monitors: BTreeMap::new(),
            recent: Vec::new(),
            now: NOW,
        }),
        reasoner: Box::new(CountingModel {
            calls: calls.clone(),
        }),
        adapters: vec![Box::new(FakeSlack {
            sent: sent.clone(),
            answers: answers.clone(),
        })],
        cache: PackCache::in_memory(),
        now: NOW,
        worker: "test-worker".into(),
    }
}

/** Build the world: an active definition, an optional RCA graph in a state, and an active
 * assignment of claude on flow:checkout subscribed to ci.* triggers
 * Input
    - auto_slack: bool - NOTIFY_SLACK auto-runs
    - rca_state: Option<RcaState> - graph state (None: no graph)
 * Output
    - World
*/
fn setup(auto_slack: bool, rca_state: Option<RcaState>) -> World {
    world_with(definition(auto_slack), rca_state)
}

/** Build the world from a definition
 * Input
    - definition: Definition - definition
    - rca_state: Option<RcaState> - graph state (None: no graph)
 * Output
    - World
*/
fn world_with(definition: Definition, rca_state: Option<RcaState>) -> World {
    let authority = Rc::new(RefCell::new(authority()));
    let sent = Rc::new(RefCell::new(Vec::new()));
    let answers = Rc::new(RefCell::new(VecDeque::new()));
    let calls = Rc::new(Cell::new(0));
    let snapshot = Rc::new(RefCell::new("snapshot-1".to_string()));
    let mut env = env_of(&authority, &sent, &answers, &calls, &snapshot);
    let mut store = ResidentStore::in_memory();
    let alice = person("alice");
    registry::save_definition(&mut store, definition, &alice, NOW).unwrap();
    registry::change_definition(
        &mut store,
        "ci-watch",
        1,
        DefinitionState::Active,
        &person("bob"),
        "reviewed",
        NOW,
    )
    .unwrap();
    if let Some(state) = rca_state {
        registry::save_rca(&mut store, checkout_graph(), &alice, NOW).unwrap();
        if state != RcaState::Proposed {
            registry::review_rca(
                &mut store,
                "checkout-ci",
                1,
                state == RcaState::Reviewed,
                &person("bob"),
                "reviewed",
                NOW,
            )
            .unwrap();
        }
    }
    registry::create_assignment(&mut store, draft(rca_state.is_some()), &alice, NOW).unwrap();
    let view = authority.borrow().clone();
    let validation =
        registry::validate_assignment(&mut store, &view, "checkout", &alice, NOW).unwrap();
    assert!(validation.passed, "{:?}", validation.problems);
    registry::change_assignment(
        &mut store,
        &mut env.cache,
        &view,
        "checkout",
        Lifecycle::Active,
        &alice,
        "start",
        NOW,
    )
    .unwrap();
    World {
        store,
        env,
        authority,
        sent,
        answers,
        calls,
        snapshot,
    }
}

/** The assignment draft of the fixture
 * Input
    - with_rca: bool - reference the graph
 * Output
    - AssignmentDraft
*/
fn draft(with_rca: bool) -> AssignmentDraft {
    AssignmentDraft {
        assignment_id: "checkout".into(),
        definition_id: "ci-watch".into(),
        definition_version: 1,
        agent: "claude".into(),
        target: "flow:checkout".into(),
        rca: with_rca.then(|| RcaRef {
            rca_id: "checkout-ci".into(),
            version: 1,
        }),
        subscriptions: vec![Subscription {
            source: TriggerSource::Ci,
            kinds: vec!["ci.*".into()],
            attributes: BTreeMap::new(),
            scope: SubscriptionScope::Target,
        }],
        timers: Vec::new(),
        lifetime_days: 14,
    }
}

/** A CI trigger for the checkout flow
 * Input
    - key: &str - idempotency key
    - kind: &str - kind
    - attributes: &[(&str, &str)] - attributes
    - now: u64 - Unix milliseconds
 * Output
    - Trigger
*/
fn ci(key: &str, kind: &str, attributes: &[(&str, &str)], now: u64) -> Trigger {
    let attributes = attributes
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    trigger::normalize(
        TriggerSource::Ci,
        kind,
        now,
        vec!["flow:checkout".into()],
        attributes,
        key,
        "test",
        "repo",
        "repo",
        now,
    )
    .unwrap()
}

/** The only activation of the world
 * Input
    - world: &World - world
 * Output
    - super::store::Activation
*/
fn only(world: &World) -> super::store::Activation {
    let live = world.store.state.activations.values().collect::<Vec<_>>();
    assert_eq!(live.len(), 1, "exactly one activation");
    live[0].clone()
}

/** Definitions and assignments are validated: ids, secrets, unsupported sources, ceilings, a
 * signed assignment, allowed sources, and no "any event" subscription
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn definitions_and_assignments_are_validated() {
    let mut bad = definition(true);
    bad.definition_id = "9bad".into();
    bad.purpose = "token ghp_abcdefghijklmnopqrstuvwxyz0123456789AB".into();
    bad.triggers.push(TriggerSource::Metric);
    bad.limits.max_steps = 99;
    let (problems, _) = registry::check_definition(&bad);
    let codes = problems
        .iter()
        .map(|(code, _)| code.as_str())
        .collect::<Vec<_>>();
    for code in [
        "DEFINITION_ID",
        "DEFINITION_SECRET",
        "TRIGGER_UNSUPPORTED",
        "LIMIT_ABOVE_CEILING",
    ] {
        assert!(codes.contains(&code), "{code} in {codes:?}");
    }
    let mut paging = definition(true);
    paging.actions.push(StepPolicy {
        step_type: StepType::PageOncall,
        auto: false,
    });
    let (_, warnings) = registry::check_definition(&paging);
    assert!(
        warnings.iter().any(|(code, _)| code == "STEP_UNAVAILABLE"),
        "PAGE_ONCALL is listed as unavailable"
    );
    assert!(super::model::check_kind("*").is_err());
    assert!(super::model::check_kind("ci.*").is_ok());

    let mut world = setup(true, Some(RcaState::Reviewed));
    // Without a signed Brick assignment, validation fails
    world.authority.borrow_mut().assignments.clear();
    let view = world.authority.borrow().clone();
    let mut other = draft(true);
    other.assignment_id = "second".into();
    other.subscriptions.push(Subscription {
        source: TriggerSource::BrickMonitor,
        kinds: vec!["monitor.*".into()],
        attributes: BTreeMap::new(),
        scope: SubscriptionScope::Target,
    });
    registry::create_assignment(&mut world.store, other, &person("alice"), NOW).unwrap();
    let validation =
        registry::validate_assignment(&mut world.store, &view, "second", &person("alice"), NOW)
            .unwrap();
    let codes = validation
        .problems
        .iter()
        .map(|(code, _)| code.as_str())
        .collect::<Vec<_>>();
    assert!(!validation.passed);
    assert!(codes.contains(&"NO_SIGNED_ASSIGNMENT"), "{codes:?}");
    assert!(
        codes.contains(&"TRIGGER_NOT_ALLOWED"),
        "brick_monitor is not a source of the definition: {codes:?}"
    );
    assert_eq!(
        world.store.assignment("second").unwrap().lifecycle,
        Lifecycle::Draft
    );
}

/** The assignment lifecycle: activation needs validation, pause and resume, expiry by the clock,
 * renewal validates again, revocation is final
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn assignment_lifecycle_and_expiry() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    let view = world.authority.borrow().clone();
    let alice = person("alice");
    let assignment = world.store.assignment("checkout").unwrap().clone();
    assert_eq!(assignment.lifecycle, Lifecycle::Active);
    assert_eq!(assignment.expires_at, NOW + 14 * super::model::DAY_MS);
    let mut extra = draft(true);
    extra.assignment_id = "drafted".into();
    registry::create_assignment(&mut world.store, extra, &alice, NOW).unwrap();
    assert!(
        registry::change_assignment(
            &mut world.store,
            &mut world.env.cache,
            &view,
            "drafted",
            Lifecycle::Active,
            &alice,
            "skip",
            NOW
        )
        .is_err(),
        "a draft cannot be activated without validation"
    );
    registry::change_assignment(
        &mut world.store,
        &mut world.env.cache,
        &view,
        "checkout",
        Lifecycle::Paused,
        &alice,
        "holiday",
        NOW,
    )
    .unwrap();
    // A paused assignment takes no trigger
    let done = worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("p1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert!(done.ingested.queued.is_empty());
    assert!(world
        .store
        .state
        .ignored
        .iter()
        .any(|item| item.reason == "PAUSED"));
    registry::change_assignment(
        &mut world.store,
        &mut world.env.cache,
        &view,
        "checkout",
        Lifecycle::Active,
        &alice,
        "back",
        NOW,
    )
    .unwrap();
    // The clock passes the lifetime: the tick expires it, and a trigger then wakes nothing
    world.env.now = NOW + 15 * super::model::DAY_MS;
    let at = world.env.now;
    let done = worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("late", "ci.failed", &[("job", "tests")], at)],
    )
    .unwrap();
    assert_eq!(done.expired, vec!["checkout".to_string()]);
    assert!(done.ingested.queued.is_empty());
    assert_eq!(
        world.store.assignment("checkout").unwrap().lifecycle,
        Lifecycle::Expired
    );
    // Renewal validates again; a revoked signed assignment refuses it
    world.authority.borrow_mut().assignments[0].active = false;
    let revoked = world.authority.borrow().clone();
    assert!(registry::change_assignment(
        &mut world.store,
        &mut world.env.cache,
        &revoked,
        "checkout",
        Lifecycle::Active,
        &alice,
        "renew",
        world.env.now
    )
    .unwrap_err()
    .contains("NO_SIGNED_ASSIGNMENT"));
    registry::change_assignment(
        &mut world.store,
        &mut world.env.cache,
        &view,
        "checkout",
        Lifecycle::Active,
        &alice,
        "renew",
        world.env.now,
    )
    .unwrap();
    assert_eq!(
        world.store.assignment("checkout").unwrap().expires_at,
        world.env.now + 14 * super::model::DAY_MS
    );
    registry::change_assignment(
        &mut world.store,
        &mut world.env.cache,
        &view,
        "checkout",
        Lifecycle::Revoked,
        &alice,
        "done",
        world.env.now,
    )
    .unwrap();
    assert!(
        registry::change_assignment(
            &mut world.store,
            &mut world.env.cache,
            &view,
            "checkout",
            Lifecycle::Active,
            &alice,
            "again",
            world.env.now
        )
        .is_err(),
        "revocation is final"
    );
}

/** Idle ticks over weeks make no model call; a matching trigger makes exactly one activation and
 * the model is called only inside it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn idle_assignments_make_no_model_calls() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    for hour in 0..(24 * 13) {
        world.env.now = NOW + hour * 60 * MINUTE;
        let done = worker::tick(&mut world.store, &mut world.env, Vec::new()).unwrap();
        assert!(done.idle);
        assert_eq!(done.model_calls, 0);
    }
    assert_eq!(world.calls.get(), 0, "no model call while idle");
    assert_eq!(world.store.state.counters.idle_model_calls, 0);
    assert_eq!(world.store.state.counters.idle_ticks, 24 * 13);
    let at = world.env.now;
    let done = worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-1", "ci.failed", &[("job", "tests")], at)],
    )
    .unwrap();
    assert!(!done.idle);
    assert_eq!(done.ingested.queued.len(), 1);
    assert_eq!(
        world.calls.get(),
        1,
        "the model is called inside the activation only"
    );
    assert_eq!(only(&world).usage.model_calls, 1);
    assert_eq!(only(&world).usage.model.as_deref(), Some("fake/model"));
}

/** Malformed, stale, future, foreign, secret-carrying, oversized, and unsupported triggers are
 * refused; delivered files only take ci and webhook
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn triggers_are_normalized_and_rejected() {
    let ok = |kind: &str, at: u64, repository: &str, attributes: BTreeMap<String, String>| {
        trigger::normalize(
            TriggerSource::Ci,
            kind,
            at,
            vec!["flow:checkout".into()],
            attributes,
            "key-1",
            "test",
            repository,
            "repo",
            NOW,
        )
    };
    assert!(ok("ci.failed", NOW, "repo", BTreeMap::new()).is_ok());
    assert!(
        ok("ci.*", NOW, "repo", BTreeMap::new()).is_err(),
        "a pattern is not a kind"
    );
    assert!(ok("CI Failed", NOW, "repo", BTreeMap::new()).is_err());
    assert!(ok("ci.failed", NOW, "other-repo", BTreeMap::new())
        .unwrap_err()
        .contains("not this one"));
    assert!(ok(
        "ci.failed",
        NOW - 31 * super::model::DAY_MS,
        "repo",
        BTreeMap::new()
    )
    .unwrap_err()
    .contains("stale"));
    assert!(ok("ci.failed", NOW + 10 * MINUTE, "repo", BTreeMap::new())
        .unwrap_err()
        .contains("future"));
    let secret = BTreeMap::from([(
        "token".to_string(),
        "ghp_abcdefghijklmnopqrstuvwxyz0123456789AB".to_string(),
    )]);
    assert!(ok("ci.failed", NOW, "repo", secret)
        .unwrap_err()
        .contains("secret"));
    let many = (0..40)
        .map(|index| (format!("a{index}"), "x".to_string()))
        .collect();
    assert!(ok("ci.failed", NOW, "repo", many).is_err());
    assert!(trigger::normalize(
        TriggerSource::Metric,
        "metric.high",
        NOW,
        Vec::new(),
        BTreeMap::new(),
        "k",
        "t",
        "repo",
        "repo",
        NOW
    )
    .unwrap_err()
    .contains("no connector"));
    assert!(trigger::normalize(
        TriggerSource::Ci,
        "ci.failed",
        NOW,
        vec!["nowhere".into()],
        BTreeMap::new(),
        "k",
        "t",
        "repo",
        "repo",
        NOW
    )
    .is_err());
    let file = serde_json::json!({"source": "governance_change", "kind": "governance.signed", "idempotency_key": "x"});
    assert!(
        trigger::delivered(&file, "alice", "repo", NOW).is_err(),
        "only ci and webhook are delivered by file"
    );
    let file = serde_json::json!({"source": "ci", "kind": "ci.failed", "idempotency_key": "x", "extra": 1});
    assert!(
        trigger::delivered(&file, "alice", "repo", NOW).is_err(),
        "unknown fields are refused"
    );
    let first = ci("same", "ci.failed", &[], NOW);
    let again = ci("same", "ci.failed", &[], NOW + MINUTE);
    assert_eq!(
        first.trigger_id, again.trigger_id,
        "the same delivery has the same id"
    );
}

/** Repeated delivery makes one activation; filters, scope, cooldown, and the daily limit are
 * applied and recorded
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn deliveries_are_deduplicated_filtered_and_cooled_down() {
    let mut definition = definition(true);
    definition.limits.cooldown_minutes = 30;
    definition.limits.max_activations_per_day = 2;
    let mut world = world_with(definition, Some(RcaState::Reviewed));
    let run = ci("run-7", "ci.failed", &[("job", "tests")], NOW);
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![run.clone(), run.clone()],
    )
    .unwrap();
    world.env.now = NOW + MINUTE;
    worker::tick(&mut world.store, &mut world.env, vec![run.clone()]).unwrap();
    assert_eq!(
        world.store.state.activations.len(),
        1,
        "one activation despite three deliveries"
    );
    assert_eq!(world.store.state.counters.duplicates, 2);
    // A trigger for another target, or a repository-wide one, does not match a target subscription
    let elsewhere = trigger::normalize(
        TriggerSource::Ci,
        "ci.failed",
        NOW,
        vec!["flow:refund".into()],
        BTreeMap::new(),
        "x1",
        "t",
        "repo",
        "repo",
        NOW,
    )
    .unwrap();
    let wide = trigger::normalize(
        TriggerSource::Ci,
        "ci.failed",
        NOW,
        Vec::new(),
        BTreeMap::new(),
        "x2",
        "t",
        "repo",
        "repo",
        NOW,
    )
    .unwrap();
    let assignment = world.store.assignment("checkout").unwrap().clone();
    assert!(trigger::assignment_matches(&assignment, &elsewhere).is_err());
    assert!(trigger::assignment_matches(&assignment, &wide)
        .unwrap_err()
        .contains("repository-wide"));
    let mut filtered = assignment.subscriptions[0].clone();
    filtered.attributes.insert("branch".into(), "main".into());
    assert!(trigger::subscription_matches(&filtered, &run, "flow:checkout").is_err());
    // Within the cooldown, a new trigger is ignored with COOLDOWN
    world.env.now = NOW + 5 * MINUTE;
    let at = world.env.now;
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-8", "ci.failed", &[("job", "tests")], at)],
    )
    .unwrap();
    assert!(world
        .store
        .state
        .ignored
        .iter()
        .any(|item| item.reason == "COOLDOWN"));
    world.env.now = NOW + 40 * MINUTE;
    let at = world.env.now;
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-9", "ci.failed", &[("job", "tests")], at)],
    )
    .unwrap();
    world.env.now = NOW + 80 * MINUTE;
    let at = world.env.now;
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-10", "ci.failed", &[("job", "tests")], at)],
    )
    .unwrap();
    assert!(world
        .store
        .state
        .ignored
        .iter()
        .any(|item| item.reason == "RATE_LIMITED"));
    assert_eq!(world.store.state.activations.len(), 2);
    // Nothing subscribes to webhook triggers
    let at = world.env.now;
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![trigger::normalize(
            TriggerSource::Webhook,
            "deploy.done",
            at,
            vec!["flow:checkout".into()],
            BTreeMap::new(),
            "w1",
            "t",
            "repo",
            "repo",
            at,
        )
        .unwrap()],
    )
    .unwrap();
    assert!(world
        .store
        .state
        .ignored
        .iter()
        .any(|item| item.reason == "NO_MATCH"));
}

/** A trigger runs the reviewed remediation end to end, verification proves it, and the
 * assignment returns to idle
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn a_trigger_runs_to_a_verified_outcome_and_back_to_idle() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let activation = only(&world);
    assert_eq!(
        activation.state,
        ActivationState::Completed,
        "{:?}",
        activation.timeline
    );
    assert_eq!(activation.outcome.as_ref().unwrap().status, "PROVEN");
    let investigation = activation.investigation.as_ref().unwrap();
    assert_eq!(investigation.conclusion, rca::Conclusion::Identified);
    assert_eq!(investigation.cause.as_deref(), Some("flaky"));
    assert!(investigation
        .consulted
        .iter()
        .any(|item| item == "edge:flaky>notify"));
    let steps = activation
        .plan
        .as_ref()
        .unwrap()
        .steps
        .iter()
        .map(|step| step.step_type)
        .collect::<Vec<_>>();
    assert_eq!(steps, [StepType::NotifySlack, StepType::Verify]);
    assert_eq!(world.sent.borrow().len(), 1);
    assert!(
        world.sent.borrow()[0].contains("activation act_"),
        "the message names its source"
    );
    assert_eq!(
        activation.steps["notify"]
            .authorization
            .as_ref()
            .unwrap()
            .decision,
        Decision::Allow
    );
    let view = world.authority.borrow().clone();
    let report = super::report::build(&world.store, Some(&view), NOW);
    assert_eq!(report["assignments"][0]["status"], "ACTIVE_IDLE");
    assert!(
        report["activations"][0]["timeline"]
            .as_array()
            .unwrap()
            .len()
            >= 6
    );
    // The timeline went through every stage in order
    let codes = activation
        .timeline
        .iter()
        .map(|entry| entry.state)
        .collect::<Vec<_>>();
    for state in [
        ActivationState::Queued,
        ActivationState::ContextLoading,
        ActivationState::Investigating,
        ActivationState::PlanReady,
        ActivationState::Executing,
        ActivationState::Verifying,
        ActivationState::Completed,
    ] {
        assert!(codes.contains(&state), "{state:?} in {codes:?}");
    }
}

/** A worker that crashed during a Slack call leaves the step uncertain: the next tick recovers
 * the activation without calling Slack again, and a person's reconciliation finishes it; a
 * store reopened from disk keeps queued and waiting work
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn restart_recovery_reconciles_instead_of_repeating() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let id = only(&world).activation_id;
    // Rewind to the moment the worker crashed: the step was dispatching, the lease still held
    let mut crashed = only(&world);
    crashed.state = ActivationState::Executing;
    crashed.outcome = None;
    crashed.lease = Some(super::store::Lease {
        holder: "dead-worker".into(),
        expires_at: NOW + MINUTE,
    });
    for (step, record) in crashed.steps.iter_mut() {
        record.state = if step == "notify" {
            StepState::Dispatching
        } else {
            StepState::Pending
        };
    }
    world.store.put_activation(crashed);
    let sent = world.sent.borrow().len();
    // While the lease is held, another worker leaves it alone
    worker::tick(&mut world.store, &mut world.env, Vec::new()).unwrap();
    assert_eq!(
        world.store.activation(&id).unwrap().state,
        ActivationState::Executing
    );
    world.env.now = NOW + 10 * MINUTE;
    let done = worker::tick(&mut world.store, &mut world.env, Vec::new()).unwrap();
    assert_eq!(done.recovered, vec![id.clone()]);
    let activation = world.store.activation(&id).unwrap().clone();
    assert_eq!(activation.state, ActivationState::ReconciliationRequired);
    assert_eq!(activation.steps["notify"].state, StepState::Uncertain);
    assert_eq!(
        world.sent.borrow().len(),
        sent,
        "an uncertain effect is never repeated blindly"
    );
    let state = worker::reconcile(
        &mut world.store,
        &mut world.env,
        &id,
        "notify",
        true,
        &person("alice"),
        "seen in the channel",
    )
    .unwrap();
    assert_eq!(state, ActivationState::Completed);
    assert_eq!(world.sent.borrow().len(), sent);

    // Durability: an activation waiting for approval survives a reopened store and resumes
    let directory = tempfile::tempdir().unwrap();
    let mut world = setup(false, Some(RcaState::Reviewed));
    let mut store = ResidentStore::open(directory.path()).unwrap();
    store.state = world.store.state.clone();
    store.save().unwrap();
    worker::tick(
        &mut store,
        &mut world.env,
        vec![ci("run-2", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let id = store.state.activations.keys().next().unwrap().clone();
    assert_eq!(
        store.activation(&id).unwrap().state,
        ActivationState::AwaitingApproval
    );
    drop(store);
    let mut reopened = ResidentStore::open(directory.path()).unwrap();
    assert_eq!(
        reopened.activation(&id).unwrap().state,
        ActivationState::AwaitingApproval
    );
    let state = worker::approve(
        &mut reopened,
        &mut world.env,
        &id,
        "notify",
        &person("bob"),
        Via::Machine {
            signers: &signers(),
        },
        true,
        "go",
    )
    .unwrap();
    assert_eq!(state, ActivationState::Completed);
    assert_eq!(
        reopened.state.activations.len(),
        1,
        "the same activation resumed"
    );
    // A store edited outside Crane is refused
    let path = directory.path().join("resident").join("store.json");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"COMPLETED\"", "\"FAILED\"");
    std::fs::write(&path, text).unwrap();
    assert!(ResidentStore::open(directory.path())
        .err()
        .unwrap()
        .contains("altered outside Crane"));
}

/** The context pack is cached by its dependencies: miss, hit, then invalidated when the context
 * snapshot or the signed generation changes; the cache never stands in for authorization
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn context_cache_hits_misses_and_invalidates() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    let cache_of = |world: &World, key: &str| {
        world
            .store
            .state
            .activations
            .values()
            .find(|activation| activation.trigger.idempotency_key == key)
            .and_then(|activation| activation.context.as_ref())
            .map(|context| context.cache.clone())
    };
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("c1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert_eq!(cache_of(&world, "c1"), Some(Cache::Miss));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("c2", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert_eq!(cache_of(&world, "c2"), Some(Cache::Hit));
    *world.snapshot.borrow_mut() = "snapshot-2".into();
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("c3", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert_eq!(
        cache_of(&world, "c3"),
        Some(Cache::Invalidated(vec!["context".into()]))
    );
    world.authority.borrow_mut().generation = 8;
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("c4", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert!(
        matches!(cache_of(&world, "c4"), Some(Cache::Invalidated(changed)) if changed.contains(&"generation".to_string()))
    );
    assert_eq!(world.store.state.counters.cache_hits, 1);
    let previous = world
        .store
        .state
        .activations
        .values()
        .find(|activation| activation.trigger.idempotency_key == "c2")
        .unwrap()
        .context
        .clone()
        .unwrap()
        .previous;
    assert_eq!(
        previous.len(),
        1,
        "the previous outcome is referenced, not dumped"
    );
    // A cache hit does not authorize: the same context with the signed assignment revoked is blocked
    world.authority.borrow_mut().generation = 7;
    *world.snapshot.borrow_mut() = "snapshot-2".into();
    world.authority.borrow_mut().assignments[0].active = false;
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("c5", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let blocked = world
        .store
        .state
        .activations
        .values()
        .find(|activation| activation.trigger.idempotency_key == "c5")
        .unwrap();
    assert_eq!(blocked.state, ActivationState::Blocked);
}

/** Crane decides every step immediately before it runs: a signed assignment revoked between
 * planning and execution stops the step even with an approval; observe and assisted ceilings,
 * restricted criticality, and Brick controls tighten; nothing loosens
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn authorization_is_checked_before_every_step() {
    let mut world = setup(false, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let id = only(&world).activation_id;
    assert_eq!(only(&world).state, ActivationState::AwaitingApproval);
    assert!(only(&world).steps["notify"]
        .authorization
        .as_ref()
        .unwrap()
        .codes()
        .contains(&"STEP_NEEDS_APPROVAL".to_string()));
    world.authority.borrow_mut().assignments[0].active = false;
    let state = worker::approve(
        &mut world.store,
        &mut world.env,
        &id,
        "notify",
        &person("bob"),
        Via::Machine {
            signers: &signers(),
        },
        true,
        "go",
    )
    .unwrap();
    assert_eq!(state, ActivationState::Blocked);
    assert_eq!(
        world.store.activation(&id).unwrap().steps["notify"].state,
        StepState::Denied
    );
    assert!(
        world.sent.borrow().is_empty(),
        "nothing was sent after the revocation"
    );

    let assignment = world.store.assignment("checkout").unwrap().clone();
    let definition = world.store.definition("ci-watch", Some(1)).unwrap().clone();
    let step = rca::step(
        "notify",
        StepType::NotifySlack,
        &[("text", "x".into())],
        &[],
        String::new(),
    );
    let decide = |change: &dyn Fn(&mut Authority)| {
        let mut view = authority();
        change(&mut view);
        super::authority::authorize(&view, &assignment, Some(&definition), &step, NOW)
    };
    assert_eq!(
        decide(&|_| {}).decision,
        Decision::RequireApproval,
        "auto is off"
    );
    assert_eq!(
        decide(&|view| view.targets.get_mut("flow:checkout").unwrap().ceiling = Ceiling::Observe)
            .decision,
        Decision::Deny
    );
    assert!(decide(
        &|view| view.targets.get_mut("flow:checkout").unwrap().ceiling = Ceiling::Assisted
    )
    .codes()
    .contains(&"ZONE_ASSISTED".to_string()));
    assert!(decide(
        &|view| view.targets.get_mut("flow:checkout").unwrap().criticality =
            Criticality::Restricted
    )
    .codes()
    .contains(&"ZONE_RESTRICTED".to_string()));
    assert!(decide(&|view| view
        .targets
        .get_mut("flow:checkout")
        .unwrap()
        .fail_closed
        .push("brick:x@1 revoked".into()))
    .codes()
    .contains(&"BRICK_FAIL_CLOSED".to_string()));
    assert_eq!(
        decide(&|view| view.trust = Trust::Unverified("rollback".into())).decision,
        Decision::Deny
    );
    assert_eq!(
        decide(
            &|view| view.targets.get_mut("flow:checkout").unwrap().status =
                TargetLifecycle::Proposed
        )
        .decision,
        Decision::Deny
    );
    assert_eq!(
        decide(&|view| {
            view.connectors.insert("slack".into(), false);
        })
        .decision,
        Decision::Unavailable
    );
    let ticket = rca::step(
        "ticket",
        StepType::CreateTicket,
        &[("summary", "x".into())],
        &[],
        String::new(),
    );
    assert_eq!(
        super::authority::authorize(&authority(), &assignment, Some(&definition), &ticket, NOW)
            .decision,
        Decision::Unavailable
    );
    let page = rca::step(
        "page",
        StepType::PageOncall,
        &[("summary", "x".into())],
        &[],
        String::new(),
    );
    assert_eq!(
        super::authority::authorize(&authority(), &assignment, Some(&definition), &page, NOW)
            .decision,
        Decision::Deny,
        "not allowed by the definition"
    );
}

/** Approvals bind to the plan and the signed state, are used once, and only signers may give them
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn approvals_are_bound_single_use_and_signer_only() {
    let mut world = setup(false, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let id = only(&world).activation_id;
    assert!(worker::approve(
        &mut world.store,
        &mut world.env,
        &id,
        "notify",
        &person("mallory"),
        Via::Machine {
            signers: &signers()
        },
        true,
        "me"
    )
    .unwrap_err()
    .contains("signer"));
    assert!(
        worker::approve(
            &mut world.store,
            &mut world.env,
            &id,
            "notify",
            &person("bob"),
            Via::Machine {
                signers: &signers()
            },
            true,
            " "
        )
        .is_err(),
        "a reason is required"
    );
    // The signed state changes after the approval: it no longer applies, a new one is needed
    let digest_before = world.authority.borrow().digest();
    let mut view = world.authority.borrow().clone();
    view.generation = 9;
    let changed = Rc::new(RefCell::new(view));
    world.env = env_of(
        &changed,
        &world.sent,
        &world.answers,
        &world.calls,
        &world.snapshot,
    );
    let first = world
        .store
        .activation(&id)
        .unwrap()
        .approval_digest("notify", &digest_before);
    let mut activation = world.store.activation(&id).unwrap().clone();
    activation.approvals.push(super::store::Approval {
        step: "notify".into(),
        digest: first,
        approved: true,
        approver: person("bob"),
        reason: "old".into(),
        at: NOW,
        expires_at: NOW + 60 * MINUTE,
        used: false,
    });
    activation.steps.get_mut("notify").unwrap().state = StepState::Pending;
    activation.state = ActivationState::Executing;
    world.store.put_activation(activation);
    let state = worker::advance(&mut world.store, &mut world.env, &id).unwrap();
    assert_eq!(
        state,
        ActivationState::AwaitingApproval,
        "an approval of an older signed state does not apply"
    );
    assert!(world.sent.borrow().is_empty());
    let state = worker::approve(
        &mut world.store,
        &mut world.env,
        &id,
        "notify",
        &person("bob"),
        Via::Machine {
            signers: &signers(),
        },
        true,
        "go",
    )
    .unwrap();
    assert_eq!(state, ActivationState::Completed);
    let activation = world.store.activation(&id).unwrap();
    assert!(
        activation
            .approvals
            .iter()
            .filter(|approval| approval.used)
            .count()
            == 1,
        "used once"
    );
    assert_eq!(world.sent.borrow().len(), 1);
    // An expired approval does not apply either
    let mut world = setup(false, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("run-2", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let id = only(&world).activation_id;
    let digest = world
        .store
        .activation(&id)
        .unwrap()
        .approval_digest("notify", &world.authority.borrow().digest());
    let mut activation = world.store.activation(&id).unwrap().clone();
    activation.approvals.push(super::store::Approval {
        step: "notify".into(),
        digest,
        approved: true,
        approver: person("bob"),
        reason: "old".into(),
        at: NOW,
        expires_at: NOW + MINUTE,
        used: false,
    });
    activation.steps.get_mut("notify").unwrap().state = StepState::Pending;
    activation.state = ActivationState::Executing;
    world.store.put_activation(activation);
    world.env.now = NOW + 2 * MINUTE;
    assert_eq!(
        worker::advance(&mut world.store, &mut world.env, &id).unwrap(),
        ActivationState::AwaitingApproval
    );
}

/** The investigation keeps observed, supported, contradicted, and unknown apart: an unreviewed
 * graph only proposes, contradicting evidence wins, missing evidence is a gap, no signal is
 * insufficient evidence; graph validation refuses unsupported confirmations and wrong relations
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rca_basis_is_explicit() {
    // A proposed graph: the remediation is proposed to a person, never run
    let mut world = setup(true, Some(RcaState::Proposed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("p", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let activation = only(&world);
    assert_eq!(
        activation.plan.as_ref().unwrap().steps[0].step_type,
        StepType::ProposeActionPlan
    );
    assert_eq!(activation.state, ActivationState::AwaitingApproval);
    assert!(world.sent.borrow().is_empty());
    // Contradicting evidence: the cause is contradicted, so nothing is identified
    let mut world = setup(true, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci(
            "c",
            "ci.failed",
            &[("job", "tests"), ("infra", "down")],
            NOW,
        )],
    )
    .unwrap();
    let investigation = only(&world).investigation.unwrap();
    assert_eq!(
        investigation.candidates[0].verdict,
        rca::Verdict::Contradicted
    );
    assert_eq!(investigation.conclusion, rca::Conclusion::Ambiguous);
    assert!(world.sent.borrow().is_empty());
    // Missing evidence: the check is unknown, never passed, and named as a gap
    let mut world = setup(true, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("u", "ci.failed", &[], NOW)],
    )
    .unwrap();
    let investigation = only(&world).investigation.unwrap();
    assert_eq!(investigation.candidates[0].verdict, rca::Verdict::Unknown);
    assert!(investigation
        .gaps
        .iter()
        .any(|gap| gap.contains("no attribute job")));
    assert_eq!(
        investigation.candidates[0].prior,
        Basis::Hypothesized,
        "the graph's basis is kept apart from this evidence"
    );
    // No signal: insufficient evidence, a proposed RCA, and a question
    let mut world = setup(true, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("n", "ci.cancelled", &[], NOW)],
    )
    .unwrap();
    let activation = only(&world);
    let investigation = activation.investigation.as_ref().unwrap();
    assert_eq!(
        investigation.conclusion,
        rca::Conclusion::InsufficientEvidence
    );
    let types = activation
        .plan
        .as_ref()
        .unwrap()
        .steps
        .iter()
        .map(|step| step.step_type)
        .collect::<Vec<_>>();
    assert_eq!(types, [StepType::ProposeRca, StepType::AskUser]);
    let proposal = world.store.rca("checkout-proposed", Some(1)).unwrap();
    assert_eq!(proposal.state, RcaState::Proposed);
    assert!(
        proposal
            .graph
            .nodes
            .iter()
            .all(|node| node.status != Basis::Confirmed),
        "agent proposals are never confirmed"
    );
    assert_eq!(activation.state, ActivationState::AwaitingInput);
    // Validation
    let mut bad = checkout_graph();
    bad.nodes[1].status = Basis::Confirmed;
    bad.nodes[1].basis.clear();
    bad.edges
        .push(edge("notify", "ci-failed", Relation::MayCause));
    let codes = rca::validate(&bad)
        .into_iter()
        .map(|(code, _)| code)
        .collect::<Vec<_>>();
    assert!(codes.contains(&"RCA_UNSUPPORTED_CONFIRMATION".to_string()));
    assert!(codes.contains(&"RCA_RELATION_KIND".to_string()));
}

/** Plans are validated: duplicates, forward dependencies, types the definition does not allow,
 * missing parameters, secrets, nested proposals, and an external step without verification
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn plans_are_typed_and_validated() {
    let definition = definition(true);
    let notify = rca::step(
        "a",
        StepType::NotifySlack,
        &[("text", "hi".into())],
        &[],
        String::new(),
    );
    let codes = |steps: Vec<plan::PlanStep>| {
        plan::validate(&Plan::new(steps), &definition)
            .into_iter()
            .map(|(code, _)| code)
            .collect::<Vec<_>>()
    };
    assert!(codes(vec![notify.clone()]).contains(&"PLAN_UNVERIFIED".to_string()));
    let mut verify = rca::step("v", StepType::Verify, &[], &["a"], String::new());
    verify.check = Some(CheckSpec::SignedStateVerifies);
    assert!(codes(vec![notify.clone(), verify.clone()]).is_empty());
    assert!(codes(vec![notify.clone(), notify.clone(), verify.clone()])
        .contains(&"PLAN_DUPLICATE_STEP".to_string()));
    assert!(
        codes(vec![verify.clone(), notify.clone()]).contains(&"PLAN_BAD_DEPENDENCY".to_string())
    );
    assert!(codes(vec![rca::step(
        "p",
        StepType::PageOncall,
        &[("summary", "x".into())],
        &[],
        String::new()
    )])
    .contains(&"PLAN_STEP_NOT_ALLOWED".to_string()));
    assert!(codes(vec![
        rca::step("m", StepType::NotifySlack, &[], &[], String::new()),
        verify.clone()
    ])
    .contains(&"PLAN_MISSING_PARAMETER".to_string()));
    assert!(codes(vec![
        rca::step(
            "a",
            StepType::NotifySlack,
            &[("text", "ghp_abcdefghijklmnopqrstuvwxyz0123456789AB".into())],
            &[],
            String::new()
        ),
        verify.clone()
    ])
    .contains(&"PLAN_SECRET".to_string()));
    let mut outer = rca::step(
        "o",
        StepType::ProposeActionPlan,
        &[("summary", "x".into())],
        &[],
        String::new(),
    );
    let mut inner = rca::step(
        "i",
        StepType::ProposeActionPlan,
        &[("summary", "y".into())],
        &[],
        String::new(),
    );
    inner.proposal = vec![notify.clone()];
    outer.proposal = vec![inner];
    assert!(codes(vec![outer]).contains(&"PLAN_NESTED_PROPOSAL".to_string()));
    let mut check = rca::step("r", StepType::ReadCheck, &[], &[], String::new());
    check.check = Some(CheckSpec::FileExists {
        path: "../etc/passwd".into(),
    });
    assert!(codes(vec![check]).contains(&"PLAN_BAD_CHECK".to_string()));
    // Accepting a proposal rewrites ids and dependencies and changes the digest
    let mut proposal = rca::step(
        "prop",
        StepType::ProposeActionPlan,
        &[("summary", "x".into())],
        &[],
        String::new(),
    );
    proposal.proposal = vec![notify.clone(), verify.clone()];
    let before = Plan::new(vec![proposal]);
    let after = before.accept("prop");
    assert_ne!(before.digest, after.digest);
    assert_eq!(
        after
            .steps
            .iter()
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>(),
        ["prop", "prop-a", "prop-v"]
    );
    assert_eq!(after.steps[2].depends_on, ["prop", "prop-a"]);
}

/** A reasoner (or model) cannot declare success: without verification evidence an activation is
 * UNKNOWN, an unavailable step is never counted as done, a failed call without effect is retried
 * within the limit, and an uncertain outcome waits for reconciliation without a retry
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn outcomes_need_evidence_and_unknown_effects_are_not_retried() {
    // No verification in the remediation, and the definition does not require one
    let mut definition = definition(true);
    definition.verification_required = false;
    let mut world = world_with(definition, None);
    let mut graph = checkout_graph();
    graph
        .edges
        .retain(|edge| edge.relation != Relation::VerifiedBy);
    graph.version = 1;
    registry::save_rca(&mut world.store, graph, &person("alice"), NOW).unwrap();
    registry::review_rca(
        &mut world.store,
        "checkout-ci",
        1,
        true,
        &person("bob"),
        "ok",
        NOW,
    )
    .unwrap();
    let mut assignment = world.store.assignment("checkout").unwrap().clone();
    assignment.rca = Some(RcaRef {
        rca_id: "checkout-ci".into(),
        version: 1,
    });
    world.store.put_assignment(assignment);
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("r", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let activation = only(&world);
    assert_eq!(world.sent.borrow().len(), 1);
    assert_eq!(
        activation.state,
        ActivationState::Unknown,
        "every step succeeded, but nothing proves the outcome"
    );
    assert_eq!(activation.outcome.unwrap().status, "UNKNOWN");

    // An unavailable step type
    let mut world = setup(true, Some(RcaState::Reviewed));
    let mut graph = checkout_graph();
    graph.version = 2;
    graph
        .nodes
        .iter_mut()
        .find(|node| node.id == "notify")
        .unwrap()
        .steps = vec![rca::step(
        "ticket",
        StepType::CreateTicket,
        &[("summary", "x".into())],
        &[],
        String::new(),
    )];
    registry::save_rca(&mut world.store, graph, &person("alice"), NOW).unwrap();
    registry::review_rca(
        &mut world.store,
        "checkout-ci",
        2,
        true,
        &person("bob"),
        "ok",
        NOW,
    )
    .unwrap();
    let mut assignment = world.store.assignment("checkout").unwrap().clone();
    assignment.rca = Some(RcaRef {
        rca_id: "checkout-ci".into(),
        version: 2,
    });
    world.store.put_assignment(assignment);
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("t", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let activation = only(&world);
    assert_eq!(activation.steps["ticket"].state, StepState::Unavailable);
    assert_ne!(activation.state, ActivationState::Completed);
    assert_eq!(activation.steps["verify-1"].state, StepState::Skipped);

    // A known failure without effect is retried once (the limit), then fails
    let mut world = setup(true, Some(RcaState::Reviewed));
    world.answers.borrow_mut().extend([
        Dispatch::Failed {
            effect: false,
            detail: "HTTP 400".into(),
        },
        Dispatch::Failed {
            effect: false,
            detail: "HTTP 400".into(),
        },
    ]);
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("f", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert_eq!(world.sent.borrow().len(), 2, "one retry");
    let activation = only(&world);
    assert_eq!(activation.steps["notify"].state, StepState::Failed);
    assert_eq!(activation.state, ActivationState::Failed);

    // An uncertain outcome is not retried
    let mut world = setup(true, Some(RcaState::Reviewed));
    world.answers.borrow_mut().push_back(Dispatch::Uncertain {
        detail: "timed out".into(),
    });
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("x", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert_eq!(world.sent.borrow().len(), 1);
    assert_eq!(only(&world).state, ActivationState::ReconciliationRequired);
    let id = only(&world).activation_id;
    let state = worker::reconcile(
        &mut world.store,
        &mut world.env,
        &id,
        "notify",
        false,
        &person("alice"),
        "not in the channel",
    )
    .unwrap();
    assert_eq!(
        state,
        ActivationState::Completed,
        "declared without effect, retried once within the limit, then verified"
    );
    assert_eq!(world.sent.borrow().len(), 2);
}

/** A test (dry run) decides every step and evaluates read-only checks, but sends nothing, creates
 * no activation, consumes no trigger, and keeps only its report
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn dry_runs_have_no_side_effects() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    let report = worker::test(
        &mut world.store,
        &mut world.env,
        "checkout",
        ci("t1", "ci.failed", &[("job", "tests")], NOW),
    )
    .unwrap();
    assert_eq!(report["result"], "PASSED", "{report}");
    assert!(report["mode"]
        .as_str()
        .unwrap()
        .contains("nothing was executed"));
    assert!(world.sent.borrow().is_empty());
    assert!(world.store.state.activations.is_empty());
    assert!(world.store.state.seen.is_empty());
    assert_eq!(world.store.state.tests.len(), 1);
    let steps = report["plan"]["steps"].as_array().unwrap();
    assert_eq!(steps[0]["decision"], "ALLOW");
    assert_eq!(steps[1]["expected"]["result"], "PASSED");
    // A stale context (a missing pin) and a revoked signed assignment are reported as blocked
    world.authority.borrow_mut().assignments[0].active = false;
    let report = worker::test(
        &mut world.store,
        &mut world.env,
        "checkout",
        ci("t2", "ci.failed", &[("job", "tests")], NOW),
    )
    .unwrap();
    assert_eq!(report["result"], "BLOCKED");
    // A trigger the assignment does not take is blocked, with the reason
    world.authority.borrow_mut().assignments[0].active = true;
    let other = trigger::normalize(
        TriggerSource::Webhook,
        "deploy.done",
        NOW,
        vec!["flow:checkout".into()],
        BTreeMap::new(),
        "w",
        "t",
        "repo",
        "repo",
        NOW,
    )
    .unwrap();
    let report = worker::test(&mut world.store, &mut world.env, "checkout", other).unwrap();
    assert_eq!(report["matching"]["matched"], false);
    assert!(world.sent.borrow().is_empty());
}

/** Questions and waits persist the activation and resume the same one: an answer continues it, a
 * later trigger satisfies a wait instead of starting a new activation, a deadline fails the wait,
 * and missing context asks a person when the definition says so
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn human_waits_and_event_waits_resume_the_same_activation() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("q", "ci.cancelled", &[], NOW)],
    )
    .unwrap();
    let id = only(&world).activation_id;
    assert_eq!(only(&world).state, ActivationState::AwaitingInput);
    let view = world.authority.borrow().clone();
    assert_eq!(
        super::report::build(&world.store, Some(&view), NOW)["inbox"]["questions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let state = worker::answer(
        &mut world.store,
        &mut world.env,
        &id,
        "ask",
        "Cancelled by the release manager",
        &person("alice"),
        Via::Machine {
            signers: &signers(),
        },
    )
    .unwrap();
    assert_eq!(
        state,
        ActivationState::Unknown,
        "a question answered proves nothing by itself"
    );
    assert_eq!(world.store.state.activations.len(), 1);

    // A remediation that waits for the next green run
    let mut world = setup(true, Some(RcaState::Reviewed));
    let mut graph = checkout_graph();
    graph.version = 2;
    let mut wait = rca::step(
        "wait",
        StepType::WaitForEvent,
        &[
            ("kind", "ci.passed".into()),
            ("within_minutes", "60".into()),
        ],
        &["notify"],
        String::new(),
    );
    wait.basis = "the next run passes".into();
    graph
        .nodes
        .iter_mut()
        .find(|node| node.id == "notify")
        .unwrap()
        .steps
        .push(wait);
    registry::save_rca(&mut world.store, graph, &person("alice"), NOW).unwrap();
    registry::review_rca(
        &mut world.store,
        "checkout-ci",
        2,
        true,
        &person("bob"),
        "ok",
        NOW,
    )
    .unwrap();
    let mut assignment = world.store.assignment("checkout").unwrap().clone();
    assignment.rca = Some(RcaRef {
        rca_id: "checkout-ci".into(),
        version: 2,
    });
    world.store.put_assignment(assignment);
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("w1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let id = only(&world).activation_id;
    assert_eq!(only(&world).state, ActivationState::WaitingForEvent);
    let calls = world.calls.get();
    // Idle ticks while waiting make no model call
    for minute in 1..10 {
        world.env.now = NOW + minute * MINUTE;
        worker::tick(&mut world.store, &mut world.env, Vec::new()).unwrap();
    }
    assert_eq!(world.calls.get(), calls);
    world.env.now = NOW + 20 * MINUTE;
    let at = world.env.now;
    let done = worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("w2", "ci.passed", &[], at)],
    )
    .unwrap();
    assert_eq!(done.ingested.resumed, vec![id.clone()]);
    assert_eq!(
        world.store.activation(&id).unwrap().state,
        ActivationState::Completed
    );
    assert_eq!(world.store.activation(&id).unwrap().triggers.len(), 1);

    // A wait whose deadline passes fails
    let mut world2 = world_with(definition(true), Some(RcaState::Reviewed));
    let graph2 = world.store.rca("checkout-ci", Some(2)).unwrap().clone();
    world2
        .store
        .state
        .rcas
        .get_mut("checkout-ci")
        .unwrap()
        .insert(2, graph2);
    let mut assignment = world2.store.assignment("checkout").unwrap().clone();
    assignment.rca = Some(RcaRef {
        rca_id: "checkout-ci".into(),
        version: 2,
    });
    world2.store.put_assignment(assignment);
    worker::tick(
        &mut world2.store,
        &mut world2.env,
        vec![ci("d1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    world2.env.now = NOW + 61 * MINUTE;
    let done = worker::tick(&mut world2.store, &mut world2.env, Vec::new()).unwrap();
    assert_eq!(done.deadlines.len(), 1);
    assert_eq!(only(&world2).steps["wait"].state, StepState::Failed);
    assert_eq!(
        only(&world2).state,
        ActivationState::Failed,
        "the wait failed, so the remediation is not proven"
    );

    // Missing context asks a person when the definition says so
    let mut definition = definition(true);
    definition.on_missing_context = MissingContext::Ask;
    definition.bricks = vec!["runbook-notes@1".into()];
    let mut world = world_with(definition, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("m", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let id = only(&world).activation_id;
    assert_eq!(only(&world).state, ActivationState::AwaitingInput);
    assert!(only(&world)
        .context
        .unwrap()
        .missing
        .iter()
        .any(|(code, _)| code == "BRICK_CONTEXT_MISSING"));
    let state = worker::answer(
        &mut world.store,
        &mut world.env,
        &id,
        "context-gap",
        "continue",
        &person("alice"),
        Via::Machine {
            signers: &signers(),
        },
    )
    .unwrap();
    assert_eq!(state, ActivationState::Completed);
    assert_eq!(world.sent.borrow().len(), 1);
}

/** Timers wake only their own assignment, one period after they are first seen, without a model
 * until their activation; events the resident runtime caused never become triggers
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn timers_and_self_caused_events() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    let mut assignment = world.store.assignment("checkout").unwrap().clone();
    assignment.timers = vec![Timer {
        timer_id: "daily".into(),
        every_minutes: 60,
    }];
    world.store.put_assignment(assignment.clone());
    let active = vec![&assignment];
    assert!(
        trigger::from_timers(&active, &BTreeMap::new(), "repo", NOW + 30 * MINUTE).is_empty(),
        "a new timer waits one period"
    );
    let due = trigger::from_timers(&active, &BTreeMap::new(), "repo", NOW + 61 * MINUTE);
    assert_eq!(due.len(), 1);
    let (_, slot, timer) = &due[0];
    assert!(
        trigger::from_timers(
            &active,
            &BTreeMap::from([("checkout:daily".to_string(), *slot)]),
            "repo",
            NOW + 62 * MINUTE
        )
        .is_empty(),
        "one firing per slot"
    );
    let mut other = assignment.clone();
    other.assignment_id = "other".into();
    assert!(
        trigger::assignment_matches(&other, timer).is_err(),
        "a timer wakes only its own assignment"
    );
    assert!(trigger::assignment_matches(&assignment, timer).is_ok());

    let mut tick_event = crate::telemetry::events::Event::new(
        "resident.tick",
        crate::telemetry::model::Domain::RecoveryOversight,
        "repo",
        "k1",
    );
    tick_event.sequence = 1;
    let mut command = crate::telemetry::events::Event::new(
        "command.executed",
        crate::telemetry::model::Domain::RecoveryOversight,
        "repo",
        "k2",
    );
    command.sequence = 2;
    command.payload = serde_json::json!({"notes": [{"kind": "RESIDENT_TRIGGER_RECEIVED"}]});
    let mut denial = crate::telemetry::events::Event::new(
        "hook.pre_tool_use",
        crate::telemetry::model::Domain::RecoveryOversight,
        "repo",
        "k3",
    );
    denial.sequence = 3;
    denial.occurred_at = NOW;
    denial.digest = "d".repeat(128);
    denial.bindings.zones = vec!["flow:checkout".into()];
    let (found, cursor) = trigger::from_events(&[tick_event, command, denial], 0, "repo", NOW);
    assert_eq!(cursor, 3);
    assert_eq!(found.len(), 1, "only the denial becomes a trigger");
    assert_eq!(found[0].kind, "crane.hook.pre_tool_use");
    assert_eq!(found[0].targets, vec!["flow:checkout".to_string()]);
    let (again, _) = trigger::from_events(&[], cursor, "repo", NOW);
    assert!(again.is_empty());
}

/** The report derives runtime status from activations and Crane: idle, awaiting approval, and
 * degraded when the signed assignment is revoked; context text never appears
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn report_status_and_redaction() {
    let mut world = setup(false, Some(RcaState::Reviewed));
    let view = world.authority.borrow().clone();
    assert_eq!(
        super::report::build(&world.store, Some(&view), NOW)["assignments"][0]["status"],
        "ACTIVE_IDLE"
    );
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("r", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let report = super::report::build(&world.store, Some(&view), NOW);
    assert_eq!(report["assignments"][0]["status"], "AWAITING_APPROVAL");
    assert_eq!(report["inbox"]["approvals"].as_array().unwrap().len(), 1);
    let id = only(&world).activation_id;
    worker::cancel(&mut world.store, &id, &person("alice"), "not needed", NOW).unwrap();
    let mut revoked = view.clone();
    revoked.assignments[0].active = false;
    let report = super::report::build(&world.store, Some(&revoked), NOW);
    assert_eq!(report["assignments"][0]["status"], "DEGRADED");
    assert!(report["assignments"][0]["health"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|code| code == "NO_SIGNED_ASSIGNMENT"));
    let text = report.to_string();
    assert!(
        !text.contains("Respond to failed checkout CI runs."),
        "the purpose text stays on the machine"
    );
    assert_eq!(report["health"]["idle_model_calls"], 0);
    assert!(report["catalog"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|step| step["type"] == "PAGE_ONCALL" && step["available"] == false));
}

/** A fake Jira: issues by label; it can answer "unknown" after creating (a timeout after the
 * effect), and it can be unreachable for reconciliation
 * Fields
    - issues: Rc<RefCell<BTreeMap<String, String>>> - label to issue key
    - created: Rc<Cell<u32>> - issues created
    - uncertain: Rc<Cell<bool>> - answer "unknown" after creating
    - unreachable: Rc<Cell<bool>> - reconciliation cannot read Jira
*/
struct FakeJira {
    issues: Rc<RefCell<BTreeMap<String, String>>>,
    created: Rc<Cell<u32>>,
    uncertain: Rc<Cell<bool>>,
    unreachable: Rc<Cell<bool>>,
}

impl ActionAdapter for FakeJira {
    /** CREATE_TICKET
     * Input
        - None (uses self)
     * Output
        - StepType
    */
    fn step_type(&self) -> StepType {
        StepType::CreateTicket
    }

    /** Create the issue unless one carries the step's label
     * Input
        - _text: &str - summary and description
        - key: &str - idempotency key
     * Output
        - Dispatch
    */
    fn dispatch(&self, _text: &str, key: &str) -> Dispatch {
        let label = crate::integrations::issue_label(key);
        if let Some(existing) = self.issues.borrow().get(&label) {
            return Dispatch::Done {
                external_ref: format!("jira:{existing}"),
                evidence: "existing".into(),
            };
        }
        self.created.set(self.created.get() + 1);
        let issue = format!("PAY-{}", self.created.get());
        self.issues.borrow_mut().insert(label, issue.clone());
        if self.uncertain.get() {
            Dispatch::Uncertain {
                detail: "timed out".into(),
            }
        } else {
            Dispatch::Done {
                external_ref: format!("jira:{issue}"),
                evidence: "created".into(),
            }
        }
    }

    /** Look the label up, unless Jira is unreachable
     * Input
        - key: &str - idempotency key
     * Output
        - Option<Dispatch>
    */
    fn reconcile(&self, key: &str) -> Option<Dispatch> {
        if self.unreachable.get() {
            return None;
        }
        Some(
            match self
                .issues
                .borrow()
                .get(&crate::integrations::issue_label(key))
            {
                Some(issue) => Dispatch::Done {
                    external_ref: format!("jira:{issue}"),
                    evidence: "found by label".into(),
                },
                None => Dispatch::Failed {
                    effect: false,
                    detail: "no issue".into(),
                },
            },
        )
    }
}

/** The fixture world whose reviewed remediation opens a Jira ticket, with a fake Jira
 * Input
    - None
 * Output
    - (World, FakeJira handles: issues, created, uncertain, unreachable)
*/
#[allow(clippy::type_complexity)]
fn ticket_world() -> (
    World,
    Rc<RefCell<BTreeMap<String, String>>>,
    Rc<Cell<u32>>,
    Rc<Cell<bool>>,
    Rc<Cell<bool>>,
) {
    let mut world = setup(true, Some(RcaState::Reviewed));
    world
        .authority
        .borrow_mut()
        .connectors
        .insert("jira".into(), true);
    let mut graph = checkout_graph();
    graph.version = 2;
    graph
        .nodes
        .iter_mut()
        .find(|node| node.id == "notify")
        .unwrap()
        .steps = vec![rca::step(
        "ticket",
        StepType::CreateTicket,
        &[("summary", "Checkout tests failed".into())],
        &[],
        String::new(),
    )];
    registry::save_rca(&mut world.store, graph, &person("alice"), NOW).unwrap();
    registry::review_rca(
        &mut world.store,
        "checkout-ci",
        2,
        true,
        &person("bob"),
        "ok",
        NOW,
    )
    .unwrap();
    let mut assignment = world.store.assignment("checkout").unwrap().clone();
    assignment.rca = Some(RcaRef {
        rca_id: "checkout-ci".into(),
        version: 2,
    });
    world.store.put_assignment(assignment);
    let issues = Rc::new(RefCell::new(BTreeMap::new()));
    let created = Rc::new(Cell::new(0));
    let uncertain = Rc::new(Cell::new(false));
    let unreachable = Rc::new(Cell::new(false));
    world.env.adapters.push(Box::new(FakeJira {
        issues: issues.clone(),
        created: created.clone(),
        uncertain: uncertain.clone(),
        unreachable: unreachable.clone(),
    }));
    (world, issues, created, uncertain, unreachable)
}

/** CREATE_TICKET opens one Jira issue per step; an unknown outcome is established by looking the
 * step's label up (no person, no second issue), and stays with a person when Jira cannot answer
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn tickets_are_created_once_and_reconciled_from_jira() {
    let (mut world, _, created, _, _) = ticket_world();
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("j1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let activation = only(&world);
    assert_eq!(
        activation.state,
        ActivationState::Completed,
        "{:?}",
        activation.timeline
    );
    assert_eq!(
        activation.steps["ticket"].external_ref.as_deref(),
        Some("jira:PAY-1")
    );
    assert_eq!(created.get(), 1);

    // Jira created the issue but the answer was lost: the next tick finds it by its label
    let (mut world, _, created, uncertain, _) = ticket_world();
    uncertain.set(true);
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("j2", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert_eq!(only(&world).state, ActivationState::ReconciliationRequired);
    let done = worker::tick(&mut world.store, &mut world.env, Vec::new()).unwrap();
    assert_eq!(
        done.reconciled.len(),
        1,
        "established from Jira without a person"
    );
    assert_eq!(
        only(&world).state,
        ActivationState::Completed,
        "{:?}",
        only(&world).timeline
    );
    assert_eq!(
        only(&world).steps["ticket"].external_ref.as_deref(),
        Some("jira:PAY-1")
    );
    assert_eq!(created.get(), 1, "never a second issue");

    // Jira cannot be read: a person reconciles, and the next tick tries Jira again first
    let (mut world, _, created, uncertain, unreachable) = ticket_world();
    uncertain.set(true);
    unreachable.set(true);
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("j3", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    assert_eq!(only(&world).state, ActivationState::ReconciliationRequired);
    worker::tick(&mut world.store, &mut world.env, Vec::new()).unwrap();
    assert_eq!(
        only(&world).state,
        ActivationState::ReconciliationRequired,
        "still unknown while Jira cannot answer"
    );
    unreachable.set(false);
    let done = worker::tick(&mut world.store, &mut world.env, Vec::new()).unwrap();
    assert_eq!(done.reconciled.len(), 1);
    assert_eq!(only(&world).state, ActivationState::Completed);
    assert_eq!(created.get(), 1);
}

/** Dashboard decisions are applied on the machine with the same checks, at most once: approvals
 * and answers only when the definition takes them and the plan is the one the admin saw; pause,
 * revoke, and cancel always; resume never
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn dashboard_decisions_are_applied_once_with_the_same_checks() {
    let decision = |id: &str, action: &str, activation: &str, plan: &str| serde_json::json!({"decision_id": id, "action": action, "activation": activation, "assignment": "checkout", "step": "notify", "plan": plan, "reason": "seen in CI", "decided_by": "octo"});
    // The definition does not take dashboard approvals
    let mut world = setup(false, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("d1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let activation = only(&world);
    let plan = activation.plan.as_ref().unwrap().digest.clone();
    let results = super::remote::apply(
        &mut world.store,
        &mut world.env,
        &[decision(
            "rsd_1",
            "approve",
            &activation.activation_id,
            &plan,
        )],
    );
    assert_eq!(results[0]["status"], "REJECTED");
    assert!(results[0]["detail"]
        .as_str()
        .unwrap()
        .contains("remote_decisions"));
    assert!(world.sent.borrow().is_empty());
    // Pausing is always accepted; resuming never is
    let results = super::remote::apply(
        &mut world.store,
        &mut world.env,
        &[
            decision("rsd_2", "pause", "", ""),
            decision("rsd_3", "resume", "", ""),
        ],
    );
    assert_eq!(results[0]["status"], "APPLIED");
    assert_eq!(
        world.store.assignment("checkout").unwrap().lifecycle,
        Lifecycle::Paused
    );
    assert_eq!(results[1]["status"], "REJECTED");

    // The definition takes them: a decision on another plan is refused, the current one applies
    let mut definition = definition(false);
    definition.remote_decisions = true;
    let mut world = world_with(definition, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("d2", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let activation = only(&world);
    let plan = activation.plan.as_ref().unwrap().digest.clone();
    let results = super::remote::apply(
        &mut world.store,
        &mut world.env,
        &[decision(
            "rsd_4",
            "approve",
            &activation.activation_id,
            "0000",
        )],
    );
    assert!(results[0]["detail"]
        .as_str()
        .unwrap()
        .contains("plan changed"));
    let results = super::remote::apply(
        &mut world.store,
        &mut world.env,
        &[decision(
            "rsd_5",
            "approve",
            &activation.activation_id,
            &plan,
        )],
    );
    assert_eq!(results[0]["status"], "APPLIED", "{results:?}");
    assert_eq!(results[0]["state"], "COMPLETED");
    assert_eq!(world.sent.borrow().len(), 1);
    let approval = &world
        .store
        .activation(&activation.activation_id)
        .unwrap()
        .approvals[0];
    assert_eq!(approval.approver.name, "@octo (dashboard, GitHub admin)");
    // A redelivered decision is not applied twice
    let results = super::remote::apply(
        &mut world.store,
        &mut world.env,
        &[decision(
            "rsd_5",
            "approve",
            &activation.activation_id,
            &plan,
        )],
    );
    assert!(results[0]["detail"]
        .as_str()
        .unwrap()
        .contains("already applied"));
    assert_eq!(world.sent.borrow().len(), 1);
}

/** Separation of duties keeps the activator from approving, and a denial blocks the step and
 * sends nothing
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn separation_of_duties_and_denials() {
    let mut definition = definition(false);
    definition.distinct_approver = true;
    let mut world = world_with(definition, Some(RcaState::Reviewed));
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("s1", "ci.failed", &[("job", "tests")], NOW)],
    )
    .unwrap();
    let id = only(&world).activation_id;
    let error = worker::approve(
        &mut world.store,
        &mut world.env,
        &id,
        "notify",
        &person("alice"),
        Via::Machine {
            signers: &signers(),
        },
        true,
        "mine",
    )
    .unwrap_err();
    assert!(error.contains("separation of duties"), "{error}");
    let state = worker::approve(
        &mut world.store,
        &mut world.env,
        &id,
        "notify",
        &person("bob"),
        Via::Machine {
            signers: &signers(),
        },
        false,
        "not now",
    )
    .unwrap();
    assert_eq!(state, ActivationState::Blocked);
    assert_eq!(
        world.store.activation(&id).unwrap().steps["notify"].state,
        StepState::Denied
    );
    assert!(world.sent.borrow().is_empty());
}

/** An event delivered late (it happened before the activation's own trigger) does not satisfy a
 * wait for a later event; the next real one does
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn out_of_order_events_do_not_satisfy_waits() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    let mut graph = checkout_graph();
    graph.version = 2;
    graph
        .nodes
        .iter_mut()
        .find(|node| node.id == "notify")
        .unwrap()
        .steps
        .push(rca::step(
            "wait",
            StepType::WaitForEvent,
            &[("kind", "ci.passed".into())],
            &["notify"],
            String::new(),
        ));
    registry::save_rca(&mut world.store, graph, &person("alice"), NOW).unwrap();
    registry::review_rca(
        &mut world.store,
        "checkout-ci",
        2,
        true,
        &person("bob"),
        "ok",
        NOW,
    )
    .unwrap();
    let mut assignment = world.store.assignment("checkout").unwrap().clone();
    assignment.rca = Some(RcaRef {
        rca_id: "checkout-ci".into(),
        version: 2,
    });
    world.store.put_assignment(assignment);
    world.env.now = NOW + 10 * MINUTE;
    worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci(
            "o1",
            "ci.failed",
            &[("job", "tests")],
            NOW + 10 * MINUTE,
        )],
    )
    .unwrap();
    let id = world
        .store
        .state
        .activations
        .values()
        .next()
        .unwrap()
        .activation_id
        .clone();
    assert_eq!(
        world.store.activation(&id).unwrap().state,
        ActivationState::WaitingForEvent
    );
    world.env.now = NOW + 12 * MINUTE;
    let done = worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("o0", "ci.passed", &[], NOW + 5 * MINUTE)],
    )
    .unwrap();
    assert!(
        done.ingested.resumed.is_empty(),
        "a pass that happened before the failure is not the awaited one"
    );
    assert_eq!(
        world.store.activation(&id).unwrap().state,
        ActivationState::WaitingForEvent
    );
    let done = worker::tick(
        &mut world.store,
        &mut world.env,
        vec![ci("o2", "ci.passed", &[], NOW + 12 * MINUTE)],
    )
    .unwrap();
    assert_eq!(done.ingested.resumed, vec![id.clone()]);
    assert_eq!(
        world.store.activation(&id).unwrap().state,
        ActivationState::Completed
    );
}

/** Recorded GitHub answers
 * Fields
    - answer: RefCell<serde_json::Value> - what every GET answers
    - calls: Cell<u32> - GETs made
*/
struct FakeGitHub {
    answer: RefCell<serde_json::Value>,
    calls: Cell<u32>,
}

impl crate::capture::resources::github::GitHubApi for FakeGitHub {
    /** Answer a GET with the recorded value
     * Input
        - _path: &str - API path
        - _deadline: Option<std::time::Instant> - deadline
     * Output
        - ApiAnswer
    */
    fn get(
        &self,
        _path: &str,
        _deadline: Option<std::time::Instant>,
    ) -> crate::capture::resources::github::ApiAnswer {
        self.calls.set(self.calls.get() + 1);
        crate::capture::resources::github::ApiAnswer::Found(self.answer.borrow().clone())
    }
}

/** Finished GitHub Actions runs become CI triggers: the first read only records where it is, a
 * new run becomes one trigger with its workflow and branch, reads are at most every 5 minutes,
 * and nothing is read without a CI subscription
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn github_actions_runs_become_ci_triggers() {
    let run = |id: u64, conclusion: &str| serde_json::json!({"id": id, "status": "completed", "conclusion": conclusion, "name": "tests", "head_branch": "main", "event": "push", "run_attempt": 1, "head_sha": "abcdef1234567890", "updated_at": "2027-01-15T08:00:00Z"});
    let api = FakeGitHub {
        answer: RefCell::new(serde_json::json!({"workflow_runs": [run(41, "success")]})),
        calls: Cell::new(0),
    };
    let mut world = setup(true, Some(RcaState::Reviewed));
    let first = worker::poll_ci(&api, "github.com/acme/shop", &mut world.store, "repo", NOW);
    assert!(first.is_empty(), "history is recorded, not replayed");
    assert_eq!(world.store.state.cursors.ci_run, 41);
    *api.answer.borrow_mut() =
        serde_json::json!({"workflow_runs": [run(42, "failure"), run(41, "success")]});
    assert!(
        worker::poll_ci(
            &api,
            "github.com/acme/shop",
            &mut world.store,
            "repo",
            NOW + MINUTE
        )
        .is_empty(),
        "within 5 minutes nothing is read"
    );
    assert_eq!(api.calls.get(), 1);
    let found = worker::poll_ci(
        &api,
        "github.com/acme/shop",
        &mut world.store,
        "repo",
        NOW + 6 * MINUTE,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, "ci.failed");
    assert_eq!(found[0].attributes["workflow"], "tests");
    assert_eq!(found[0].attributes["branch"], "main");
    assert_eq!(found[0].idempotency_key, "gh-run:42:1");
    assert!(
        found[0].targets.is_empty(),
        "a workflow run belongs to the repository"
    );
    // A repository-scope subscription takes it
    let mut assignment = world.store.assignment("checkout").unwrap().clone();
    assignment.subscriptions[0].scope = SubscriptionScope::Repository;
    assert!(trigger::assignment_matches(&assignment, &found[0]).is_ok());
    // No CI subscription: GitHub is not read at all
    world
        .store
        .state
        .assignments
        .values_mut()
        .for_each(|assignment| assignment.subscriptions.clear());
    assert!(worker::poll_ci(
        &api,
        "github.com/acme/shop",
        &mut world.store,
        "repo",
        NOW + 20 * MINUTE
    )
    .is_empty());
    assert_eq!(api.calls.get(), 2);
}

/** Duplicating an assignment makes a draft; on another target the RCA graph is dropped
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn assignments_are_duplicated_as_drafts() {
    let mut world = setup(true, Some(RcaState::Reviewed));
    let copy = registry::duplicate_assignment(
        &mut world.store,
        "checkout",
        "checkout-2",
        None,
        &person("alice"),
        NOW,
    )
    .unwrap();
    assert_eq!(copy.lifecycle, Lifecycle::Draft);
    assert!(copy.rca.is_some());
    let other = registry::duplicate_assignment(
        &mut world.store,
        "checkout",
        "refund",
        Some(crate::brick::model::Target::parse("flow:refund").unwrap()),
        &person("alice"),
        NOW,
    )
    .unwrap();
    assert_eq!(other.target.key(), "flow:refund");
    assert!(other.rca.is_none(), "a graph explains one target");
    assert!(
        registry::duplicate_assignment(
            &mut world.store,
            "checkout",
            "refund",
            None,
            &person("alice"),
            NOW
        )
        .is_err(),
        "ids are unique"
    );
}
