// The deterministic discovery demonstration ('crane discover demo'), from fixtures only. It
// builds a registry through the earlier components' own interfaces (nothing is written into the
// registry directly):
//
//   refund-change-with-verified-deploy  compiled, validated, published; inconclusive lab report
//   refund-change-legacy                published, then revoked
//   refund-change-unchecked             published; its lab report failed a scenario
//   refund-change-draft                 stored, never validated (a candidate)
//   refund-reporting                    published, for another task class
//
// and runs four discoveries for a refund change: with the task's Work Capture context (one
// recommendation, every alternative explained), without an environment (insufficient context),
// in staging (no match), and after the refunds zone moved to v4 (blocked by staleness).

use serde::Serialize;

use super::report::{discover, DiscoveryReport};
use super::resolve::{ContextRef, DiscoveryRequest};
use crate::capability::demo as capability_demo;
use crate::capability::lifecycle::{LifecycleState, PublicationRequest};
use crate::capability::model::CapabilityDefinition;
use crate::capability::refs::Sources;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capability_lab::demo::{compiled, Compiled, PLAN};
use crate::capability_lab::report::LabReport;
use crate::capability_lab::scenarios::TestPlan;
use crate::compiler::demo::success_scenario;

/** Fixed time of the demonstration */
pub(crate) const NOW: u64 = 1_760_000_500_000;

/** A populated registry and its lab reports
 * Fields
    - world: Compiled - Work Capture, registry, current versions, people, compilation report
    - lab_reports: Vec<LabReport> - lab reports
    - pack: (String, String) - the task's Evidence Pack id and digest
*/
pub(crate) struct Catalog {
    pub(crate) world: Compiled,
    pub(crate) lab_reports: Vec<LabReport>,
    pub(crate) pack: (String, String),
}

impl Catalog {
    /** Run a discovery as the engineer
     * Input
        - request: &DiscoveryRequest - request
     * Output
        - CapabilityResult<DiscoveryReport>
    */
    pub(crate) fn discover(&self, request: &DiscoveryRequest) -> CapabilityResult<DiscoveryReport> {
        let reader = capability_demo::reader(&self.world.cast.engineer)?;
        let sources = Sources {
            capture: &self.world.capture.store,
            current: &self.world.current,
            reader: &reader,
        };
        Ok(discover(
            &self.world.registry,
            &sources,
            &self.world.cast.engineer,
            request,
            &self.lab_reports,
            NOW,
        ))
    }

    /** The task's request: a refund change with its Work Capture context
     * Input
        - None (uses self)
     * Output
        - DiscoveryRequest
    */
    pub(crate) fn request(&self) -> DiscoveryRequest {
        DiscoveryRequest {
            task_class: "refund-change".into(),
            environment: None,
            repository: None,
            confirmed_preconditions: vec![
                "repository-access".into(),
                "deployment-target-known".into(),
            ],
            entities: Vec::new(),
            context: Some(ContextRef {
                pack_id: self.pack.0.clone(),
                pack_digest: self.pack.1.clone(),
            }),
        }
    }
}

/** Validate a stored version as the lead and publish it, acknowledging its warnings
 * Input
    - world: &mut Compiled - world
    - id: &str - capability
 * Output
    - CapabilityResult<()>
*/
fn validate_and_publish(world: &mut Compiled, id: &str) -> CapabilityResult<()> {
    let reader = capability_demo::reader(&world.cast.lead)?;
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    let run = world
        .registry
        .validate(&world.cast.lead, id, 1, &sources, false, NOW)?;
    let request = PublicationRequest {
        capability_id: id.into(),
        version: 1,
        reason: "demo".into(),
        acknowledged_warnings: run.warning_codes().into_iter().collect(),
    };
    world
        .registry
        .publish(&world.cast.lead, &request, NOW)
        .map(|_| ())
}

/** Store a variant of a definition under another id
 * Input
    - world: &mut Compiled - world
    - base: &CapabilityDefinition - definition to vary
    - id: &str - new id
    - change: impl FnOnce(&mut CapabilityDefinition) - variation
 * Output
    - CapabilityResult<()>
*/
fn variant(
    world: &mut Compiled,
    base: &CapabilityDefinition,
    id: &str,
    change: impl FnOnce(&mut CapabilityDefinition),
) -> CapabilityResult<()> {
    let mut definition = base.clone();
    definition.capability_id = id.into();
    change(&mut definition);
    world
        .registry
        .create_candidate(&world.cast.engineer, definition, Some("conservative"), NOW)
        .map(|_| ())
}

