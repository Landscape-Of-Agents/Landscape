// The deterministic lab demonstration ('crane lab demo'), from fixtures only. It compiles the
// successful refund-change episode (Component 3's fixture), then runs the approved test plan
// (src/capability_lab/fixtures/refund_change_plan.json) four times:
//
//   1. on the compiled candidate: every step conforms, every scenario passes, every claim and the
//      business invariant verify; the run is still inconclusive overall because the single-episode
//      generalization and the unresolvable repository reference cannot be established
//   2. on a candidate compiled without its test report (needs_more_evidence): the compiler's
//      blocking gaps and the unverified test claim are blocked, never passed
//   3. with a compilation report altered after it was written: preflight fails
//   4. after the refunds zone moved to v4: the stale dependency fails with a counterexample

use serde::Serialize;

use super::preflight::Target;
use super::report::{run_lab, LabReport};
use super::scenarios::TestPlan;
use crate::capability::demo::{self as capability_demo, Cast};
use crate::capability::refs::{CurrentVersions, Sources};
use crate::capability::registry::CapabilityRegistry;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capture::broker::run::EvidenceQueryBroker;
use crate::capture::demo::Scenario;
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::episode::EpisodeOutcome;
use crate::compiler::assemble::{compile, CompilationReport};
use crate::compiler::demo::{compile_request, investigate, success_scenario};

/** The approved test plan fixture */
pub(crate) const PLAN: &str = include_str!("fixtures/refund_change_plan.json");

/** Fixed time of the demonstration */
pub(crate) const NOW: u64 = 1_760_000_400_000;

/** A compiled candidate and everything a lab run needs
 * Fields
    - capture: EvidenceQueryBroker - Work Capture with the episode's pack
    - registry: CapabilityRegistry - registry holding the candidate
    - current: CurrentVersions - current versions
    - cast: Cast - people
    - report: CompilationReport - compilation report
*/
pub(crate) struct Compiled {
    pub(crate) capture: EvidenceQueryBroker,
    pub(crate) registry: CapabilityRegistry,
    pub(crate) current: CurrentVersions,
    pub(crate) cast: Cast,
    pub(crate) report: CompilationReport,
}

impl Compiled {
    /** The exact target of the compiled candidate
     * Input
        - None (uses self)
     * Output
        - CapabilityResult<Target>
    */
    pub(crate) fn target(&self) -> CapabilityResult<Target> {
        let candidate = self.report.candidate.as_ref().ok_or_else(|| {
            CapabilityError::new(ErrorCode::NotFound, "the compilation stored no candidate")
        })?;
        Ok(Target {
            capability_id: candidate.capability_id.clone(),
            version: candidate.version,
            expected_digest: candidate.digest.clone(),
        })
    }

    /** Run the lab as the engineer
     * Input
        - report: &CompilationReport - compilation report to use
        - plan: &TestPlan - plan
     * Output
        - CapabilityResult<LabReport>
    */
    pub(crate) fn lab(
        &self,
        report: &CompilationReport,
        plan: &TestPlan,
    ) -> CapabilityResult<LabReport> {
        let reader = capability_demo::reader(&self.cast.engineer)?;
        let sources = Sources {
            capture: &self.capture.store,
            current: &self.current,
            reader: &reader,
        };
        run_lab(
            &self.registry,
            &sources,
            &self.cast.engineer,
            &self.target()?,
            Some(report),
            plan,
            NOW,
        )
    }
}

/** Compile a scenario's episode
 * Input
    - scenario: &Scenario - scenario
 * Output
    - CapabilityResult<Compiled>
*/
pub(crate) fn compiled(scenario: &Scenario) -> CapabilityResult<Compiled> {
    let cast = capability_demo::cast();
    let (capture, pack) = investigate(scenario, Some(EpisodeOutcome::Succeeded), &cast.engineer)?;
    let mut registry = CapabilityRegistry::in_memory(capability_demo::grants(&cast));
    let current = capability_demo::current_versions();
    let reader = capability_demo::reader(&cast.engineer)?;
    let sources = Sources {
        capture: &capture.store,
        current: &current,
        reader: &reader,
    };
    let report = compile(
        &mut registry,
        &sources,
        &cast.engineer,
        &compile_request(&pack),
        NOW - 100,
    );
    Ok(Compiled {
        capture,
        registry,
        current,
        cast,
        report,
    })
}

/** What the demonstration produced
 * Fields
    - compiled: LabReport - the compiled candidate
    - needs_evidence: LabReport - a candidate compiled without its test report
    - tampered: LabReport - an altered compilation report
    - stale: LabReport - after the refunds zone moved to v4
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DemoReport {
    pub(crate) compiled: LabReport,
    pub(crate) needs_evidence: LabReport,
    pub(crate) tampered: LabReport,
    pub(crate) stale: LabReport,
}

/** Run the demonstration
 * Input
    - None
 * Output
    - CapabilityResult<DemoReport>
*/
pub(crate) fn run() -> CapabilityResult<DemoReport> {
    let plan = TestPlan::parse(PLAN)?;
    let mut success = compiled(&success_scenario()?)?;
    let compiled_report = success.lab(&success.report, &plan)?;
    let mut altered = success.report.clone();
    altered.warnings.clear();
    altered.gaps.retain(|gap| !gap.blocking);
    altered.reasons.push("edited after compilation".into());
    let tampered = success.lab(&altered, &plan)?;
    success.current.zones.insert("refunds".into(), "v4".into());
    let stale = success.lab(&success.report, &plan)?;
    let mut without_tests = success_scenario()?;
    if let Some(ci) = without_tests
        .resources
        .iter_mut()
        .find(|resource| resource.resource_id == "ci")
    {
        ci.records
            .retain(|record| record.evidence_type != EvidenceType::TestReport);
    }
    let missing = compiled(&without_tests)?;
    let needs_evidence = missing.lab(&missing.report, &plan)?;
    Ok(DemoReport {
        compiled: compiled_report,
        needs_evidence,
        tampered,
        stale,
    })
}
