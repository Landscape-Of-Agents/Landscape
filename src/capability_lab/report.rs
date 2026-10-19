// The lab report: a versioned, integrity-protected record of one lab run. It references the exact
// candidate (id, version, digest, state), its compilation report (id, digest, outcome), the test
// plan (id, version, digest) and its scenarios, every evidence reference the checks relied on, the
// environment the checks ran in (deterministic evaluation; nothing executed), and the Landscape and
// Crane versions they ran against. Every check keeps its status and counterexample. Limitations
// carried from the compilation (chronological order is not causation, branch notes live only in
// the compilation report, unrecognized commands were assumed to be changes) are stated explicitly.
// The overall status is the worst check status; "passed" means only that every tested condition
// held. The report changes no lifecycle state and no publication eligibility.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::preflight::{preflight, Target};
use super::scenarios::{self, TestPlan};
use super::{
    conformance, invariants, CheckStatus, LabCheck, LAB_REPORT_SCHEMA_VERSION, LAB_VERSION,
};
use crate::capability::lifecycle::LifecycleState;
use crate::capability::refs::{CurrentVersions, Sources};
use crate::capability::registry::CapabilityRegistry;
use crate::capability::{CapabilityResult, Role};
use crate::capture::{stable_id, Actor};
use crate::compiler::assemble::CompilationReport;
use crate::compiler::extract::recognized;
use crate::compiler::{Basis, CompilationOutcome};
use crate::trust::crypto::digest_json;

/** What every lab report states about its meaning */
pub(crate) const MEANING: &str = "Passing checks prove only the conditions they tested, against the cited evidence and the recorded versions; nothing was executed, and this report does not validate, publish, or change the eligibility of the capability (Component 2 holds that authority).";

/** The compilation report a lab run used
 * Fields
    - report_id: String - compilation report
    - digest: String - its digest
    - outcome: CompilationOutcome - its outcome
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompilationRef {
    pub(crate) report_id: String,
    pub(crate) digest: String,
    pub(crate) outcome: CompilationOutcome,
}

/** The test plan a lab run used
 * Fields
    - plan_id: String - plan
    - version: u32 - version
    - digest: String - content digest
    - scenarios: Vec<String> - scenario ids
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanRef {
    pub(crate) plan_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) scenarios: Vec<String>,
}

/** Where and how the checks ran
 * Fields
    - mode: String - "deterministic-evaluation": records and contracts were evaluated, nothing ran
    - executed: bool - always false: no candidate instruction was executed
    - scope_environments: Vec<String> - the candidate's verification scope
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LabEnvironment {
    pub(crate) mode: String,
    pub(crate) executed: bool,
    pub(crate) scope_environments: Vec<String>,
}

/** Counts of check statuses
 * Fields
    - passed, failed, not_run, blocked, inconclusive: usize - counts
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) not_run: usize,
    pub(crate) blocked: usize,
    pub(crate) inconclusive: usize,
}

/** A lab report
 * Fields
    - report_id: String - stable identifier (from the digest)
    - schema_version: u32 - LAB_REPORT_SCHEMA_VERSION
    - lab_version: String - LAB_VERSION
    - capability_id: String - capability
    - version: u32 - version
    - candidate_digest: String - definition digest examined
    - candidate_state: Option<LifecycleState> - its lifecycle state (unchanged by the lab)
    - compilation: Option<CompilationRef> - compilation report used
    - test_plan: PlanRef - test plan used
    - environment: LabEnvironment - how the checks ran
    - governance: CurrentVersions - Landscape and Crane versions checked against
    - evidence_refs: Vec<String> - every reference the checks relied on
    - checks: Vec<LabCheck> - checks with statuses and counterexamples
    - summary: Summary - status counts
    - overall: CheckStatus - worst status (failed > blocked > inconclusive > not_run > passed)
    - limitations: Vec<String> - stated limitations
    - meaning: String - MEANING
    - actor: Actor - who ran it
    - created_at: u64 - Unix milliseconds
    - digest: String - digest of the report without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LabReport {
    pub(crate) report_id: String,
    pub(crate) schema_version: u32,
    pub(crate) lab_version: String,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) candidate_digest: String,
    pub(crate) candidate_state: Option<LifecycleState>,
    pub(crate) compilation: Option<CompilationRef>,
    pub(crate) test_plan: PlanRef,
    pub(crate) environment: LabEnvironment,
    pub(crate) governance: CurrentVersions,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) checks: Vec<LabCheck>,
    pub(crate) summary: Summary,
    pub(crate) overall: CheckStatus,
    pub(crate) limitations: Vec<String>,
    pub(crate) meaning: String,
    pub(crate) actor: Actor,
    pub(crate) created_at: u64,
    pub(crate) digest: String,
}

impl LabReport {
    /** The digest of the report's content (without its digest and identifier)
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn content_digest(&self) -> String {
        let mut content = self.clone();
        content.digest = String::new();
        content.report_id = String::new();
        digest_json("lab/report/v1", &json!(content))[..32].to_string()
    }

    /** Check that the report still matches its digest and identifier
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn verify_digest(&self) -> bool {
        let digest = self.content_digest();
        digest == self.digest && self.report_id == stable_id("lab", &[&digest])
    }
}

/** The overall status: the worst status of any check
 * Input
    - checks: &[LabCheck] - checks
 * Output
    - CheckStatus
*/
fn overall(checks: &[LabCheck]) -> CheckStatus {
    let has = |status: CheckStatus| checks.iter().any(|check| check.status == status);
    [
        CheckStatus::Failed,
        CheckStatus::Blocked,
        CheckStatus::Inconclusive,
        CheckStatus::NotRun,
    ]
    .into_iter()
    .find(|status| has(*status))
    .unwrap_or(CheckStatus::Passed)
}

