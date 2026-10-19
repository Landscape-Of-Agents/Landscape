// Tests of the invocation engine: lifecycle transitions, authorization before retrieval and
// dispatch, digest, revocation, and staleness, lab reports by exact digest, stale discovery,
// missing inputs, unknown applicability, unsupported branches and steps, adapter validation and
// effect classes, precondition and evidence failures, approval binding and invalidation,
// idempotency, timeouts, reconciliation, transactions and compensation, verification evidence,
// secret redaction, scope, ledger tampering, concurrency, interrupted persistence, events, and the
// end-to-end demonstration.

use std::rc::Rc;

use serde_json::json;

use super::adapter::{
    validate_params, AdapterRegistry, FakeRuntime, Fault, FixtureAdapter, InvocationAdapter,
};
use super::demo::{
    self, deploy_fixture, diagnostics_request, diagnostics_world, refund_request, refund_world,
    status_fixture, World,
};
use super::engine::{DataSeaSink, InvocationRecord, VerificationStatus};
use super::ledger::{Insertion, Ledger};
use super::lifecycle::{allowed, EffectStatus, InvocationState, OperationStatus, ALL_STATES};
use super::plan::{ParamSource, VerificationBinding, VerificationMethod};
use super::preflight::PreflightStatus;
use crate::capability::demo as capability_demo;
use crate::capability::lifecycle::LifecycleState;
use crate::capability::refs::Sources;
use crate::capability::registry::InvocationStatus;
use crate::capability::ErrorCode;
use crate::capability_lab::report::run_lab;
use crate::capability_lab::scenarios::TestPlan;
use crate::capture::{stable_id, AccessPolicy, Actor, ActorKind};
use crate::compiler::Basis;
use crate::trust::crypto::digest_json;

/** The engineer's access policy in a world
 * Input
    - world: &World - world
 * Output
    - AccessPolicy
*/
fn engineer(world: &World) -> AccessPolicy {
    capability_demo::reader(&world.cast.engineer).unwrap()
}

/** Submit a request as the engineer
 * Input
    - world: &mut World - world
    - request: super::engine::InvocationRequest - request
 * Output
    - crate::capability::CapabilityResult<InvocationRecord>
*/
fn submit(
    world: &mut World,
    request: super::engine::InvocationRequest,
) -> crate::capability::CapabilityResult<InvocationRecord> {
    let reader = engineer(world);
    let actor = world.cast.engineer.clone();
    world.engine(&reader).submit(&actor, request)
}

/** Submit the diagnostics request with a key as the engineer
 * Input
    - world: &mut World - world
    - key: &str - idempotency key
 * Output
    - crate::capability::CapabilityResult<InvocationRecord>
*/
fn submit_diagnostics(
    world: &mut World,
    key: &str,
) -> crate::capability::CapabilityResult<InvocationRecord> {
    let request = diagnostics_request(world, key);
    submit(world, request)
}

/** The status of a preflight check
 * Input
    - record: &InvocationRecord - record
    - id: &str - check id
 * Output
    - Option<PreflightStatus>
*/
fn preflight(record: &InvocationRecord, id: &str) -> Option<PreflightStatus> {
    record
        .preflight
        .iter()
        .find(|check| check.check_id == id)
        .map(|check| check.status)
}

/** Dispatches made to the deploy fixture's read operations
 * Input
    - world: &World - world
 * Output
    - usize
*/
fn reads(world: &World) -> usize {
    [
        "deployment.get",
        "checks.list",
        "logs.query",
        "bundle.assemble",
    ]
    .iter()
    .map(|operation| world.deploy.calls(operation))
    .sum()
}

/** Submit the refund change, approve it as the lead, and return the invocation id
 * Input
    - world: &mut World - refund world
    - key: &str - idempotency key
 * Output
    - String
*/
fn approved_refund(world: &mut World, key: &str) -> String {
    let request = refund_request(world, key).unwrap();
    let pending = submit(world, request).unwrap();
    assert_eq!(
        pending.state,
        InvocationState::AwaitingApproval,
        "{:?}",
        pending.failures
    );
    let digest = pending.plan.as_ref().unwrap().plan_digest.clone();
    let reader = engineer(world);
    let lead = world.cast.lead.clone();
    world
        .engine(&reader)
        .approve(&lead, &pending.invocation_id, &digest)
        .unwrap();
    pending.invocation_id
}

/** Run an invocation as the engineer
 * Input
    - world: &mut World - world
    - id: &str - invocation
 * Output
    - InvocationRecord
*/
fn run(world: &mut World, id: &str) -> InvocationRecord {
    let reader = engineer(world);
    let actor = world.cast.engineer.clone();
    world.engine(&reader).run(&actor, id).unwrap()
}

