// Runbook tests over the invocation demonstration's worlds (fixture systems; nothing external):
// static validation, a verified run that settles its budget, idempotent starts, approval points
// bound to their digest and decided by another person, a skill approval pausing the run, branches
// on failure, cancellation, revocation, and retries refused after effects.

use std::collections::BTreeMap;

use super::model::{validate, Budget, FailurePolicy, InputSource, Outcome, Runbook, Step, When};
use super::run::{account, Orchestrator};
use super::store::{RunState, RunbookState, RunbookStore, StepState};
use super::RUNBOOK_SCHEMA_VERSION;
use crate::capability::demo as capability_demo;
use crate::invocation::demo::{
    diagnostics_request, diagnostics_world, refund_request, refund_world, status_fixture, World,
    EPOCH,
};
use crate::telemetry::ledger::Ledger;

/** A step running the diagnostics capability of a world
 * Input
    - world: &World - world
    - id: &str - step id
 * Output
    - Step
*/
fn diagnose(world: &World, id: &str) -> Step {
    let request = diagnostics_request(world, "unused");
    Step {
        step_id: id.into(),
        skill_id: request.capability_id.clone(),
        skill_version: 1,
        skill_digest: request.digest.clone(),
        bindings: request.bindings.clone(),
        inputs: [(
            "deployment_id".to_string(),
            InputSource::RunInput {
                name: "deployment".into(),
            },
        )]
        .into_iter()
        .collect(),
        preconditions: request.confirmed_preconditions.clone(),
        depends_on: Vec::new(),
        when: None,
        approval: false,
        max_retries: 0,
        on_failure: FailurePolicy::Stop,
    }
}

/** A runbook around steps
 * Input
    - steps: Vec<Step> - steps
    - operations: u64 - budget
 * Output
    - Runbook
*/
fn runbook(steps: Vec<Step>, operations: u64) -> Runbook {
    Runbook {
        runbook_id: "triage".into(),
        version: 1,
        schema_version: RUNBOOK_SCHEMA_VERSION,
        name: "Triage a failed deployment".into(),
        description: "Collect the deployment, its checks, and its logs, and confirm the outcome."
            .into(),
        department: Some("operations".into()),
        owner: Some("team:sre".into()),
        environment: Some("production".into()),
        repository: Some("acme/payments".into()),
        inputs: vec!["deployment".into()],
        steps,
        completion: Vec::new(),
        budget: Budget { operations },
    }
}

/** Store a runbook and activate it as the lead (another person than the engineer who wrote it)
 * Input
    - store: &mut RunbookStore - store
    - definition: Runbook - runbook
    - world: &World - world (its cast)
 * Output
    - None
*/
fn activate(store: &mut RunbookStore, definition: Runbook, world: &World) {
    assert!(
        validate(&definition).is_empty(),
        "{:?}",
        validate(&definition)
    );
    store
        .create(definition, &world.cast.engineer, EPOCH)
        .unwrap();
    store
        .transition(
            "triage",
            1,
            RunbookState::Active,
            &world.cast.lead,
            "reviewed",
            EPOCH,
        )
        .unwrap();
}

/** Run inputs
 * Input
    - None
 * Output
    - BTreeMap<String, String>
*/
fn inputs() -> BTreeMap<String, String> {
    [("deployment".to_string(), "dep-7731".to_string())]
        .into_iter()
        .collect()
}

