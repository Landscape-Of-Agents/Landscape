// The deterministic invocation demonstration ('crane capability invoke demo'). Fixture external
// systems only; no credentials. Published capabilities are produced through the earlier
// components' own interfaces:
//
//   deployment-failure-diagnostics  read-only, low risk (Component 2's demonstration)
//   refund-change-with-verified-deploy  state-changing, medium risk (Components 1-3)
//
// Two fixture systems stand in for external ones: "deploy-fixture" (where operations run) and
// "status-fixture" (an independent monitoring system used only for verification). The
// demonstration shows a verified read-only success, idempotent repetition, blocking on a changed
// digest, a stale dependency, revocation, and an unsupported operation, a timeout after an external
// effect resolved by reconciliation instead of a blind retry, and a failed verification.

use std::collections::BTreeMap;
use std::rc::Rc;

use serde::Serialize;
use serde_json::{json, Value};

use super::adapter::{
    spec, AdapterRegistry, Fault, FixtureAdapter, ParamKind, TransactionRuntime, UnavailableRuntime,
};
use super::engine::{Engine, InvocationRecord, InvocationRequest, MemorySink};
use super::ledger::Ledger;
use super::lifecycle::{EffectClass, InvocationState};
use super::plan::{
    BindingSet, ParamSource, PlanLimits, StepBinding, VerificationBinding, VerificationMethod,
};
use crate::capability::demo::{self as capability_demo, Cast, DIAGNOSTICS};
use crate::capability::lifecycle::{LifecycleState, PublicationRequest};
use crate::capability::model::StepKind;
use crate::capability::refs::{CurrentVersions, Sources};
use crate::capability::registry::CapabilityRegistry;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capability_lab::report::LabReport;
use crate::capture::broker::run::EvidenceQueryBroker;
use crate::capture::format::record::EvidenceType;
use crate::capture::{AccessPolicy, Clock};
use crate::compiler::assemble::CompilationReport;
use crate::compiler::demo::success_scenario;
use crate::discovery::report::DiscoveryReport;

/** Fixed start of the demonstration clock */
pub(crate) const EPOCH: u64 = 1_760_000_600_000;