/** Build the demonstration catalog
 * Input
    - None
 * Output
    - CapabilityResult<Catalog>
*/
pub(crate) fn catalog() -> CapabilityResult<Catalog> {
    let mut world = compiled(&success_scenario()?)?;
    let target = world.target()?;
    let base = world
        .registry
        .state
        .record(&target.capability_id, 1)?
        .definition
        .clone();
    validate_and_publish(&mut world, &target.capability_id)?;
    let plan = TestPlan::parse(PLAN)?;
    let mut lab_reports = vec![world.lab(&world.report, &plan)?];
    variant(&mut world, &base, "refund-change-legacy", |definition| {
        definition.name = "Legacy refund change".into()
    })?;
    validate_and_publish(&mut world, "refund-change-legacy")?;
    world.registry.withdraw(
        &world.cast.lead,
        "refund-change-legacy",
        None,
        LifecycleState::Revoked,
        "superseded by the verified procedure",
        NOW,
    )?;
    variant(&mut world, &base, "refund-change-unchecked", |definition| {
        definition.name = "Unchecked refund change".into()
    })?;
    validate_and_publish(&mut world, "refund-change-unchecked")?;
    let mut wrong: serde_json::Value = serde_json::from_str(PLAN)
        .map_err(|error| CapabilityError::new(ErrorCode::InvalidSchema, error.to_string()))?;
    wrong["scenarios"][1]["check"]["expect"] = serde_json::json!("yes");
    let failing = TestPlan::parse(&wrong.to_string())?;
    let unchecked = crate::capability_lab::preflight::Target {
        capability_id: "refund-change-unchecked".into(),
        version: 1,
        expected_digest: world
            .registry
            .state
            .record("refund-change-unchecked", 1)?
            .definition_digest
            .clone(),
    };
    let reader = capability_demo::reader(&world.cast.engineer)?;
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    lab_reports.push(crate::capability_lab::report::run_lab(
        &world.registry,
        &sources,
        &world.cast.engineer,
        &unchecked,
        None,
        &failing,
        NOW,
    )?);
    variant(&mut world, &base, "refund-change-draft", |definition| {
        definition.name = "Draft refund change".into()
    })?;
    variant(&mut world, &base, "refund-reporting", |definition| {
        definition.applicability.task_classes = vec!["refund-report".into()]
    })?;
    validate_and_publish(&mut world, "refund-reporting")?;
    let pack = world
        .capture
        .store
        .packs
        .values()
        .next()
        .map(|pack| (pack.pack_id.clone(), pack.manifest.digest.clone()))
        .ok_or_else(|| CapabilityError::new(ErrorCode::NotFound, "no pack"))?;
    Ok(Catalog {
        world,
        lab_reports,
        pack,
    })
}

/** What the demonstration produced
 * Fields
    - with_context: DiscoveryReport - the task with its Work Capture context
    - unknown_environment: DiscoveryReport - without context or environment
    - staging: DiscoveryReport - in staging
    - stale: DiscoveryReport - after the refunds zone moved to v4
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DemoReport {
    pub(crate) with_context: DiscoveryReport,
    pub(crate) unknown_environment: DiscoveryReport,
    pub(crate) staging: DiscoveryReport,
    pub(crate) stale: DiscoveryReport,
}

/** Run the demonstration
 * Input
    - None
 * Output
    - CapabilityResult<DemoReport>
*/
pub(crate) fn run() -> CapabilityResult<DemoReport> {
    let mut catalog = catalog()?;
    let with_context = catalog.discover(&catalog.request())?;
    let unknown_environment = catalog.discover(&DiscoveryRequest {
        context: None,
        ..catalog.request()
    })?;
    let staging = catalog.discover(&DiscoveryRequest {
        context: None,
        environment: Some("staging".into()),
        ..catalog.request()
    })?;
    catalog
        .world
        .current
        .zones
        .insert("refunds".into(), "v4".into());
    let stale = catalog.discover(&catalog.request())?;
    Ok(DemoReport {
        with_context,
        unknown_environment,
        staging,
        stale,
    })
}
