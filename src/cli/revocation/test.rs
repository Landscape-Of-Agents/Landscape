// crane revocation test: revocation propagates where the code says it does, on the invocation
// demonstration's fixture world (in memory): a locally revoked version is blocked; a version revoked
// in the shared store is blocked even where this machine still lists it as published; a shared store
// that cannot be verified blocks everything; and a revoked runbook stops a paused run at its next
// step.

use std::collections::BTreeMap;

use serde_json::json;

use crate::capability::demo as capability_demo;
use crate::capability::lifecycle::LifecycleState;
use crate::capability::refs::Sources;
use crate::capability::shared::{key, SharedState, SharedSummary, SharedView};
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::demo::{diagnostics_request, diagnostics_world, World, EPOCH};
use crate::invocation::engine::Engine;
use crate::invocation::lifecycle::InvocationState;
use crate::runbook::model::{Budget, FailurePolicy, InputSource, Runbook, Step};
use crate::runbook::run::Orchestrator;
use crate::runbook::store::{RunbookState, RunbookStore, StepState};
use crate::telemetry::ledger::Ledger;

/** Submit the diagnostics request with a given shared store view
 * Input
    - world: &mut World - fixture world
    - shared: &SharedView - view the engine sees
    - key: &str - idempotency key
 * Output
    - Result<InvocationState, String>
*/
fn submit(world: &mut World, shared: &SharedView, key: &str) -> Result<InvocationState, String> {
    let reader = capability_demo::reader(&world.cast.engineer).map_err(String::from)?;
    let actor = world.cast.engineer.clone();
    let request = diagnostics_request(world, key);
    let mut engine = Engine {
        registry: &mut world.registry,
        sources: Sources {
            capture: &world.capture.store,
            current: &world.current,
            reader: &reader,
        },
        lab_reports: &world.lab_reports,
        discovery_reports: &world.discovery_reports,
        compilation_reports: &world.compilation_reports,
        adapters: &world.adapters,
        runtime: world.runtime.as_ref(),
        ledger: &mut world.ledger,
        events: &mut world.events,
        clock: &world.clock,
        shared,
    };
    engine
        .submit(&actor, request)
        .map(|record| record.state)
        .map_err(String::from)
}

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane revocation test")?;
    let mut world = diagnostics_world().map_err(String::from)?;
    let id = "deployment-failure-diagnostics";
    let digest = world.digest(id);
    let revoked_view = SharedView::Verified(
        [(
            key(id, 1),
            SharedSummary {
                state: SharedState::Revoked,
                digest: digest.clone(),
                changed_by: Some("lead".into()),
            },
        )]
        .into_iter()
        .collect(),
    );
    let published_view = SharedView::Verified(
        [(
            key(id, 1),
            SharedSummary {
                state: SharedState::Published,
                digest,
                changed_by: None,
            },
        )]
        .into_iter()
        .collect(),
    );
    let mut cases = vec![
        ("a published version runs while the shared store agrees", submit(&mut world, &published_view, "rev-1")? == InvocationState::Succeeded),
        ("a version revoked in the shared store is blocked although this machine lists it as published", submit(&mut world, &revoked_view, "rev-2")? == InvocationState::Blocked),
        ("a shared store that cannot be verified blocks everything", submit(&mut world, &SharedView::Unverified("tampered".into()), "rev-3")? == InvocationState::Blocked),
    ];
    world
        .registry
        .withdraw(
            &world.cast.lead,
            id,
            Some(1),
            LifecycleState::Revoked,
            "test",
            EPOCH,
        )
        .map_err(String::from)?;
    cases.push((
        "a locally revoked version is blocked",
        submit(&mut world, &SharedView::Absent, "rev-4")? == InvocationState::Blocked,
    ));
    // A paused run of a revoked runbook stops at its next step
    let mut fresh = diagnostics_world().map_err(String::from)?;
    let request = diagnostics_request(&fresh, "unused");
    let step = Step {
        step_id: "diagnose".into(),
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
        approval: true,
        max_retries: 0,
        on_failure: FailurePolicy::Stop,
    };
    let cost = step.cost();
    let definition = Runbook {
        runbook_id: "revocation-test".into(),
        version: 1,
        schema_version: crate::runbook::RUNBOOK_SCHEMA_VERSION,
        name: "test".into(),
        description: "revocation test".into(),
        department: None,
        owner: None,
        environment: request.environment.clone(),
        repository: request.repository.clone(),
        inputs: Vec::new(),
        steps: vec![step.clone()],
        completion: Vec::new(),
        budget: Budget { operations: cost },
    };
    let (engineer, lead) = (fresh.cast.engineer.clone(), fresh.cast.lead.clone());
    let mut store = RunbookStore::in_memory();
    store.create(definition, &engineer, EPOCH)?;
    store.transition(
        "revocation-test",
        1,
        RunbookState::Active,
        &lead,
        "test",
        EPOCH,
    )?;
    let directory =
        std::env::temp_dir().join(format!("crane-revocation-test-{}", std::process::id()));
    let budget = Ledger::open(&directory);
    let reader = capability_demo::reader(&engineer).map_err(String::from)?;
    let mut engine = fresh.engine(&reader);
    let mut orchestrator = Orchestrator {
        engine: &mut engine,
        store: &mut store,
        budget: &budget,
        actor: engineer,
        now: EPOCH,
    };
    let run = orchestrator.start(
        "revocation-test",
        1,
        BTreeMap::new(),
        request.confirmed_preconditions.clone(),
        "k1",
    )?;
    let digest = run.approval_digest(&step);
    orchestrator.store.transition(
        "revocation-test",
        1,
        RunbookState::Revoked,
        &lead,
        "test",
        EPOCH,
    )?;
    orchestrator.actor = lead;
    let after = orchestrator.approve(&run.run_id, "diagnose", &digest)?;
    let _ = std::fs::remove_dir_all(&directory);
    cases.push((
        "a revoked runbook stops a paused run at its next step",
        after.steps["diagnose"].state == StepState::Blocked,
    ));
    let failed = cases.iter().filter(|(_, passed)| !passed).count();
    let detail = cases
        .iter()
        .map(|(name, passed)| format!("  {:<5} {name}", if *passed { "PASS" } else { "FAIL" }))
        .collect::<Vec<_>>();
    let response = Response::new("revocation.test", if failed == 0 { Status::Passed } else { Status::Failed }, format!("{} revocation cases on fixture systems, {failed} failed", cases.len()))
        .data(json!({"cases": cases.iter().map(|(name, passed)| json!({"case": name, "status": if *passed { "PASS" } else { "FAIL" }})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