/** Reconcile as the engineer
 * Input
    - world: &mut World - world
    - id: &str - invocation
 * Output
    - InvocationRecord
*/
fn reconcile(world: &mut World, id: &str) -> InvocationRecord {
    let reader = engineer(world);
    let actor = world.cast.engineer.clone();
    world.engine(&reader).reconcile(&actor, id).unwrap()
}

/** Reseal an altered compilation report the way the compiler seals one
 * Input
    - report: &mut crate::compiler::assemble::CompilationReport - report
 * Output
    - None
*/
fn reseal(report: &mut crate::compiler::assemble::CompilationReport) {
    report.digest = String::new();
    report.report_id = String::new();
    let digest = digest_json("compiler/report/v1", &json!(report))[..32].to_string();
    report.report_id = stable_id("cmp", &[&digest]);
    report.digest = digest;
}

/** Every permitted transition is accepted and every other one refused without changing anything
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn validates_every_transition() {
    use InvocationState::*;
    let permitted = [
        (Requested, Authorizing),
        (Requested, Blocked),
        (Authorizing, Preflighting),
        (Authorizing, Blocked),
        (Preflighting, PlanReady),
        (Preflighting, Blocked),
        (PlanReady, AwaitingApproval),
        (PlanReady, Executing),
        (PlanReady, Blocked),
        (PlanReady, Abandoned),
        (AwaitingApproval, Executing),
        (AwaitingApproval, Blocked),
        (AwaitingApproval, Abandoned),
        (Executing, Verifying),
        (Executing, Failed),
        (Executing, ReconciliationRequired),
        (Executing, Blocked),
        (Executing, Abandoned),
        (Verifying, Succeeded),
        (Verifying, Failed),
        (Verifying, PartiallyVerified),
        (Verifying, Unknown),
        (ReconciliationRequired, Executing),
        (ReconciliationRequired, Verifying),
        (ReconciliationRequired, Failed),
        (ReconciliationRequired, Unknown),
        (ReconciliationRequired, Abandoned),
        (Unknown, ReconciliationRequired),
        (Unknown, Abandoned),
    ];
    for from in ALL_STATES {
        for to in ALL_STATES {
            assert_eq!(
                allowed(from, to),
                permitted.contains(&(from, to)),
                "{from:?} -> {to:?}"
            );
        }
    }
    let mut world = diagnostics_world().unwrap();
    let record = submit_diagnostics(&mut world, "transitions").unwrap();
    let mut copy = record.clone();
    let refused = copy.transition(InvocationState::Executing, &world.cast.engineer, "again", 1);
    assert_eq!(refused.unwrap_err().code, ErrorCode::InvalidTransition);
    assert_eq!(copy, record, "a refused transition changes nothing");
    let sequence = record
        .transitions
        .iter()
        .map(|transition| transition.to)
        .collect::<Vec<_>>();
    assert_eq!(
        sequence,
        vec![
            Requested,
            Authorizing,
            Preflighting,
            PlanReady,
            Executing,
            Verifying,
            Succeeded
        ]
    );
}

/** Authorization precedes retrieval and dispatch; refusals are audited
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn authorizes_before_retrieval_and_dispatch() {
    let mut world = diagnostics_world().unwrap();
    let stranger = Actor::new("stranger@acme.example", ActorKind::Human);
    let reader = AccessPolicy {
        actor: stranger.clone(),
        ..engineer(&world)
    };
    let request = diagnostics_request(&world, "stranger");
    let error = world
        .engine(&reader)
        .submit(&stranger, request)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);
    let borrowed = engineer(&world);
    let lead = world.cast.lead.clone();
    let request = diagnostics_request(&world, "borrowed");
    assert_eq!(
        world
            .engine(&borrowed)
            .submit(&lead, request)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized,
        "an access policy must be the requester's own"
    );
    assert!(world.ledger.state.records.is_empty());
    assert_eq!(world.ledger.state.denials.len(), 2);
    assert_eq!(reads(&world), 0);
    let agent = world.cast.agent.clone();
    let agent_reader = AccessPolicy {
        actor: agent.clone(),
        ..engineer(&world)
    };
    let request = diagnostics_request(&world, "agent-read-only");
    let record = world.engine(&agent_reader).submit(&agent, request).unwrap();
    assert_eq!(
        record.state,
        InvocationState::Succeeded,
        "an agent may run a read-only plan"
    );
    let mut refund = refund_world().unwrap();
    let agent = refund.cast.agent.clone();
    let agent_reader = AccessPolicy {
        actor: agent.clone(),
        ..engineer(&refund)
    };
    let request = refund_request(&refund, "agent-mutating").unwrap();
    let blocked = refund
        .engine(&agent_reader)
        .submit(&agent, request)
        .unwrap();
    assert_eq!(
        (blocked.state, preflight(&blocked, "policy")),
        (InvocationState::Blocked, Some(PreflightStatus::Failed))
    );
}

/** The exact digest, revocation, and staleness are checked now, before any dispatch
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn checks_digest_revocation_and_staleness() {
    let mut world = diagnostics_world().unwrap();
    let mut changed = diagnostics_request(&world, "changed");
    changed.digest = "f".repeat(32);
    let record = submit(&mut world, changed).unwrap();
    assert_eq!(
        (record.state, preflight(&record, "identity")),
        (InvocationState::Blocked, Some(PreflightStatus::Failed))
    );
    world
        .current
        .flows
        .insert("refund-flow".into(), "v9".into());
    let record = submit_diagnostics(&mut world, "live-stale").unwrap();
    assert_eq!(
        (record.state, preflight(&record, "staleness")),
        (InvocationState::Blocked, Some(PreflightStatus::Failed))
    );
    let reader = engineer(&world);
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    world
        .registry
        .mark_stale(&world.cast.engineer, &sources, 1)
        .unwrap();
    world
        .current
        .flows
        .insert("refund-flow".into(), "v2".into());
    let record = submit_diagnostics(&mut world, "marked-stale").unwrap();
    assert_eq!(
        (record.state, preflight(&record, "eligibility")),
        (InvocationState::Blocked, Some(PreflightStatus::Failed)),
        "marked staleness stays until revalidation"
    );
    let mut fresh = diagnostics_world().unwrap();
    fresh
        .registry
        .withdraw(
            &fresh.cast.lead,
            "deployment-failure-diagnostics",
            None,
            LifecycleState::Revoked,
            "unsafe",
            1,
        )
        .unwrap();
    let record = submit_diagnostics(&mut fresh, "revoked").unwrap();
    assert_eq!(record.state, InvocationState::Blocked);
    assert_eq!(reads(&world) + reads(&fresh), 0);
}

/** Lab reports count only for the exact digest; a failed one blocks; others are not "passed"
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn applies_lab_reports_by_exact_digest() {
    let mut world = diagnostics_world().unwrap();
    let target = crate::capability_lab::preflight::Target {
        capability_id: "deployment-failure-diagnostics".into(),
        version: 1,
        expected_digest: world.digest("deployment-failure-diagnostics"),
    };
    let plan = |declares: &str| {
        TestPlan::parse(&json!({"plan_id": "p", "version": 1, "description": "d", "scenarios": [{"scenario_id": "s", "description": "d", "check": {"kind": "negative_case", "declares": declares}}]}).to_string()).unwrap()
    };
    let reader = engineer(&world);
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let failing = run_lab(
        &world.registry,
        &sources,
        &world.cast.engineer,
        &target,
        None,
        &plan("something never declared"),
        1,
    )
    .unwrap();
    let not_run = run_lab(
        &world.registry,
        &sources,
        &world.cast.engineer,
        &target,
        None,
        &plan("never started"),
        1,
    )
    .unwrap();
    let mut elsewhere = failing.clone();
    elsewhere.candidate_digest = "0".repeat(32);
    world.lab_reports = vec![elsewhere, not_run];
    let record = submit_diagnostics(&mut world, "lab-not-run").unwrap();
    assert_eq!(
        preflight(&record, "lab"),
        Some(PreflightStatus::Unresolved),
        "a report that did not pass is never counted as passed"
    );
    assert_eq!(
        record.state,
        InvocationState::Succeeded,
        "for a low-risk capability an unresolved lab report is recorded, not blocking"
    );
    world.lab_reports.push(failing);
    let record = submit_diagnostics(&mut world, "lab-failed").unwrap();
    assert_eq!(
        (record.state, preflight(&record, "lab")),
        (InvocationState::Blocked, Some(PreflightStatus::Failed))
    );
    let mut refund = refund_world().unwrap();
    let request = refund_request(&refund, "refund-lab").unwrap();
    let pending = submit(&mut refund, request).unwrap();
    assert!(pending
        .approval
        .reasons
        .iter()
        .any(|reason| reason.contains("no passing lab report")));
}

/** A discovery that became stale is re-evaluated, never trusted
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn re_evaluates_stale_discovery() {
    let mut world = diagnostics_world().unwrap();
    let pack = world.capture.store.packs.values().next().unwrap().clone();
    let reader = engineer(&world);
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let request = crate::discovery::resolve::DiscoveryRequest {
        task_class: "deployment-failure-triage".into(),
        environment: None,
        repository: None,
        confirmed_preconditions: vec!["deployment-known".into(), "read-access".into()],
        entities: Vec::new(),
        context: Some(crate::discovery::resolve::ContextRef {
            pack_id: pack.pack_id.clone(),
            pack_digest: pack.manifest.digest.clone(),
        }),
    };
    let discovery = crate::discovery::report::discover(
        &world.registry,
        &sources,
        &world.cast.engineer,
        &request,
        &[],
        1,
    );
    assert_eq!(
        discovery.recommendations[0].capability_id,
        "deployment-failure-diagnostics"
    );
    world.discovery_reports.push(discovery.clone());
    world.current.zones.insert("refunds".into(), "v4".into());
    let mut invocation = diagnostics_request(&world, "after-discovery");
    invocation.discovery_report = Some(discovery.report_id.clone());
    let record = submit(&mut world, invocation).unwrap();
    assert_eq!(
        preflight(&record, "discovery"),
        Some(PreflightStatus::Passed),
        "the recommendation is recorded as advisory"
    );
    assert_eq!(
        record.state,
        InvocationState::Blocked,
        "but the current state decides"
    );
}

/** Missing inputs, unknown applicability, and unconfirmed preconditions block
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn blocks_missing_inputs_and_unknown_applicability() {
    let mut world = diagnostics_world().unwrap();
    let mut missing = diagnostics_request(&world, "missing-input");
    missing.inputs.clear();
    let record = submit(&mut world, missing).unwrap();
    assert_eq!(record.state, InvocationState::Blocked);
    assert!(record
        .failures
        .iter()
        .any(|failure| failure.contains("input 'deployment_id' is missing")));
    let mut unknown = diagnostics_request(&world, "unknown-environment");
    unknown.environment = None;
    let record = submit(&mut world, unknown).unwrap();
    assert_eq!(
        (record.state, preflight(&record, "applicability")),
        (InvocationState::Blocked, Some(PreflightStatus::Unresolved))
    );
    let mut unconfirmed = diagnostics_request(&world, "unconfirmed");
    unconfirmed.confirmed_preconditions.pop();
    let record = submit(&mut world, unconfirmed).unwrap();
    assert_eq!(record.state, InvocationState::Blocked);
    assert_eq!(reads(&world), 0);
}

/** Unresolved procedure branches, proposed steps, and assumed effects block the plan
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn blocks_unsupported_branches_and_steps() {
    let mut world = refund_world().unwrap();
    let original = world.compilation_reports[0].clone();
    let mut branched = original.clone();
    branched
        .branches
        .push(crate::compiler::extract::BranchNote {
            condition: "if the deployment fails, roll it back".into(),
            basis: Basis::Inferred,
            source: "observed recovery evidence".into(),
        });
    reseal(&mut branched);
    world.compilation_reports = vec![branched];
    let request = refund_request(&world, "branched").unwrap();
    let record = submit(&mut world, request).unwrap();
    assert_eq!(record.state, InvocationState::Blocked);
    assert!(record
        .failures
        .iter()
        .any(|failure| failure.contains("unresolved procedure branch")));
    let mut proposed = original.clone();
    proposed.step_bases[0].basis = Basis::Hypothesis;
    reseal(&mut proposed);
    world.compilation_reports = vec![proposed];
    let request = refund_request(&world, "proposed").unwrap();
    assert!(submit(&mut world, request)
        .unwrap()
        .failures
        .iter()
        .any(|failure| failure.contains("unverified proposal")));
    world.compilation_reports = Vec::new();
    let request = refund_request(&world, "no-report").unwrap();
    assert!(submit(&mut world, request)
        .unwrap()
        .failures
        .iter()
        .any(|failure| failure.contains("no intact compilation report")));
    let mut tampered = original;
    tampered.warnings.clear();
    tampered.reasons.push("edited".into());
    world.compilation_reports = vec![tampered];
    let request = refund_request(&world, "tampered-report").unwrap();
    assert_eq!(
        submit(&mut world, request).unwrap().state,
        InvocationState::Blocked
    );
    assert_eq!(world.deploy.applied(), 0);
}

/** Operation schemas are typed and bounded; effect classes must match the steps; policy
 * references and unregistered operations are refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn validates_adapters_and_effect_classes() {
    let fixture = deploy_fixture("failed");
    let spec = fixture
        .operations()
        .into_iter()
        .find(|spec| spec.operation_id == "deployment.get")
        .unwrap();
    let params = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    };
    assert!(validate_params(&spec, &params(&[("deployment", "dep-7731")])).is_ok());
    assert!(validate_params(&spec, &params(&[("deployment", "dep-7731; rm -rf /")])).is_err());
    assert!(validate_params(
        &spec,
        &params(&[("deployment", "dep-7731"), ("command", "ls")])
    )
    .is_err());
    assert!(validate_params(&spec, &params(&[])).is_err());
    let mut world = diagnostics_world().unwrap();
    let mut wrong = diagnostics_request(&world, "wrong-effect");
    wrong.bindings.steps[0].operation_id = "repo.commit".into();
    wrong.bindings.steps[0].params = [(
        "change_ref".to_string(),
        ParamSource::Literal {
            value: "chg-1".into(),
        },
    )]
    .into_iter()
    .collect();
    let record = submit(&mut world, wrong).unwrap();
    assert!(record
        .failures
        .iter()
        .any(|failure| failure.contains("does not implement")
            || failure.contains("disagree about effects")));
    let mut policy = diagnostics_request(&world, "policy-param");
    policy
        .inputs
        .insert("deployment_id".into(), ".crane/policies/x".into());
    let record = submit(&mut world, policy).unwrap();
    assert_eq!(record.state, InvocationState::Blocked);
    let mut independent = diagnostics_request(&world, "self-verifying");
    independent
        .bindings
        .verifications
        .push(VerificationBinding {
            claim_id: "logs-retrieved".into(),
            method: VerificationMethod::AdapterCheck {
                adapter_id: "deploy-fixture".into(),
                operation_id: "logs.query".into(),
                params: [(
                    "deployment".to_string(),
                    ParamSource::Input {
                        name: "deployment_id".into(),
                    },
                )]
                .into_iter()
                .collect(),
                field: "lines".into(),
                equals: "1".into(),
            },
        });
    let record = submit(&mut world, independent).unwrap();
    assert!(record
        .failures
        .iter()
        .any(|failure| failure.contains("would not be independent")));
    assert_eq!(reads(&world), 0);
}

/** A required claim that no longer verifies, an unhealthy adapter, and a failing later operation
 * stop execution without hiding earlier effects
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn enforces_preconditions_and_invariants() {
    let mut world = diagnostics_world().unwrap();
    let mut narrow = engineer(&world);
    narrow.resources.remove("deploy");
    let actor = world.cast.engineer.clone();
    let request = diagnostics_request(&world, "evidence-gone");
    let record = world.engine(&narrow).submit(&actor, request).unwrap();
    assert_eq!(
        (record.state, preflight(&record, "evidence")),
        (InvocationState::Blocked, Some(PreflightStatus::Failed)),
        "required evidence the requester cannot read stays missing"
    );
    world.deploy.set_healthy(false);
    let record = submit_diagnostics(&mut world, "unhealthy").unwrap();
    assert_eq!(
        (record.state, preflight(&record, "adapters")),
        (InvocationState::Blocked, Some(PreflightStatus::Failed))
    );
    let mut refund = refund_world().unwrap();
    let id = approved_refund(&mut refund, "commit-fails");
    refund.deploy.inject("repo.commit", Fault::Fail);
    let record = run(&mut refund, &id);
    assert_eq!(record.state, InvocationState::Failed);
    assert!(
        record
            .unresolved_effects
            .iter()
            .any(|effect| effect.contains("repo.apply_change") && effect.contains("Applied")),
        "{:?}",
        record.unresolved_effects
    );
    assert_eq!(refund.deploy.calls("deploy.release"), 0);
}

/** Approvals bind to the exact plan digest and scope; a changed plan invalidates them
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn binds_and_invalidates_approvals() {
    let mut world = refund_world().unwrap();
    let request = refund_request(&world, "approval").unwrap();
    let pending = submit(&mut world, request).unwrap();
    let digest = pending.plan.as_ref().unwrap().plan_digest.clone();
    let reader = engineer(&world);
    let (engineer_actor, lead, agent) = (
        world.cast.engineer.clone(),
        world.cast.lead.clone(),
        world.cast.agent.clone(),
    );
    assert_eq!(
        world
            .engine(&reader)
            .approve(&engineer_actor, &pending.invocation_id, &digest)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized,
        "the requester cannot approve"
    );
    assert_eq!(
        world
            .engine(&reader)
            .approve(&agent, &pending.invocation_id, &digest)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        world
            .engine(&reader)
            .approve(&lead, &pending.invocation_id, &"0".repeat(32))
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
    assert_eq!(
        world
            .engine(&reader)
            .run(&engineer_actor, &pending.invocation_id)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized,
        "no run without approval"
    );
    assert_eq!(
        world.ledger.get(&pending.invocation_id).unwrap().state,
        InvocationState::AwaitingApproval
    );
    world
        .engine(&reader)
        .approve(&lead, &pending.invocation_id, &digest)
        .unwrap();
    let mut changed = deploy_fixture("succeeded");
    changed = FixtureAdapter::new(
        "deploy-fixture",
        changed
            .operations()
            .into_iter()
            .map(|mut spec| {
                spec.max_output_bytes = 4096;
                spec
            })
            .collect(),
        Default::default(),
    );
    world.deploy = Rc::new(changed);
    world.adapters = AdapterRegistry::default()
        .with(world.deploy.clone())
        .with(world.status.clone());
    let record = run(&mut world, &pending.invocation_id);
    assert_eq!(record.state, InvocationState::Blocked);
    assert!(record
        .failures
        .iter()
        .any(|failure| failure.contains("approval is invalidated")));
    assert_eq!(world.deploy.applied(), 0);
}

/** The same key and request return the same invocation; a different request conflicts
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn enforces_idempotency() {
    let mut world = diagnostics_world().unwrap();
    let first = submit_diagnostics(&mut world, "same").unwrap();
    let dispatched = reads(&world);
    let second = submit_diagnostics(&mut world, "same").unwrap();
    assert_eq!(
        (second.invocation_id.clone(), reads(&world)),
        (first.invocation_id.clone(), dispatched)
    );
    assert_eq!(second, world.ledger.get(&first.invocation_id).unwrap());
    let mut different = diagnostics_request(&world, "same");
    different.environment = Some("staging".into());
    assert_eq!(
        submit(&mut world, different).unwrap_err().code,
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(world.ledger.state.records.len(), 1);
}

/** Timeouts before dispatch are retried; timeouts after dispatch are reconciled, never retried
 * blindly; an effect proven absent is dispatched again
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn handles_timeouts_by_what_is_known() {
    let mut world = diagnostics_world().unwrap();
    world
        .deploy
        .inject("deployment.get", Fault::TimeoutBeforeDispatch);
    let record = submit_diagnostics(&mut world, "read-timeout").unwrap();
    assert_eq!(
        (record.state, record.operations[0].attempts),
        (InvocationState::Succeeded, 2)
    );
    let mut refund = refund_world().unwrap();
    let id = approved_refund(&mut refund, "before-dispatch");
    refund
        .deploy
        .inject("deploy.release", Fault::TimeoutBeforeDispatch);
    let record = run(&mut refund, &id);
    assert_eq!(
        (record.state, refund.deploy.calls("deploy.release")),
        (InvocationState::Succeeded, 2),
        "no effect was possible, so one retry is safe"
    );
    let mut refund = refund_world().unwrap();
    let id = approved_refund(&mut refund, "after-dispatch");
    refund
        .deploy
        .inject("deploy.release", Fault::TimeoutAfterDispatch);
    let interrupted = run(&mut refund, &id);
    assert_eq!(interrupted.state, InvocationState::ReconciliationRequired);
    let release = interrupted
        .operations
        .iter()
        .find(|operation| operation.operation_id == "deploy.release")
        .unwrap();
    assert_eq!(
        (release.status, release.effect_status),
        (OperationStatus::TimedOut, EffectStatus::Unknown)
    );
    assert_eq!(
        refund.deploy.calls("deploy.release"),
        1,
        "not retried blindly"
    );
    let resumed = reconcile(&mut refund, &id);
    assert_eq!(resumed.state, InvocationState::Succeeded);
    assert_eq!(
        refund.deploy.calls("deploy.release"),
        2,
        "dispatched again only after reconciliation proved no effect"
    );
    let mut refund = refund_world().unwrap();
    let id = approved_refund(&mut refund, "after-effect");
    refund
        .deploy
        .inject("deploy.release", Fault::TimeoutAfterEffect);
    run(&mut refund, &id);
    let resumed = reconcile(&mut refund, &id);
    assert_eq!(
        (resumed.state, refund.deploy.calls("deploy.release")),
        (InvocationState::Succeeded, 1)
    );
    let release = resumed
        .operations
        .iter()
        .find(|operation| operation.operation_id == "deploy.release")
        .unwrap();
    assert_eq!(
        (release.effect_status, release.reconciliations.len()),
        (EffectStatus::Applied, 1)
    );
}

/** Without reconciliation the invocation stays unknown until an authorized decision; compensation
 * is recorded separately and never treated as proof of no effect
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn keeps_unresolvable_effects_unknown() {
    let mut world = refund_world().unwrap();
    let id = approved_refund(&mut world, "no-reconciliation");
    world.deploy.inject("deploy.release", Fault::FailUnknown);
    world.deploy.set_reconcile(false);
    assert_eq!(
        run(&mut world, &id).state,
        InvocationState::ReconciliationRequired
    );
    let unknown = reconcile(&mut world, &id);
    assert_eq!(unknown.state, InvocationState::Unknown);
    assert!(unknown
        .recovery_status
        .contains("explicit, authorized decision"));
    let again = reconcile(&mut world, &id);
    assert_eq!(again.state, InvocationState::Unknown);
    let reader = engineer(&world);
    let (requester, lead) = (world.cast.engineer.clone(), world.cast.lead.clone());
    assert_eq!(
        world
            .engine(&reader)
            .abandon(&requester, &id, "give up")
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let abandoned = world
        .engine(&reader)
        .abandon(&lead, &id, "handed to on-call for manual recovery")
        .unwrap();
    assert_eq!(abandoned.state, InvocationState::Abandoned);
    assert!(abandoned
        .unresolved_effects
        .iter()
        .any(|effect| effect.contains("deploy.release") && effect.contains("Unknown")));
    assert_eq!(
        world
            .engine(&reader)
            .abandon(&lead, &id, "again")
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
    let mut transactional = refund_world().unwrap();
    let mut request = refund_request(&transactional, "transaction").unwrap();
    let commit = request
        .bindings
        .steps
        .iter_mut()
        .find(|binding| binding.operation_id == "repo.commit")
        .unwrap();
    commit.operation_id = "ledger.adjust".into();
    commit.params = [(
        "account".to_string(),
        ParamSource::Literal {
            value: "refunds".into(),
        },
    )]
    .into_iter()
    .collect();
    let blocked = submit(&mut transactional, request.clone()).unwrap();
    assert_eq!(
        (blocked.state, preflight(&blocked, "adapters")),
        (InvocationState::Blocked, Some(PreflightStatus::Failed)),
        "no transaction runtime: fail closed"
    );
    transactional.runtime = Box::new(FakeRuntime {
        fail_and_compensate: true,
    });
    request.idempotency_key = "transaction-fake".into();
    let pending = submit(&mut transactional, request).unwrap();
    let digest = pending.plan.as_ref().unwrap().plan_digest.clone();
    let reader = engineer(&transactional);
    let lead = transactional.cast.lead.clone();
    transactional
        .engine(&reader)
        .approve(&lead, &pending.invocation_id, &digest)
        .unwrap();
    let record = run(&mut transactional, &pending.invocation_id);
    assert_eq!(record.state, InvocationState::ReconciliationRequired);
    let adjust = record
        .operations
        .iter()
        .find(|operation| operation.operation_id == "ledger.adjust")
        .unwrap();
    assert_eq!(
        adjust.effect_status,
        EffectStatus::Unknown,
        "compensation does not prove the effect did not happen"
    );
    assert!(adjust
        .compensation
        .as_ref()
        .is_some_and(|compensation| compensation.attempted));
}

/** Missing, contradictory, and failed verification never produce success
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn never_converts_missing_evidence_to_success() {
    let mut world = diagnostics_world().unwrap();
    let mut unbound = diagnostics_request(&world, "unbound");
    unbound
        .bindings
        .verifications
        .retain(|binding| binding.claim_id != "logs-retrieved");
    let record = submit(&mut world, unbound).unwrap();
    assert_eq!(record.state, InvocationState::PartiallyVerified);
    assert!(record
        .verifications
        .iter()
        .any(|result| result.claim_id == "logs-retrieved"
            && result.result == VerificationStatus::Unresolved));
    world.set_status(status_fixture("succeeded", true));
    let record = submit_diagnostics(&mut world, "contradicted").unwrap();
    assert_eq!(
        record.state,
        InvocationState::PartiallyVerified,
        "the monitoring system and Work Capture disagree; no side is chosen"
    );
    let statuses = record
        .verifications
        .iter()
        .filter(|result| result.claim_id == "outcome-recorded")
        .map(|result| result.result)
        .collect::<Vec<_>>();
    assert!(
        statuses.contains(&VerificationStatus::Verified)
            && statuses.contains(&VerificationStatus::Failed)
    );
    world.set_status(status_fixture("failed", false));
    let record = submit_diagnostics(&mut world, "failed-check").unwrap();
    assert_eq!(record.state, InvocationState::Failed);
    let mut missing_field = diagnostics_request(&world, "missing-field");
    if let VerificationMethod::AdapterCheck { field, .. } =
        &mut missing_field.bindings.verifications[2].method
    {
        *field = "nonexistent".into();
    }
    world.set_status(status_fixture("failed", true));
    let record = submit(&mut world, missing_field).unwrap();
    assert_ne!(record.state, InvocationState::Succeeded);
    let handed = &world
        .registry
        .state
        .record("deployment-failure-diagnostics", 1)
        .unwrap()
        .invocations;
    assert!(handed
        .iter()
        .all(|invocation| invocation.status != InvocationStatus::Succeeded));
}

/** Secrets never enter records; scope and access are enforced; tampered history is refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn redacts_secrets_and_refuses_tampering() {
    let mut world = diagnostics_world().unwrap();
    let mut secret = diagnostics_request(&world, "secret");
    secret.inputs.insert(
        "token".into(),
        "ghp_0123456789abcdefghijklmnopqrstuvwxyz".into(),
    );
    assert_eq!(
        submit(&mut world, secret).unwrap_err().code,
        ErrorCode::InvalidSchema
    );
    let leaky = FixtureAdapter::new(
        "deploy-fixture",
        super::demo::deploy_operations(),
        [(
            "deployment.get".to_string(),
            json!({"status": "failed", "note": "password=Sup3rS3cretValue"}),
        )]
        .into_iter()
        .chain(
            ["checks.list", "logs.query", "bundle.assemble"]
                .map(|name| (name.to_string(), json!({"ok": true}))),
        )
        .collect(),
    );
    world.deploy = Rc::new(leaky);
    world.adapters = AdapterRegistry::default()
        .with(world.deploy.clone())
        .with(world.status.clone());
    let record = submit_diagnostics(&mut world, "leaky").unwrap();
    let text = serde_json::to_string(&record).unwrap();
    assert!(!text.contains("Sup3rS3cretValue") && text.contains("REDACTED"));
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = Ledger::open(directory.path()).unwrap();
    assert!(matches!(
        ledger.insert(record.clone()).unwrap(),
        Insertion::Inserted(_)
    ));
    let path = directory.path().join("ledger.json");
    let tampered = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"succeeded\"", "\"failed\"");
    std::fs::write(&path, tampered).unwrap();
    assert_eq!(
        Ledger::open(directory.path()).unwrap_err().code,
        ErrorCode::IntegrityMismatch,
        "tampered history is refused, not repaired"
    );
}

/** Concurrent requests with one key make one invocation; interrupted writes leave the ledger
 * intact; truncated ledgers fail loudly
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn survives_concurrency_and_interruption() {
    let mut world = diagnostics_world().unwrap();
    let record = submit_diagnostics(&mut world, "shared").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_path_buf();
    let workers = (0..4)
        .map(|_| {
            let path = path.clone();
            let record = record.clone();
            std::thread::spawn(move || {
                matches!(
                    Ledger::open(&path).unwrap().insert(record).unwrap(),
                    Insertion::Inserted(_)
                )
            })
        })
        .collect::<Vec<_>>();
    let inserted = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .filter(|inserted| *inserted)
        .count();
    assert_eq!(inserted, 1);
    std::fs::write(path.join(".ledger.json.crane-tmp-4242"), "{ partial").unwrap();
    assert_eq!(
        Ledger::open(&path).unwrap().state.records.len(),
        1,
        "an interrupted temporary write is ignored"
    );
    std::fs::write(path.join("ledger.json"), "{\"schema_version\": 1, \"gen").unwrap();
    assert_eq!(
        Ledger::open(&path).unwrap_err().code,
        ErrorCode::Persistence
    );
}

/** Lifecycle events reach DataSea's event store; the definition never changes; the outcome is
 * handed to Component 2's invocation history
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn emits_events_and_hands_off_outcomes() {
    let mut world = diagnostics_world().unwrap();
    let before = world
        .registry
        .state
        .record("deployment-failure-diagnostics", 1)
        .unwrap()
        .clone();
    let record = submit_diagnostics(&mut world, "events").unwrap();
    assert_eq!(world.events.events.len(), record.transitions.len());
    let after = world
        .registry
        .state
        .record("deployment-failure-diagnostics", 1)
        .unwrap();
    assert_eq!(
        (&after.definition, after.state),
        (&before.definition, before.state)
    );
    assert_eq!(
        after.invocations.last().unwrap().invocation_id,
        record.invocation_id
    );
    assert_eq!(
        after.invocations.last().unwrap().status,
        InvocationStatus::Succeeded
    );
    let directory = tempfile::tempdir().unwrap();
    let mut sink = DataSeaSink {
        store: crate::telemetry::events::EventStore::open(directory.path()),
        repository_id: "repo".into(),
    };
    for transition in &record.transitions {
        super::engine::EventSink::emit(&mut sink, &record, transition);
    }
    let stored = sink.store.read_all().unwrap();
    assert_eq!(stored.len(), record.transitions.len());
    assert_eq!(
        stored.last().unwrap().event_type,
        "capability.invocation.succeeded"
    );
}

/** The demonstration covers every required outcome and is deterministic
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn runs_the_demonstration() {
    let report = demo::run().unwrap();
    assert_eq!(report.succeeded.state, InvocationState::Succeeded);
    assert!(report.ledger_verified && report.repeat_returns_same);
    assert_eq!(report.dispatches_after_repeat, 1);
    assert_eq!(
        (report.changed_digest, report.stale, report.revoked),
        (
            InvocationState::Blocked,
            InvocationState::Blocked,
            InvocationState::Blocked
        )
    );
    assert_eq!(
        (report.unsupported.0, report.unsupported.2),
        (InvocationState::Blocked, 0)
    );
    assert_eq!(
        (report.reconciled.state, report.release_dispatches),
        (InvocationState::Succeeded, 1)
    );
    assert_eq!(report.failed_verification, InvocationState::Failed);
    assert_eq!(
        serde_json::to_value(demo::run().unwrap()).unwrap(),
        serde_json::to_value(&report).unwrap()
    );
}