/** Static validation refuses cycles, branches on non-dependencies, unpinned skills, mismatched
 * bindings, unbounded retries, undeclared inputs, and budgets that cannot cover the plan
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn validation_refuses_unsafe_runbooks() {
    let world = diagnostics_world().unwrap();
    let codes = |definition: &Runbook| {
        validate(definition)
            .iter()
            .map(|finding| finding["code"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert!(codes(&runbook(vec![diagnose(&world, "a")], 4)).is_empty());
    let mut a = diagnose(&world, "a");
    let mut b = diagnose(&world, "b");
    a.depends_on = vec!["b".into()];
    b.depends_on = vec!["a".into()];
    assert!(codes(&runbook(vec![a, b], 8)).contains(&"dependency_cycle".to_string()));
    let mut branch = diagnose(&world, "b");
    branch.when = Some(When {
        step: "a".into(),
        outcome: Outcome::Failed,
    });
    assert!(codes(&runbook(vec![diagnose(&world, "a"), branch], 8))
        .contains(&"branch_not_dependency".to_string()));
    let mut unpinned = diagnose(&world, "a");
    unpinned.skill_digest.clear();
    unpinned.max_retries = 3;
    unpinned.bindings.capability_version = 2;
    unpinned.inputs.insert(
        "x".into(),
        InputSource::RunInput {
            name: "nope".into(),
        },
    );
    let found = codes(&runbook(vec![unpinned], 64));
    for code in [
        "skill_unpinned",
        "retries_unbounded",
        "bindings_mismatch",
        "undeclared_input",
    ] {
        assert!(found.contains(&code.to_string()), "{code} in {found:?}");
    }
    assert!(codes(&runbook(vec![diagnose(&world, "a")], 3))
        .contains(&"budget_insufficient".to_string()));
    let mut retried = diagnose(&world, "a");
    retried.max_retries = 1;
    assert!(codes(&runbook(vec![retried], 4)).contains(&"budget_insufficient".to_string()));
}

/** A run of a verified skill succeeds, its budget is settled from the invocation's own record, a
 * repeated start returns the same run, and a run of a draft or revoked version is refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn runs_succeed_and_settle_their_budget() {
    let mut world = diagnostics_world().unwrap();
    let confirmed = diagnose(&world, "x").preconditions;
    let mut store = RunbookStore::in_memory();
    store
        .create(
            runbook(vec![diagnose(&world, "diagnose")], 4),
            &world.cast.engineer,
            EPOCH,
        )
        .unwrap();
    let lead = world.cast.lead.clone();
    let directory = tempfile::tempdir().unwrap();
    let budget = Ledger::open(directory.path());
    let engineer = world.cast.engineer.clone();
    let reader = capability_demo::reader(&engineer).unwrap();
    let mut engine = world.engine(&reader);
    let mut orchestrator = Orchestrator {
        engine: &mut engine,
        store: &mut store,
        budget: &budget,
        actor: engineer.clone(),
        now: EPOCH,
    };
    let draft = orchestrator
        .start("triage", 1, inputs(), confirmed.clone(), "k1")
        .unwrap_err();
    assert!(draft.contains("Draft"), "{draft}");
    orchestrator
        .store
        .transition("triage", 1, RunbookState::Active, &lead, "reviewed", EPOCH)
        .unwrap();
    let run = orchestrator
        .start("triage", 1, inputs(), confirmed.clone(), "k1")
        .unwrap();
    assert_eq!(run.state, RunState::Succeeded, "{:?}", run.log);
    assert_eq!(run.steps["diagnose"].state, StepState::Succeeded);
    let balance = Ledger::balance_of(&budget.entries().unwrap(), &account(&run.run_id));
    assert_eq!(
        (
            balance.granted,
            balance.consumed,
            balance.reserved,
            balance.available
        ),
        (4, 4, 0, 0)
    );
    let again = orchestrator
        .start("triage", 1, inputs(), confirmed.clone(), "k1")
        .unwrap();
    assert_eq!(again.run_id, run.run_id);
    assert!(again.verify_digest());
    assert!(orchestrator
        .start("triage", 1, BTreeMap::new(), confirmed.clone(), "k3")
        .unwrap_err()
        .contains("required"));
    orchestrator
        .store
        .transition("triage", 1, RunbookState::Revoked, &lead, "replaced", EPOCH)
        .unwrap();
    assert!(orchestrator
        .start("triage", 1, inputs(), confirmed, "k2")
        .unwrap_err()
        .contains("Revoked"));
}

/** An approval point pauses the run until another person approves the exact step digest; the
 * starter cannot approve, a wrong digest is refused, a used approval cannot be revoked, and a
 * denial blocks the step
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn approval_points_are_bound_and_decided_by_another_person() {
    let mut world = diagnostics_world().unwrap();
    let mut gated = diagnose(&world, "diagnose");
    gated.approval = true;
    let confirmed = gated.preconditions.clone();
    let mut store = RunbookStore::in_memory();
    activate(&mut store, runbook(vec![gated.clone()], 4), &world);
    let (engineer, lead) = (world.cast.engineer.clone(), world.cast.lead.clone());
    let directory = tempfile::tempdir().unwrap();
    let budget = Ledger::open(directory.path());
    let reader = capability_demo::reader(&engineer).unwrap();
    let mut engine = world.engine(&reader);
    let mut orchestrator = Orchestrator {
        engine: &mut engine,
        store: &mut store,
        budget: &budget,
        actor: engineer.clone(),
        now: EPOCH,
    };
    let run = orchestrator
        .start("triage", 1, inputs(), confirmed.clone(), "k1")
        .unwrap();
    assert_eq!(run.state, RunState::AwaitingApproval);
    assert_eq!(run.steps["diagnose"].state, StepState::AwaitingApproval);
    let digest = run.approval_digest(&gated);
    assert!(orchestrator
        .approve(&run.run_id, "diagnose", &digest)
        .unwrap_err()
        .contains("other than"));
    orchestrator.actor = lead.clone();
    assert!(orchestrator
        .approve(&run.run_id, "diagnose", "0000")
        .unwrap_err()
        .contains("another step digest"));
    let approved = orchestrator
        .approve(&run.run_id, "diagnose", &digest)
        .unwrap();
    assert_eq!(approved.state, RunState::Succeeded, "{:?}", approved.log);
    assert!(orchestrator
        .revoke_approval(&run.run_id, "diagnose", "late")
        .unwrap_err()
        .contains("already started"));
    orchestrator.actor = engineer;
    let second = orchestrator
        .start("triage", 1, inputs(), confirmed, "k2")
        .unwrap();
    orchestrator.actor = lead;
    let denied = orchestrator
        .deny(&second.run_id, "diagnose", "not today")
        .unwrap();
    assert_eq!(denied.steps["diagnose"].state, StepState::Blocked);
    assert_eq!(denied.state, RunState::Blocked);
    assert!(orchestrator
        .cancel(&second.run_id, "done")
        .unwrap_err()
        .contains("already ended"));
}

/** A skill that needs its own approval pauses the run; after the skill's approver approves the
 * exact plan, resuming runs it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn skill_approvals_pause_the_run() {
    let mut world = refund_world().unwrap();
    let request = refund_request(&world, "unused").unwrap();
    let step = Step {
        step_id: "refund".into(),
        skill_id: request.capability_id.clone(),
        skill_version: 1,
        skill_digest: request.digest.clone(),
        bindings: request.bindings.clone(),
        inputs: request
            .inputs
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    InputSource::Literal {
                        value: value.clone(),
                    },
                )
            })
            .collect(),
        preconditions: request.confirmed_preconditions.clone(),
        depends_on: Vec::new(),
        when: None,
        approval: false,
        max_retries: 0,
        on_failure: FailurePolicy::Stop,
    };
    let cost = step.cost();
    let mut definition = runbook(vec![step], cost);
    definition.inputs.clear();
    let mut store = RunbookStore::in_memory();
    activate(&mut store, definition, &world);
    let (engineer, lead) = (world.cast.engineer.clone(), world.cast.lead.clone());
    let directory = tempfile::tempdir().unwrap();
    let budget = Ledger::open(directory.path());
    let reader = capability_demo::reader(&engineer).unwrap();
    let mut engine = world.engine(&reader);
    let mut orchestrator = Orchestrator {
        engine: &mut engine,
        store: &mut store,
        budget: &budget,
        actor: engineer,
        now: EPOCH,
    };
    let run = orchestrator
        .start(
            "triage",
            1,
            BTreeMap::new(),
            request.confirmed_preconditions.clone(),
            "k1",
        )
        .unwrap();
    assert_eq!(run.state, RunState::AwaitingApproval, "{:?}", run.log);
    assert_eq!(run.steps["refund"].state, StepState::AwaitingSkillApproval);
    let paused = orchestrator.resume(&run.run_id, None).unwrap();
    assert_eq!(paused.state, RunState::AwaitingApproval);
    let invocation = run.steps["refund"].invocation_id.clone().unwrap();
    let digest = orchestrator
        .engine
        .ledger
        .get(&invocation)
        .unwrap()
        .plan
        .unwrap()
        .plan_digest;
    orchestrator
        .engine
        .approve(&lead, &invocation, &digest)
        .unwrap();
    let resumed = orchestrator.resume(&run.run_id, None).unwrap();
    assert_eq!(
        resumed.steps["refund"].state,
        StepState::Succeeded,
        "{:?}",
        resumed.log
    );
    assert_eq!(resumed.state, RunState::Succeeded);
}

/** A failed step skips the steps that wait for its success and runs the one that branches on its
 * failure; it cannot be retried after its effects or beyond its retries
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn branches_follow_failures() {
    let mut world = diagnostics_world().unwrap();
    world.set_status(status_fixture("failed", false));
    let first = diagnose(&world, "diagnose");
    let mut follow = diagnose(&world, "follow");
    follow.depends_on = vec!["diagnose".into()];
    let mut fallback = diagnose(&world, "fallback");
    fallback.depends_on = vec!["diagnose".into()];
    fallback.when = Some(When {
        step: "diagnose".into(),
        outcome: Outcome::Failed,
    });
    let confirmed = first.preconditions.clone();
    let mut store = RunbookStore::in_memory();
    activate(
        &mut store,
        runbook(vec![first, follow, fallback], 12),
        &world,
    );
    let engineer = world.cast.engineer.clone();
    let directory = tempfile::tempdir().unwrap();
    let budget = Ledger::open(directory.path());
    let reader = capability_demo::reader(&engineer).unwrap();
    let mut engine = world.engine(&reader);
    let mut orchestrator = Orchestrator {
        engine: &mut engine,
        store: &mut store,
        budget: &budget,
        actor: engineer,
        now: EPOCH,
    };
    let run = orchestrator
        .start("triage", 1, inputs(), confirmed, "k1")
        .unwrap();
    assert_eq!(
        run.steps["diagnose"].state,
        StepState::Failed,
        "{:?}",
        run.log
    );
    assert_eq!(run.steps["follow"].state, StepState::Skipped);
    assert_ne!(
        run.steps["fallback"].state,
        StepState::Skipped,
        "{:?}",
        run.log
    );
    assert_eq!(run.state, RunState::Failed);
    let retry = orchestrator
        .resume(&run.run_id, Some("diagnose"))
        .unwrap_err();
    assert!(
        retry.contains("retries") || retry.contains("effects"),
        "{retry}"
    );
}