/** The limitations a run must state
 * Input
    - compilation: Option<&CompilationReport> - compilation report
    - sources: &Sources - canonical sources
 * Output
    - Vec<String>
*/
fn limitations(compilation: Option<&CompilationReport>, sources: &Sources) -> Vec<String> {
    let mut limitations = vec!["The capability schema has no executable branches and the lab has no execution runtime: procedures were evaluated as descriptions, not run.".to_string()];
    let Some(report) = compilation else {
        return limitations;
    };
    if let Some(reconstruction) = &report.reconstruction {
        limitations.push(format!(
            "Step order is chronological only: {}",
            reconstruction.ordering_note
        ));
    }
    if !report.branches.is_empty() {
        limitations.push(format!("{} branch note(s) exist only in the compilation report and are not part of the procedure; none was tested.", report.branches.len()));
    }
    let assumed = report
        .step_bases
        .iter()
        .filter(|basis| basis.basis == Basis::Observed)
        .filter_map(|basis| {
            basis
                .evidence_refs
                .first()
                .and_then(|id| sources.capture.evidence.get(id))
                .map(|record| (basis.step_id.clone(), recognized(record)))
        })
        .filter(|(_, recognized)| !recognized)
        .map(|(step, _)| step)
        .collect::<Vec<_>>();
    if !assumed.is_empty() {
        limitations.push(format!(
            "Steps {} were classified as changes because their commands were not recognized.",
            assumed.join(", ")
        ));
    }
    limitations
}

/** Run the lab on one exact candidate version
 * Input
    - registry: &CapabilityRegistry - Component 2 registry (read only)
    - sources: &Sources - canonical sources, current versions, and the requester's access
    - actor: &Actor - requester (needs the Contributor role)
    - target: &Target - exact version and digest
    - compilation: Option<&CompilationReport> - its compilation report
    - plan: &TestPlan - approved test plan
    - now: u64 - Unix milliseconds
 * Output
    - CapabilityResult<LabReport>
    - Error (Unauthorized) when the requester holds no role
*/
pub(crate) fn run_lab(
    registry: &CapabilityRegistry,
    sources: &Sources,
    actor: &Actor,
    target: &Target,
    compilation: Option<&CompilationReport>,
    plan: &TestPlan,
    now: u64,
) -> CapabilityResult<LabReport> {
    registry.grants.require(actor, Role::Contributor)?;
    let pre = preflight(&registry.state, target, compilation);
    let mut checks = pre.checks.clone();
    let usable = if pre.compilation_usable {
        compilation
    } else {
        None
    };
    match &pre.definition {
        Some(definition) => {
            checks.extend(conformance::conformance(definition, usable, sources));
            checks.extend(scenarios::run(plan, definition, &registry.state, sources));
            checks.extend(invariants::verify(definition, &registry.state, sources));
        }
        None => {
            for scenario in &plan.scenarios {
                checks.push(LabCheck::new(
                    format!("scenario:{}", scenario.scenario_id),
                    super::CheckCategory::Scenario,
                    CheckStatus::NotRun,
                    "the candidate failed preflight",
                ));
            }
        }
    }
    let mut summary = Summary::default();
    for check in &checks {
        *match check.status {
            CheckStatus::Passed => &mut summary.passed,
            CheckStatus::Failed => &mut summary.failed,
            CheckStatus::NotRun => &mut summary.not_run,
            CheckStatus::Blocked => &mut summary.blocked,
            CheckStatus::Inconclusive => &mut summary.inconclusive,
        } += 1;
    }
    let evidence_refs = checks
        .iter()
        .flat_map(|check| check.evidence_refs.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut report = LabReport {
        report_id: String::new(),
        schema_version: LAB_REPORT_SCHEMA_VERSION,
        lab_version: LAB_VERSION.into(),
        capability_id: target.capability_id.clone(),
        version: target.version,
        candidate_digest: target.expected_digest.clone(),
        candidate_state: pre.state,
        compilation: compilation.map(|report| CompilationRef {
            report_id: report.report_id.clone(),
            digest: report.digest.clone(),
            outcome: report.outcome,
        }),
        test_plan: PlanRef {
            plan_id: plan.plan_id.clone(),
            version: plan.version,
            digest: plan.digest(),
            scenarios: plan
                .scenarios
                .iter()
                .map(|scenario| scenario.scenario_id.clone())
                .collect(),
        },
        environment: LabEnvironment {
            mode: "deterministic-evaluation".into(),
            executed: false,
            scope_environments: pre
                .definition
                .as_ref()
                .map(|definition| definition.verification.scope_environments.clone())
                .unwrap_or_default(),
        },
        governance: sources.current.clone(),
        evidence_refs,
        overall: overall(&checks),
        checks,
        summary,
        limitations: limitations(usable, sources),
        meaning: MEANING.into(),
        actor: actor.clone(),
        created_at: now,
        digest: String::new(),
    };
    let digest = report.content_digest();
    report.report_id = stable_id("lab", &[&digest]);
    report.digest = digest;
    Ok(report)
}