/** The deploy fixture's operations
 * Input
    - None
 * Output
    - Vec<super::adapter::OperationSpec>
*/
pub(crate) fn deploy_operations() -> Vec<super::adapter::OperationSpec> {
    let id = "deploy-fixture";
    let read = (true, false, false);
    vec![
        spec(
            id,
            "deployment.get",
            EffectClass::ReadOnly,
            &[StepKind::Query],
            &[("deployment", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "checks.list",
            EffectClass::ReadOnly,
            &[StepKind::Query],
            &[("deployment", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "logs.query",
            EffectClass::ReadOnly,
            &[StepKind::Diagnostic, StepKind::Query],
            &[("deployment", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "bundle.assemble",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("deployment", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "repo.apply_change",
            EffectClass::StateChanging,
            &[StepKind::Change],
            &[
                ("change_ref", ParamKind::Identifier),
                ("path", ParamKind::Text),
            ],
            (true, true, false),
        ),
        spec(
            id,
            "repo.commit",
            EffectClass::StateChanging,
            &[StepKind::Change],
            &[("change_ref", ParamKind::Identifier)],
            (true, true, false),
        ),
        spec(
            id,
            "tests.run",
            EffectClass::ReadOnly,
            &[StepKind::Test],
            &[("suite", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "ci.status",
            EffectClass::ReadOnly,
            &[StepKind::Test],
            &[("commit", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "tests.report",
            EffectClass::ReadOnly,
            &[StepKind::Test],
            &[("commit", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "deploy.release",
            EffectClass::ExternallyConsequential,
            &[StepKind::Deploy],
            &[
                ("environment", ParamKind::Identifier),
                ("commit", ParamKind::Identifier),
            ],
            (false, true, false),
        ),
        spec(
            id,
            "checks.smoke",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("deployment", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "checks.reconcile",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("invariant", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "ledger.adjust",
            EffectClass::StateChanging,
            &[StepKind::Change],
            &[("account", ParamKind::Identifier)],
            (false, true, true),
        ),
    ]
}

/** The deploy fixture system with its read results
 * Input
    - status: &str - the deployment status it reports
 * Output
    - FixtureAdapter
*/
pub(crate) fn deploy_fixture(status: &str) -> FixtureAdapter {
    let responses: BTreeMap<String, Value> = [
        (
            "deployment.get",
            json!({"status": status, "environment": "production"}),
        ),
        (
            "checks.list",
            json!({"checks": [{"check": "smoke", "healthy": status == "succeeded"}]}),
        ),
        (
            "logs.query",
            json!({"lines": 1, "excerpt": "refund_worker log line"}),
        ),
        ("bundle.assemble", json!({"complete": true})),
        ("tests.run", json!({"passed": 42, "failed": 0})),
        ("ci.status", json!({"status": "success"})),
        ("tests.report", json!({"passed": 42, "failed": 0})),
        ("checks.smoke", json!({"healthy": true})),
        ("checks.reconcile", json!({"duplicates": 0})),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();
    FixtureAdapter::new("deploy-fixture", deploy_operations(), responses)
}

/** The independent monitoring fixture used only for verification
 * Input
    - deployment_status: &str - status it reports for the deployment
    - logs_available: bool - whether it sees the logs
 * Output
    - FixtureAdapter
*/
pub(crate) fn status_fixture(deployment_status: &str, logs_available: bool) -> FixtureAdapter {
    let id = "status-fixture";
    let read = (true, false, false);
    let operations = vec![
        spec(
            id,
            "deployment.status",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("deployment", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "logs.count",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("deployment", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "ci.result",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("commit", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "tests.result",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("commit", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "change.verified",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("commit", ParamKind::Identifier)],
            read,
        ),
        spec(
            id,
            "ledger.duplicates",
            EffectClass::ReadOnly,
            &[StepKind::Verify],
            &[("deployment", ParamKind::Identifier)],
            read,
        ),
    ];
    let responses: BTreeMap<String, Value> = [
        ("deployment.status", json!({"status": deployment_status})),
        (
            "logs.count",
            json!({"available": logs_available, "count": if logs_available { 1 } else { 0 }}),
        ),
        ("ci.result", json!({"status": "success"})),
        ("tests.result", json!({"failed": 0})),
        ("change.verified", json!({"verified": true})),
        ("ledger.duplicates", json!({"duplicates": 0})),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();
    FixtureAdapter::new(id, operations, responses)
}

/** An input parameter source
 * Input
    - name: &str - input
 * Output
    - ParamSource
*/
fn input(name: &str) -> ParamSource {
    ParamSource::Input { name: name.into() }
}

/** A literal parameter source
 * Input
    - value: &str - value
 * Output
    - ParamSource
*/
fn literal(value: &str) -> ParamSource {
    ParamSource::Literal {
        value: value.into(),
    }
}

/** A fresh-read verification binding on the status fixture
 * Input
    - claim: &str - claim
    - operation: &str - status operation
    - param: (&str, &str) - parameter name and input name
    - field: &str - result field
    - equals: &str - expected value
 * Output
    - VerificationBinding
*/
fn check(
    claim: &str,
    operation: &str,
    param: (&str, &str),
    field: &str,
    equals: &str,
) -> VerificationBinding {
    VerificationBinding {
        claim_id: claim.into(),
        method: VerificationMethod::AdapterCheck {
            adapter_id: "status-fixture".into(),
            operation_id: operation.into(),
            params: [(param.0.to_string(), input(param.1))]
                .into_iter()
                .collect(),
            field: field.into(),
            equals: equals.into(),
        },
    }
}

/** A step binding on the deploy fixture
 * Input
    - step: &str - step id
    - operation: &str - operation
    - params: Vec<(&str, ParamSource)> - parameters
 * Output
    - StepBinding
*/
fn bind(step: &str, operation: &str, params: Vec<(&str, ParamSource)>) -> StepBinding {
    StepBinding {
        step_id: step.into(),
        adapter_id: "deploy-fixture".into(),
        operation_id: operation.into(),
        params: params
            .into_iter()
            .map(|(name, source)| (name.to_string(), source))
            .collect(),
    }
}

/** A demonstration world
 * Fields
    - capture: EvidenceQueryBroker - Work Capture
    - registry: CapabilityRegistry - registry
    - current: CurrentVersions - current versions
    - cast: Cast - people and agent
    - deploy: Rc<FixtureAdapter> - deploy fixture
    - status: Rc<FixtureAdapter> - monitoring fixture
    - adapters: AdapterRegistry - registered adapters
    - runtime: Box<dyn TransactionRuntime> - transaction runtime
    - ledger: Ledger - ledger
    - events: MemorySink - events
    - clock: Clock - clock
    - lab_reports, discovery_reports, compilation_reports - reports
*/
pub(crate) struct World {
    pub(crate) capture: EvidenceQueryBroker,
    pub(crate) registry: CapabilityRegistry,
    pub(crate) current: CurrentVersions,
    pub(crate) cast: Cast,
    pub(crate) deploy: Rc<FixtureAdapter>,
    pub(crate) status: Rc<FixtureAdapter>,
    pub(crate) adapters: AdapterRegistry,
    pub(crate) runtime: Box<dyn TransactionRuntime>,
    pub(crate) ledger: Ledger,
    pub(crate) events: MemorySink,
    pub(crate) clock: Clock,
    pub(crate) lab_reports: Vec<LabReport>,
    pub(crate) discovery_reports: Vec<DiscoveryReport>,
    pub(crate) compilation_reports: Vec<CompilationReport>,
}

impl World {
    /** A world around a capture and registry, with fresh fixtures
     * Input
        - capture: EvidenceQueryBroker - Work Capture
        - registry: CapabilityRegistry - registry
        - deploy_status: &str - status the systems report
     * Output
        - World
    */
    fn new(
        capture: EvidenceQueryBroker,
        registry: CapabilityRegistry,
        deploy_status: &str,
    ) -> Self {
        let deploy = Rc::new(deploy_fixture(deploy_status));
        let status = Rc::new(status_fixture(deploy_status, true));
        let adapters = AdapterRegistry::default()
            .with(deploy.clone())
            .with(status.clone());
        Self {
            capture,
            registry,
            current: capability_demo::current_versions(),
            cast: capability_demo::cast(),
            deploy,
            status,
            adapters,
            runtime: Box::new(UnavailableRuntime),
            ledger: Ledger::in_memory(),
            events: MemorySink::default(),
            clock: Clock::fixed(EPOCH),
            lab_reports: Vec::new(),
            discovery_reports: Vec::new(),
            compilation_reports: Vec::new(),
        }
    }

    /** Replace the monitoring fixture
     * Input
        - status: FixtureAdapter - new monitoring fixture
     * Output
        - None
    */
    pub(crate) fn set_status(&mut self, status: FixtureAdapter) {
        self.status = Rc::new(status);
        self.adapters = AdapterRegistry::default()
            .with(self.deploy.clone())
            .with(self.status.clone());
    }

    /** The engine, acting with an access policy
     * Input
        - reader: &'a AccessPolicy - the actor's Work Capture access
     * Output
        - Engine<'a>
    */
    pub(crate) fn engine<'a>(&'a mut self, reader: &'a AccessPolicy) -> Engine<'a> {
        Engine {
            registry: &mut self.registry,
            sources: Sources {
                capture: &self.capture.store,
                current: &self.current,
                reader,
            },
            lab_reports: &self.lab_reports,
            discovery_reports: &self.discovery_reports,
            compilation_reports: &self.compilation_reports,
            adapters: &self.adapters,
            runtime: self.runtime.as_ref(),
            ledger: &mut self.ledger,
            events: &mut self.events,
            clock: &self.clock,
        }
    }

    /** The exact digest of a stored version
     * Input
        - id: &str - capability
     * Output
        - String
    */
    pub(crate) fn digest(&self, id: &str) -> String {
        self.registry
            .state
            .record(id, 1)
            .map(|record| record.definition_digest.clone())
            .unwrap_or_default()
    }
}

/** The world with the published read-only diagnostics capability
 * Input
    - None
 * Output
    - CapabilityResult<World>
*/
pub(crate) fn diagnostics_world() -> CapabilityResult<World> {
    let cast = capability_demo::cast();
    let (capture, pack) = capability_demo::captured()?;
    let mut registry = CapabilityRegistry::in_memory(capability_demo::grants(&cast));
    let current = capability_demo::current_versions();
    let engineer = capability_demo::reader(&cast.engineer)?;
    let sources = Sources {
        capture: &capture.store,
        current: &current,
        reader: &engineer,
    };
    let intake = registry
        .submit_intake(
            &capability_demo::diagnostics_intake(&pack),
            &sources,
            &cast.engineer,
            EPOCH,
        )?
        .intake;
    let definition = capability_demo::bind(DIAGNOSTICS, &pack, &intake.intake_id)?;
    registry.create_candidate(
        &cast.engineer,
        definition,
        Some("low-risk-technical"),
        EPOCH,
    )?;
    let run = registry.validate(
        &cast.engineer,
        "deployment-failure-diagnostics",
        1,
        &sources,
        false,
        EPOCH,
    )?;
    registry.publish(
        &cast.lead,
        &PublicationRequest {
            capability_id: "deployment-failure-diagnostics".into(),
            version: 1,
            reason: "triage".into(),
            acknowledged_warnings: run.warning_codes().into_iter().collect(),
        },
        EPOCH,
    )?;
    Ok(World::new(capture, registry, "failed"))
}

/** The invocation request for the diagnostics capability
 * Input
    - world: &World - world
    - key: &str - idempotency key
 * Output
    - InvocationRequest
*/
pub(crate) fn diagnostics_request(world: &World, key: &str) -> InvocationRequest {
    InvocationRequest {
        capability_id: "deployment-failure-diagnostics".into(),
        version: 1,
        digest: world.digest("deployment-failure-diagnostics"),
        task_id: Some("PAY-142".into()),
        episode_id: None,
        environment: Some("production".into()),
        repository: Some("acme/payments".into()),
        confirmed_preconditions: vec!["deployment-known".into(), "read-access".into()],
        inputs: [("deployment_id".to_string(), "dep-7731".to_string())]
            .into_iter()
            .collect(),
        bindings: BindingSet {
            binding_id: "diagnostics-bindings".into(),
            version: 1,
            capability_id: "deployment-failure-diagnostics".into(),
            capability_version: 1,
            steps: vec![
                bind(
                    "fetch-deployment",
                    "deployment.get",
                    vec![("deployment", input("deployment_id"))],
                ),
                bind(
                    "fetch-checks",
                    "checks.list",
                    vec![("deployment", input("deployment_id"))],
                ),
                bind(
                    "fetch-logs",
                    "logs.query",
                    vec![("deployment", input("deployment_id"))],
                ),
                bind(
                    "bundle",
                    "bundle.assemble",
                    vec![("deployment", input("deployment_id"))],
                ),
            ],
            verifications: vec![
                check(
                    "outcome-recorded",
                    "deployment.status",
                    ("deployment", "deployment_id"),
                    "status",
                    "failed",
                ),
                VerificationBinding {
                    claim_id: "outcome-recorded".into(),
                    method: VerificationMethod::WorkCaptureEvidence {
                        evidence_type: EvidenceType::Deployment,
                        field: "status".into(),
                        equals: "failed".into(),
                    },
                },
                check(
                    "logs-retrieved",
                    "logs.count",
                    ("deployment", "deployment_id"),
                    "available",
                    "true",
                ),
            ],
        },
        discovery_report: None,
        compilation_report: None,
        idempotency_key: key.into(),
        execute: true,
        limits: PlanLimits::default(),
    }
}

/** The world with the published, state-changing refund change capability
 * Input
    - None
 * Output
    - CapabilityResult<World>
*/
pub(crate) fn refund_world() -> CapabilityResult<World> {
    let compiled = crate::capability_lab::demo::compiled(&success_scenario()?)?;
    let crate::capability_lab::demo::Compiled {
        capture,
        mut registry,
        current,
        cast,
        report,
    } = compiled;
    let lead = capability_demo::reader(&cast.lead)?;
    let sources = Sources {
        capture: &capture.store,
        current: &current,
        reader: &lead,
    };
    let run = registry.validate(
        &cast.lead,
        "refund-change-with-verified-deploy",
        1,
        &sources,
        false,
        EPOCH,
    )?;
    registry.publish(
        &cast.lead,
        &PublicationRequest {
            capability_id: "refund-change-with-verified-deploy".into(),
            version: 1,
            reason: "ship refund changes safely".into(),
            acknowledged_warnings: run.warning_codes().into_iter().collect(),
        },
        EPOCH,
    )?;
    let mut world = World::new(capture, registry, "succeeded");
    world.compilation_reports.push(report);
    Ok(world)
}

/** The invocation request for the refund change, its steps bound by their descriptions
 * Input
    - world: &World - world
    - key: &str - idempotency key
 * Output
    - CapabilityResult<InvocationRequest>
*/
pub(crate) fn refund_request(world: &World, key: &str) -> CapabilityResult<InvocationRequest> {
    let definition = &world
        .registry
        .state
        .record("refund-change-with-verified-deploy", 1)?
        .definition;
    let mut steps = Vec::new();
    for step in &definition.execution.steps {
        let description = step.description.as_str();
        let binding = match step.kind {
            StepKind::Approval => continue,
            StepKind::Change if description.contains("app/refunds.py") => bind(
                &step.step_id,
                "repo.apply_change",
                vec![
                    ("change_ref", input("change_ref")),
                    ("path", literal("app/refunds.py")),
                ],
            ),
            StepKind::Change => bind(
                &step.step_id,
                "repo.commit",
                vec![("change_ref", input("change_ref"))],
            ),
            StepKind::Test if description.contains("CI") => bind(
                &step.step_id,
                "ci.status",
                vec![("commit", input("commit"))],
            ),
            StepKind::Test if description.contains("suite") => bind(
                &step.step_id,
                "tests.report",
                vec![("commit", input("commit"))],
            ),
            StepKind::Test => bind(
                &step.step_id,
                "tests.run",
                vec![("suite", literal("refunds"))],
            ),
            StepKind::Deploy => bind(
                &step.step_id,
                "deploy.release",
                vec![
                    ("environment", input("environment")),
                    ("commit", input("commit")),
                ],
            ),
            StepKind::Verify if description.contains("reconciliation") => bind(
                &step.step_id,
                "checks.reconcile",
                vec![("invariant", literal("inv-no-double-refund"))],
            ),
            StepKind::Verify => bind(
                &step.step_id,
                "checks.smoke",
                vec![("deployment", input("deployment_id"))],
            ),
            other => {
                return Err(CapabilityError::new(
                    ErrorCode::InvalidSchema,
                    format!("no demonstration binding for {other:?}"),
                ))
            }
        };
        steps.push(binding);
    }
    Ok(InvocationRequest {
        capability_id: "refund-change-with-verified-deploy".into(),
        version: 1,
        digest: world.digest("refund-change-with-verified-deploy"),
        task_id: Some("PAY-150".into()),
        episode_id: None,
        environment: Some("production".into()),
        repository: Some("acme/payments".into()),
        confirmed_preconditions: vec!["repository-access".into(), "deployment-target-known".into()],
        inputs: [
            ("task_id", "PAY-150"),
            ("environment", "production"),
            ("change_ref", "chg-150"),
            ("commit", "4be81d0c2a"),
            ("deployment_id", "dep-7802"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect(),
        bindings: BindingSet {
            binding_id: "refund-change-bindings".into(),
            version: 1,
            capability_id: "refund-change-with-verified-deploy".into(),
            capability_version: 1,
            steps,
            verifications: vec![
                check(
                    "ci-passed",
                    "ci.result",
                    ("commit", "commit"),
                    "status",
                    "success",
                ),
                check(
                    "tests-passed",
                    "tests.result",
                    ("commit", "commit"),
                    "failed",
                    "0",
                ),
                check(
                    "deployment-succeeded",
                    "deployment.status",
                    ("deployment", "deployment_id"),
                    "status",
                    "succeeded",
                ),
                check(
                    "change-verified",
                    "change.verified",
                    ("commit", "commit"),
                    "verified",
                    "true",
                ),
                check(
                    "invariant-inv-no-double-refund",
                    "ledger.duplicates",
                    ("deployment", "deployment_id"),
                    "duplicates",
                    "0",
                ),
            ],
        },
        discovery_report: None,
        compilation_report: None,
        idempotency_key: key.into(),
        execute: true,
        limits: PlanLimits::default(),
    })
}

/** What the demonstration produced
 * Fields
    - succeeded: InvocationRecord - the verified read-only invocation
    - ledger_verified: bool - its stored record matches its digest
    - repeat_returns_same: bool - the same key and request returned the same invocation
    - dispatches_after_repeat: usize - deployment.get dispatches after the repetition (still 1)
    - changed_digest, stale, revoked: InvocationState - blocked outcomes
    - unsupported: (InvocationState, Vec<String>, usize) - state, reasons, dispatches made
    - reconciled: InvocationRecord - the refund change after reconciliation
    - release_dispatches: usize - deploy.release dispatches (1: no blind retry)
    - failed_verification: InvocationState - outcome when a postcondition fails
    - events: usize - lifecycle events emitted
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DemoReport {
    pub(crate) succeeded: InvocationRecord,
    pub(crate) ledger_verified: bool,
    pub(crate) repeat_returns_same: bool,
    pub(crate) dispatches_after_repeat: usize,
    pub(crate) changed_digest: InvocationState,
    pub(crate) stale: InvocationState,
    pub(crate) revoked: InvocationState,
    pub(crate) unsupported: (InvocationState, Vec<String>, usize),
    pub(crate) reconciled: InvocationRecord,
    pub(crate) release_dispatches: usize,
    pub(crate) failed_verification: InvocationState,
    pub(crate) events: usize,
}

/** Run the demonstration
 * Input
    - None
 * Output
    - CapabilityResult<DemoReport>
*/
pub(crate) fn run() -> CapabilityResult<DemoReport> {
    let mut world = diagnostics_world()?;
    let engineer = capability_demo::reader(&world.cast.engineer)?;
    let actor = world.cast.engineer.clone();
    let request = diagnostics_request(&world, "demo-diagnostics-1");
    let succeeded = world.engine(&engineer).submit(&actor, request.clone())?;
    let ledger_verified = world.ledger.get(&succeeded.invocation_id)?.verify_digest();
    let repeat = world.engine(&engineer).submit(&actor, request)?;
    let dispatches_after_repeat = world.deploy.calls("deployment.get");
    let mut changed = diagnostics_request(&world, "demo-changed-digest");
    changed.digest = "0".repeat(32);
    let changed_digest = world.engine(&engineer).submit(&actor, changed)?.state;
    let mut unsupported_request = diagnostics_request(&world, "demo-unsupported");
    unsupported_request.bindings.steps[2].operation_id = "shell.exec".into();
    let before = [
        "deployment.get",
        "checks.list",
        "logs.query",
        "bundle.assemble",
    ]
    .iter()
    .map(|operation| world.deploy.calls(operation))
    .sum::<usize>();
    let unsupported_record = world
        .engine(&engineer)
        .submit(&actor, unsupported_request)?;
    let after = [
        "deployment.get",
        "checks.list",
        "logs.query",
        "bundle.assemble",
    ]
    .iter()
    .map(|operation| world.deploy.calls(operation))
    .sum::<usize>();
    world.current.zones.insert("refunds".into(), "v4".into());
    let stale = {
        let request = diagnostics_request(&world, "demo-stale");
        world.engine(&engineer).submit(&actor, request)?.state
    };
    world.current.zones.insert("refunds".into(), "v3".into());
    world.set_status(status_fixture("failed", false));
    let failed_verification = {
        let request = diagnostics_request(&world, "demo-failed-verification");
        world.engine(&engineer).submit(&actor, request)?.state
    };
    world.registry.withdraw(
        &world.cast.lead,
        "deployment-failure-diagnostics",
        None,
        LifecycleState::Revoked,
        "replaced",
        EPOCH,
    )?;
    let revoked = {
        let request = diagnostics_request(&world, "demo-revoked");
        world.engine(&engineer).submit(&actor, request)?.state
    };
    let mut events = world.events.events.len();
    let mut refund = refund_world()?;
    let requester = refund.cast.engineer.clone();
    let lead = refund.cast.lead.clone();
    let refund_reader = capability_demo::reader(&requester)?;
    let request = refund_request(&refund, "demo-refund-1")?;
    let pending = refund.engine(&refund_reader).submit(&requester, request)?;
    let digest = pending
        .plan
        .as_ref()
        .map(|plan| plan.plan_digest.clone())
        .unwrap_or_default();
    refund
        .engine(&refund_reader)
        .approve(&lead, &pending.invocation_id, &digest)?;
    refund
        .deploy
        .inject("deploy.release", Fault::TimeoutAfterEffect);
    let interrupted = refund
        .engine(&refund_reader)
        .run(&requester, &pending.invocation_id)?;
    if interrupted.state != InvocationState::ReconciliationRequired {
        return Err(CapabilityError::new(
            ErrorCode::InvalidTransition,
            format!(
                "expected reconciliation, got {:?}: {:?}",
                interrupted.state, interrupted.failures
            ),
        ));
    }
    let reconciled = refund
        .engine(&refund_reader)
        .reconcile(&requester, &pending.invocation_id)?;
    events += refund.events.events.len();
    Ok(DemoReport {
        repeat_returns_same: repeat.invocation_id == succeeded.invocation_id
            && repeat == world.ledger.get(&succeeded.invocation_id)?,
        succeeded,
        ledger_verified,
        dispatches_after_repeat,
        changed_digest,
        stale,
        revoked,
        unsupported: (
            unsupported_record.state,
            unsupported_record.failures,
            after - before,
        ),
        release_dispatches: refund.deploy.calls("deploy.release"),
        reconciled,
        failed_verification,
        events,
    })
}
